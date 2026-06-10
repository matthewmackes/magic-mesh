//! PLANES-15 (W65–W68, W77/W78) — the network desired-state engine.
//!
//! Networking is desired-state the same way packages/services are: a
//! node carries a [`NetState`] in its fleet baseline ([`crate::BaselineSpec`],
//! W67) and converges to it locally. The model is an **nmstate** subset
//! (W65) applied through NetworkManager (W66) — interfaces with their
//! IP config, static routes, and the resolver — because nmstate is the
//! Red-Hat-native declarative network layer and it already has the one
//! safety primitive a remote network change MUST have: a **rollback
//! checkpoint** that auto-reverts if you don't confirm in time.
//!
//! That checkpoint is why network apply is special (W77/W78). A bad
//! address/route can sever the very overlay the operator is managing
//! the box through. So netstate never does a bare apply: it takes a
//! checkpoint, applies the whole desired state **at once**, runs a
//! **reachability self-test** (must still reach the lighthouse AND at
//! least one peer over the overlay), and **commits only if the test
//! passes** — otherwise the checkpoint rolls the box back to where it
//! was. This is the nmstate `--commit` / timed-rollback contract,
//! orchestrated here so it's the same on every node with no operator
//! babysitting.
//!
//! This module is the model + the YAML render (to feed `nmstatectl`) +
//! the desired-vs-actual diff (W68, the panel's data) + the
//! checkpoint-guarded apply orchestration. The actual `nmstatectl`
//! invocation and the overlay ping live behind the [`NetOps`] trait so
//! the orchestration is exercised by tests with no root and no NICs;
//! [`SystemNetOps`] is the real one.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Whether an interface (or the IP family on it) is up, down, or removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkState {
    /// Bring the link up (the default).
    #[default]
    Up,
    /// Administratively down but keep the config.
    Down,
    /// Remove the interface definition entirely.
    Absent,
}

impl LinkState {
    /// The nmstate `state:` string for this link state.
    #[must_use]
    pub const fn as_nmstate(self) -> &'static str {
        match self {
            LinkState::Up => "up",
            LinkState::Down => "down",
            LinkState::Absent => "absent",
        }
    }
}

/// One static address on an interface (`10.42.0.7/24`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct IpAddress {
    /// The address without prefix (`10.42.0.7`).
    pub ip: String,
    /// CIDR prefix length (`24`).
    pub prefix_len: u8,
}

impl IpAddress {
    /// `ip/prefix` rendering.
    #[must_use]
    pub fn cidr(&self) -> String {
        format!("{}/{}", self.ip, self.prefix_len)
    }
}

/// The IP config for one family on an interface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct IpConfig {
    /// Whether this family is enabled at all.
    pub enabled: bool,
    /// Acquire the address by DHCP (mutually exclusive with `addresses`).
    pub dhcp: bool,
    /// Static addresses (used when `dhcp` is false).
    pub addresses: Vec<IpAddress>,
}

/// One network interface's desired state (nmstate `interfaces[]` entry).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct NetInterface {
    /// Interface name (`eth0`, `nebula1`, `bond0`).
    pub name: String,
    /// nmstate interface `type` (`ethernet`, `bond`, `vlan`, `loopback`).
    /// Free-form so new nmstate types pass through without a model bump.
    #[serde(rename = "type")]
    pub iface_type: String,
    /// Link state.
    pub state: LinkState,
    /// IPv4 config (omitted in YAML when default/empty).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<IpConfig>,
    /// IPv6 config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6: Option<IpConfig>,
}

/// One static route (nmstate `routes.config[]` entry).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Route {
    /// Destination CIDR (`0.0.0.0/0` for the default route).
    pub destination: String,
    /// Gateway / next hop address.
    pub next_hop_address: String,
    /// Outgoing interface name.
    pub next_hop_interface: String,
    /// Route metric (lower = preferred); `None` lets the kernel choose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metric: Option<i64>,
}

/// The resolver config (nmstate `dns-resolver.config`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct DnsConfig {
    /// Nameserver addresses in priority order.
    pub server: Vec<String>,
    /// Search domains.
    pub search: Vec<String>,
}

impl DnsConfig {
    /// Nothing to manage.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.server.is_empty() && self.search.is_empty()
    }
}

/// A node's full network desired-state — the nmstate subset the fleet
/// baseline carries (W67). Every section defaults empty: a baseline
/// declares only the interfaces/routes/DNS it manages, leaving the rest
/// to NetworkManager's own state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetState {
    /// Managed interfaces.
    pub interfaces: Vec<NetInterface>,
    /// Managed static routes.
    pub routes: Vec<Route>,
    /// Managed resolver config.
    pub dns: DnsConfig,
}

impl NetState {
    /// Nothing declared — the engine is then a no-op on this node.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.interfaces.is_empty() && self.routes.is_empty() && self.dns.is_empty()
    }

    /// Parse from YAML.
    ///
    /// # Errors
    /// On malformed YAML or an unknown field.
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }

    /// Render to an **nmstate** state document (what `nmstatectl apply`
    /// reads on stdin). Shapes the model into nmstate's exact schema:
    /// `interfaces:` with per-family `ipv4`/`ipv6` blocks, `routes.config`,
    /// and `dns-resolver.config`.
    ///
    /// # Errors
    /// On YAML serialisation failure (practically never for this shape).
    pub fn to_nmstate_yaml(&self) -> Result<String, serde_yaml::Error> {
        use serde_json::{json, Value};

        let render_family = |c: &IpConfig| -> Value {
            let mut m = serde_json::Map::new();
            m.insert("enabled".into(), json!(c.enabled));
            if c.enabled {
                m.insert("dhcp".into(), json!(c.dhcp));
                if !c.dhcp {
                    let addrs: Vec<Value> = c
                        .addresses
                        .iter()
                        .map(|a| json!({"ip": a.ip, "prefix-length": a.prefix_len}))
                        .collect();
                    m.insert("address".into(), json!(addrs));
                }
            }
            Value::Object(m)
        };

        let interfaces: Vec<Value> = self
            .interfaces
            .iter()
            .map(|i| {
                let mut m = serde_json::Map::new();
                m.insert("name".into(), json!(i.name));
                m.insert("type".into(), json!(i.iface_type));
                m.insert("state".into(), json!(i.state.as_nmstate()));
                if let Some(v4) = &i.ipv4 {
                    m.insert("ipv4".into(), render_family(v4));
                }
                if let Some(v6) = &i.ipv6 {
                    m.insert("ipv6".into(), render_family(v6));
                }
                Value::Object(m)
            })
            .collect();

        let mut doc = serde_json::Map::new();
        doc.insert("interfaces".into(), json!(interfaces));
        if !self.routes.is_empty() {
            let routes: Vec<Value> = self
                .routes
                .iter()
                .map(|r| {
                    let mut m = serde_json::Map::new();
                    m.insert("destination".into(), json!(r.destination));
                    m.insert("next-hop-address".into(), json!(r.next_hop_address));
                    m.insert("next-hop-interface".into(), json!(r.next_hop_interface));
                    if let Some(metric) = r.metric {
                        m.insert("metric".into(), json!(metric));
                    }
                    Value::Object(m)
                })
                .collect();
            doc.insert("routes".into(), json!({ "config": routes }));
        }
        if !self.dns.is_empty() {
            doc.insert(
                "dns-resolver".into(),
                json!({ "config": { "server": self.dns.server, "search": self.dns.search } }),
            );
        }
        serde_yaml::to_string(&Value::Object(doc))
    }

    /// Parse the **nmstate** wire schema (`nmstatectl show` output) into
    /// the managed subset of this model. nmstate uses hyphenated keys
    /// (`next-hop-address`, `dns-resolver`) and nests routes/DNS under a
    /// `config` list, which differs from our flat authoring schema — so
    /// this is a tolerant `Value` walk, not a serde derive: unknown
    /// fields and extra interfaces nmstate reports are simply ignored, so
    /// it survives nmstate version drift. This is what [`SystemNetOps`]
    /// reads to diff desired-vs-actual on a real box.
    #[must_use]
    pub fn from_nmstate_show(yaml: &str) -> Self {
        let Ok(v) = serde_yaml::from_str::<serde_json::Value>(yaml) else {
            return Self::default();
        };
        let parse_family = |fam: &serde_json::Value| -> Option<IpConfig> {
            let o = fam.as_object()?;
            let addresses = o
                .get("address")
                .and_then(|a| a.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| {
                            Some(IpAddress {
                                ip: a.get("ip")?.as_str()?.to_string(),
                                prefix_len: u8::try_from(
                                    a.get("prefix-length").and_then(serde_json::Value::as_u64)?,
                                )
                                .ok()?,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(IpConfig {
                enabled: o
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                dhcp: o
                    .get("dhcp")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                addresses,
            })
        };
        let interfaces = v
            .get("interfaces")
            .and_then(|i| i.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|i| {
                        let o = i.as_object()?;
                        Some(NetInterface {
                            name: o.get("name")?.as_str()?.to_string(),
                            iface_type: o
                                .get("type")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            state: match o.get("state").and_then(serde_json::Value::as_str) {
                                Some("down") => LinkState::Down,
                                Some("absent") => LinkState::Absent,
                                _ => LinkState::Up,
                            },
                            ipv4: o.get("ipv4").and_then(&parse_family),
                            ipv6: o.get("ipv6").and_then(&parse_family),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let routes = v
            .get("routes")
            .and_then(|r| r.get("config"))
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| {
                        let o = r.as_object()?;
                        Some(Route {
                            destination: o.get("destination")?.as_str()?.to_string(),
                            next_hop_address: o
                                .get("next-hop-address")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            next_hop_interface: o
                                .get("next-hop-interface")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            metric: o.get("metric").and_then(serde_json::Value::as_i64),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let dns = v
            .get("dns-resolver")
            .and_then(|d| d.get("config"))
            .map(|c| DnsConfig {
                server: c
                    .get("server")
                    .and_then(|s| s.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                search: c
                    .get("search")
                    .and_then(|s| s.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .unwrap_or_default();
        Self {
            interfaces,
            routes,
            dns,
        }
    }

    /// Desired-vs-actual diff (W68) — what would change if this desired
    /// state were applied over `actual`. Pure; this is the panel's data
    /// and the "would-change" signal for the drift loop. Compares the
    /// fields the model manages, keyed by interface name / route
    /// destination / the resolver as a whole.
    #[must_use]
    pub fn diff(&self, actual: &NetState) -> Vec<NetChange> {
        let mut out = Vec::new();

        // Interfaces, by name.
        let actual_ifaces: BTreeMap<&str, &NetInterface> = actual
            .interfaces
            .iter()
            .map(|i| (i.name.as_str(), i))
            .collect();
        for want in &self.interfaces {
            match actual_ifaces.get(want.name.as_str()) {
                None => out.push(NetChange::interface(&want.name, "add", "not present", want)),
                Some(have) if *have != want => {
                    out.push(NetChange::interface(
                        &want.name,
                        "update",
                        &summarize(have),
                        want,
                    ));
                }
                Some(_) => {}
            }
        }

        // Routes, by (destination, next_hop_interface).
        let route_key = |r: &Route| format!("{} via {}", r.destination, r.next_hop_interface);
        let have_routes: BTreeMap<String, &Route> =
            actual.routes.iter().map(|r| (route_key(r), r)).collect();
        for want in &self.routes {
            let k = route_key(want);
            match have_routes.get(&k) {
                None => out.push(NetChange {
                    kind: NetKind::Route,
                    target: k,
                    action: "add".into(),
                    from: "not present".into(),
                    to: want.next_hop_address.clone(),
                }),
                Some(have) if **have != *want => out.push(NetChange {
                    kind: NetKind::Route,
                    target: k,
                    action: "update".into(),
                    from: have.next_hop_address.clone(),
                    to: want.next_hop_address.clone(),
                }),
                Some(_) => {}
            }
        }

        // DNS, as a unit.
        if !self.dns.is_empty() && self.dns != actual.dns {
            out.push(NetChange {
                kind: NetKind::Dns,
                target: "dns-resolver".into(),
                action: "update".into(),
                from: actual.dns.server.join(","),
                to: self.dns.server.join(","),
            });
        }
        out
    }
}

/// The domain a [`NetChange`] touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetKind {
    /// An interface entry.
    Interface,
    /// A static route.
    Route,
    /// The resolver.
    Dns,
}

/// One desired-vs-actual difference (W68), shaped for the panel: what,
/// the action (`add`/`update`), and a human from→to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetChange {
    /// Which domain.
    pub kind: NetKind,
    /// The entry identity (interface name / `dest via iface` / `dns-resolver`).
    pub target: String,
    /// `add` | `update`.
    pub action: String,
    /// Current value summary.
    pub from: String,
    /// Desired value summary.
    pub to: String,
}

impl NetChange {
    fn interface(name: &str, action: &str, from: &str, want: &NetInterface) -> Self {
        Self {
            kind: NetKind::Interface,
            target: name.to_string(),
            action: action.to_string(),
            from: from.to_string(),
            to: summarize(want),
        }
    }
}

/// A one-line summary of an interface's managed state (for the diff's
/// from/to columns).
fn summarize(i: &NetInterface) -> String {
    let v4 = i.ipv4.as_ref().map_or(String::new(), |c| {
        if !c.enabled {
            "v4:off".into()
        } else if c.dhcp {
            "v4:dhcp".into()
        } else {
            format!(
                "v4:{}",
                c.addresses
                    .iter()
                    .map(IpAddress::cidr)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    });
    format!("{} {} {}", i.iface_type, i.state.as_nmstate(), v4)
        .trim()
        .to_string()
}

/// The outcome of a checkpoint-guarded apply (W77/W78).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Nothing to do — the desired state matched actual (no diff).
    NoChange,
    /// Applied, the self-test passed, and the checkpoint was committed.
    Committed,
    /// Applied but the self-test failed — the checkpoint rolled the box
    /// back. Carries which probe targets were unreachable after apply.
    RolledBack {
        /// The probe targets still unreachable after apply (why it reverted).
        unreachable: Vec<String>,
    },
    /// The apply itself errored before any self-test; the checkpoint (if
    /// one was taken) rolled back. Carries the error string.
    Failed {
        /// The apply/commit error that triggered the revert.
        error: String,
    },
}

/// The side-effecting network operations the checkpoint-guarded apply
/// drives. Behind a trait so the orchestration ([`apply_with_self_test`])
/// is fully testable with no root, no NICs, and no real overlay.
pub trait NetOps {
    /// Read the node's current managed network state (for the diff).
    fn read_actual(&self) -> NetState;
    /// Take a rollback checkpoint, returning an opaque handle.
    ///
    /// # Errors
    /// If the checkpoint can't be created.
    fn checkpoint(&self) -> Result<String, String>;
    /// Apply the nmstate document. Does NOT commit — the checkpoint is
    /// still armed and will auto-revert unless [`commit`](NetOps::commit)
    /// is called.
    ///
    /// # Errors
    /// On apply failure.
    fn apply(&self, nmstate_yaml: &str) -> Result<(), String>;
    /// Probe overlay reachability to each target; return the targets that
    /// are NOT reachable. The self-test passes when this is empty.
    fn unreachable(&self, targets: &[String]) -> Vec<String>;
    /// Commit (keep) the applied state, disarming the checkpoint.
    ///
    /// # Errors
    /// On commit failure.
    fn commit(&self, checkpoint: &str) -> Result<(), String>;
    /// Roll back to the checkpoint, reverting the apply.
    fn rollback(&self, checkpoint: &str);
}

/// Apply `desired` under a rollback checkpoint with a post-apply
/// reachability self-test (W77/W78).
///
/// The contract, in order:
/// 1. diff against actual — if there's no change, return [`ApplyOutcome::NoChange`]
///    without touching the network;
/// 2. take a checkpoint (the timed auto-revert safety net);
/// 3. apply the WHOLE desired state at once (W77 — never a partial set);
/// 4. self-test: the box must still reach **every** `probe_targets` entry
///    over the overlay (the caller passes the lighthouse + at least one
///    peer — W78);
/// 5. commit if the self-test passed, else roll back.
///
/// The checkpoint guarantees that even a panic / lost connection between
/// apply and commit reverts the box, so a network change can never
/// permanently strand a node.
pub fn apply_with_self_test(
    ops: &dyn NetOps,
    desired: &NetState,
    probe_targets: &[String],
) -> ApplyOutcome {
    if desired.diff(&ops.read_actual()).is_empty() {
        return ApplyOutcome::NoChange;
    }
    let yaml = match desired.to_nmstate_yaml() {
        Ok(y) => y,
        Err(e) => {
            return ApplyOutcome::Failed {
                error: e.to_string(),
            }
        }
    };
    let checkpoint = match ops.checkpoint() {
        Ok(c) => c,
        Err(e) => {
            return ApplyOutcome::Failed {
                error: format!("checkpoint: {e}"),
            }
        }
    };
    if let Err(e) = ops.apply(&yaml) {
        ops.rollback(&checkpoint);
        return ApplyOutcome::Failed {
            error: format!("apply: {e}"),
        };
    }
    let unreachable = ops.unreachable(probe_targets);
    if unreachable.is_empty() {
        match ops.commit(&checkpoint) {
            Ok(()) => ApplyOutcome::Committed,
            Err(e) => {
                ops.rollback(&checkpoint);
                ApplyOutcome::Failed {
                    error: format!("commit: {e}"),
                }
            }
        }
    } else {
        ops.rollback(&checkpoint);
        ApplyOutcome::RolledBack { unreachable }
    }
}

/// The real [`NetOps`] over `nmstatectl` + an overlay ping. Each method
/// is best-effort against the host tools; on a box without `nmstatectl`
/// the apply errors cleanly (and so rolls back to no-op) rather than
/// half-applying.
pub struct SystemNetOps;

impl SystemNetOps {
    fn nmstatectl(args: &[&str], stdin: Option<&str>) -> Result<String, String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut cmd = Command::new("nmstatectl");
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if stdin.is_some() {
            cmd.stdin(Stdio::piped());
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn nmstatectl: {e}"))?;
        if let Some(s) = stdin {
            child
                .stdin
                .take()
                .ok_or("no stdin")?
                .write_all(s.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }
}

impl NetOps for SystemNetOps {
    fn read_actual(&self) -> NetState {
        // `nmstatectl show` emits the live state in the same schema; we
        // parse only the managed subset, tolerating the extra fields.
        Self::nmstatectl(&["show"], None)
            .map(|yaml| NetState::from_nmstate_show(&yaml))
            .unwrap_or_default()
    }

    fn checkpoint(&self) -> Result<String, String> {
        // `nmstatectl` arms a checkpoint with an apply; we model the
        // explicit-checkpoint flow via the NetworkManager checkpoint the
        // apply step creates. Returning the sentinel keeps the rollback
        // path symmetric for the dev/no-tool case.
        Ok("nm-checkpoint".to_string())
    }

    fn apply(&self, nmstate_yaml: &str) -> Result<(), String> {
        // `--no-commit` arms the auto-revert; `--timeout` bounds it so a
        // lost session reverts on its own.
        Self::nmstatectl(
            &["apply", "--no-commit", "--timeout", "60"],
            Some(nmstate_yaml),
        )
        .map(|_| ())
    }

    fn unreachable(&self, targets: &[String]) -> Vec<String> {
        targets
            .iter()
            .filter(|t| {
                !std::process::Command::new("ping")
                    .args(["-c", "1", "-W", "2", t])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    }

    fn commit(&self, _checkpoint: &str) -> Result<(), String> {
        Self::nmstatectl(&["commit"], None).map(|_| ())
    }

    fn rollback(&self, _checkpoint: &str) {
        let _ = Self::nmstatectl(&["rollback"], None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, ip: &str) -> NetInterface {
        NetInterface {
            name: name.into(),
            iface_type: "ethernet".into(),
            state: LinkState::Up,
            ipv4: Some(IpConfig {
                enabled: true,
                dhcp: false,
                addresses: vec![IpAddress {
                    ip: ip.into(),
                    prefix_len: 24,
                }],
            }),
            ipv6: None,
        }
    }

    #[test]
    fn nmstate_yaml_has_the_nmstate_schema_shape() {
        let ns = NetState {
            interfaces: vec![iface("eth0", "10.42.0.7")],
            routes: vec![Route {
                destination: "0.0.0.0/0".into(),
                next_hop_address: "10.42.0.1".into(),
                next_hop_interface: "eth0".into(),
                metric: Some(100),
            }],
            dns: DnsConfig {
                server: vec!["10.42.0.1".into()],
                search: vec!["mesh".into()],
            },
        };
        let yaml = ns.to_nmstate_yaml().unwrap();
        // The exact nmstate key names — these feed `nmstatectl apply`.
        assert!(yaml.contains("interfaces:"));
        assert!(yaml.contains("prefix-length: 24"));
        assert!(yaml.contains("next-hop-address: 10.42.0.1"));
        assert!(yaml.contains("dns-resolver:"));
        // The render is the nmstate WIRE schema; parsing it back through
        // the nmstate-show adapter recovers the managed subset (this is
        // exactly the read_actual path on a real box).
        let back = NetState::from_nmstate_show(&yaml);
        assert_eq!(back.interfaces[0].name, "eth0");
        assert_eq!(
            back.interfaces[0].ipv4.as_ref().unwrap().addresses[0].ip,
            "10.42.0.7"
        );
        assert_eq!(back.routes[0].destination, "0.0.0.0/0");
        assert_eq!(back.routes[0].next_hop_address, "10.42.0.1");
        assert_eq!(back.dns.server, vec!["10.42.0.1"]);
        // And a no-op diff: actual == desired after the round-trip.
        assert!(ns.diff(&back).is_empty(), "round-trip is a fixed point");
    }

    #[test]
    fn diff_reports_adds_updates_and_skips_matches() {
        let actual = NetState {
            interfaces: vec![iface("eth0", "10.42.0.7")],
            ..Default::default()
        };
        let desired = NetState {
            interfaces: vec![
                iface("eth0", "10.42.0.7"), // unchanged → no diff entry
                iface("eth1", "10.42.0.8"), // new → add
            ],
            ..Default::default()
        };
        let d = desired.diff(&actual);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].target, "eth1");
        assert_eq!(d[0].action, "add");

        // Change eth0's address → update.
        let changed = NetState {
            interfaces: vec![iface("eth0", "10.42.0.99")],
            ..Default::default()
        };
        let d2 = changed.diff(&actual);
        assert_eq!(d2.len(), 1);
        assert_eq!(d2[0].action, "update");
        assert!(d2[0].to.contains("10.42.0.99"));
    }

    /// A mock NetOps recording the call sequence, with a togglable
    /// self-test verdict.
    struct MockOps {
        actual: NetState,
        reachable: bool,
        log: std::cell::RefCell<Vec<String>>,
    }
    impl NetOps for MockOps {
        fn read_actual(&self) -> NetState {
            self.actual.clone()
        }
        fn checkpoint(&self) -> Result<String, String> {
            self.log.borrow_mut().push("checkpoint".into());
            Ok("cp1".into())
        }
        fn apply(&self, _: &str) -> Result<(), String> {
            self.log.borrow_mut().push("apply".into());
            Ok(())
        }
        fn unreachable(&self, targets: &[String]) -> Vec<String> {
            if self.reachable {
                Vec::new()
            } else {
                targets.to_vec()
            }
        }
        fn commit(&self, _: &str) -> Result<(), String> {
            self.log.borrow_mut().push("commit".into());
            Ok(())
        }
        fn rollback(&self, _: &str) {
            self.log.borrow_mut().push("rollback".into());
        }
    }

    #[test]
    fn self_test_pass_commits() {
        let ops = MockOps {
            actual: NetState::default(),
            reachable: true,
            log: Default::default(),
        };
        let desired = NetState {
            interfaces: vec![iface("eth0", "10.42.0.7")],
            ..Default::default()
        };
        let out = apply_with_self_test(&ops, &desired, &["10.42.0.1".into()]);
        assert_eq!(out, ApplyOutcome::Committed);
        assert_eq!(*ops.log.borrow(), ["checkpoint", "apply", "commit"]);
    }

    #[test]
    fn self_test_fail_rolls_back() {
        let ops = MockOps {
            actual: NetState::default(),
            reachable: false, // post-apply we can't reach the overlay
            log: Default::default(),
        };
        let desired = NetState {
            interfaces: vec![iface("eth0", "10.42.0.7")],
            ..Default::default()
        };
        let out = apply_with_self_test(&ops, &desired, &["lighthouse".into(), "peer1".into()]);
        assert!(matches!(out, ApplyOutcome::RolledBack { .. }));
        // Critically: rolled back, never committed.
        assert_eq!(*ops.log.borrow(), ["checkpoint", "apply", "rollback"]);
        if let ApplyOutcome::RolledBack { unreachable } = out {
            assert_eq!(unreachable, vec!["lighthouse", "peer1"]);
        }
    }

    #[test]
    fn no_diff_is_a_noop_no_checkpoint_taken() {
        let state = NetState {
            interfaces: vec![iface("eth0", "10.42.0.7")],
            ..Default::default()
        };
        let ops = MockOps {
            actual: state.clone(),
            reachable: true,
            log: Default::default(),
        };
        let out = apply_with_self_test(&ops, &state, &["10.42.0.1".into()]);
        assert_eq!(out, ApplyOutcome::NoChange);
        // Never touched the network — no checkpoint, no apply.
        assert!(ops.log.borrow().is_empty());
    }
}
