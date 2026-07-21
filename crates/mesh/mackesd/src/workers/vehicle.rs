//! Rolling Node — the mackesd `vehicle` worker: the workstation-side adapter that
//! SSH/HTTP-polls a mobile **Sierra AirLink MG90** (oMG) gateway and publishes a
//! latest-wins `state/vehicle/<node>` Bus mirror.
//!
//! The worker is the mesh-side runner + status publisher for one on-owned-vehicle
//! gateway. It:
//!
//! 1. **Reads three raw sources** through the injectable [`VehicleProbe`] seam
//!    (production [`SshHttpProbe`]; tests inject a fake):
//!    - the GNSS/IMU NMEA blob (`/var/run/omgtime.g.info`, over SSH),
//!    - the LCI **general** status page (over the authed Tomcat HTTP session),
//!    - the LCI **WAN** status page (same session).
//! 2. **Folds them into a neutral [`VehicleState`]** — GPS via the pure
//!    [`parse_gpgga`], IMU via [`parse_psiwmmpu`], and TOLERANT label→value
//!    extractors over the (tag-stripped) HTML. Anything it cannot extract goes into
//!    `gaps` (honest-partial, §7) rather than being fabricated.
//! 3. **Publishes `state/vehicle/<node>`** (latest-wins) on a ~5 s poll that
//!    doubles as the heartbeat, via [`crate::bus_publish::publish_json`] — exactly
//!    like the `cloud` worker's mirror publish.
//! 4. **Drains `action/vehicle/*` control verbs** off the Bus
//!    ([`VEHICLE_ACTION_PREFIX`]) and answers each on `reply/<ulid>` with a
//!    [`VehicleReply`] — `get-config` (a READ that pulls a committed oMG config
//!    file over SSH) and `reboot` (a destructive MUTATION, typed-armed on the
//!    gateway ESN + audited). Only a node WITH a gateway (`MDE_VEHICLE_GATEWAY`
//!    set) drains; every other node idles and ignores the queue.
//!
//! ## Config (env for now; mde-seal later)
//! - `MDE_VEHICLE_GATEWAY` — the gateway endpoint, an IP or `ip:sshport`. **When
//!   unset the worker is a no-op** (logs once, publishes nothing) — most nodes have
//!   no vehicle gateway attached.
//! - `MDE_VEHICLE_ROOT_PW` — the gateway's `root` SSH password (the oMG SSH host is
//!   a legacy-crypto box, hence the explicit `+ssh-rsa` / `diffie-hellman-group1`
//!   options on the real probe). HTTP auth is the fixed oMG `admin`/`admin` LCI
//!   login.
//!
//! On any anchor-probe error (the LCI general read — the gateway's reachability
//! signal) the worker publishes an honest [`VehicleState::offline`] snapshot; the
//! GPS (SSH) and WAN (HTTP) reads degrade to a `gaps` note without blanking the
//! whole mirror.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use mackes_mesh_types::vehicle::{
    parse_gpgga, vehicle_state_topic, CellLink, GpsFix, ImuSample, VehicleReply, VehicleState,
    VehicleTelem, WanStatus, VEHICLE_ACTION_PREFIX,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde::Deserialize;

use super::{ShutdownToken, Worker};

/// Env: the gateway endpoint (an IP or `ip:sshport`). Unset ⇒ the worker is a no-op.
pub const GATEWAY_ENV: &str = "MDE_VEHICLE_GATEWAY";

/// Env: the gateway `root` SSH password (later mde-seal; env is fine for now).
pub const ROOT_PW_ENV: &str = "MDE_VEHICLE_ROOT_PW";

/// Poll cadence — build + publish a fresh mirror every ~5 s. Doubles as the
/// latest-wins heartbeat (a stale consumer always sees a recent stamp).
pub const POLL: Duration = Duration::from_secs(5);

/// The oMG GNSS/IMU NMEA blob the SSH read `cat`s.
const GPS_INFO_PATH: &str = "/var/run/omgtime.g.info";

/// The LCI general status page (relative to the gateway root).
const LCI_GENERAL_URL: &str = "MG-LCI/status/general.html";

/// The LCI extended WAN status page.
const LCI_WAN_URL: &str = "MG-LCI/wan/status/status.html?displayExtended=true";

// ─────────────────────────── the injectable probe seam ───────────────────────────

/// The raw-text read seam the worker folds into a [`VehicleState`]: three methods,
/// each returning the RAW text the adapter reads, so tests inject a fake without a
/// live gateway (the same applier-injection idiom as the `cloud` worker's
/// `CloudRunner` seam).
pub trait VehicleProbe: Send + Sync {
    /// The GNSS/IMU NMEA blob (real: SSH `cat /var/run/omgtime.g.info`).
    ///
    /// # Errors
    /// The SSH transport's failure (host unreachable / `sshpass` absent / auth).
    fn read_gps_nmea(&self) -> io::Result<String>;

    /// The LCI **general** status HTML (real: authed HTTP GET of
    /// `.../MG-LCI/status/general.html`). This is the worker's reachability anchor.
    ///
    /// # Errors
    /// The HTTP transport's failure (host unreachable / `curl` absent / auth).
    fn read_lci_general(&self) -> io::Result<String>;

    /// The LCI extended **WAN** status HTML (real: authed HTTP GET of
    /// `.../MG-LCI/wan/status/status.html?displayExtended=true`).
    ///
    /// # Errors
    /// The HTTP transport's failure.
    fn read_lci_wan(&self) -> io::Result<String>;

    /// Run an arbitrary command on the gateway over SSH, returning its stdout — the
    /// seam the `action/vehicle/*` control verbs (`get-config` / `reboot`) shell
    /// through. Real: the same legacy-crypto SSH as [`Self::read_gps_nmea`]; tests
    /// inject a canned response + record the invocation.
    ///
    /// # Errors
    /// The SSH transport's failure (host unreachable / `sshpass` absent / auth).
    fn run_ssh(&self, cmd: &str) -> io::Result<String>;
}

/// The production probe: shells `sshpass`/`ssh` for the NMEA blob and `curl` for the
/// Tomcat FORM-auth'd LCI pages (single cookie-jar session, follow the 303).
pub struct SshHttpProbe {
    /// The gateway IP (no port).
    ip: String,
    /// The SSH port (default 22).
    ssh_port: u16,
    /// The `root` SSH password (from [`ROOT_PW_ENV`]).
    ssh_pw: String,
}

impl SshHttpProbe {
    /// Build from a raw `MDE_VEHICLE_GATEWAY` value (an IP or `ip:sshport`) + the
    /// `root` password from [`ROOT_PW_ENV`] (empty when unset).
    #[must_use]
    pub fn from_env(gateway: &str) -> Self {
        let (ip, ssh_port) = parse_endpoint(gateway);
        Self {
            ip,
            ssh_port,
            ssh_pw: std::env::var(ROOT_PW_ENV).unwrap_or_default(),
        }
    }

    /// The LCI base URL (`http://<ip>/`).
    fn base_url(&self) -> String {
        format!("http://{}/", self.ip)
    }

    /// The per-process cookie jar the single authed session shares across the LCI
    /// login + page fetches (mirrors the `-c jar -b jar` pattern).
    fn jar_path(&self) -> PathBuf {
        std::env::temp_dir().join(format!("mde-vehicle-cookies-{}.jar", std::process::id()))
    }

    /// Run `curl` with `args`, returning stdout. An empty `Ok("")` is a legitimate
    /// (empty-page) result; only a spawn failure / non-zero exit is an `Err`.
    fn curl(args: &[&str]) -> io::Result<String> {
        let out = Command::new("curl").args(args).output()?;
        if !out.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "curl exited {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Fetch `page_url` off the authed LCI session: prime the session (GET `/MG-LCI/`),
    /// POST the fixed `admin`/`admin` FORM login (`j_security_check`, follow the 303),
    /// then GET the target page carrying the session cookie.
    fn http_authed_get(&self, page_url: &str) -> io::Result<String> {
        let jar = self.jar_path();
        let jar_str = jar.display().to_string();
        let base = self.base_url();
        let lci = format!("{base}MG-LCI/");
        let login = format!("{base}MG-LCI/j_security_check");
        let page = format!("{base}{page_url}");
        // 1) prime the Tomcat session (sets JSESSIONID in the jar).
        Self::curl(&["-s", "-c", &jar_str, "-b", &jar_str, "-L", &lci])?;
        // 2) FORM auth — follow the 303 back to the app.
        Self::curl(&[
            "-s",
            "-c",
            &jar_str,
            "-b",
            &jar_str,
            "-L",
            "--data-urlencode",
            "j_username=admin",
            "--data-urlencode",
            "j_password=admin",
            &login,
        ])?;
        // 3) the authed page fetch.
        Self::curl(&["-s", "-b", &jar_str, &page])
    }

    /// Run `remote_cmd` on the gateway over SSH, returning stdout. The oMG SSH host
    /// runs legacy crypto — hence the explicit `+ssh-rsa` / `group1` / `aes128-cbc`
    /// allowances (a modern OpenSSH refuses it otherwise). Shared by
    /// [`VehicleProbe::read_gps_nmea`] and [`VehicleProbe::run_ssh`].
    fn ssh(&self, remote_cmd: &str) -> io::Result<String> {
        let port = self.ssh_port.to_string();
        let target = format!("root@{}", self.ip);
        let out = Command::new("sshpass")
            .args([
                "-p",
                &self.ssh_pw,
                "ssh",
                "-p",
                &port,
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "HostKeyAlgorithms=+ssh-rsa,ssh-dss",
                "-o",
                "KexAlgorithms=+diffie-hellman-group1-sha1,diffie-hellman-group14-sha1",
                "-o",
                "PubkeyAcceptedAlgorithms=+ssh-rsa",
                "-o",
                "Ciphers=+aes128-cbc,3des-cbc",
                &target,
                remote_cmd,
            ])
            .output()?;
        if !out.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "ssh exited {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl VehicleProbe for SshHttpProbe {
    fn read_gps_nmea(&self) -> io::Result<String> {
        self.ssh(&format!("cat {GPS_INFO_PATH}"))
    }

    fn read_lci_general(&self) -> io::Result<String> {
        self.http_authed_get(LCI_GENERAL_URL)
    }

    fn read_lci_wan(&self) -> io::Result<String> {
        self.http_authed_get(LCI_WAN_URL)
    }

    fn run_ssh(&self, cmd: &str) -> io::Result<String> {
        self.ssh(cmd)
    }
}

/// Split a `MDE_VEHICLE_GATEWAY` value into `(ip, ssh_port)`. `ip:port` yields the
/// parsed port; a bare `ip` (or an unparsable suffix) defaults to port 22. IPv4-only
/// (the MG90 is an IPv4 box) — no bracketed-IPv6 handling.
fn parse_endpoint(raw: &str) -> (String, u16) {
    match raw.trim().rsplit_once(':') {
        Some((ip, port)) => match port.trim().parse::<u16>() {
            Ok(p) => (ip.trim().to_string(), p),
            Err(_) => (raw.trim().to_string(), 22),
        },
        None => (raw.trim().to_string(), 22),
    }
}

// ─────────────────────────── the worker ───────────────────────────

/// The `vehicle` worker (per-node, rank-0 universal — but a genuine no-op on the
/// overwhelming majority of nodes that have no gateway). Mirrors the `cloud`
/// worker's lifecycle: an injectable transport seam, a `bus_root: Option<PathBuf>`
/// (`None` ⇒ publish is a no-op), and a poll-and-publish run loop.
pub struct VehicleWorker {
    /// This node's id — the `state/vehicle/<host>` namespace + the mirror `host` stamp.
    host: String,
    /// The transport seam (production [`SshHttpProbe`]). `None` ⇒ no
    /// `MDE_VEHICLE_GATEWAY` configured ⇒ the worker idles (publishes nothing).
    probe: Option<Arc<dyn VehicleProbe>>,
    /// The Bus root the mirror publish targets + the `action/vehicle/*` drain reads
    /// (`None` ⇒ publish/drain is a swallowed no-op — a pre-RPM dev box / a test).
    bus_root: Option<PathBuf>,
    /// The hash-chain audit DB (a performed `reboot` audits here — mirrors the
    /// `cloud` worker's destructive-op audit).
    db_path: PathBuf,
    /// Poll + heartbeat cadence.
    poll: Duration,
}

impl VehicleWorker {
    /// Construct with production wiring: the [`SshHttpProbe`] from
    /// [`GATEWAY_ENV`]/[`ROOT_PW_ENV`] (absent gateway ⇒ `None` ⇒ idle) and the
    /// persisted Bus tree. `host` is this node's id (the `peer:`-stripped node id).
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe: Option<Arc<dyn VehicleProbe>> = match std::env::var(GATEWAY_ENV) {
            Ok(g) if !g.trim().is_empty() => Some(Arc::new(SshHttpProbe::from_env(g.trim()))),
            _ => None,
        };
        Self {
            host,
            probe,
            bus_root: crate::bus_publish::default_bus_root(),
            db_path: crate::default_db_path(),
            poll: POLL,
        }
    }

    /// Inject a probe (tests supply a fake; also the seam a future mde-seal wiring
    /// swaps the real transport through).
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn VehicleProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override the Bus root (tests point it at a tempdir; `None` disables publish).
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    /// Override the audit DB path (tests point it at a tempdir).
    #[must_use]
    pub fn with_db_path(mut self, p: PathBuf) -> Self {
        self.db_path = p;
        self
    }

    /// Override the poll cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Build the current `state/vehicle/<host>` mirror from the probe's three raw
    /// reads. The LCI general read is the reachability anchor: its failure ⇒ an
    /// honest [`VehicleState::offline`] snapshot. GPS (SSH) + WAN (HTTP) failures
    /// degrade to a `gaps` note rather than blanking the mirror.
    #[must_use]
    pub fn build_state(&self, probe: &dyn VehicleProbe) -> VehicleState {
        let general = match probe.read_lci_general() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::vehicle",
                    host = %self.host, error = %e,
                    "vehicle gateway LCI unreachable — publishing offline mirror"
                );
                let mut s = VehicleState::offline(&self.host);
                s.published_at_ms = now_ms();
                return s;
            }
        };

        let mut gaps: Vec<String> = Vec::new();
        let general_text = strip_html(&general);

        // ── general.html: MCU power/board + identity ──
        let battery_v =
            find_number_after(&general_text, "Main Battery Voltage").unwrap_or_else(|| {
                gaps.push("telem.battery_v not reported by general.html".to_string());
                0.0
            });
        let internal_temp_c = find_number_after(&general_text, "Internal Temperature")
            .unwrap_or_else(|| {
                gaps.push("telem.internal_temp_c not reported by general.html".to_string());
                0.0
            });
        let esn = find_token_after(&general_text, "ESN").unwrap_or_else(|| {
            gaps.push("esn not reported by general.html".to_string());
            String::new()
        });
        let mgos_version = find_token_after(&general_text, "Version").unwrap_or_else(|| {
            gaps.push("mgos_version not reported by general.html".to_string());
            String::new()
        });
        let model = find_token_after(&general_text, "Model").unwrap_or_else(|| {
            gaps.push("model not reported by general.html".to_string());
            String::new()
        });

        // ── GNSS/IMU over SSH ──
        let (gps, imu) = match probe.read_gps_nmea() {
            Ok(nmea) => parse_gps_imu(&nmea, &mut gaps),
            Err(e) => {
                gaps.push(format!("gps/imu unavailable (ssh): {e}"));
                (GpsFix::default(), None)
            }
        };

        // ── WAN status over HTTP ──
        let wan = match probe.read_lci_wan() {
            Ok(html) => parse_wan(&html, &mut gaps),
            Err(e) => {
                gaps.push(format!("wan status unavailable (http): {e}"));
                WanStatus::default()
            }
        };

        // ── vehicle power + OBD telemetry ──
        // ignition/OBD source (the MCU ignition-sense line + an OBD-II dongle) is a
        // follow-up; leave the flags false with an honest gap rather than guessing.
        gaps.push(
            "ignition/OBD not wired (MCU ignition-sense + OBD-II source is a follow-up)"
                .to_string(),
        );
        let telem = VehicleTelem {
            battery_v,
            internal_temp_c,
            ignition_on: false,
            moving: gps.speed_mph > 0.5,
            obd_present: false,
            ..Default::default()
        };

        VehicleState {
            host: self.host.clone(),
            model,
            esn,
            mgos_version,
            online: true,
            gps,
            imu,
            wan,
            telem,
            gaps,
            published_at_ms: now_ms(),
        }
    }

    /// Publish the current mirror to `state/vehicle/<host>` (best-effort, exactly
    /// like the `cloud` worker's `publish_state`).
    fn publish(&self, state: &VehicleState) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(&mut persist, &vehicle_state_topic(&self.host), state);
        }
    }

    // ─────────────────────── Phase 4 · action/vehicle/* control drain ───────────────────────

    /// Handle one `action/vehicle/<verb>` request end to end → a typed
    /// [`VehicleReply`]. A node with no gateway attached (`probe: None`) honestly
    /// gates every verb (`no gateway on this node`) rather than faking a result; the
    /// run loop only reaches this on a gateway node (a no-gateway worker idles).
    #[must_use]
    pub fn handle(&self, verb_name: &str, body: &str) -> VehicleReply {
        let Some(verb) = VehicleVerb::from_verb(verb_name) else {
            return VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!("unknown vehicle verb `{verb_name}`")),
                ..Default::default()
            };
        };
        let Some(probe) = self.probe.clone() else {
            return VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                gated: Some("no gateway on this node".to_string()),
                ..Default::default()
            };
        };
        let body = VehicleActionBody::parse(body);
        match verb {
            VehicleVerb::GetConfig => self.handle_get_config(probe.as_ref(), verb_name, &body),
            VehicleVerb::Reboot => self.handle_reboot(probe.as_ref(), verb_name, &body),
        }
    }

    /// `get-config` (READ) — pull a committed oMG config file over SSH
    /// (`omgconf latest <file>`). `file` MUST be a bare `*.yaml` name (no path
    /// components / traversal), else an honest rejection.
    fn handle_get_config(
        &self,
        probe: &dyn VehicleProbe,
        verb_name: &str,
        body: &VehicleActionBody,
    ) -> VehicleReply {
        let Some(file) = body
            .file
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some("`get-config` requires a `file` field in the request body".to_string()),
                ..Default::default()
            };
        };
        if !is_safe_yaml_name(file) {
            return VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!(
                    "`file` must be a bare `*.yaml` name with no path components: `{file}`"
                )),
                ..Default::default()
            };
        }
        match probe.run_ssh(&format!("omgconf latest {file}")) {
            Ok(yaml) => VehicleReply {
                ok: true,
                verb: verb_name.to_string(),
                applied: Some(yaml),
                ..Default::default()
            },
            Err(e) => VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                gated: Some(format!("gateway ssh unavailable: {e}")),
                ..Default::default()
            },
        }
    }

    /// `reboot` (MUTATION, destructive) — typed-armed on the gateway ESN. The body's
    /// `typed_name` MUST equal the live gateway ESN BEFORE the SSH `reboot` runs;
    /// otherwise nothing is performed and the reply is honestly gated. A performed
    /// reboot is audited on the events plane (so `audited: true` is truthful),
    /// mirroring the `cloud` worker's destructive-op gate + audit.
    fn handle_reboot(
        &self,
        probe: &dyn VehicleProbe,
        verb_name: &str,
        body: &VehicleActionBody,
    ) -> VehicleReply {
        // Typed-arming: `typed_name` must equal the live gateway ESN.
        let esn = self.gateway_esn(probe);
        let typed = body.typed_name.as_deref().map(str::trim).unwrap_or("");
        let armed = !typed.is_empty() && !esn.is_empty() && typed == esn;
        if !armed {
            return VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                gated: Some(
                    "typed-arm required: `typed_name` must equal the gateway ESN".to_string(),
                ),
                ..Default::default()
            };
        }
        match probe.run_ssh("reboot") {
            Ok(_) => {
                self.audit_reboot(&esn);
                VehicleReply {
                    ok: true,
                    verb: verb_name.to_string(),
                    applied: Some("reboot issued".to_string()),
                    audited: true,
                    ..Default::default()
                }
            }
            Err(e) => VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!("reboot ssh failed: {e}")),
                ..Default::default()
            },
        }
    }

    /// The live gateway ESN (the reboot typed-arming target) — parsed from the LCI
    /// general page. Empty when the gateway is unreachable / the page omits it (so a
    /// reboot can NEVER arm without a confirmed ESN).
    fn gateway_esn(&self, probe: &dyn VehicleProbe) -> String {
        probe
            .read_lci_general()
            .ok()
            .map(|h| strip_html(&h))
            .and_then(|t| find_token_after(&t, "ESN"))
            .unwrap_or_default()
    }

    /// Write one hash-chain audit row for a performed `reboot` through the EXISTING
    /// events plane (best-effort — a store fault is logged, never fatal). Makes the
    /// reply's `audited: true` truthful. Mirrors [`CloudWorker::audit`].
    fn audit_reboot(&self, esn: &str) {
        crate::events::append_and_alert(
            &self.db_path,
            &format!("peer:{}", self.host),
            crate::events::EventKind::AdminAction,
            serde_json::json!({
                "action": "vehicle",
                "verb": "reboot",
                "host": self.host,
                "esn": esn,
            }),
        );
    }

    /// Drain every new `action/vehicle/*` request, advance the per-topic cursors, and
    /// answer each on `reply/<ulid>` with a typed [`VehicleReply`]. Returns `true`
    /// when any request was handled. A no-bus worker is a swallowed no-op.
    fn drain_actions(&self, cursors: &mut HashMap<String, String>) -> bool {
        let Some(root) = self.bus_root.clone() else {
            return false;
        };
        let Ok(persist) = Persist::open(root) else {
            return false;
        };
        let Ok(topics) = persist.list_topics() else {
            return false;
        };
        let mut acted = false;
        for topic in topics {
            let Some(verb_name) = topic.strip_prefix(VEHICLE_ACTION_PREFIX) else {
                continue;
            };
            let verb_name = verb_name.to_string();
            let cursor = cursors.get(&topic).cloned();
            let Ok(msgs) = persist.list_since(&topic, cursor.as_deref()) else {
                continue;
            };
            for msg in msgs {
                cursors.insert(topic.clone(), msg.ulid.clone());
                let body = msg.body.as_deref().unwrap_or("{}");
                let reply = self.handle(&verb_name, body);
                tracing::info!(
                    target: "mackesd::vehicle",
                    ulid = %msg.ulid, verb = %verb_name, ok = reply.ok,
                    audited = reply.audited, "vehicle action handled"
                );
                self.write_reply(&persist, &msg.ulid, &reply);
                acted = true;
            }
        }
        acted
    }

    /// Seed each existing `action/vehicle/*` topic's cursor to its newest message so
    /// a (re)start doesn't replay a backlog of verbs.
    fn prime_cursors(&self, cursors: &mut HashMap<String, String>) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let Ok(topics) = persist.list_topics() else {
            return;
        };
        for topic in topics {
            if !topic.starts_with(VEHICLE_ACTION_PREFIX) {
                continue;
            }
            if let Ok(Some(ulid)) = persist.latest_ulid(&topic) {
                cursors.insert(topic, ulid);
            }
        }
    }

    /// Write a typed reply to `reply/<request-ulid>` (best-effort).
    fn write_reply(&self, persist: &Persist, req_ulid: &str, reply: &VehicleReply) {
        let body = serde_json::to_string(reply).unwrap_or_default();
        if let Err(e) = persist.write(&reply_topic(req_ulid), Priority::Default, None, Some(&body))
        {
            tracing::warn!(target: "mackesd::vehicle", ulid = %req_ulid, error = %e, "vehicle reply write failed");
        }
    }
}

/// A drained `action/vehicle/<verb>` classified for dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VehicleVerb {
    /// `get-config` — pull a committed oMG config file over SSH (READ).
    GetConfig,
    /// `reboot` — reboot the gateway (MUTATION, destructive; typed-armed on the ESN).
    Reboot,
}

impl VehicleVerb {
    /// Classify a verb token, or `None` for an unrecognized verb (never guessed).
    fn from_verb(verb: &str) -> Option<Self> {
        Some(match verb {
            "get-config" => Self::GetConfig,
            "reboot" => Self::Reboot,
            _ => return None,
        })
    }
}

/// The parsed `action/vehicle/*` request body — the fields the verbs read off the
/// wire JSON. Every field is optional so a legacy `{}` request still parses; each
/// handler enforces what it actually requires.
#[derive(Debug, Clone, Default, Deserialize)]
struct VehicleActionBody {
    /// `get-config`'s target config file (a bare `*.yaml` name).
    #[serde(default)]
    file: Option<String>,
    /// `reboot`'s typed-arming confirmation (must equal the gateway ESN).
    #[serde(default)]
    typed_name: Option<String>,
}

impl VehicleActionBody {
    /// Parse a request body, degrading a malformed body to an all-empty request
    /// (the per-verb handlers then honestly reject what they require).
    fn parse(body: &str) -> Self {
        serde_json::from_str(body.trim()).unwrap_or_default()
    }
}

/// Whether `name` is a safe bare `*.yaml` config-file name — no path components, no
/// `..` traversal, only sane filename chars. Guards the `get-config` SSH arg.
fn is_safe_yaml_name(name: &str) -> bool {
    name.len() > ".yaml".len()
        && name.ends_with(".yaml")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

#[async_trait::async_trait]
impl Worker for VehicleWorker {
    fn name(&self) -> &'static str {
        "vehicle"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // No gateway on this node ⇒ log once + idle (publish nothing). Most nodes
        // never have a vehicle gateway attached.
        let Some(probe) = self.probe.clone() else {
            tracing::info!(
                target: "mackesd::vehicle",
                host = %self.host,
                env = GATEWAY_ENV,
                "no vehicle gateway configured — vehicle worker idle"
            );
            shutdown.wait().await;
            return Ok(());
        };
        // Seed the action cursors so a (re)start doesn't replay a backlog of verbs.
        let mut cursors: HashMap<String, String> = HashMap::new();
        self.prime_cursors(&mut cursors);
        loop {
            // Phase 4 — drain any queued `action/vehicle/*` control verbs first.
            self.drain_actions(&mut cursors);
            let state = self.build_state(probe.as_ref());
            self.publish(&state);
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.poll) => {}
            }
        }
    }
}

// ─────────────────────────── raw-text folds ───────────────────────────

/// Parse the GNSS `$GPGGA` + IMU `$PSIWMMPU` lines out of an oMG NMEA blob. GPS via
/// the pure [`parse_gpgga`]; IMU best-effort (a missing line ⇒ `None` + a gap).
fn parse_gps_imu(nmea: &str, gaps: &mut Vec<String>) -> (GpsFix, Option<ImuSample>) {
    let gps = nmea
        .lines()
        .find(|l| l.contains("GGA,"))
        .and_then(parse_gpgga)
        .unwrap_or_else(|| {
            gaps.push("no $GPGGA line in the gateway NMEA".to_string());
            GpsFix::default()
        });
    let imu = nmea
        .lines()
        .find(|l| l.contains("PSIWMMPU"))
        .and_then(parse_psiwmmpu);
    if imu.is_none() {
        gaps.push("no $PSIWMMPU IMU line in the gateway NMEA".to_string());
    }
    (gps, imu)
}

/// Parse an oMG `$PSIWMMPU,<t>,<ax>,<ay>,<az>,<gx>,<gy>,<gz>` line into an
/// [`ImuSample`] (accel g, gyro deg/s). `None` when the line is malformed.
fn parse_psiwmmpu(line: &str) -> Option<ImuSample> {
    let tag = "PSIWMMPU,";
    let start = line.find(tag)?;
    let body = &line[start + tag.len()..];
    // Drop the checksum suffix if present.
    let body = body.split('*').next().unwrap_or(body);
    let f: Vec<&str> = body.split(',').collect();
    // f: 0=timestamp, 1..4 = accel x/y/z, 4..7 = gyro x/y/z.
    if f.len() < 7 {
        return None;
    }
    let ax: f32 = f.get(1)?.trim().parse().ok()?;
    let ay: f32 = f.get(2)?.trim().parse().ok()?;
    let az: f32 = f.get(3)?.trim().parse().ok()?;
    let gx: f32 = f.get(4)?.trim().parse().ok()?;
    let gy: f32 = f.get(5)?.trim().parse().ok()?;
    let gz: f32 = f.get(6)?.trim().parse().ok()?;
    Some(ImuSample {
        accel_g: [ax, ay, az],
        gyro_dps: [gx, gy, gz],
    })
}

/// TOLERANT WAN-status fold. Strips the HTML, then parses the extended status table's
/// per-interface rows: each cellular A/B section yields a full [`CellLink`]
/// (signal/technology/SIM/carrier/WAN-IP), and the section carrying an `IP Address` is
/// the active uplink. Degrades to a `gaps` note for anything genuinely absent (never a
/// fabricated value, §7). The simplified/general format (an explicit `Active WAN` label
/// + a single `dBm` reading, no per-modem rows) still folds through the fallbacks.
fn parse_wan(html: &str, gaps: &mut Vec<String>) -> WanStatus {
    let text = strip_html(html);
    let mut wan = WanStatus::default();

    // The extended table's per-interface sections (empty on the simplified format).
    let sections = wan_sections(&text);
    let section = |label: &str| sections.iter().find(|(l, _)| *l == label).map(|(_, s)| *s);

    // ── Cellular A / B — a full per-modem link when the extended rows are present ──
    if let Some(s) = section("Cellular A") {
        wan.cellular_a = parse_cell_link(s);
    } else {
        // Simplified format: fold the single dBm reading into cellular A best-effort.
        match find_signal_dbm(&text) {
            Some(dbm) => {
                wan.cellular_a.signal_dbm = dbm;
                wan.cellular_a.healthy = dbm > -110;
            }
            None => gaps.push("wan.cellular_a signal_dbm not reported".to_string()),
        }
    }
    if let Some(s) = section("Cellular B") {
        wan.cellular_b = parse_cell_link(s);
    }

    // ── active WAN — the explicit label (simplified) or the IP-bearing section ──
    if let Some(v) =
        find_token_after(&text, "Active WAN").or_else(|| find_token_after(&text, "Active Link"))
    {
        wan.active_wan = v;
    } else if let Some((label, _)) = sections.iter().find(|(_, s)| s.contains("IP Address")) {
        wan.active_wan = (*label).to_string();
    } else {
        gaps.push("wan.active_wan not reported".to_string());
    }

    // ── Ethernet / Wi-Fi state — derived from their extended section (active vs a
    // present-but-backup standby), else the simplified label, else a gap ──
    match section("Ethernet") {
        Some(_) => {
            wan.ethernet_state = if wan.active_wan == "Ethernet" {
                "active".to_string()
            } else {
                "standby".to_string()
            };
        }
        None => match find_token_after(&text, "Ethernet") {
            Some(v) => wan.ethernet_state = v,
            None => gaps.push("wan.ethernet_state not reported".to_string()),
        },
    }
    match section("WiFi") {
        Some(_) => {
            wan.wifi_state = if wan.active_wan == "WiFi" {
                "active".to_string()
            } else {
                "standby".to_string()
            };
        }
        None => {
            match find_token_after(&text, "Wi-Fi").or_else(|| find_token_after(&text, "Wifi")) {
                Some(v) => wan.wifi_state = v,
                None => gaps.push("wan.wifi_state not reported".to_string()),
            }
        }
    }
    match find_token_after(&text, "VPN") {
        Some(v) => wan.vpn_state = v,
        None => gaps.push("wan.vpn_state not reported".to_string()),
    }
    wan
}

/// The extended WAN table's per-interface section markers: `(section-label, needle)`.
/// Each WAN row starts with a device descriptor carrying one of these needles (e.g.
/// `... (Cellular A)`, `Panel Ethernet 5`, `... PCIe WiFi A`).
const WAN_SECTION_MARKERS: &[(&str, &str)] = &[
    ("Cellular A", "Cellular A"),
    ("Cellular B", "Cellular B"),
    ("Ethernet", "Panel Ethernet"),
    ("WiFi", "WiFi A"),
];

/// Slice the stripped WAN text into per-interface sections (document order). Each
/// section runs from its marker to the start of the next present marker, so a label
/// scan within a section stays scoped to that one interface's row.
fn wan_sections(text: &str) -> Vec<(&'static str, &str)> {
    let mut found: Vec<(&'static str, usize)> = WAN_SECTION_MARKERS
        .iter()
        .filter_map(|(label, needle)| text.find(needle).map(|i| (*label, i)))
        .collect();
    found.sort_by_key(|&(_, i)| i);
    let mut out = Vec::with_capacity(found.len());
    for k in 0..found.len() {
        let (label, start) = found[k];
        let end = found.get(k + 1).map_or(text.len(), |&(_, i)| i);
        out.push((label, &text[start..end]));
    }
    out
}

/// Fold one cellular section (scoped to a single modem's extended row) into a
/// [`CellLink`]: the primary RSSI dBm, the RAT, the SIM presence, the carrier, and
/// the WAN IP (present ⇒ this modem is the active uplink). Honest defaults for
/// anything the section omits.
fn parse_cell_link(section: &str) -> CellLink {
    let signal_dbm = rssi_dbm_in(section).unwrap_or(0);
    let sim_present = find_token_after(section, "SIM ID").is_some();
    let sim_state = if sim_present { "ready" } else { "absent" }.to_string();
    let carrier = find_token_after(section, "Carrier PRI ID").unwrap_or_default();
    let technology = if section.contains("5G") {
        "5G"
    } else if section.contains("LTE") {
        "LTE"
    } else {
        ""
    }
    .to_string();
    let wan_ip =
        find_token_after(section, "IP Address").unwrap_or_else(|| "not active".to_string());
    let healthy = signal_dbm > -110 && sim_present;
    CellLink {
        sim_state,
        carrier,
        signal_dbm,
        technology,
        wan_ip,
        healthy,
    }
}

/// The primary RSSI reading (dBm) in a cellular section: the FIRST `dBm` value after
/// the `RSSI` label, e.g. `RSSI  -98.0dBm / -102.0dBm` ⇒ `-98`. Parses the leading
/// signed (possibly-decimal) number before `dBm`, truncated to a whole dBm. `None`
/// when the section has no `RSSI` reading.
fn rssi_dbm_in(section: &str) -> Option<i32> {
    let rssi = section.find("RSSI")?;
    let after = &section[rssi..];
    let dbm = after.find("dBm")?;
    let prefix = &after[..dbm];
    // Walk back over the trailing float run (digits, one '.', a leading '-').
    let tail: String = prefix
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    let num: String = tail.chars().rev().collect();
    num.trim().parse::<f32>().ok().map(|f| f as i32)
}

// ─────────────────────────── tolerant HTML extractors ───────────────────────────

/// Replace every `<...>` tag with a space so a label→value scan works over the
/// text content (e.g. `Foo </td><td> 12.3` ⇒ `Foo    12.3`).
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if in_tag => {}
            _ => out.push(c),
        }
    }
    out
}

/// The first number appearing AFTER `label` (optional sign, digits, one dot),
/// ignoring any non-numeric run (tags-turned-spaces, a leading unit) between them.
/// A trailing unit (e.g. the `v` in `12.60v`) is not consumed. `None` when the label
/// is absent or no number follows it.
fn find_number_after(text: &str, label: &str) -> Option<f32> {
    let idx = text.find(label)?;
    let rest = &text[idx + label.len()..];
    let bytes = rest.as_bytes();
    let mut i = 0;
    // Find the start of a numeric run (a digit, or a '-' immediately before one).
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_digit() || (c == b'-' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            break;
        }
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let start = i;
    if bytes[i] == b'-' {
        i += 1;
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    rest[start..i].parse::<f32>().ok()
}

/// The first whitespace-delimited token appearing after `label` (the value cell in a
/// stripped `Label </td><td> VALUE` row). `None` when the label is absent or nothing
/// non-whitespace follows.
fn find_token_after(text: &str, label: &str) -> Option<String> {
    let idx = text.find(label)?;
    let rest = text[idx + label.len()..].trim_start();
    let tok: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    if tok.is_empty() {
        None
    } else {
        Some(tok)
    }
}

/// The signed integer immediately preceding the first `dBm` token (e.g. `-72 dBm` ⇒
/// `-72`). `None` when there is no `dBm` reading.
fn find_signal_dbm(text: &str) -> Option<i32> {
    let idx = text.find("dBm")?;
    let prefix = text[..idx].trim_end();
    // Walk back over the trailing digit/sign run, then parse it forwards.
    let tail: String = prefix
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    let num: String = tail.chars().rev().collect();
    num.parse::<i32>().ok()
}

/// Wall-clock milliseconds since the Unix epoch (the mirror stamp).
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canned `omgconf latest <file>` YAML the fake SSH seam returns for
    /// `get-config`.
    const FAKE_YAML: &str = "gateway:\n  mode: failover\nwan:\n  primary: cellular-a\n";

    /// The extended MG-LCI `wan/status` structure (tags already stripped, per the
    /// live layout): a per-modem A/B cellular table + a panel-ethernet + a Wi-Fi row.
    /// Cellular A carries the `IP Address` (the active uplink); B is SIM-ready but
    /// idle. Fed straight through `strip_html` (a no-op on tag-free text).
    const WAN_EXTENDED: &str = "\
Sierra Wireless EM75XX @ MiniCard USB3 CA (Cellular A)   Cellular   IP Address 100.65.12.34   \
Cellular Info   SIM ID 8901410123456789012   LTE   Band Number 4   Bandwidth 20MHz   \
RSSI  -98.0dBm / -102.0dBm   RSRP  -123.0dBm / -131.0dBm   Carrier PRI ID 9990198   LTE   \
Panel Ethernet 5   Ethernet   Standby   \
Sierra Wireless EM75XX @ MiniCard USB3 CB (Cellular B)   Cellular   Cellular Info   \
SIM ID 8901410987654321098   RSSI  -105.0dBm / -110.0dBm   Carrier PRI ID 9990199   LTE   \
WLE900VX 802.11AC @ MiniCard PCIe WiFi A   WiFi   Disabled";

    /// A scripted fake probe: each read yields a canned `Ok(text)` or `Err(msg)`, and
    /// `run_ssh` returns [`Self::ssh_out`] while recording the command in
    /// [`Self::ssh_calls`] (shared through the `Arc` across clones, so a test asserts
    /// what ran). (`Result<String, String>` is `Clone`, unlike `io::Result`, so the
    /// fixtures are reusable across the per-read calls `build_state` makes.)
    #[derive(Clone)]
    struct FakeProbe {
        nmea: Result<String, String>,
        general: Result<String, String>,
        wan: Result<String, String>,
        ssh_out: Result<String, String>,
        ssh_calls: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl FakeProbe {
        /// The captured bench-MG90 fixtures — a no-lock GGA + a real IMU line, and
        /// the general.html rows carrying battery/temp/esn/version.
        fn real() -> Self {
            let nmea = "$GPGGA,111504.000,3210.07993,N,09550.95445,W,0,00,99.0,081.94,M,-24.2,M,,*66\n\
                        $PSIWMMPU,49.050,0.25218,0.12537,-10.02395,-3.39966,-0.99182,-0.90637,*3C\n"
                .to_string();
            let general = "<table>\
                <tr><td>Model </td><td> MG90</td></tr>\
                <tr><td>ESN </td><td> ND84720078011035</td></tr>\
                <tr><td>Version </td><td> 4.3.0.1</td></tr>\
                <tr><td>Main Battery Voltage </td><td> 12.60v</td></tr>\
                <tr><td>Internal Temperature </td><td> 33.89</td></tr>\
                </table>"
                .to_string();
            let wan = "<table>\
                <tr><td>Active WAN </td><td> CellularA</td></tr>\
                <tr><td>Wi-Fi </td><td> Disabled</td></tr>\
                <tr><td>Ethernet </td><td> Down</td></tr>\
                <tr><td>VPN </td><td> Connected</td></tr>\
                <tr><td>Signal </td><td> -72 dBm</td></tr>\
                </table>"
                .to_string();
            Self {
                nmea: Ok(nmea),
                general: Ok(general),
                wan: Ok(wan),
                ssh_out: Ok(FAKE_YAML.to_string()),
                ssh_calls: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        /// The commands `run_ssh` has been asked to run (a shared, clone-stable log).
        fn ssh_calls(&self) -> Vec<String> {
            self.ssh_calls.lock().unwrap().clone()
        }
    }

    fn to_io(r: &Result<String, String>) -> io::Result<String> {
        r.clone()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    impl VehicleProbe for FakeProbe {
        fn read_gps_nmea(&self) -> io::Result<String> {
            to_io(&self.nmea)
        }
        fn read_lci_general(&self) -> io::Result<String> {
            to_io(&self.general)
        }
        fn read_lci_wan(&self) -> io::Result<String> {
            to_io(&self.wan)
        }
        fn run_ssh(&self, cmd: &str) -> io::Result<String> {
            self.ssh_calls.lock().unwrap().push(cmd.to_string());
            to_io(&self.ssh_out)
        }
    }

    fn worker() -> VehicleWorker {
        VehicleWorker::new("rig-1".to_string()).with_bus_root(None)
    }

    #[test]
    fn builds_state_from_real_fixtures() {
        let probe = FakeProbe::real();
        let state = worker().build_state(&probe);

        assert!(state.online, "a reachable LCI is online");
        assert_eq!(state.host, "rig-1");

        // GPS — the captured no-lock GGA (quality 0 / 0 sats).
        assert_eq!(state.gps.satellites, 0);
        assert!(!state.gps.has_fix(), "quality 0 / 0 sats ⇒ no lock");
        assert!((state.gps.altitude_m - 81.94).abs() < 0.01);

        // IMU — the $PSIWMMPU accel/gyro parsed (non-zero, honest values).
        let imu = state.imu.expect("IMU sample parsed");
        assert!(
            (imu.accel_g[0] - 0.25218).abs() < 1e-4,
            "ax {}",
            imu.accel_g[0]
        );
        assert!(
            (imu.accel_g[2] + 10.02395).abs() < 1e-4,
            "az {}",
            imu.accel_g[2]
        );
        assert!(
            (imu.gyro_dps[0] + 3.39966).abs() < 1e-4,
            "gx {}",
            imu.gyro_dps[0]
        );

        // general.html — battery / temp / esn / version / model.
        assert!(
            (state.telem.battery_v - 12.60).abs() < 0.01,
            "battery {}",
            state.telem.battery_v
        );
        assert!(
            (state.telem.internal_temp_c - 33.89).abs() < 0.01,
            "temp {}",
            state.telem.internal_temp_c
        );
        assert_eq!(state.esn, "ND84720078011035");
        assert_eq!(state.mgos_version, "4.3.0.1");
        assert_eq!(state.model, "MG90");

        // WAN — the tolerant fold picked up the states + the signal.
        assert_eq!(state.wan.active_wan, "CellularA");
        assert_eq!(state.wan.vpn_state, "Connected");
        assert_eq!(state.wan.cellular_a.signal_dbm, -72);

        // Honest-partial: OBD/ignition is explicitly gapped, never fabricated.
        assert!(
            state.gaps.iter().any(|g| g.contains("ignition/OBD")),
            "ignition/OBD is honestly gapped: {:?}",
            state.gaps
        );
        assert!(!state.telem.obd_present);
        assert!(!state.telem.ignition_on);
    }

    #[test]
    fn probe_error_yields_offline_snapshot() {
        // The LCI general read is the reachability anchor — its failure ⇒ offline.
        let probe = FakeProbe {
            general: Err("connection refused".to_string()),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);
        assert!(!state.online, "an unreachable gateway is offline");
        assert!(!state.gps.has_fix());
        assert!(
            state.gaps.iter().any(|g| g.contains("unreachable")),
            "offline snapshot carries the honest gap: {:?}",
            state.gaps
        );
        assert!(state.published_at_ms > 0, "offline mirror is still stamped");
    }

    #[test]
    fn partial_reads_degrade_to_gaps_not_offline() {
        // GPS (SSH) + WAN (HTTP) fail, but the anchor LCI general succeeds — the
        // mirror stays online with honest gaps rather than blanking.
        let probe = FakeProbe {
            nmea: Err("ssh: connect timeout".to_string()),
            wan: Err("curl: 28".to_string()),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);
        assert!(state.online, "the anchor read succeeded ⇒ online");
        assert!(state.imu.is_none());
        assert!(state.gaps.iter().any(|g| g.contains("gps/imu unavailable")));
        assert!(state
            .gaps
            .iter()
            .any(|g| g.contains("wan status unavailable")));
        // The general.html data still landed.
        assert_eq!(state.esn, "ND84720078011035");
    }

    #[test]
    fn parse_endpoint_splits_ip_and_ssh_port() {
        assert_eq!(
            parse_endpoint("192.168.13.31"),
            ("192.168.13.31".to_string(), 22)
        );
        assert_eq!(
            parse_endpoint("192.168.13.31:2222"),
            ("192.168.13.31".to_string(), 2222)
        );
        // An unparsable suffix falls back to the whole string + default port.
        assert_eq!(
            parse_endpoint("host.local:ssh"),
            ("host.local:ssh".to_string(), 22)
        );
    }

    #[test]
    fn tolerant_extractors_ignore_tags_and_units() {
        let t = strip_html("<td>Main Battery Voltage </td><td> 12.60v</td>");
        assert!((find_number_after(&t, "Main Battery Voltage").unwrap() - 12.60).abs() < 0.01);
        assert_eq!(
            find_token_after(&t, "Main Battery Voltage"),
            Some("12.60v".to_string())
        );
        assert_eq!(find_signal_dbm("Signal   -72 dBm"), Some(-72));
        assert!(find_number_after("no number here", "Label").is_none());
    }

    #[tokio::test]
    async fn run_loop_publishes_then_exits_promptly_on_shutdown() {
        let mut w = worker()
            .with_probe(Arc::new(FakeProbe::real()))
            .with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }

    #[tokio::test]
    async fn no_gateway_worker_idles_without_publishing() {
        // No probe (the MDE_VEHICLE_GATEWAY-unset path) ⇒ idle until shutdown.
        let mut w = worker();
        w.probe = None;
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "the idle worker still exits promptly");
        assert!(joined.unwrap().expect("join").is_ok());
    }

    // ─────────────────────── Change 1 · per-modem cellular A/B parser ───────────────────────

    #[test]
    fn parses_per_modem_cellular_a_b_from_extended_wan_status() {
        let probe = FakeProbe {
            wan: Ok(WAN_EXTENDED.to_string()),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);

        // Cellular A — the active modem (carries the WAN IP).
        assert_eq!(state.wan.cellular_a.signal_dbm, -98, "primary RSSI dBm");
        assert_eq!(state.wan.cellular_a.technology, "LTE");
        assert_eq!(state.wan.cellular_a.sim_state, "ready");
        assert_eq!(state.wan.cellular_a.carrier, "9990198");
        assert_eq!(state.wan.cellular_a.wan_ip, "100.65.12.34");
        assert!(state.wan.cellular_a.healthy, "-98 dBm + SIM ⇒ healthy");

        // Cellular B — SIM-ready but idle (no IP Address ⇒ not active).
        assert_eq!(state.wan.cellular_b.signal_dbm, -105);
        assert_eq!(state.wan.cellular_b.sim_state, "ready");
        assert_eq!(state.wan.cellular_b.wan_ip, "not active");

        // The active WAN is derived from the IP-bearing section.
        assert_eq!(state.wan.active_wan, "Cellular A");
        assert_eq!(state.wan.active_cellular().map(|c| c.signal_dbm), Some(-98));

        // The now-satisfied "per-modem … not yet parsed" gap is gone.
        assert!(
            !state.gaps.iter().any(|g| g.contains("not yet parsed")),
            "stale per-modem gap removed: {:?}",
            state.gaps
        );
    }

    #[test]
    fn old_simplified_wan_fixture_still_folds_through_the_fallback() {
        // The simplified format (explicit `Active WAN` + a single `Signal … dBm`)
        // has no per-modem rows — it must still fold through the fallbacks.
        let state = worker().build_state(&FakeProbe::real());
        assert_eq!(state.wan.active_wan, "CellularA");
        assert_eq!(state.wan.cellular_a.signal_dbm, -72);
        assert_eq!(state.wan.vpn_state, "Connected");
        assert_eq!(state.wan.ethernet_state, "Down");
        assert_eq!(state.wan.wifi_state, "Disabled");
    }

    // ─────────────────────── Change 2 · action/vehicle/* control drain ───────────────────────

    #[test]
    fn get_config_returns_the_committed_yaml_over_ssh() {
        let fake = FakeProbe::real();
        let w = worker().with_probe(Arc::new(fake.clone()));
        let reply = w.handle("get-config", r#"{"file":"wan.yaml"}"#);
        assert!(reply.ok, "gated: {:?} err: {:?}", reply.gated, reply.error);
        assert_eq!(reply.applied.as_deref(), Some(FAKE_YAML));
        assert_eq!(fake.ssh_calls().as_slice(), &["omgconf latest wan.yaml"]);
    }

    #[test]
    fn get_config_rejects_a_non_bare_yaml_or_path_traversal() {
        let fake = FakeProbe::real();
        let w = worker().with_probe(Arc::new(fake.clone()));
        for bad in [
            "../etc/passwd.yaml",
            "/etc/wan.yaml",
            "sub/wan.yaml",
            "wan.txt",
            "wan",
            "..yaml",
        ] {
            let reply = w.handle("get-config", &format!(r#"{{"file":"{bad}"}}"#));
            assert!(!reply.ok, "`{bad}` must be rejected");
            assert!(reply.error.is_some(), "`{bad}` is an honest error");
        }
        // A missing `file` is an honest error, too.
        assert!(!w.handle("get-config", "{}").ok);
        // Nothing was ever shelled for the rejected inputs.
        assert!(fake.ssh_calls().is_empty(), "no rejected input reached ssh");
    }

    #[test]
    fn reboot_without_typed_name_is_gated_and_runs_no_ssh() {
        let fake = FakeProbe::real();
        let w = worker().with_probe(Arc::new(fake.clone()));
        let reply = w.handle("reboot", "{}");
        assert!(!reply.ok);
        assert!(
            reply.gated.unwrap().contains("typed-arm"),
            "a bare reboot is typed-arm gated"
        );
        assert!(!reply.audited);
        assert!(fake.ssh_calls().is_empty(), "a gated reboot never runs ssh");
    }

    #[test]
    fn reboot_with_the_wrong_typed_name_is_gated() {
        let fake = FakeProbe::real();
        let w = worker().with_probe(Arc::new(fake.clone()));
        // A typed_name that is NOT the gateway ESN does not arm.
        let reply = w.handle("reboot", r#"{"typed_name":"reboot"}"#);
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("typed-arm"));
        assert!(fake.ssh_calls().is_empty());
    }

    #[test]
    fn reboot_with_the_correct_esn_performs_and_audits() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeProbe::real();
        let w = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_db_path(tmp.path().join("events.sqlite"));
        // The FakeProbe general.html reports ESN ND84720078011035.
        let reply = w.handle("reboot", r#"{"typed_name":"ND84720078011035"}"#);
        assert!(reply.ok, "gated: {:?} err: {:?}", reply.gated, reply.error);
        assert!(reply.audited, "a performed reboot is audited");
        assert_eq!(reply.applied.as_deref(), Some("reboot issued"));
        assert_eq!(fake.ssh_calls().as_slice(), &["reboot"]);
    }

    #[test]
    fn verbs_gate_when_this_node_has_no_gateway() {
        let mut w = worker();
        w.probe = None; // the MDE_VEHICLE_GATEWAY-unset node.
        let reply = w.handle("get-config", r#"{"file":"wan.yaml"}"#);
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("no gateway on this node"));
    }

    #[test]
    fn an_unknown_verb_is_an_honest_error() {
        let w = worker().with_probe(Arc::new(FakeProbe::real()));
        let reply = w.handle("frobnicate", "{}");
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("unknown vehicle verb"));
    }

    #[tokio::test]
    async fn drain_answers_get_config_on_the_reply_topic() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        let req = persist
            .write(
                "action/vehicle/get-config",
                Priority::Default,
                None,
                Some(r#"{"file":"wan.yaml"}"#),
            )
            .unwrap();
        let w = worker()
            .with_probe(Arc::new(FakeProbe::real()))
            .with_bus_root(Some(bus.clone()));
        let mut cursors = HashMap::new();
        assert!(w.drain_actions(&mut cursors), "the gateway node acted");
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        assert_eq!(replies.len(), 1, "exactly one reply");
        let reply: VehicleReply =
            serde_json::from_str(replies[0].body.as_deref().unwrap()).unwrap();
        assert!(reply.ok);
        assert_eq!(reply.applied.as_deref(), Some(FAKE_YAML));
    }

    #[tokio::test]
    async fn prime_cursors_skips_the_backlog_so_a_restart_does_not_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        persist
            .write(
                "action/vehicle/get-config",
                Priority::Default,
                None,
                Some(r#"{"file":"wan.yaml"}"#),
            )
            .unwrap();
        let w = worker()
            .with_probe(Arc::new(FakeProbe::real()))
            .with_bus_root(Some(bus.clone()));
        let mut cursors = HashMap::new();
        w.prime_cursors(&mut cursors);
        assert!(
            !w.drain_actions(&mut cursors),
            "the backlog is not replayed after prime"
        );
    }
}
