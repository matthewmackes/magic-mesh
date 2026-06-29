//! DATACENTER-16 (action layer) — `action/dc/wol` → Wake-on-LAN, the
//! power-orchestration primitive that turns a sleeping/powered-off machine
//! back on.
//!
//! Companion to the host power responder ([`crate::ipc::host_ops`]): where that
//! enters/leaves maintenance and reboots an already-running dom0 over SSH, this
//! brings a machine up from cold by broadcasting the standard Wake-on-LAN
//! "magic packet" on the LAN. Same dedicated-OS-thread, `action/dc/<verb>`
//! Bus-RPC shape.
//!
//! Request body `{ "mac": "aa:bb:cc:dd:ee:ff" }`:
//!   * `mac` MUST be six hex octets separated by `:` or `-`
//!     ([`build_magic_packet`]); anything else is rejected without a send.
//! The 102-byte magic packet (6×`0xFF` then the 6-byte MAC repeated 16×) is
//! sent as a UDP broadcast to `255.255.255.255:9` (the classic discard-port WoL
//! target).
//! Reply `{"ok":true}` once the packet is sent, `{"error":"<message>"}` otherwise.

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// The power-orchestration responder — rooted at the shared workgroup root, used
/// to resolve the dc state dir for the idle policy + the learned wake-ETA model
/// (DATACENTER-16).
#[derive(Debug, Clone)]
pub struct DcPowerService {
    workgroup_root: PathBuf,
}

impl DcPowerService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

/// Action verbs served on `action/dc/<verb>`.
///
/// DATACENTER-16: `wol` (the magic-packet wake) is joined by `power-ipmi` (the
/// IPMI-primary chassis control), `host-idle-policy` (the zero-running-VM
/// auto-shutdown gate), and the learned wake-ETA surfaces `power-eta` (read the
/// phased ETA) + `power-wake-done` (feed a measured wake duration).
pub const ACTION_VERBS: [&str; 5] = [
    "wol",
    "power-ipmi",
    "host-idle-policy",
    "power-eta",
    "power-wake-done",
];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/dc/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/dc/{verb}")
}

/// Parse a 6-octet MAC and build the 102-byte Wake-on-LAN magic packet. PURE.
///
/// The MAC must be six hexadecimal octets separated by `:` or `-`
/// (e.g. `aa:bb:cc:dd:ee:ff` or `AA-BB-CC-DD-EE-FF`); any other shape — wrong
/// octet count, a non-hex digit, an octet that isn't exactly two hex chars, or
/// mixed/empty separators — is rejected.
///
/// The returned packet is the standard WoL frame: six `0xFF` sync bytes
/// followed by the 6-byte target MAC repeated 16 times (`6 + 6*16 = 102`).
///
/// # Errors
/// Returns `Err` with a human-readable message for any MAC that isn't exactly
/// six valid hex octets.
pub fn build_magic_packet(mac: &str) -> Result<Vec<u8>, String> {
    // Accept ':' or '-' as the separator, but not a mix (split on both then
    // require the separator count to be consistent by re-checking each piece).
    let parts: Vec<&str> = if mac.contains(':') && !mac.contains('-') {
        mac.split(':').collect()
    } else if mac.contains('-') && !mac.contains(':') {
        mac.split('-').collect()
    } else {
        return Err(format!("invalid mac: {mac}"));
    };

    if parts.len() != 6 {
        return Err(format!(
            "invalid mac: expected 6 octets, got {}",
            parts.len()
        ));
    }

    let mut octets = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        if part.len() != 2 || !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("invalid mac octet: {part}"));
        }
        octets[i] =
            u8::from_str_radix(part, 16).map_err(|e| format!("invalid mac octet {part}: {e}"))?;
    }

    // 6 sync bytes of 0xFF, then the MAC repeated 16 times = 102 bytes.
    let mut packet = Vec::with_capacity(102);
    packet.extend_from_slice(&[0xFF; 6]);
    for _ in 0..16 {
        packet.extend_from_slice(&octets);
    }
    Ok(packet)
}

/// Send a Wake-on-LAN magic packet as a UDP broadcast to `255.255.255.255:9`.
///
/// # Errors
/// Returns `Err` if the socket can't be bound, broadcast can't be enabled, or
/// the datagram can't be sent.
fn send_magic_packet(packet: &[u8]) -> Result<(), String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("bind failed: {e}"))?;
    socket
        .set_broadcast(true)
        .map_err(|e| format!("set_broadcast failed: {e}"))?;
    socket
        .send_to(packet, "255.255.255.255:9")
        .map_err(|e| format!("send failed: {e}"))?;
    Ok(())
}

// ---- DATACENTER-16: IPMI chassis power (IPMI primary, WoL is the fallback) -----

/// Map a power `op` to its `ipmitool chassis power` subcommand. PURE allow-list —
/// the only values that reach the remote tool. `status` is read-only; the others
/// drive chassis power.
///
/// # Errors
/// Returns `Err` for any `op` outside `{on,off,cycle,reset,soft,status}`.
pub fn ipmi_power_subcommand(op: &str) -> Result<&'static str, String> {
    match op {
        "on" => Ok("on"),
        "off" => Ok("off"),
        "cycle" => Ok("cycle"),
        "reset" => Ok("reset"),
        "soft" => Ok("soft"),
        "status" => Ok("status"),
        other => Err(format!("unknown ipmi op: {other}")),
    }
}

/// Whether an IPMI `op` mutates power state (everything except the read-only
/// `status`). PURE.
#[must_use]
pub fn ipmi_op_is_mutating(op: &str) -> bool {
    op != "status"
}

/// Whether an IPMI `op` is *disruptive* enough to require an explicit
/// `confirm:true` (the hard power transitions). A wake (`on`) and the read-only
/// `status` are NOT confirm-gated; `off`/`cycle`/`reset`/`soft` are. PURE.
#[must_use]
pub fn ipmi_op_needs_confirm(op: &str) -> bool {
    matches!(op, "off" | "cycle" | "reset" | "soft")
}

/// Assemble the `ipmitool` argv for a chassis-power op over LAN+. PURE — the
/// password is passed out-of-band via `-E` (the `IPMITOOL_PASSWORD` env), never on
/// the command line.
#[must_use]
pub fn ipmi_chassis_argv(host: &str, user: &str, sub: &str) -> Vec<String> {
    vec![
        "-I".into(),
        "lanplus".into(),
        "-H".into(),
        host.into(),
        "-U".into(),
        user.into(),
        "-E".into(),
        "chassis".into(),
        "power".into(),
        sub.into(),
    ]
}

/// Parse a stored IPMI BMC credential. `user:pass` splits once on the first `:`;
/// a bare value is the password with the default BMC user `"ADMIN"` (overridable
/// via `MCNF_IPMI_USER`). PURE.
#[must_use]
pub fn parse_ipmi_cred(raw: &str, default_user: &str) -> (String, String) {
    let raw = raw.trim();
    match raw.split_once(':') {
        Some((u, p)) => (u.trim().to_string(), p.trim().to_string()),
        None => (default_user.to_string(), raw.to_string()),
    }
}

/// Read the IPMI BMC credential: the mesh secret store key `ipmi-cred` first, then
/// the `MCNF_IPMI_USER`/`MCNF_IPMI_PASS` env fallback. `None` when neither yields a
/// password. The default BMC user is `MCNF_IPMI_USER` (or `"ADMIN"`).
fn ipmi_cred() -> Option<(String, String)> {
    let default_user = std::env::var("MCNF_IPMI_USER").unwrap_or_else(|_| "ADMIN".to_string());
    // 1. mesh secret store.
    if let Ok(o) = std::process::Command::new("bash")
        .args(["-lc", "automation/secrets/mcnf-secret.sh get ipmi-cred"])
        .output()
    {
        if o.status.success() {
            let raw = String::from_utf8_lossy(&o.stdout);
            let raw = raw.trim();
            if !raw.is_empty() {
                return Some(parse_ipmi_cred(raw, &default_user));
            }
        }
    }
    // 2. env fallback.
    let pass = std::env::var("MCNF_IPMI_PASS")
        .ok()
        .filter(|s| !s.is_empty())?;
    Some((default_user, pass))
}

/// Handle a `power-ipmi` request: RBAC (for mutating ops) + confirm (for the
/// disruptive ops) + IPv4-validated `host` + allow-listed `op`, then run
/// `ipmitool … chassis power <sub>`. Body `{ host, op, confirm?, principal? }`.
fn power_ipmi_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("power-ipmi: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("power-ipmi: bad json: {e}")),
    };
    let host = req
        .get("host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let op = req
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let sub = match ipmi_power_subcommand(op) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    // RBAC gates every state-changing op (status reads are open).
    if ipmi_op_is_mutating(op) {
        if let Err(e) =
            crate::ipc::dc_common::rbac_gate_mutating(crate::ipc::dc_common::body_principal(&req))
        {
            return err(e);
        }
    }
    if ipmi_op_needs_confirm(op)
        && req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return err(format!("power-ipmi {op} requires confirm:true"));
    }
    // The BMC host is a plain IPv4 (reuse the host_ops guard) so it can never carry
    // a shell metachar even though ipmitool is spawned argv-style (defence in depth).
    if !crate::ipc::host_ops::valid_ipv4(host) {
        return err("host must be a plain IPv4 address".into());
    }
    let Some((user, pass)) = ipmi_cred() else {
        return err("no ipmi cred (set ipmi-cred in the store or MCNF_IPMI_PASS)".into());
    };
    let out = std::process::Command::new("ipmitool")
        .args(ipmi_chassis_argv(host, &user, sub))
        .env("IPMITOOL_PASSWORD", &pass)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let detail = String::from_utf8_lossy(&o.stdout).trim().to_string();
            json!({ "ok": true, "op": op, "detail": detail }).to_string()
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err(format!("ipmi {op} failed"))
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ipmitool failed: {e}")),
    }
}

// ---- DATACENTER-16: zero-running-VM host idle-shutdown policy gate -------------

/// Handle a `host-idle-policy` request: read or set the auto-shutdown gate that
/// the `datacenter_orchestrator` consults before powering down a zero-running-VM
/// host. A set carries `{"on": <bool>}` (RBAC-gated + persisted); a bare read
/// omits `on`. Reply `{"ok":true,"enabled":<bool>}`.
fn host_idle_policy_reply(svc: &DcPowerService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("host-idle-policy: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("host-idle-policy: bad json: {e}")),
    };
    let state_dir = crate::ipc::dc_common::dc_state_dir(&svc.workgroup_root);
    if let Some(on) = req.get("on").and_then(serde_json::Value::as_bool) {
        if let Err(e) =
            crate::ipc::dc_common::rbac_gate_mutating(crate::ipc::dc_common::body_principal(&req))
        {
            return err(e);
        }
        if let Err(e) = crate::ipc::dc_common::write_arm(&state_dir, "idle-policy", on) {
            return err(format!("host-idle-policy: persist failed: {e}"));
        }
        return json!({ "ok": true, "enabled": on }).to_string();
    }
    json!({ "ok": true, "enabled": crate::ipc::dc_common::read_arm(&state_dir, "idle-policy") })
        .to_string()
}

// ---- DATACENTER-16: learned per-host wake-ETA model (rolling average) ----------

/// How many recent wake samples per host the rolling average keeps.
pub const ETA_WINDOW: usize = 8;

/// The learned per-host wake-ETA model: a rolling window of measured wake
/// durations (WoL/IPMI power-on → toolstack-ready, in seconds) per host, whose
/// average is the predicted ETA. PURE data + arithmetic; the I/O wrappers
/// [`load_wake_model`]/[`save_wake_model`] persist it.
#[derive(Default, Clone, Debug)]
pub struct WakeEtaModel {
    per_host: std::collections::BTreeMap<String, Vec<u64>>,
}

impl WakeEtaModel {
    /// Empty model.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one measured wake duration (seconds) for `host`, keeping only the
    /// most recent [`ETA_WINDOW`] samples.
    pub fn record(&mut self, host: &str, secs: u64) {
        let v = self.per_host.entry(host.to_string()).or_default();
        v.push(secs);
        if v.len() > ETA_WINDOW {
            let drop = v.len() - ETA_WINDOW;
            v.drain(0..drop);
        }
    }

    /// The number of samples held for `host`.
    #[must_use]
    pub fn sample_count(&self, host: &str) -> usize {
        self.per_host.get(host).map_or(0, Vec::len)
    }

    /// The predicted wake ETA (rounded rolling-average seconds) for `host`, or
    /// `None` when no sample has been recorded yet (honest: never a guessed ETA).
    #[must_use]
    pub fn eta(&self, host: &str) -> Option<u64> {
        let v = self.per_host.get(host)?;
        if v.is_empty() {
            return None;
        }
        let sum: u64 = v.iter().sum();
        let n = v.len() as u64;
        Some((sum + n / 2) / n) // rounded average
    }

    /// Serialize to a compact JSON object `{host: [secs,…]}`.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.per_host).unwrap_or_else(|_| "{}".to_string())
    }

    /// Parse from the JSON object written by [`to_json`]; garbage → an empty model.
    #[must_use]
    pub fn from_json(s: &str) -> Self {
        let per_host = serde_json::from_str::<std::collections::BTreeMap<String, Vec<u64>>>(s)
            .unwrap_or_default();
        Self { per_host }
    }
}

/// Compute the phased wake progress for `elapsed` seconds against a predicted
/// `eta`. PURE. Phases follow the design's POST → XCP up → toolstack-ready arc:
/// `post` (<20%), `xcp-up` (<70%), `toolstack` (<100%), `ready` (≥100%). An
/// unknown ETA (`0`) yields `("unknown", 0)`.
#[must_use]
pub fn wake_phase(elapsed_secs: u64, eta_secs: u64) -> (&'static str, u8) {
    if eta_secs == 0 {
        return ("unknown", 0);
    }
    let pct = u8::try_from((elapsed_secs.saturating_mul(100) / eta_secs).min(100)).unwrap_or(100);
    let phase = if pct < 20 {
        "post"
    } else if pct < 70 {
        "xcp-up"
    } else if pct < 100 {
        "toolstack"
    } else {
        "ready"
    };
    (phase, pct)
}

/// The persisted wake-ETA model file under the dc state dir.
fn wake_model_path(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("wake-eta.json")
}

/// Load the persisted wake-ETA model (empty when absent/unreadable).
fn load_wake_model(state_dir: &std::path::Path) -> WakeEtaModel {
    std::fs::read_to_string(wake_model_path(state_dir))
        .map(|s| WakeEtaModel::from_json(&s))
        .unwrap_or_default()
}

/// Persist the wake-ETA model (best-effort; creates the state dir as needed).
fn save_wake_model(state_dir: &std::path::Path, model: &WakeEtaModel) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(wake_model_path(state_dir), model.to_json())
}

// ---- DATACENTER-16: live wake-progress publisher (model → Bus → Hosts tab) -----
//
// The model (rolling ETA), the phasing (`wake_phase`), the IPC (`power-eta`/
// `power-wake-done`), and the Workbench Hosts-tab progress bar all existed, but
// nothing DROVE them live: `event/dc/power/<host>` (the topic the GUI reads) had
// no publisher and `power-wake-done` had no caller, so the learned average never
// got a sample and the bar never appeared. This closes that gap: when a wake is
// issued (`wol`/`power-ipmi on`) the responder spawns a timed driver that
// publishes a phased `event/dc/power/<host>` sample each tick (from the learned
// ETA) until the dom0 answers, then records the measured duration + publishes the
// terminal `ready` event. The pure body builders + the generic driver are
// unit-tested with no clock/network/Bus.

/// Tick spacing for live wake-progress publishes (seconds).
const WAKE_PROGRESS_TICK: u64 = 3;
/// Hard ceiling on a tracked wake before giving up (seconds) — a host that never
/// answers must not publish forever or poison the rolling average with a sample.
const WAKE_PROGRESS_TIMEOUT: u64 = 600;
/// TCP port whose acceptance means the dom0's SSH/toolstack is up (ready).
const DOM0_READY_PORT: u16 = 22;

/// The Bus topic + JSON body for one live wake-progress sample, shaped EXACTLY
/// for the Workbench Hosts tab (`parse_power_event`/`PowerStatus`:
/// `{host, phase, pct, eta_secs}`, with `pct`/`eta_secs` as strings). PURE.
/// `eta` `None` ⇒ the model has no sample for this host yet, so show `post` at
/// 0% with an empty ETA. The GUI's phase vocabulary uses `xcp` where the model's
/// [`wake_phase`] says `xcp-up`, so map it.
#[must_use]
pub fn power_progress_event(host: &str, elapsed_secs: u64, eta: Option<u64>) -> (String, String) {
    let (phase, pct) = match eta {
        Some(e) => wake_phase(elapsed_secs, e),
        None => ("post", 0),
    };
    let gui_phase = if phase == "xcp-up" { "xcp" } else { phase };
    let body = json!({
        "host": host,
        "phase": gui_phase,
        "pct": pct.to_string(),
        "eta_secs": eta.map(|e| e.to_string()).unwrap_or_default(),
    })
    .to_string();
    (format!("event/dc/power/{host}"), body)
}

/// The terminal `ready` event that clears the host's progress bar (the Hosts
/// tab drops `ready` rows). PURE.
#[must_use]
pub fn power_ready_event(host: &str) -> (String, String) {
    let body = json!({ "host": host, "phase": "ready", "pct": "100", "eta_secs": "0" }).to_string();
    (format!("event/dc/power/{host}"), body)
}

/// The dom0 IP to follow a wake for, or `None` when this request isn't a
/// host-power-ON worth tracking. `power-ipmi` carries `host` and is a wake only
/// for `op:"on"`; `wol` optionally carries `host` (the dom0 IP to probe + key
/// the bar) alongside `mac`. PURE.
#[must_use]
pub fn wake_host_to_track(verb: &str, req_body: Option<&str>) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(req_body?).ok()?;
    match verb {
        "power-ipmi" => {
            let op = v.get("op").and_then(serde_json::Value::as_str)?;
            op.eq_ignore_ascii_case("on")
                .then(|| v.get("host").and_then(serde_json::Value::as_str))
                .flatten()
                .map(str::to_string)
        }
        "wol" => v
            .get("host")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

/// Drive one host's live wake progress: publish a phased `event/dc/power/<host>`
/// sample every tick (computed from the learned ETA) until the host answers
/// (ready) or `timeout_secs` elapses; on ready, record the measured duration
/// into `model` + publish the terminal `ready` event. Returns the measured wake
/// seconds on success, `None` on timeout. Generic over time / readiness /
/// publish / sleep so it unit-tests with no real clock, network, or Bus.
///
/// A zero-second "ready" (the host was already up — not actually asleep) is NOT
/// recorded, so an immediate answer can't skew the average toward zero.
pub fn drive_wake_progress<Now, Ready, Sink, Tick>(
    model: &mut WakeEtaModel,
    host: &str,
    timeout_secs: u64,
    now: Now,
    ready: Ready,
    mut publish: Sink,
    tick: Tick,
) -> Option<u64>
where
    Now: Fn() -> u64,
    Ready: Fn(&str) -> bool,
    Sink: FnMut(&str, &str),
    Tick: Fn(),
{
    let start = now();
    loop {
        let elapsed = now().saturating_sub(start);
        if ready(host) {
            if elapsed > 0 {
                model.record(host, elapsed);
            }
            let (t, b) = power_ready_event(host);
            publish(&t, &b);
            return Some(elapsed);
        }
        if elapsed >= timeout_secs {
            return None;
        }
        let (t, b) = power_progress_event(host, elapsed, model.eta(host));
        publish(&t, &b);
        tick();
    }
}

/// Current Unix time in whole seconds (0 on a pre-epoch clock).
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Live dom0 readiness: a TCP connect to `host:22` succeeds (SSH/toolstack up).
/// A refused/timed-out connect ⇒ not ready yet.
fn dom0_ready(host: &str) -> bool {
    use std::net::ToSocketAddrs;
    format!("{host}:{DOM0_READY_PORT}")
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.next())
        .is_some_and(|addr| {
            std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(2)).is_ok()
        })
}

/// Publish one event onto the Bus (best-effort, fire-and-reap) — the same lane
/// shape the datacenter orchestrator uses.
fn bus_publish(topic: &str, body: &str) {
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", topic, "--body-flag", body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Spawn the live wake-progress thread for `host` after a wake is issued: real
/// clock, a TCP dom0-readiness probe, real `mde-bus` publishes, and the rolling
/// model loaded from + (on a real wake) saved back to the dc state dir.
/// Best-effort + detached — a failed probe/publish is a silent no-op and never
/// blocks the responder.
fn spawn_wake_progress(svc: &DcPowerService, host: String) {
    let state_dir = crate::ipc::dc_common::dc_state_dir(&svc.workgroup_root);
    let _ = std::thread::Builder::new()
        .name(format!("dc-wake-progress:{host}"))
        .spawn(move || {
            let mut model = load_wake_model(&state_dir);
            let measured = drive_wake_progress(
                &mut model,
                &host,
                WAKE_PROGRESS_TIMEOUT,
                now_unix_secs,
                dom0_ready,
                bus_publish,
                || std::thread::sleep(std::time::Duration::from_secs(WAKE_PROGRESS_TICK)),
            );
            // Only persist when a real wake completed (a sample was recorded);
            // a timeout records nothing, so there's nothing to save.
            if measured.is_some_and(|secs| secs > 0) {
                let _ = save_wake_model(&state_dir, &model);
            }
        });
}

/// True when a verb reply is a success (`{"ok":true}`), not an `{"error":…}`.
fn reply_is_ok(reply: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(reply)
        .ok()
        .and_then(|v| v.get("ok").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Handle a `power-eta` request (read-only): the learned phased ETA for `host`.
/// Body `{ host, elapsed? }` — when `elapsed` (seconds since the wake was issued)
/// is supplied, the reply also carries the current phase + percent.
fn power_eta_reply(svc: &DcPowerService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("power-eta: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("power-eta: bad json: {e}")),
    };
    let host = req
        .get("host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if host.is_empty() {
        return err("power-eta: missing `host`".into());
    }
    let state_dir = crate::ipc::dc_common::dc_state_dir(&svc.workgroup_root);
    let model = load_wake_model(&state_dir);
    let eta = model.eta(host);
    let samples = model.sample_count(host);
    let elapsed = req.get("elapsed").and_then(serde_json::Value::as_u64);
    let (phase, pct) = match (elapsed, eta) {
        (Some(e), Some(t)) => wake_phase(e, t),
        _ => ("unknown", 0),
    };
    json!({
        "ok": true, "host": host,
        "eta_secs": eta, "samples": samples,
        "phase": phase, "pct": pct,
    })
    .to_string()
}

/// Handle a `power-wake-done` request: feed a measured wake duration into the
/// learned model (RBAC-gated). Body `{ host, secs, principal? }`. Reply carries the
/// updated ETA so the caller can confirm the model advanced.
fn power_wake_done_reply(svc: &DcPowerService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("power-wake-done: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("power-wake-done: bad json: {e}")),
    };
    if let Err(e) =
        crate::ipc::dc_common::rbac_gate_mutating(crate::ipc::dc_common::body_principal(&req))
    {
        return err(e);
    }
    let host = req
        .get("host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if host.is_empty() {
        return err("power-wake-done: missing `host`".into());
    }
    let Some(secs) = req.get("secs").and_then(serde_json::Value::as_u64) else {
        return err("power-wake-done: `secs` must be a non-negative integer".into());
    };
    let state_dir = crate::ipc::dc_common::dc_state_dir(&svc.workgroup_root);
    let mut model = load_wake_model(&state_dir);
    model.record(host, secs);
    if let Err(e) = save_wake_model(&state_dir, &model) {
        return err(format!("power-wake-done: persist failed: {e}"));
    }
    json!({ "ok": true, "host": host, "eta_secs": model.eta(host), "samples": model.sample_count(host) })
        .to_string()
}

/// Handle a `wol` request: parse, validate the MAC, broadcast the magic packet.
fn wol_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("wol: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("wol: bad json: {e}")),
    };
    let mac = req
        .get("mac")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let packet = match build_magic_packet(mac) {
        Ok(p) => p,
        Err(e) => return err(e),
    };

    match send_magic_packet(&packet) {
        Ok(()) => json!({ "ok": true }).to_string(),
        Err(e) => err(e),
    }
}

/// Build the reply for one `action/dc/<verb>` request.
#[must_use]
pub fn build_reply(svc: &DcPowerService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    // DATACENTER-7 (RBAC): `wol`/`power-ipmi` power a machine — mutations; gate the
    // mesh principal against the role policy BEFORE dispatch (deny + audit a viewer).
    if let Err(reason) = crate::ipc::dc_rbac::enforce(verb, req_body) {
        crate::ipc::dc_rbac::audit_denial(verb, req_body, &reason);
        return err(reason);
    }
    match verb {
        "wol" => wol_reply(req_body),
        "power-ipmi" => power_ipmi_reply(req_body),
        "host-idle-policy" => host_idle_policy_reply(svc, req_body),
        "power-eta" => power_eta_reply(svc, req_body),
        "power-wake-done" => power_wake_done_reply(svc, req_body),
        _ => json!({ "error": "unknown dc verb" }).to_string(),
    }
}

/// Run the dc-power Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &DcPowerService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(persist: &Persist, svc: &DcPowerService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "dc-power responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_reply(svc, verb, msg.body.as_deref())
            } else {
                crate::ipc::body_too_large_reply(verb)
            };
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "dc-power responder: reply write failed");
            }
            // DATACENTER-16: a successful host-power-ON drives the live wake
            // progress bar — publish phased `event/dc/power/<host>` until the
            // dom0 answers, then feed the rolling ETA model.
            if reply_is_ok(&reply) {
                if let Some(host) = wake_host_to_track(verb, msg.body.as_deref()) {
                    spawn_wake_progress(svc, host);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_and_verbs_lock() {
        assert_eq!(action_topic("wol"), "action/dc/wol");
        assert!(ACTION_VERBS.contains(&"wol"));
    }

    #[test]
    fn magic_packet_is_102_bytes() {
        let p = build_magic_packet("aa:bb:cc:dd:ee:ff").unwrap();
        assert_eq!(p.len(), 102);
    }

    #[test]
    fn magic_packet_first_six_are_ff() {
        let p = build_magic_packet("aa:bb:cc:dd:ee:ff").unwrap();
        assert_eq!(&p[0..6], &[0xFF; 6]);
    }

    #[test]
    fn magic_packet_carries_the_mac_sixteen_times() {
        let mac = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        let p = build_magic_packet("aa:bb:cc:dd:ee:ff").unwrap();
        // Bytes 6..12 are the first MAC copy.
        assert_eq!(&p[6..12], &mac);
        // All 16 repetitions match.
        for i in 0..16 {
            let start = 6 + i * 6;
            assert_eq!(&p[start..start + 6], &mac, "repetition {i} mismatch");
        }
    }

    #[test]
    fn dash_separator_is_accepted() {
        let p = build_magic_packet("AA-BB-CC-DD-EE-FF").unwrap();
        assert_eq!(p.len(), 102);
        assert_eq!(&p[6..12], &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    }

    #[test]
    fn uppercase_and_lowercase_hex_both_parse() {
        let lower = build_magic_packet("01:23:45:67:89:ab").unwrap();
        let upper = build_magic_packet("01:23:45:67:89:AB").unwrap();
        assert_eq!(lower, upper);
        assert_eq!(&lower[6..12], &[0x01, 0x23, 0x45, 0x67, 0x89, 0xab]);
    }

    #[test]
    fn too_few_octets_rejected() {
        assert!(build_magic_packet("aa:bb:cc:dd:ee").is_err());
    }

    #[test]
    fn too_many_octets_rejected() {
        assert!(build_magic_packet("aa:bb:cc:dd:ee:ff:00").is_err());
    }

    #[test]
    fn non_hex_digit_rejected() {
        assert!(build_magic_packet("aa:bb:cc:dd:ee:zz").is_err());
        assert!(build_magic_packet("gg:bb:cc:dd:ee:ff").is_err());
    }

    #[test]
    fn wrong_octet_width_rejected() {
        // A single-digit octet is not a valid two-char hex octet.
        assert!(build_magic_packet("a:bb:cc:dd:ee:ff").is_err());
        // A three-digit octet is rejected too.
        assert!(build_magic_packet("aaa:bb:cc:dd:ee:ff").is_err());
    }

    #[test]
    fn missing_or_mixed_separator_rejected() {
        assert!(build_magic_packet("aabbccddeeff").is_err());
        assert!(build_magic_packet("aa:bb-cc:dd:ee:ff").is_err());
        assert!(build_magic_packet("").is_err());
    }

    #[test]
    fn unknown_verb_and_missing_body_error() {
        let s = DcPowerService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "bogus", None).contains("unknown dc verb"));
        assert!(build_reply(&s, "wol", None).contains("missing request body"));
    }

    #[test]
    fn bad_json_and_bad_mac_error() {
        let s = DcPowerService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "wol", Some("not json")).contains("bad json"));
        let body = json!({ "mac": "nope" }).to_string();
        let r = build_reply(&s, "wol", Some(&body));
        assert!(r.contains("error"), "{r}");
        assert!(r.contains("invalid mac"), "{r}");
    }

    // ---- DATACENTER-16: IPMI + idle policy + wake-ETA --------------------------

    #[test]
    fn dc16_verbs_in_lock() {
        for v in [
            "power-ipmi",
            "host-idle-policy",
            "power-eta",
            "power-wake-done",
        ] {
            assert_eq!(action_topic(v), format!("action/dc/{v}"));
            assert!(ACTION_VERBS.contains(&v), "{v} missing");
        }
    }

    #[test]
    fn ipmi_subcommand_maps_and_rejects() {
        for (op, sub) in [
            ("on", "on"),
            ("off", "off"),
            ("cycle", "cycle"),
            ("reset", "reset"),
            ("soft", "soft"),
            ("status", "status"),
        ] {
            assert_eq!(ipmi_power_subcommand(op).unwrap(), sub);
        }
        assert!(ipmi_power_subcommand("nuke").is_err());
        assert!(ipmi_power_subcommand("").is_err());
    }

    #[test]
    fn ipmi_op_classification() {
        assert!(!ipmi_op_is_mutating("status"));
        assert!(ipmi_op_is_mutating("on"));
        assert!(ipmi_op_is_mutating("off"));
        // confirm only for the hard transitions
        assert!(!ipmi_op_needs_confirm("on"));
        assert!(!ipmi_op_needs_confirm("status"));
        assert!(ipmi_op_needs_confirm("off"));
        assert!(ipmi_op_needs_confirm("cycle"));
        assert!(ipmi_op_needs_confirm("reset"));
        assert!(ipmi_op_needs_confirm("soft"));
    }

    #[test]
    fn ipmi_chassis_argv_uses_lanplus_and_env_password() {
        let argv = ipmi_chassis_argv("10.0.0.7", "ADMIN", "status");
        assert_eq!(
            argv,
            vec![
                "-I", "lanplus", "-H", "10.0.0.7", "-U", "ADMIN", "-E", "chassis", "power",
                "status"
            ]
        );
        // The password is NOT on the command line (passed via -E / env).
        assert!(!argv.iter().any(|a| a.contains("password") || a == "-P"));
    }

    #[test]
    fn parse_ipmi_cred_user_pass_and_bare() {
        assert_eq!(
            parse_ipmi_cred("root:calvin", "ADMIN"),
            ("root".to_string(), "calvin".to_string())
        );
        // bare → default user
        assert_eq!(
            parse_ipmi_cred("hunter2", "ADMIN"),
            ("ADMIN".to_string(), "hunter2".to_string())
        );
        // password with ':' preserved
        assert_eq!(
            parse_ipmi_cred("root:a:b", "ADMIN"),
            ("root".to_string(), "a:b".to_string())
        );
    }

    #[test]
    fn power_ipmi_gates_before_exec() {
        let s = DcPowerService::new(std::path::PathBuf::from("/tmp"));
        // missing body
        assert!(build_reply(&s, "power-ipmi", None).contains("missing request body"));
        // unknown op
        let body = json!({ "host": "10.0.0.7", "op": "nuke" }).to_string();
        assert!(build_reply(&s, "power-ipmi", Some(&body)).contains("unknown ipmi op"));
        // disruptive op without confirm
        let body = json!({ "host": "10.0.0.7", "op": "off" }).to_string();
        let r = build_reply(&s, "power-ipmi", Some(&body));
        assert!(r.contains("requires confirm:true"), "{r}");
        // a wake (no confirm needed) with a bad host is rejected on the host guard
        let body = json!({ "host": "1.2.3.4; reboot", "op": "on" }).to_string();
        let r = build_reply(&s, "power-ipmi", Some(&body));
        assert!(r.contains("host must be a plain IPv4 address"), "{r}");
    }

    #[test]
    fn host_idle_policy_read_reports_enabled() {
        let s = DcPowerService::new(std::path::PathBuf::from("/tmp"));
        let r = build_reply(&s, "host-idle-policy", Some("{}"));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], true);
        assert!(v.get("enabled").and_then(|e| e.as_bool()).is_some(), "{r}");
    }

    #[test]
    fn wake_eta_model_rolling_average_and_window() {
        let mut m = WakeEtaModel::new();
        assert_eq!(m.eta("h1"), None);
        m.record("h1", 100);
        m.record("h1", 200);
        // rounded average of [100,200] = 150
        assert_eq!(m.eta("h1"), Some(150));
        assert_eq!(m.sample_count("h1"), 2);
        // window cap: push more than ETA_WINDOW, only the last ETA_WINDOW remain
        for i in 0..ETA_WINDOW as u64 + 4 {
            m.record("h2", 60 + i);
        }
        assert_eq!(m.sample_count("h2"), ETA_WINDOW);
        // hosts are independent
        assert_eq!(m.eta("h1"), Some(150));
        // json round trip
        let j = m.to_json();
        let back = WakeEtaModel::from_json(&j);
        assert_eq!(back.eta("h1"), Some(150));
        assert_eq!(back.sample_count("h2"), ETA_WINDOW);
        // garbage → empty
        assert_eq!(WakeEtaModel::from_json("not json").sample_count("h1"), 0);
    }

    #[test]
    fn wake_phase_arc() {
        // unknown eta
        assert_eq!(wake_phase(10, 0), ("unknown", 0));
        // 0% → post
        assert_eq!(wake_phase(0, 100), ("post", 0));
        // 10% post, 50% xcp-up, 80% toolstack, ≥100 ready (and clamps)
        assert_eq!(wake_phase(10, 100), ("post", 10));
        assert_eq!(wake_phase(50, 100), ("xcp-up", 50));
        assert_eq!(wake_phase(80, 100), ("toolstack", 80));
        assert_eq!(wake_phase(100, 100), ("ready", 100));
        assert_eq!(wake_phase(250, 100), ("ready", 100));
    }

    #[test]
    fn wake_model_persists_to_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let mut m = load_wake_model(dir);
        assert_eq!(m.sample_count("xcp-a"), 0);
        m.record("xcp-a", 180);
        m.record("xcp-a", 220);
        save_wake_model(dir, &m).expect("save");
        let reloaded = load_wake_model(dir);
        assert_eq!(reloaded.eta("xcp-a"), Some(200));
    }

    #[test]
    fn power_eta_and_wake_done_gate_inputs() {
        let s = DcPowerService::new(std::path::PathBuf::from("/tmp"));
        // power-eta needs a host
        let r = build_reply(&s, "power-eta", Some("{}"));
        assert!(r.contains("missing `host`"), "{r}");
        // power-wake-done needs host + secs
        let r = build_reply(&s, "power-wake-done", Some(r#"{"secs":5}"#));
        assert!(r.contains("missing `host`"), "{r}");
        let r = build_reply(&s, "power-wake-done", Some(r#"{"host":"x"}"#));
        assert!(r.contains("`secs`"), "{r}");
    }

    // ---- DATACENTER-16: live wake-progress publisher ----

    #[test]
    fn power_progress_event_matches_the_hosts_tab_contract() {
        // The body must be exactly what the Workbench `parse_power_event` reads:
        // {host, phase, pct, eta_secs} with pct/eta_secs as STRINGS, and the
        // model's `xcp-up` phase mapped to the GUI's `xcp`.
        let (topic, body) = power_progress_event("172.20.0.9", 50, Some(100));
        assert_eq!(topic, "event/dc/power/172.20.0.9");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["host"], "172.20.0.9");
        assert_eq!(v["phase"], "xcp"); // wake_phase says "xcp-up" → GUI "xcp"
        assert_eq!(v["pct"], "50"); // string, not number
        assert_eq!(v["eta_secs"], "100");

        // No learned sample yet → post / 0% / empty ETA (never a bogus number).
        let (_, body) = power_progress_event("h", 12, None);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["phase"], "post");
        assert_eq!(v["pct"], "0");
        assert_eq!(v["eta_secs"], "");

        // toolstack phase carries through unmapped.
        let (_, body) = power_progress_event("h", 80, Some(100));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["phase"], "toolstack");
    }

    #[test]
    fn power_ready_event_is_the_terminal_clear() {
        let (topic, body) = power_ready_event("172.20.0.8");
        assert_eq!(topic, "event/dc/power/172.20.0.8");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["phase"], "ready"); // the Hosts tab drops `ready` rows
        assert_eq!(v["pct"], "100");
    }

    #[test]
    fn wake_host_to_track_picks_only_real_power_ons() {
        // power-ipmi: tracked only for op:on, keyed by host.
        assert_eq!(
            wake_host_to_track("power-ipmi", Some(r#"{"op":"on","host":"172.20.0.9"}"#)),
            Some("172.20.0.9".to_string())
        );
        assert_eq!(
            wake_host_to_track("power-ipmi", Some(r#"{"op":"off","host":"172.20.0.9"}"#)),
            None
        );
        // wol: tracked when a host (dom0 IP) is supplied alongside the mac.
        assert_eq!(
            wake_host_to_track(
                "wol",
                Some(r#"{"mac":"aa:bb:cc:dd:ee:ff","host":"172.20.0.5"}"#)
            ),
            Some("172.20.0.5".to_string())
        );
        assert_eq!(
            wake_host_to_track("wol", Some(r#"{"mac":"aa:bb:cc:dd:ee:ff"}"#)),
            None
        );
        // other verbs / bad input are never tracked.
        assert_eq!(
            wake_host_to_track("power-eta", Some(r#"{"host":"x"}"#)),
            None
        );
        assert_eq!(wake_host_to_track("wol", None), None);
        assert_eq!(wake_host_to_track("wol", Some("not json")), None);
    }

    #[test]
    fn drive_wake_progress_publishes_phases_then_records_on_ready() {
        // Injected clock (advances only on tick) + ready-at-9 + a capturing sink.
        let clock = std::cell::Cell::new(0u64);
        let mut published: Vec<(String, String)> = Vec::new();
        let mut model = WakeEtaModel::new();
        let measured = drive_wake_progress(
            &mut model,
            "172.20.0.9",
            600,
            || clock.get(),
            |_| clock.get() >= 9,
            |t, b| published.push((t.to_string(), b.to_string())),
            || clock.set(clock.get() + 3),
        );
        // Ready at elapsed 9 → that's the recorded sample (avg of one = 9).
        assert_eq!(measured, Some(9));
        assert_eq!(model.eta("172.20.0.9"), Some(9));
        // Three progress publishes (elapsed 0, 3, 6) then the terminal ready.
        assert_eq!(published.len(), 4);
        assert!(published[0].0 == "event/dc/power/172.20.0.9");
        assert!(published[3].1.contains("\"phase\":\"ready\""));
        assert!(!published[0].1.contains("\"phase\":\"ready\""));
    }

    #[test]
    fn drive_wake_progress_times_out_without_recording() {
        // Never ready; the clock runs past the timeout → no sample, no `ready`
        // (a stuck wake must not poison the rolling average).
        let clock = std::cell::Cell::new(0u64);
        let mut published: Vec<(String, String)> = Vec::new();
        let mut model = WakeEtaModel::new();
        let measured = drive_wake_progress(
            &mut model,
            "h",
            9,
            || clock.get(),
            |_| false,
            |t, b| published.push((t.to_string(), b.to_string())),
            || clock.set(clock.get() + 3),
        );
        assert_eq!(measured, None);
        assert_eq!(model.sample_count("h"), 0);
        assert!(published
            .iter()
            .all(|(_, b)| !b.contains("\"phase\":\"ready\"")));
    }
}
