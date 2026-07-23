//! DATACENTER-16 (action layer) — energy-aware host power.
//!
//! Companion to the host power responder ([`crate::ipc::host_ops`]): where that
//! enters/leaves maintenance and reboots an already-running dom0 over SSH, this
//! brings a machine up from cold (IPMI primary, Wake-on-LAN fallback), reasons
//! about whether an idle host should power itself down, and turns each timed
//! wake into a phased progress bar with a learned ETA. Same dedicated-OS-thread,
//! `action/dc/<verb>` Bus-RPC shape.
//!
//! Verbs served on `action/dc/<verb>`:
//!   * `wol`         — broadcast the 102-byte Wake-on-LAN magic packet (the cold
//!     wake fallback when a host has no BMC). Body `{ "mac": "aa:bb:cc:dd:ee:ff" }`.
//!   * `ipmi-power`  — drive a host's BMC via `ipmitool chassis power <op>`
//!     (`on` | `off` | `cycle` | `status`); the PRIMARY wake path. Body
//!     `{ "bmc", "user", "pass", "op", "mac"? }` — `mac` is the WoL fallback used
//!     when the BMC is unreachable on a power-`on`.
//!   * `idle-policy` — read a dom0's running-VM count over SSH and apply the
//!     idle-shutdown policy: a host with zero running guests is a graceful
//!     shutdown candidate. Body `{ "dom0", "min_idle_secs"?, "idle_secs"? }`.
//!     Read-only — it RECOMMENDS; the actual `host-disable`/`host-shutdown` runs
//!     through the existing confirm-gated `action/dc/host-power` op.
//!   * `wake-eta`    — given a host's recorded wake samples, compute the phased
//!     (POST → XCP → toolstack) progress + a live ETA. PURE math; no I/O. Body
//!     `{ "samples": [<secs>...], "elapsed": <secs> }`.
//!
//! Reply `{"ok":true, ...}` on success, `{"error":"<message>"}` otherwise.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

/// Authorization node scope for cluster-wide Datacenter actions.
pub const DC_ACTION_NODE_SCOPE: &str = "fleet-control";

/// The energy-aware power responder — rooted at the shared workgroup root (the
/// repo root, carried for parity with the other action services; the dom0 SSH
/// key + allow-list come from the orchestrator env config, not here).
#[derive(Debug, Clone)]
pub struct DcPowerService {
    // Carried for parity with the other action services and the
    // `new(workgroup_root)` spawn contract; the WoL/IPMI/ETA paths read no
    // per-host rooted state.
    #[allow(dead_code)]
    workgroup_root: PathBuf,
    authorizer: Arc<ActionAuthorizer>,
}

impl DcPowerService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            authorizer: Arc::new(ActionAuthorizer::production()),
        }
    }

    /// Inject an isolated verifier and replay ledger (tests).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }
}

/// Action verbs served on `action/dc/<verb>`.
pub const ACTION_VERBS: [&str; 4] = ["wol", "ipmi-power", "idle-policy", "wake-eta"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/dc/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/dc/{verb}")
}

/// Return the stable capability target for a privileged dc-power verb.
/// `wake-eta` is pure arithmetic and intentionally remains available without a
/// root-shell capability. `idle-policy` is non-destructive, but it still opens
/// a root SSH control-plane read and is therefore privileged.
fn mutation_target(verb: &str, req_body: Option<&str>) -> Result<Option<String>, String> {
    if verb == "wake-eta" || !ACTION_VERBS.contains(&verb) {
        return Ok(None);
    }
    let body = req_body.ok_or_else(|| format!("{verb}: missing request body"))?;
    let request: serde_json::Value =
        serde_json::from_str(body).map_err(|_| format!("{verb}: bad json"))?;
    let string_field = |field: &str| -> Result<&str, String> {
        request
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{verb}: missing `{field}`"))
    };
    let target = match verb {
        "wol" => {
            let mac = string_field("mac")?;
            let packet = build_magic_packet(mac)?;
            let octets = &packet[6..12];
            format!(
                "host-mac:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                octets[0], octets[1], octets[2], octets[3], octets[4], octets[5]
            )
        }
        "ipmi-power" => {
            let bmc = string_field("bmc")?;
            if !valid_bmc_host(bmc) {
                return Err(format!("{verb}: invalid BMC host"));
            }
            format!("bmc:{}", bmc.to_ascii_lowercase())
        }
        "idle-policy" => format!("host:{}", string_field("dom0")?),
        _ => unreachable!("the privileged verb list and target map must stay closed"),
    };
    Ok(Some(target))
}

/// Gate a privileged power request before [`build_reply`] can broadcast a
/// packet, invoke `ipmitool`, or open root SSH.
fn authorize_mutation(
    svc: &DcPowerService,
    verb: &str,
    req_body: Option<&str>,
) -> Result<(), String> {
    let Some(target) = mutation_target(verb, req_body)? else {
        return Ok(());
    };
    svc.authorizer.authorize(
        req_body.expect("a mutation target requires a body"),
        MutationContext {
            verb,
            node: DC_ACTION_NODE_SCOPE,
            target: &target,
        },
    )
}

fn build_authorized_reply(svc: &DcPowerService, verb: &str, req_body: Option<&str>) -> String {
    if let Err(error) = authorize_mutation(svc, verb, req_body) {
        tracing::warn!(
            target: "mackesd::action_auth",
            verb,
            %error,
            "refused unauthorized Datacenter power action"
        );
        return json!({ "error": format!("{verb}: authorization refused: {error}") }).to_string();
    }
    build_reply(svc, verb, req_body)
}

// ───────────────────────── Wake-on-LAN (fallback wake) ──────────────────────

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

/// Validate + send a WoL magic packet for `mac`. Shared by the `wol` verb and
/// the `ipmi-power` on-fallback.
fn wol_wake(mac: &str) -> Result<(), String> {
    let packet = build_magic_packet(mac)?;
    send_magic_packet(&packet)
}

// ──────────────────────── IPMI (primary cold wake) ──────────────────────────

/// The IPMI chassis-power ops this responder allows. PURE — maps the requested
/// `op` to the `ipmitool chassis power <verb>` argument, rejecting anything else
/// so the verb can never become an arbitrary `ipmitool` passthrough.
///
/// * `on`     → power the host up (the primary cold-wake path);
/// * `off`    → hard power-off (energy reclaim — reserved for an unresponsive
///   host that won't drain gracefully; the graceful path is `host-power`);
/// * `cycle`  → power-cycle;
/// * `status` → read the current chassis power state (read-only).
///
/// # Errors
/// Returns `Err` for any `op` outside the four above.
pub fn ipmi_power_verb(op: &str) -> Result<&'static str, String> {
    match op {
        "on" => Ok("on"),
        "off" => Ok("off"),
        "cycle" => Ok("cycle"),
        "status" => Ok("status"),
        other => Err(format!("unknown ipmi op: {other}")),
    }
}

/// Whether a BMC endpoint is a safe `ipmitool -H` argument: a plain IPv4 or a
/// hostname made only of `[A-Za-z0-9.-]` (no shell metacharacters, no spaces).
/// PURE — the injection guard before the BMC ever reaches a spawned process.
#[must_use]
pub fn valid_bmc_host(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Whether an IPMI credential (user/pass) is free of characters that could break
/// out of an argv slot or carry control bytes. PURE. `ipmitool` takes these as
/// distinct argv entries (no shell), so the bar is just "no NUL / no control /
/// non-empty user"; we keep it conservative and reject control chars in both.
#[must_use]
pub fn valid_ipmi_cred(user: &str, pass: &str) -> bool {
    !user.is_empty() && !user.chars().any(char::is_control) && !pass.chars().any(char::is_control)
}

/// Run `ipmitool -I lanplus -H <bmc> -U <user> -P <pass> chassis power <op>` and
/// turn the outcome into a status line. The args are validated by the caller
/// ([`valid_bmc_host`] / [`valid_ipmi_cred`] / [`ipmi_power_verb`]) and passed as
/// distinct argv entries (no shell), so this is not an injection surface.
///
/// # Errors
/// Returns `Err` on a spawn failure or a non-zero exit (with the trimmed
/// stderr/stdout).
fn ipmitool_chassis(bmc: &str, user: &str, pass: &str, verb: &str) -> Result<String, String> {
    let o = std::process::Command::new("ipmitool")
        .args([
            "-I", "lanplus", "-H", bmc, "-U", user, "-P", pass, "chassis", "power", verb,
        ])
        .output()
        .map_err(|e| format!("ipmitool spawn failed: {e}"))?;
    if o.status.success() {
        let out = String::from_utf8_lossy(&o.stdout);
        let line = out.trim();
        Ok(if line.is_empty() {
            format!("chassis power {verb}")
        } else {
            line.to_string()
        })
    } else {
        let mut out = String::from_utf8_lossy(&o.stdout).into_owned();
        out.push_str(&String::from_utf8_lossy(&o.stderr));
        let msg = out.trim();
        Err(if msg.is_empty() {
            format!("ipmitool chassis power {verb} failed")
        } else {
            msg.to_string()
        })
    }
}

/// Handle an `ipmi-power` request: validate the BMC + cred + op, drive the BMC,
/// and on a power-`on` that the BMC can't service, fall back to Wake-on-LAN if a
/// `mac` was supplied. Returns the reply JSON string.
///
/// Reply `{"ok":true,"result":"<line>","via":"ipmi"|"wol"}` on success.
fn ipmi_power(req_body: Option<&str>) -> Result<(String, &'static str), String> {
    let Some(body) = req_body else {
        return Err("ipmi-power: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("ipmi-power: bad json: {e}"))?;

    let op = req
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let verb = ipmi_power_verb(op)?;

    let bmc = req
        .get("bmc")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_bmc_host(bmc) {
        return Err("bmc must be a plain IPv4/hostname".into());
    }
    let user = req
        .get("user")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let pass = req
        .get("pass")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_ipmi_cred(user, pass) {
        return Err("ipmi user/pass invalid".into());
    }

    match ipmitool_chassis(bmc, user, pass, verb) {
        Ok(line) => Ok((line, "ipmi")),
        // On a wake (`on`) the BMC may be unreachable (cold/unconfigured); fall
        // back to Wake-on-LAN when a MAC was supplied, so the wake still lands.
        Err(e) if verb == "on" => {
            let mac = req
                .get("mac")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if mac.is_empty() {
                Err(format!("ipmi on failed ({e}); no mac for WoL fallback"))
            } else {
                wol_wake(mac).map_err(|we| format!("ipmi on failed ({e}); WoL fallback: {we}"))?;
                Ok(("WoL magic packet sent".to_string(), "wol"))
            }
        }
        Err(e) => Err(e),
    }
}

// ───────────────────────── Idle-shutdown policy ─────────────────────────────

/// The idle-shutdown decision for one host. PURE.
///
/// A host is a graceful-shutdown candidate when it carries **zero running
/// guests** AND has been idle at least `min_idle_secs` (so a host that just
/// drained isn't yanked mid-rebalance; `idle_secs == 0` with `min_idle_secs > 0`
/// means "idle now but not long enough"). A host with running guests is never a
/// candidate.
///
/// Returns `(shutdown, reason)`: `shutdown` is whether to power the host down,
/// `reason` a short human-readable explanation for the panel / audit line.
#[must_use]
pub fn idle_shutdown_decision(
    running: usize,
    idle_secs: u64,
    min_idle_secs: u64,
) -> (bool, String) {
    if running > 0 {
        return (false, format!("{running} running guest(s) — keep powered"));
    }
    if idle_secs < min_idle_secs {
        return (
            false,
            format!("idle {idle_secs}s < {min_idle_secs}s hold — wait"),
        );
    }
    (
        true,
        format!("0 running guests, idle {idle_secs}s ≥ {min_idle_secs}s — shut down"),
    )
}

/// Parse the `xe vm-list resident-on=<uuid> power-state=running --minimal` reply
/// (a comma-separated list of running-guest uuids, possibly empty) into the count
/// of running guests on the host. PURE.
#[must_use]
pub fn parse_running_count(minimal: &str) -> usize {
    minimal
        .trim()
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .count()
}

/// Run one allow-listed `xe` read on `dom0` over the mesh SSH key. The dom0 is
/// validated against the orchestrator allow-list FIRST so an attacker-supplied
/// host never reaches SSH. Returns the trimmed stdout.
fn ssh_xe_read(dom0: &str, remote: &str) -> Result<String, String> {
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return Err("dom0 not in allowed set".into());
    }
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let o = std::process::Command::new("ssh")
        .args([
            "-i",
            &key,
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            &format!("root@{dom0}"),
            remote,
        ])
        .output()
        .map_err(|e| format!("ssh failed: {e}"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&o.stderr);
        let msg = stderr.trim();
        Err(if msg.is_empty() {
            "xe read failed".into()
        } else {
            msg.to_string()
        })
    }
}

/// Handle an `idle-policy` request: count `dom0`'s running guests over SSH and
/// apply [`idle_shutdown_decision`]. Read-only — recommends; the actual shutdown
/// runs through the confirm-gated `action/dc/host-power` op.
///
/// Reply `{"ok":true,"running":N,"shutdown":<bool>,"reason":"…"}`.
fn idle_policy(req_body: Option<&str>) -> Result<(usize, bool, String), String> {
    let Some(body) = req_body else {
        return Err("idle-policy: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("idle-policy: bad json: {e}"))?;
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if dom0.is_empty() {
        return Err("idle-policy: missing dom0".into());
    }
    let min_idle_secs = req
        .get("min_idle_secs")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    // The caller passes how long the host has been observed idle (the panel
    // tracks this off the Bus); 0 = idle right now / unknown.
    let idle_secs = req
        .get("idle_secs")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    // Resolve the host uuid then count its running guests in one round trip.
    let out = ssh_xe_read(
        dom0,
        "U=$(xe host-list params=uuid --minimal | cut -d, -f1); \
         xe vm-list resident-on=$U power-state=running is-control-domain=false --minimal",
    )?;
    let running = parse_running_count(&out);
    let (shutdown, reason) = idle_shutdown_decision(running, idle_secs, min_idle_secs);
    Ok((running, shutdown, reason))
}

// ───────────────── Learned wake ETA / phased progress ───────────────────────

/// The three observable phases of a cold XCP-ng wake. PURE classification used by
/// [`wake_phase`] to label the phased progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakePhase {
    /// Firmware POST — power applied, host not yet pinging.
    Post,
    /// XCP-ng / dom0 booting — kernel up, toolstack not yet answering.
    Xcp,
    /// Toolstack (XAPI) coming online — almost ready.
    Toolstack,
    /// XAPI is answering — the host is ready.
    Ready,
}

impl WakePhase {
    /// A short stable slug for the reply JSON / panel label. PURE.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Post => "post",
            Self::Xcp => "xcp",
            Self::Toolstack => "toolstack",
            Self::Ready => "ready",
        }
    }
}

/// The rolling per-host average wake time (seconds) from the recorded samples.
/// PURE. Empty samples → a conservative 180s default (a cold XCP host is rarely
/// faster) so the first wake still shows an honest, bounded ETA. Caps the window
/// to the most recent [`WAKE_SAMPLE_WINDOW`] samples so the average tracks the
/// host's current behavior rather than ancient ones.
#[must_use]
pub fn rolling_avg_wake_secs(samples: &[u64]) -> u64 {
    if samples.is_empty() {
        return DEFAULT_WAKE_SECS;
    }
    let window = if samples.len() > WAKE_SAMPLE_WINDOW {
        &samples[samples.len() - WAKE_SAMPLE_WINDOW..]
    } else {
        samples
    };
    let sum: u64 = window.iter().sum();
    // Round to nearest, never below 1s.
    ((sum + window.len() as u64 / 2) / window.len() as u64).max(1)
}

/// The default learned-wake estimate (seconds) before any sample exists.
pub const DEFAULT_WAKE_SECS: u64 = 180;

/// How many recent wake samples the rolling average windows over.
pub const WAKE_SAMPLE_WINDOW: usize = 10;

/// The phase a wake is in given `elapsed` seconds against the learned `avg`. PURE.
///
/// Phased against fixed fractions of the learned average: POST for the first
/// ~25%, XCP boot through ~70%, toolstack-coming-up through 100%, and Ready once
/// elapsed meets/exceeds the average (the XAPI poll is what actually flips a wake
/// to ready; this is the visual prediction in between).
#[must_use]
pub fn wake_phase(elapsed: u64, avg: u64) -> WakePhase {
    let avg = avg.max(1);
    if elapsed >= avg {
        return WakePhase::Ready;
    }
    let frac = elapsed as f64 / avg as f64;
    if frac < 0.25 {
        WakePhase::Post
    } else if frac < 0.70 {
        WakePhase::Xcp
    } else {
        WakePhase::Toolstack
    }
}

/// The phased progress fraction (0.0..=1.0) for `elapsed` against `avg`. PURE.
///
/// Linear toward the learned average, clamped to `[0, 0.99]` until the XAPI poll
/// confirms ready — an honest bar never claims 100% before the host actually
/// answers (live-verify discipline: a full bar must mean ready, not "should be").
#[must_use]
pub fn wake_progress(elapsed: u64, avg: u64) -> f64 {
    let avg = avg.max(1);
    let frac = elapsed as f64 / avg as f64;
    frac.clamp(0.0, 0.99)
}

/// The remaining-seconds ETA for `elapsed` against `avg`. PURE. Saturating: a
/// wake that's run past the learned average reports 0 (it's overdue, not
/// negative).
#[must_use]
pub fn wake_eta_secs(elapsed: u64, avg: u64) -> u64 {
    avg.saturating_sub(elapsed)
}

/// Handle a `wake-eta` request: from the recorded `samples` + `elapsed`, compute
/// the learned average, phase, progress fraction, and remaining ETA. PURE math —
/// no I/O. Reply
/// `{"ok":true,"avg":A,"phase":"…","progress":F,"eta":E,"ready":<bool>}`.
fn wake_eta(req_body: Option<&str>) -> Result<serde_json::Value, String> {
    let Some(body) = req_body else {
        return Err("wake-eta: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("wake-eta: bad json: {e}"))?;
    let samples: Vec<u64> = req
        .get("samples")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(serde_json::Value::as_u64)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let elapsed = req
        .get("elapsed")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    let avg = rolling_avg_wake_secs(&samples);
    let phase = wake_phase(elapsed, avg);
    Ok(json!({
        "ok": true,
        "avg": avg,
        "phase": phase.slug(),
        "progress": wake_progress(elapsed, avg),
        "eta": wake_eta_secs(elapsed, avg),
        "ready": phase == WakePhase::Ready,
    }))
}

// ────────────────────────────── Dispatch ────────────────────────────────────

/// Build the reply for one `action/dc/<verb>` request.
#[must_use]
pub fn build_reply(_svc: &DcPowerService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    match verb {
        "wol" => {
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
            match wol_wake(mac) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(e) => err(e),
            }
        }
        "ipmi-power" => match ipmi_power(req_body) {
            Ok((line, via)) => json!({ "ok": true, "result": line, "via": via }).to_string(),
            Err(m) => err(m),
        },
        "idle-policy" => match idle_policy(req_body) {
            Ok((running, shutdown, reason)) => {
                json!({ "ok": true, "running": running, "shutdown": shutdown, "reason": reason })
                    .to_string()
            }
            Err(m) => err(m),
        },
        "wake-eta" => match wake_eta(req_body) {
            Ok(v) => v.to_string(),
            Err(m) => err(m),
        },
        _ => err("unknown dc verb".into()),
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
                build_authorized_reply(svc, verb, msg.body.as_deref())
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::authorize_test_body;

    const AUTH_KEY: &[u8] = b"dc-power-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn authorized_service(root: &std::path::Path) -> DcPowerService {
        DcPowerService::new(root.to_path_buf()).with_authorizer(Arc::new(
            ActionAuthorizer::for_test(AUTH_KEY, root.join("auth"), AUTH_NOW),
        ))
    }

    fn power_context(verb: &'static str, target: &'static str) -> MutationContext<'static> {
        MutationContext {
            verb,
            node: DC_ACTION_NODE_SCOPE,
            target,
        }
    }

    #[test]
    fn topic_and_verbs_lock() {
        assert_eq!(action_topic("wol"), "action/dc/wol");
        assert_eq!(action_topic("ipmi-power"), "action/dc/ipmi-power");
        assert_eq!(action_topic("idle-policy"), "action/dc/idle-policy");
        assert_eq!(action_topic("wake-eta"), "action/dc/wake-eta");
        for v in ["wol", "ipmi-power", "idle-policy", "wake-eta"] {
            assert!(ACTION_VERBS.contains(&v), "missing verb {v}");
        }
    }

    #[test]
    fn wake_eta_is_open_but_every_privileged_power_verb_is_classified() {
        assert_eq!(mutation_target("wake-eta", None), Ok(None));
        assert_eq!(
            mutation_target(
                "wol",
                Some(&json!({ "mac": "AA-BB-CC-DD-EE-FF" }).to_string())
            ),
            Ok(Some("host-mac:aa:bb:cc:dd:ee:ff".to_string()))
        );
        assert_eq!(
            mutation_target(
                "ipmi-power",
                Some(&json!({ "bmc": "BMC-01.Local" }).to_string())
            ),
            Ok(Some("bmc:bmc-01.local".to_string()))
        );
        assert_eq!(
            mutation_target(
                "idle-policy",
                Some(&json!({ "dom0": "172.20.0.9" }).to_string())
            ),
            Ok(Some("host:172.20.0.9".to_string()))
        );
    }

    #[test]
    fn unsigned_tampered_replayed_and_future_schema_power_actions_never_reach_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = authorized_service(tmp.path());
        let unsigned_requests = [
            (
                "wol",
                json!({ "schema_version": 1, "mac": "aa:bb:cc:dd:ee:ff" }).to_string(),
            ),
            (
                "ipmi-power",
                json!({
                    "schema_version": 1,
                    "bmc": "bmc-01.local",
                    "user": "ADMIN",
                    "pass": "secret",
                    "op": "on",
                })
                .to_string(),
            ),
            (
                "idle-policy",
                json!({ "schema_version": 1, "dom0": "172.20.0.9" }).to_string(),
            ),
        ];
        for (verb, body) in &unsigned_requests {
            assert!(
                authorize_mutation(&svc, verb, Some(body)).is_err(),
                "{verb}"
            );
            assert!(
                build_authorized_reply(&svc, verb, Some(body)).contains("authorization refused"),
                "{verb} must return before its backend"
            );
        }

        let unsigned_wol = &unsigned_requests[0].1;
        let armed = authorize_test_body(
            AUTH_KEY,
            unsigned_wol,
            power_context("wol", "host-mac:aa:bb:cc:dd:ee:ff"),
            "power-once",
            AUTH_NOW + 30_000,
        );
        let tampered = armed.replace("aa:bb:cc:dd:ee:ff", "aa:bb:cc:dd:ee:00");
        assert!(authorize_mutation(&svc, "wol", Some(&tampered)).is_err());
        assert!(authorize_mutation(&svc, "wol", Some(&armed)).is_ok());
        assert!(authorize_mutation(&svc, "wol", Some(&armed))
            .unwrap_err()
            .contains("already used"));

        let future = json!({
            "schema_version": 2,
            "dom0": "172.20.0.9",
        })
        .to_string();
        let future = authorize_test_body(
            AUTH_KEY,
            &future,
            power_context("idle-policy", "host:172.20.0.9"),
            "power-future",
            AUTH_NOW + 30_000,
        );
        assert!(authorize_mutation(&svc, "idle-policy", Some(&future))
            .unwrap_err()
            .contains("schema_version 1"));
    }

    // ── WoL ──────────────────────────────────────────────────────────────────

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
        assert_eq!(&p[6..12], &mac);
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
    fn bad_macs_rejected() {
        assert!(build_magic_packet("aa:bb:cc:dd:ee").is_err());
        assert!(build_magic_packet("aa:bb:cc:dd:ee:ff:00").is_err());
        assert!(build_magic_packet("aa:bb:cc:dd:ee:zz").is_err());
        assert!(build_magic_packet("a:bb:cc:dd:ee:ff").is_err());
        assert!(build_magic_packet("aabbccddeeff").is_err());
        assert!(build_magic_packet("aa:bb-cc:dd:ee:ff").is_err());
        assert!(build_magic_packet("").is_err());
    }

    // ── IPMI ─────────────────────────────────────────────────────────────────

    #[test]
    fn ipmi_verb_maps_known_ops() {
        assert_eq!(ipmi_power_verb("on").unwrap(), "on");
        assert_eq!(ipmi_power_verb("off").unwrap(), "off");
        assert_eq!(ipmi_power_verb("cycle").unwrap(), "cycle");
        assert_eq!(ipmi_power_verb("status").unwrap(), "status");
    }

    #[test]
    fn ipmi_verb_rejects_unknown_and_injection() {
        assert!(ipmi_power_verb("reset").is_err());
        assert!(ipmi_power_verb("on; rm -rf /").is_err());
        assert!(ipmi_power_verb("").is_err());
    }

    #[test]
    fn bmc_host_guard() {
        assert!(valid_bmc_host("172.20.0.9"));
        assert!(valid_bmc_host("bmc-host1.local"));
        assert!(!valid_bmc_host(""));
        assert!(!valid_bmc_host("172.20.0.9; reboot"));
        assert!(!valid_bmc_host("$(whoami)"));
        assert!(!valid_bmc_host("a b"));
        assert!(!valid_bmc_host("a`b`"));
    }

    #[test]
    fn ipmi_cred_guard() {
        assert!(valid_ipmi_cred("ADMIN", "secret"));
        assert!(valid_ipmi_cred("ADMIN", "")); // empty pass allowed (some BMCs)
        assert!(!valid_ipmi_cred("", "secret")); // user required
        assert!(!valid_ipmi_cred("AD\nMIN", "secret")); // no control chars
        assert!(!valid_ipmi_cred("ADMIN", "se\tcret"));
    }

    #[test]
    fn ipmi_power_rejects_bad_body_and_args() {
        let s = DcPowerService::new(PathBuf::from("/tmp"));
        assert!(build_reply(&s, "ipmi-power", None).contains("missing request body"));
        assert!(build_reply(&s, "ipmi-power", Some("nope")).contains("bad json"));
        let r = build_reply(
            &s,
            "ipmi-power",
            Some(&json!({ "op": "reset" }).to_string()),
        );
        assert!(r.contains("unknown ipmi op"), "{r}");
        let r = build_reply(
            &s,
            "ipmi-power",
            Some(&json!({ "op": "on", "bmc": "x;y" }).to_string()),
        );
        assert!(r.contains("bmc must be"), "{r}");
        let r = build_reply(
            &s,
            "ipmi-power",
            Some(&json!({ "op": "on", "bmc": "10.0.0.1", "user": "" }).to_string()),
        );
        assert!(r.contains("user/pass invalid"), "{r}");
    }

    // ── Idle-shutdown policy ─────────────────────────────────────────────────

    #[test]
    fn idle_decision_keeps_host_with_running_guests() {
        let (shut, why) = idle_shutdown_decision(3, 9_999, 0);
        assert!(!shut);
        assert!(why.contains("3 running"), "{why}");
    }

    #[test]
    fn idle_decision_waits_for_hold_window() {
        // Idle now (0 running) but not idle long enough → wait.
        let (shut, why) = idle_shutdown_decision(0, 30, 120);
        assert!(!shut);
        assert!(why.contains("wait"), "{why}");
    }

    #[test]
    fn idle_decision_shuts_down_a_long_idle_empty_host() {
        let (shut, why) = idle_shutdown_decision(0, 300, 120);
        assert!(shut);
        assert!(why.contains("shut down"), "{why}");
    }

    #[test]
    fn idle_decision_no_hold_window_shuts_immediately() {
        let (shut, _) = idle_shutdown_decision(0, 0, 0);
        assert!(shut);
    }

    #[test]
    fn running_count_parsing() {
        assert_eq!(parse_running_count(""), 0);
        assert_eq!(parse_running_count("   "), 0);
        assert_eq!(parse_running_count("a"), 1);
        assert_eq!(parse_running_count("a,b,c"), 3);
        assert_eq!(parse_running_count(" a , b "), 2);
    }

    #[test]
    fn idle_policy_rejects_bad_body() {
        let s = DcPowerService::new(PathBuf::from("/tmp"));
        assert!(build_reply(&s, "idle-policy", None).contains("missing request body"));
        assert!(build_reply(&s, "idle-policy", Some("nope")).contains("bad json"));
        let r = build_reply(&s, "idle-policy", Some(&json!({}).to_string()));
        assert!(r.contains("missing dom0"), "{r}");
        // A dom0 not in the (empty by default) allow-list is rejected before SSH.
        let r = build_reply(
            &s,
            "idle-policy",
            Some(&json!({ "dom0": "10.9.9.9" }).to_string()),
        );
        assert!(r.contains("not in allowed set"), "{r}");
    }

    // ── Learned ETA / phased progress ────────────────────────────────────────

    #[test]
    fn rolling_avg_defaults_then_averages() {
        assert_eq!(rolling_avg_wake_secs(&[]), DEFAULT_WAKE_SECS);
        assert_eq!(rolling_avg_wake_secs(&[100]), 100);
        assert_eq!(rolling_avg_wake_secs(&[100, 200]), 150);
        // Rounds to nearest.
        assert_eq!(rolling_avg_wake_secs(&[100, 101]), 101);
        assert!(rolling_avg_wake_secs(&[0, 0]) >= 1, "never below 1s");
    }

    #[test]
    fn rolling_avg_windows_recent_samples() {
        // 12 samples: the window keeps only the last 10, so the two leading
        // outliers don't drag the average.
        let mut s = vec![1000, 1000];
        s.extend(std::iter::repeat_n(100, 10));
        assert_eq!(rolling_avg_wake_secs(&s), 100);
    }

    #[test]
    fn wake_phase_sequences_post_xcp_toolstack_ready() {
        let avg = 100;
        assert_eq!(wake_phase(0, avg), WakePhase::Post);
        assert_eq!(wake_phase(20, avg), WakePhase::Post);
        assert_eq!(wake_phase(30, avg), WakePhase::Xcp);
        assert_eq!(wake_phase(69, avg), WakePhase::Xcp);
        assert_eq!(wake_phase(70, avg), WakePhase::Toolstack);
        assert_eq!(wake_phase(99, avg), WakePhase::Toolstack);
        assert_eq!(wake_phase(100, avg), WakePhase::Ready);
        assert_eq!(wake_phase(200, avg), WakePhase::Ready);
    }

    #[test]
    fn wake_progress_never_claims_full_before_ready() {
        assert_eq!(wake_progress(0, 100), 0.0);
        assert!((wake_progress(50, 100) - 0.5).abs() < 1e-9);
        // Clamped to 0.99 until the XAPI poll confirms ready.
        assert!((wake_progress(100, 100) - 0.99).abs() < 1e-9);
        assert!((wake_progress(500, 100) - 0.99).abs() < 1e-9);
    }

    #[test]
    fn wake_eta_saturates_at_zero() {
        assert_eq!(wake_eta_secs(0, 180), 180);
        assert_eq!(wake_eta_secs(60, 180), 120);
        assert_eq!(wake_eta_secs(180, 180), 0);
        assert_eq!(wake_eta_secs(500, 180), 0);
    }

    #[test]
    fn phase_slugs_are_stable() {
        assert_eq!(WakePhase::Post.slug(), "post");
        assert_eq!(WakePhase::Xcp.slug(), "xcp");
        assert_eq!(WakePhase::Toolstack.slug(), "toolstack");
        assert_eq!(WakePhase::Ready.slug(), "ready");
    }

    #[test]
    fn wake_eta_verb_computes_from_samples() {
        let s = DcPowerService::new(PathBuf::from("/tmp"));
        let body = json!({ "samples": [120, 120], "elapsed": 60 }).to_string();
        let r = build_reply(&s, "wake-eta", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["avg"], json!(120));
        assert_eq!(v["eta"], json!(60));
        assert_eq!(v["phase"], json!("xcp"));
        assert_eq!(v["ready"], json!(false));
    }

    #[test]
    fn wake_eta_verb_marks_ready_past_average() {
        let s = DcPowerService::new(PathBuf::from("/tmp"));
        let body = json!({ "samples": [60], "elapsed": 90 }).to_string();
        let r = build_reply(&s, "wake-eta", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ready"], json!(true));
        assert_eq!(v["eta"], json!(0));
        assert_eq!(v["phase"], json!("ready"));
    }

    #[test]
    fn wake_eta_verb_rejects_bad_body() {
        let s = DcPowerService::new(PathBuf::from("/tmp"));
        assert!(build_reply(&s, "wake-eta", None).contains("missing request body"));
        assert!(build_reply(&s, "wake-eta", Some("nope")).contains("bad json"));
    }

    #[test]
    fn unknown_verb_errors() {
        let s = DcPowerService::new(PathBuf::from("/tmp"));
        assert!(build_reply(&s, "bogus", None).contains("unknown dc verb"));
    }
}
