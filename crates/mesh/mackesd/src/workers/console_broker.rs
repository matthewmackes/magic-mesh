//! VDI-VM-1 — `console_broker`: makes a **local** KVM VM's loopback console
//! reachable on the mesh so a remote peer can actually attach frames.
//!
//! ## The gap this closes
//!
//! Every VM `vm_lifecycle` defines binds its SPICE graphics to `127.0.0.1`
//! ([`super::vm_lifecycle::build_domain_xml`] — `<listen address='127.0.0.1'/>`,
//! autoport). That console is invisible off the host: [`super::desktop_sources`]
//! advertises the VM with a **port-less** Spice offer (`ProtocolOffer{port:None}`)
//! and [`super::session_broker`] tracks the session lifecycle but publishes **no
//! reachable endpoint**. So a peer's Chooser can open a broker session for a
//! local VM yet never has a `host:port` to dial — the "use a VM desktop on any
//! mesh peer" promise silently doesn't deliver frames for local VMs.
//!
//! ## The fix (approach (a): relay onto the overlay, reusing the proven pattern)
//!
//! On the **serving** peer, for each VDI `Open` that names a VM this node serves,
//! this worker:
//!
//! 1. **Resolves** the live console via `virsh domdisplay <vm>` (SPICE first, VNC
//!    fallback) — the concrete autoport libvirt actually assigned.
//! 2. **Relays** that loopback port onto the Nebula overlay (`nebula1`) with a
//!    scoped `socat` proxy — exactly how [`super::compute_expose`] forwards a VM
//!    port onto the overlay, but for a host-loopback console rather than a
//!    firewalld forward to a VM's own overlay IP (a local VM's graphics live on
//!    the *host's* loopback, so a userspace relay is the right shape).
//! 3. **Publishes** the resulting overlay `host:port` back on the session record
//!    ([`CONSOLE_TOPIC`], keyed by the globally-unique session id) so the client
//!    peer's shell resolves the brokered endpoint from the record instead of
//!    needing a discovery-time port it never had.
//!
//! Serving-peer-gated, **not** leader-gated (unlike `session_broker`'s
//! convergence): the relay and the loopback console are physically on the serving
//! host, so brokering must run there.
//!
//! ## Honest non-connectability (§7)
//!
//! If a real reachable endpoint cannot be brokered — the VM is shut off (no live
//! console), it has no graphics, `virsh`/`socat` is absent, or the overlay isn't
//! up — the worker publishes a typed [`ConsoleStatus::Unbrokerable`] with the
//! reason, **never a fake endpoint**. The shell greys that lane honestly rather
//! than attaching a transport that can't deliver frames.
//!
//! ## Live gate
//!
//! The brokering ORCHESTRATION (fold `action/vdi/session`, publish, teardown) and
//! all the pure parsing/relay-arg/serving-peer logic are unit-tested here headless
//! over an injected [`ConsoleRelay`]. The end-to-end proof — broker a real local
//! VM's console on one peer and connect from another — needs two live nodes and a
//! running VM, and is deferred to the operator's two-node test bed.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::desktop_sources::DesktopProtocol;
use super::proc::{output_with_timeout, DEFAULT_CMD_TIMEOUT};
use super::scheduler::NodeId;
use super::session_broker::{parse_request, SessionId, SessionRequest, VmId};
use super::{ShutdownToken, Worker};

/// The session-lifecycle topic this worker folds (the same log
/// [`super::session_broker`] drains). Two independent cursors on one
/// mesh-replicated log — the established pattern.
pub const SESSION_TOPIC: &str = super::session_broker::ACTION_TOPIC;

/// The retained topic the brokered-console records are published to. Keyed by the
/// globally-unique session id inside the record (NOT the topic), so a peer's shell
/// resolves purely by session id — no dependence on the serving node's id spelling
/// (`peer:host` vs bare `host` vs an overlay IP all differ across the discovery
/// lanes).
pub const CONSOLE_TOPIC: &str = "state/vdi/console";

/// Nebula overlay interface — the relay's bind address comes from here (matches
/// [`super::compute_expose::DEFAULT_NEBULA_INTERFACE`]).
pub const DEFAULT_NEBULA_INTERFACE: &str = "nebula1";

/// Fold/broker cadence. A session is a slow, human-paced event, so a 2 s poll is
/// responsive without spinning virsh (the same cadence `session_broker` drains at).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Unconditional-republish heartbeat so a late shell subscriber / freshly-pruned
/// topic still finds a live console record while a connect is pending.
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(30);

// ───────────────────────────── pure: data model ─────────────────────────────

/// A resolved live console address on the serving host (the loopback endpoint
/// `virsh domdisplay` reports).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleAddr {
    /// The console protocol (SPICE or VNC).
    pub protocol: DesktopProtocol,
    /// The host libvirt bound the console to (loopback for a local VM).
    pub host: String,
    /// The concrete TCP port libvirt autoported.
    pub port: u16,
}

/// The brokered-console status published back on the session record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConsoleStatus {
    /// A reachable overlay endpoint was brokered — the shell dials `host:port`.
    Brokered {
        /// The transport protocol.
        protocol: DesktopProtocol,
        /// The Nebula overlay address the relay listens on.
        host: String,
        /// The overlay port the relay listens on.
        port: u16,
    },
    /// No reachable endpoint could be brokered — the honest reason (VM off, no
    /// graphics, no `socat`, overlay down, …). The shell greys the lane; it NEVER
    /// attaches a transport (§7).
    Unbrokerable {
        /// Human-readable reason surfaced on the greyed card.
        reason: String,
    },
}

/// One brokered-console record — published to [`CONSOLE_TOPIC`], resolved by the
/// client shell by matching `session_id`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BrokeredConsole {
    /// The session this console serves (the globally-unique correlation key).
    pub session_id: SessionId,
    /// The peer that brokered it (this node's id — for logs/debug).
    pub serving_node: NodeId,
    /// The VM whose console was brokered (libvirt domain name).
    pub vm_id: VmId,
    /// The brokered endpoint, or the honest reason none could be brokered.
    pub status: ConsoleStatus,
}

/// A typed console-broker failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleBrokerError {
    /// A prerequisite tool is absent on this box (`virsh` / `socat`) or the
    /// overlay isn't up — the honest headless/degraded gate.
    Gated(String),
    /// The console couldn't be resolved (VM off, no graphics, domain not found).
    Resolve(String),
    /// The relay couldn't be started (socat spawn failed).
    Relay(String),
}

impl std::fmt::Display for ConsoleBrokerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gated(m) => write!(f, "gated: {m}"),
            Self::Resolve(m) => write!(f, "unresolved: {m}"),
            Self::Relay(m) => write!(f, "relay: {m}"),
        }
    }
}

impl std::error::Error for ConsoleBrokerError {}

impl ConsoleBrokerError {
    /// The operator-facing reason folded into [`ConsoleStatus::Unbrokerable`].
    #[must_use]
    pub fn reason(&self) -> String {
        self.to_string()
    }
}

// ───────────────────────────── pure: parsers ─────────────────────────────

/// `true` for a loopback host — the console is host-local and needs relaying onto
/// the overlay to be reachable off-box.
#[must_use]
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim();
    h == "localhost" || h == "::1" || h == "[::1]" || h.starts_with("127.")
}

/// Strip a `peer:` identity prefix (the `default_node_id` spelling) so a bare
/// hostname and a `peer:<hostname>` compare equal.
fn strip_peer_prefix(s: &str) -> &str {
    s.strip_prefix("peer:").unwrap_or(s)
}

/// Whether an `Open`'s `serving_peer` names THIS node. The discovery lanes spell a
/// serving peer three ways — bare `hostname` (peer-advertised VM), `peer:<hostname>`
/// (local node id), or the overlay IP (a future registry) — so all three match.
/// A cheap pre-filter; the authority is whether `virsh domdisplay` resolves the VM
/// locally (a non-local domain simply fails to resolve and is skipped).
#[must_use]
pub fn serves_here(serving_peer: &str, node_id: &str, overlay_addr: &str) -> bool {
    let want = strip_peer_prefix(serving_peer.trim());
    let me = strip_peer_prefix(node_id.trim());
    want.eq_ignore_ascii_case(me) || (!overlay_addr.is_empty() && want == overlay_addr.trim())
}

/// Parse a `virsh domdisplay` URI into a [`ConsoleAddr`].
///
/// Handles the shapes libvirt emits: `spice://127.0.0.1:5900`,
/// `spice://localhost?port=5900&tls-port=5901`, `vnc://127.0.0.1:5901`, and the
/// legacy VNC display-number form `vnc://127.0.0.1:0` (→ 5900). Returns `None` for
/// empty output (no graphics / VM off) or a URI with no derivable port.
#[must_use]
pub fn parse_domdisplay(stdout: &str) -> Option<ConsoleAddr> {
    let line = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    let (scheme, rest) = line.split_once("://")?;
    let protocol = match scheme.to_ascii_lowercase().as_str() {
        "spice" => DesktopProtocol::Spice,
        "vnc" | "rfb" => DesktopProtocol::Vnc,
        "rdp" => DesktopProtocol::Rdp,
        _ => return None,
    };
    // Split off an optional `?query` (carries `port=` / `tls-port=`).
    let (authority, query) = match rest.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (rest, None),
    };
    // Prefer an explicit `port=` in the query, else the `:port` in the authority.
    let mut port: Option<u16> = None;
    let mut host = authority.to_string();
    if let Some((h, p)) = authority.rsplit_once(':') {
        // Guard against an IPv6 authority without a port (`[::1]`): only treat the
        // suffix as a port when it parses.
        if let Ok(n) = p.parse::<u16>() {
            host = h.to_string();
            port = Some(n);
        }
    }
    if let Some(q) = query {
        for kv in q.split('&') {
            if let Some(val) = kv.strip_prefix("port=") {
                if let Ok(n) = val.parse::<u16>() {
                    port = Some(n);
                }
            }
        }
    }
    let mut port = port?;
    if host.trim().is_empty() {
        return None;
    }
    // Legacy VNC display-number form (`vnc://host:0` = display 0 = TCP 5900+0).
    // SPICE always reports a real autoport, so this only applies to VNC.
    if protocol == DesktopProtocol::Vnc && port < 1024 {
        port = 5900u16.saturating_add(port);
    }
    Some(ConsoleAddr {
        protocol,
        host,
        port,
    })
}

/// The overlay port the relay listens on for a given loopback console port. A 1:1
/// mapping (the overlay address is distinct from loopback, so there's no
/// collision) — the same 1:1 host↔guest choice [`super::compute_expose`] makes.
#[must_use]
pub const fn overlay_port_for(console_port: u16) -> u16 {
    console_port
}

/// Build the `socat` args that relay `overlay_addr:overlay_port` (on the Nebula
/// interface) to the host-loopback console `target`. `fork` handles reconnects,
/// `reuseaddr` survives a quick relay restart. Scoped exactly like
/// `compute_expose`'s per-VM forward: the listen side is bound to the overlay
/// address only, so the relay is reachable ONLY on the mesh, never the LAN/WAN.
#[must_use]
pub fn build_relay_args(
    overlay_addr: &str,
    overlay_port: u16,
    target_host: &str,
    target_port: u16,
) -> Vec<String> {
    vec![
        format!("TCP-LISTEN:{overlay_port},bind={overlay_addr},fork,reuseaddr"),
        format!("TCP:{target_host}:{target_port}"),
    ]
}

// ───────────────────────────── relay seam ─────────────────────────────

/// A live relay process handle — killed on drop/teardown so a closed session
/// leaves no dangling overlay listener. A fake (tests) holds no child.
#[derive(Debug, Default)]
pub struct RelayHandle {
    child: Option<Child>,
}

impl RelayHandle {
    /// Wrap a spawned relay child.
    #[must_use]
    pub fn from_child(child: Child) -> Self {
        Self { child: Some(child) }
    }

    /// A handle with no process (the test fake / a no-op teardown).
    #[must_use]
    pub const fn detached() -> Self {
        Self { child: None }
    }

    /// Kill the relay process (idempotent; a no-op for a detached handle).
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for RelayHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The injectable console-broker seam: resolve a VM's live console, know the local
/// overlay address, and start a relay. Production wires [`LiveConsoleRelay`]
/// (`virsh domdisplay` + `socat`); tests inject a fake so the whole fold → broker →
/// publish → teardown pipeline runs without KVM.
pub trait ConsoleRelay: Send + Sync {
    /// Resolve `vm_id`'s live console on this host. A shut-off / graphics-less /
    /// non-local VM yields a typed error (never a fabricated addr).
    ///
    /// # Errors
    /// [`ConsoleBrokerError::Gated`] when `virsh` is absent;
    /// [`ConsoleBrokerError::Resolve`] when the console can't be resolved.
    fn resolve(&self, vm_id: &str) -> Result<ConsoleAddr, ConsoleBrokerError>;

    /// This node's Nebula overlay address (empty when the overlay isn't up).
    fn overlay_addr(&self) -> String;

    /// Start the overlay relay for a resolved `target` console.
    ///
    /// # Errors
    /// [`ConsoleBrokerError::Gated`] when `socat` is absent;
    /// [`ConsoleBrokerError::Relay`] on a spawn failure.
    fn start_relay(
        &self,
        overlay_addr: &str,
        overlay_port: u16,
        target: &ConsoleAddr,
    ) -> Result<RelayHandle, ConsoleBrokerError>;
}

/// Production [`ConsoleRelay`]: `virsh domdisplay` + a `socat` overlay relay,
/// bounded by the EFF-20 proc timeout so a wedged virsh can't pin a thread.
#[derive(Debug, Clone)]
pub struct LiveConsoleRelay {
    nebula_interface: String,
}

impl Default for LiveConsoleRelay {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveConsoleRelay {
    /// Production defaults (relay binds the `nebula1` overlay).
    #[must_use]
    pub fn new() -> Self {
        Self {
            nebula_interface: DEFAULT_NEBULA_INTERFACE.to_string(),
        }
    }

    /// Run `virsh domdisplay [--type <t>] <vm>` and return its trimmed stdout.
    fn domdisplay(vm_id: &str, type_arg: Option<&str>) -> Option<String> {
        let mut cmd = Command::new("virsh");
        cmd.arg("domdisplay");
        if let Some(t) = type_arg {
            cmd.arg("--type").arg(t);
        }
        cmd.arg(vm_id);
        let out = output_with_timeout(cmd, DEFAULT_CMD_TIMEOUT).ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

impl ConsoleRelay for LiveConsoleRelay {
    fn resolve(&self, vm_id: &str) -> Result<ConsoleAddr, ConsoleBrokerError> {
        if !super::mesh_mount::binary_on_path("virsh") {
            return Err(ConsoleBrokerError::Gated(
                "virsh not found — no local hypervisor toolchain".to_string(),
            ));
        }
        // SPICE is what `vm_lifecycle`'s domain XML gives every guest; fall back to
        // VNC for a guest defined with a VNC console instead.
        let raw = Self::domdisplay(vm_id, None)
            .or_else(|| Self::domdisplay(vm_id, Some("vnc")))
            .ok_or_else(|| {
                ConsoleBrokerError::Resolve(format!(
                    "virsh domdisplay returned no console for `{vm_id}` (VM off or no graphics)"
                ))
            })?;
        parse_domdisplay(&raw).ok_or_else(|| {
            ConsoleBrokerError::Resolve(format!(
                "unparseable domdisplay output for `{vm_id}`: {raw}"
            ))
        })
    }

    fn overlay_addr(&self) -> String {
        local_nebula_addr(&self.nebula_interface)
    }

    fn start_relay(
        &self,
        overlay_addr: &str,
        overlay_port: u16,
        target: &ConsoleAddr,
    ) -> Result<RelayHandle, ConsoleBrokerError> {
        if !super::mesh_mount::binary_on_path("socat") {
            return Err(ConsoleBrokerError::Gated(
                "socat not found — cannot relay the console onto the overlay".to_string(),
            ));
        }
        let args = build_relay_args(overlay_addr, overlay_port, &target.host, target.port);
        Command::new("socat")
            .args(&args)
            .spawn()
            .map(RelayHandle::from_child)
            .map_err(|e| ConsoleBrokerError::Relay(format!("spawn socat: {e}")))
    }
}

/// Read the local Nebula overlay IPv4 from `ip -4 addr show <interface>` (empty
/// when the interface is absent / has no address). Mirrors the same private helper
/// in [`super::compute_expose`] / `compute_migrate` (the workers repeat it rather
/// than share a seam).
fn local_nebula_addr(interface: &str) -> String {
    let Ok(output) = Command::new("ip")
        .args(["-4", "addr", "show", interface])
        .output()
    else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(rest) = line.trim().strip_prefix("inet ") {
            if let Some(ip) = rest.split('/').next() {
                return ip.to_string();
            }
        }
    }
    String::new()
}

// ───────────────────────────── pure: broker decision ─────────────────────────────

/// Broker one open session's console into a publishable record (pure over the
/// injected `relay`). Resolves the console, checks it's loopback-local, resolves
/// the overlay address, and starts the relay — folding any failure into an honest
/// [`ConsoleStatus::Unbrokerable`] (never a fabricated endpoint, §7). On success
/// returns the [`ConsoleStatus::Brokered`] record plus the live [`RelayHandle`] the
/// caller must retain (dropping it tears the relay down).
#[must_use]
pub fn broker_console(
    relay: &dyn ConsoleRelay,
    vm_id: &str,
) -> (ConsoleStatus, Option<RelayHandle>) {
    let console = match relay.resolve(vm_id) {
        Ok(c) => c,
        Err(e) => return (ConsoleStatus::Unbrokerable { reason: e.reason() }, None),
    };
    // A non-loopback console is already reachable on whatever address libvirt bound
    // — publish it directly, no relay needed.
    if !is_loopback(&console.host) {
        return (
            ConsoleStatus::Brokered {
                protocol: console.protocol,
                host: console.host.clone(),
                port: console.port,
            },
            Some(RelayHandle::detached()),
        );
    }
    let overlay = relay.overlay_addr();
    if overlay.trim().is_empty() {
        return (
            ConsoleStatus::Unbrokerable {
                reason: "nebula overlay interface has no address (mesh not up)".to_string(),
            },
            None,
        );
    }
    let overlay_port = overlay_port_for(console.port);
    match relay.start_relay(&overlay, overlay_port, &console) {
        Ok(handle) => (
            ConsoleStatus::Brokered {
                protocol: console.protocol,
                host: overlay,
                port: overlay_port,
            },
            Some(handle),
        ),
        Err(e) => (ConsoleStatus::Unbrokerable { reason: e.reason() }, None),
    }
}

// ───────────────────────────── bus + worker ─────────────────────────────

/// One live brokered console: its published record plus the relay handle keeping
/// it up (dropped on session close).
struct BrokerEntry {
    record: BrokeredConsole,
    _relay: Option<RelayHandle>,
}

/// Read new [`SESSION_TOPIC`] messages since `cursor`, advancing it. A short
/// sync open-read-drop, mirroring `session_broker::read_new_actions`.
fn read_new_actions(bus_root: &Path, cursor: &mut Option<String>) -> Vec<SessionRequest> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(SESSION_TOPIC, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_request(body) {
            Ok(r) => out.push(r),
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "console_broker: bad session request");
            }
        }
    }
    out
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// The console-broker worker. Serving-peer-gated + best-effort.
pub struct ConsoleBrokerWorker {
    /// This node's id (stamped on the published record + the `serves_here` match).
    node_id: NodeId,
    /// The injectable relay seam (production: [`LiveConsoleRelay`]).
    relay: Box<dyn ConsoleRelay>,
    /// Fold/broker cadence.
    poll: Duration,
    /// Unconditional-republish heartbeat.
    heartbeat: Duration,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
    /// Live brokered consoles, keyed by session id.
    brokered: HashMap<SessionId, BrokerEntry>,
}

impl ConsoleBrokerWorker {
    /// Construct with production defaults: the `virsh`+`socat` [`LiveConsoleRelay`].
    #[must_use]
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            relay: Box::new(LiveConsoleRelay::new()),
            poll: DEFAULT_POLL_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            bus_root_override: None,
            brokered: HashMap::new(),
        }
    }

    /// Inject a relay seam (tests). Production uses [`LiveConsoleRelay`].
    #[must_use]
    pub fn with_relay(mut self, relay: Box<dyn ConsoleRelay>) -> Self {
        self.relay = relay;
        self
    }

    /// Override the fold cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the Bus root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    fn bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override.clone().or_else(default_bus_root)
    }

    /// Whether this session's `Open` targets a VM THIS node serves.
    fn serves(&self, serving_peer: &str) -> bool {
        // Cheap id match first (bare hostname / `peer:host`); only shell `ip addr`
        // for the overlay-IP spelling when that misses, so a session served by
        // another peer doesn't cost a subprocess per drained `Open`.
        serves_here(serving_peer, &self.node_id, "")
            || serves_here(serving_peer, &self.node_id, &self.relay.overlay_addr())
    }

    /// Apply one drained session op to the live broker state. Returns whether the
    /// brokered set changed (so the caller republishes). Pure over the relay seam.
    fn apply(&mut self, req: &SessionRequest) -> bool {
        match req {
            SessionRequest::Open {
                id,
                serving_peer,
                vm_id,
                ..
            } => {
                if self.brokered.contains_key(id) || !self.serves(serving_peer) {
                    return false;
                }
                let (status, handle) = broker_console(self.relay.as_ref(), vm_id);
                match &status {
                    ConsoleStatus::Brokered { host, port, .. } => tracing::info!(
                        session = %id, vm = %vm_id, overlay = %host, port,
                        "console_broker: brokered local VM console onto the overlay"
                    ),
                    ConsoleStatus::Unbrokerable { reason } => tracing::info!(
                        session = %id, vm = %vm_id, reason,
                        "console_broker: local VM console not brokerable — publishing honest gate"
                    ),
                }
                self.brokered.insert(
                    id.clone(),
                    BrokerEntry {
                        record: BrokeredConsole {
                            session_id: id.clone(),
                            serving_node: self.node_id.clone(),
                            vm_id: vm_id.clone(),
                            status,
                        },
                        _relay: handle,
                    },
                );
                true
            }
            // A closed session's relay is torn down (its handle is dropped) and the
            // record retired. `Active`/`Disconnect` don't change the console.
            SessionRequest::Close { id } => self.brokered.remove(id).is_some(),
            SessionRequest::Active { .. } | SessionRequest::Disconnect { .. } => false,
        }
    }

    /// Publish every live brokered-console record (one message per record, keyed by
    /// session id — the shell scans for its own). Best-effort; a write failure is
    /// logged and retried next tick/heartbeat.
    fn publish(&self, persist: &Persist) {
        for entry in self.brokered.values() {
            let Ok(body) = serde_json::to_string(&entry.record) else {
                continue;
            };
            if let Err(e) = persist.write(CONSOLE_TOPIC, Priority::Default, None, Some(&body)) {
                tracing::warn!(error = %e, "console_broker: publish failed");
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for ConsoleBrokerWorker {
    fn name(&self) -> &'static str {
        "console_broker"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root() else {
            tracing::debug!("console_broker: no bus root; worker idle");
            return Ok(());
        };
        // Fold the FULL session log from the start (like `session_broker`): a
        // session's console is a function of its whole lifecycle.
        let mut cursor: Option<String> = None;
        let mut last_pub = Instant::now();
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let mut changed = false;
                    for req in read_new_actions(&bus_root, &mut cursor) {
                        changed |= self.apply(&req);
                    }
                    let due = last_pub.elapsed() >= self.heartbeat;
                    if (changed || due) && !self.brokered.is_empty() {
                        if let Ok(persist) = Persist::open(bus_root.clone()) {
                            self.publish(&persist);
                            last_pub = Instant::now();
                        }
                    }
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── pure: parse_domdisplay ──

    #[test]
    fn parse_spice_host_port() {
        let c = parse_domdisplay("spice://127.0.0.1:5900\n").expect("parse");
        assert_eq!(c.protocol, DesktopProtocol::Spice);
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(c.port, 5900);
    }

    #[test]
    fn parse_spice_query_port_wins() {
        // libvirt sometimes emits the port in a query (`?port=`), esp. with tls.
        let c = parse_domdisplay("spice://localhost?port=5931&tls-port=5932").expect("parse");
        assert_eq!(c.protocol, DesktopProtocol::Spice);
        assert_eq!(c.host, "localhost");
        assert_eq!(c.port, 5931);
    }

    #[test]
    fn parse_vnc_real_port() {
        let c = parse_domdisplay("vnc://127.0.0.1:5901").expect("parse");
        assert_eq!(c.protocol, DesktopProtocol::Vnc);
        assert_eq!(c.port, 5901);
    }

    #[test]
    fn parse_vnc_display_number_folds_to_tcp_port() {
        // Legacy display-number form: `:0` = display 0 = TCP 5900.
        let c = parse_domdisplay("vnc://127.0.0.1:0").expect("parse");
        assert_eq!(c.port, 5900);
        let c2 = parse_domdisplay("vnc://127.0.0.1:2").expect("parse");
        assert_eq!(c2.port, 5902);
    }

    #[test]
    fn parse_rejects_empty_and_portless_and_unknown() {
        assert!(
            parse_domdisplay("").is_none(),
            "empty = VM off / no graphics"
        );
        assert!(parse_domdisplay("\n  \n").is_none());
        assert!(parse_domdisplay("spice://127.0.0.1").is_none(), "no port");
        assert!(
            parse_domdisplay("http://127.0.0.1:80").is_none(),
            "not a console scheme"
        );
    }

    // ── pure: is_loopback / serves_here / relay args ──

    #[test]
    fn loopback_detection() {
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("127.0.1.1"));
        assert!(is_loopback("localhost"));
        assert!(is_loopback("::1"));
        assert!(!is_loopback("10.42.0.7"));
        assert!(!is_loopback("oak.mesh"));
    }

    #[test]
    fn serves_here_matches_all_serving_peer_spellings() {
        // bare hostname (peer-advertised lane) vs the local `peer:host` node id.
        assert!(serves_here("oak", "peer:oak", "10.42.0.7"));
        assert!(serves_here("peer:oak", "peer:oak", "10.42.0.7"));
        assert!(serves_here("OAK", "peer:oak", ""), "case-insensitive");
        // the overlay IP form.
        assert!(serves_here("10.42.0.7", "peer:oak", "10.42.0.7"));
        // a different peer's session is not ours.
        assert!(!serves_here("elm", "peer:oak", "10.42.0.7"));
    }

    #[test]
    fn relay_args_bind_the_overlay_only() {
        let args = build_relay_args("10.42.0.7", 5900, "127.0.0.1", 5900);
        assert_eq!(args[0], "TCP-LISTEN:5900,bind=10.42.0.7,fork,reuseaddr");
        assert_eq!(args[1], "TCP:127.0.0.1:5900");
        // The listen side is bound to the overlay IP, so the relay is mesh-only —
        // never the LAN/WAN (the compute_expose scoping property).
        assert!(args[0].contains("bind=10.42.0.7"));
    }

    #[test]
    fn overlay_port_is_one_to_one() {
        assert_eq!(overlay_port_for(5930), 5930);
    }

    // ── the fake relay + broker_console ──

    #[derive(Default)]
    struct FakeRelay {
        resolve: Option<Result<ConsoleAddr, ConsoleBrokerError>>,
        overlay: String,
        relay_ok: bool,
        started: Mutex<Vec<(String, u16, u16)>>, // (overlay_addr, overlay_port, target_port)
    }

    impl ConsoleRelay for FakeRelay {
        fn resolve(&self, _vm_id: &str) -> Result<ConsoleAddr, ConsoleBrokerError> {
            self.resolve
                .clone()
                .unwrap_or_else(|| Err(ConsoleBrokerError::Resolve("no fake".into())))
        }
        fn overlay_addr(&self) -> String {
            self.overlay.clone()
        }
        fn start_relay(
            &self,
            overlay_addr: &str,
            overlay_port: u16,
            target: &ConsoleAddr,
        ) -> Result<RelayHandle, ConsoleBrokerError> {
            self.started.lock().unwrap().push((
                overlay_addr.to_string(),
                overlay_port,
                target.port,
            ));
            if self.relay_ok {
                Ok(RelayHandle::detached())
            } else {
                Err(ConsoleBrokerError::Relay("fake relay refused".into()))
            }
        }
    }

    fn spice(port: u16) -> ConsoleAddr {
        ConsoleAddr {
            protocol: DesktopProtocol::Spice,
            host: "127.0.0.1".into(),
            port,
        }
    }

    #[test]
    fn broker_relays_a_loopback_console_onto_the_overlay() {
        let relay = FakeRelay {
            resolve: Some(Ok(spice(5900))),
            overlay: "10.42.0.7".into(),
            relay_ok: true,
            ..FakeRelay::default()
        };
        let (status, handle) = broker_console(&relay, "win11");
        assert!(handle.is_some(), "a live relay handle is retained");
        match status {
            ConsoleStatus::Brokered {
                protocol,
                host,
                port,
            } => {
                assert_eq!(protocol, DesktopProtocol::Spice);
                assert_eq!(host, "10.42.0.7", "the overlay addr, not loopback");
                assert_eq!(port, 5900);
            }
            other => panic!("expected Brokered, got {other:?}"),
        }
        // The relay was started binding the overlay to the loopback console port.
        assert_eq!(
            relay.started.lock().unwrap().as_slice(),
            &[("10.42.0.7".to_string(), 5900, 5900)]
        );
    }

    #[test]
    fn broker_publishes_the_console_directly_when_not_loopback() {
        // A console already bound to a reachable address needs no relay.
        let relay = FakeRelay {
            resolve: Some(Ok(ConsoleAddr {
                protocol: DesktopProtocol::Spice,
                host: "10.42.0.7".into(),
                port: 5930,
            })),
            overlay: "10.42.0.7".into(),
            relay_ok: true,
            ..FakeRelay::default()
        };
        let (status, _handle) = broker_console(&relay, "vm");
        assert!(matches!(status, ConsoleStatus::Brokered { port: 5930, .. }));
        assert!(
            relay.started.lock().unwrap().is_empty(),
            "no relay started for an already-reachable console"
        );
    }

    #[test]
    fn broker_honestly_gates_a_shut_off_vm() {
        // domdisplay resolve fails (VM off / no graphics) → Unbrokerable, no fake.
        let relay = FakeRelay {
            resolve: Some(Err(ConsoleBrokerError::Resolve("VM off".into()))),
            overlay: "10.42.0.7".into(),
            relay_ok: true,
            ..FakeRelay::default()
        };
        let (status, handle) = broker_console(&relay, "dev");
        assert!(handle.is_none(), "no relay for an unbrokerable console");
        assert!(matches!(status, ConsoleStatus::Unbrokerable { .. }));
    }

    #[test]
    fn broker_honestly_gates_when_overlay_is_down() {
        let relay = FakeRelay {
            resolve: Some(Ok(spice(5900))),
            overlay: String::new(), // nebula not up
            relay_ok: true,
            ..FakeRelay::default()
        };
        let (status, handle) = broker_console(&relay, "win11");
        assert!(handle.is_none());
        match status {
            ConsoleStatus::Unbrokerable { reason } => assert!(reason.contains("overlay")),
            other => panic!("expected Unbrokerable, got {other:?}"),
        }
    }

    #[test]
    fn broker_honestly_gates_when_socat_absent() {
        let relay = FakeRelay {
            resolve: Some(Ok(spice(5900))),
            overlay: "10.42.0.7".into(),
            relay_ok: false, // socat spawn refused
            ..FakeRelay::default()
        };
        let (status, handle) = broker_console(&relay, "win11");
        assert!(handle.is_none());
        assert!(matches!(status, ConsoleStatus::Unbrokerable { .. }));
    }

    // ── the worker fold: apply() + serves-gate + close teardown ──

    fn open(id: &str, serving_peer: &str, vm: &str) -> SessionRequest {
        SessionRequest::Open {
            id: id.into(),
            serving_peer: serving_peer.into(),
            vm_id: vm.into(),
            client_peer: "peer:elm".into(),
        }
    }

    fn worker_with(relay: FakeRelay) -> ConsoleBrokerWorker {
        ConsoleBrokerWorker::new("peer:oak".into()).with_relay(Box::new(relay))
    }

    #[test]
    fn apply_open_brokers_a_served_local_vm() {
        let mut w = worker_with(FakeRelay {
            resolve: Some(Ok(spice(5900))),
            overlay: "10.42.0.7".into(),
            relay_ok: true,
            ..FakeRelay::default()
        });
        assert!(w.apply(&open("s1", "oak", "win11")), "brokered → changed");
        let rec = &w.brokered["s1"].record;
        assert_eq!(rec.session_id, "s1");
        assert_eq!(rec.serving_node, "peer:oak");
        assert_eq!(rec.vm_id, "win11");
        assert!(matches!(
            rec.status,
            ConsoleStatus::Brokered { port: 5900, .. }
        ));
    }

    #[test]
    fn apply_open_skips_a_session_served_by_another_peer() {
        let mut w = worker_with(FakeRelay {
            resolve: Some(Ok(spice(5900))),
            overlay: "10.42.0.7".into(),
            relay_ok: true,
            ..FakeRelay::default()
        });
        assert!(!w.apply(&open("s2", "elm", "win11")), "not our session");
        assert!(w.brokered.is_empty());
    }

    #[test]
    fn apply_open_publishes_the_honest_gate_for_a_served_but_unbrokerable_vm() {
        // A served VM that can't be brokered STILL yields a record (the honest
        // greyed lane) — never silently dropped.
        let mut w = worker_with(FakeRelay {
            resolve: Some(Err(ConsoleBrokerError::Resolve("VM off".into()))),
            overlay: "10.42.0.7".into(),
            relay_ok: true,
            ..FakeRelay::default()
        });
        assert!(w.apply(&open("s3", "peer:oak", "dev")));
        assert!(matches!(
            w.brokered["s3"].record.status,
            ConsoleStatus::Unbrokerable { .. }
        ));
    }

    #[test]
    fn apply_open_is_idempotent() {
        let mut w = worker_with(FakeRelay {
            resolve: Some(Ok(spice(5900))),
            overlay: "10.42.0.7".into(),
            relay_ok: true,
            ..FakeRelay::default()
        });
        assert!(w.apply(&open("s1", "oak", "win11")));
        assert!(!w.apply(&open("s1", "oak", "win11")), "already brokered");
    }

    #[test]
    fn apply_close_tears_down_the_relay() {
        let mut w = worker_with(FakeRelay {
            resolve: Some(Ok(spice(5900))),
            overlay: "10.42.0.7".into(),
            relay_ok: true,
            ..FakeRelay::default()
        });
        assert!(w.apply(&open("s1", "oak", "win11")));
        assert!(
            w.apply(&SessionRequest::Close { id: "s1".into() }),
            "close removes the entry"
        );
        assert!(w.brokered.is_empty());
        // Closing an unknown session is a no-op.
        assert!(!w.apply(&SessionRequest::Close { id: "ghost".into() }));
    }

    #[test]
    fn record_round_trips_the_wire_shape() {
        // The shell decodes this exact body off CONSOLE_TOPIC.
        let rec = BrokeredConsole {
            session_id: "vdi-1-win11".into(),
            serving_node: "peer:oak".into(),
            vm_id: "win11".into(),
            status: ConsoleStatus::Brokered {
                protocol: DesktopProtocol::Spice,
                host: "10.42.0.7".into(),
                port: 5900,
            },
        };
        let body = serde_json::to_string(&rec).expect("serialize");
        assert!(body.contains(r#""session_id":"vdi-1-win11""#));
        assert!(body.contains(r#""state":"brokered""#));
        assert!(body.contains(r#""host":"10.42.0.7""#));
        let back: BrokeredConsole = serde_json::from_str(&body).expect("round-trip");
        assert_eq!(back, rec);
    }

    #[tokio::test]
    async fn worker_publishes_a_brokered_record_end_to_end() {
        // Full pipeline over a temp Bus: seed an Open on the session topic, run one
        // fold, and assert the console record lands on CONSOLE_TOPIC.
        let dir = std::env::temp_dir().join(format!("mde-console-broker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let persist = Persist::open(dir.clone()).expect("open bus");
        persist
            .write(
                SESSION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&open("s1", "oak", "win11")).unwrap()),
            )
            .expect("seed open");

        let mut w = worker_with(FakeRelay {
            resolve: Some(Ok(spice(5900))),
            overlay: "10.42.0.7".into(),
            relay_ok: true,
            ..FakeRelay::default()
        })
        .with_bus_root(dir.clone())
        .with_poll(Duration::from_millis(20));

        // Drive one fold+publish by hand (deterministic — no timing on the tick).
        let mut cursor = None;
        for req in read_new_actions(&dir, &mut cursor) {
            w.apply(&req);
        }
        w.publish(&persist);

        let records: Vec<BrokeredConsole> = persist
            .list_since(CONSOLE_TOPIC, None)
            .expect("list console")
            .into_iter()
            .filter_map(|m| m.body)
            .filter_map(|b| serde_json::from_str(&b).ok())
            .collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "s1");
        assert!(matches!(
            records[0].status,
            ConsoleStatus::Brokered { port: 5900, .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
