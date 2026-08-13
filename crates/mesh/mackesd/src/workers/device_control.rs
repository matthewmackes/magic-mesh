//! DEVMGR-8 — the **device-control executor**: the privileged node-side seam that
//! makes DEVMGR-7's omitted hardware-mutating verbs (Enable/Disable, reload kernel
//! module, rescan bus) real.
//!
//! `mde-shell-egui`'s Device-Manager surface is a §6 consumer of the published
//! `device-inventory/<host>.json` — it holds **no privileged-exec seam**, so
//! DEVMGR-7 honestly OMITTED the mutating verbs. This worker is that seam. It polls
//! this host's replicated `<root>/fleet/device-control/<self>/` for typed
//! [`DeviceControlRequest`]s (written by any seat's Device-Manager, carried by
//! Syncthing replication — the identical PD-11 `lifecycle` transport, so a remote
//! action reaches the target with **no push-SSH**, §9), and for each one:
//!
//! 1. **Gates** the device against **what this box actually publishes** (the L9
//!    rail, exactly like `lifecycle_exec`'s live-probe gate): a request naming a
//!    device absent from this node's own inventory is refused, never acted on.
//! 2. **Plans** the op onto a FIXED real seam ([`command_plan`], pure): a sysfs
//!    write (driver `bind`/`unbind`, USB `authorized`, the PCI bus `rescan`) or a
//!    FIXED binary (`ip link set`, `rmmod`, `modprobe`). There is no command
//!    string (§9) — an op inapplicable to a device kind is a **typed error**, not
//!    a fabricated success (§7).
//! 3. **Executes** it for real, capturing the sysfs/`modprobe` stderr.
//! 4. **Audits** every op (success OR refusal) on the KDC hash-chained `events`
//!    plane ([`crate::events::append_and_alert`], `AdminAction`), the same chain
//!    the `action`/reconcile writers append to (§8).
//! 5. **Notifies on failure** — publishes an alert on `event/notify/device-control`
//!    (the lane the `chat` worker folds, mirroring `node_grade`) so a failed
//!    hardware op reaches the operator's Chat, never silently.
//! 6. Writes a typed [`DeviceControlResult`] back for the requester to poll.
//!
//! Rank-0 / universal (every node can be a device-action target); no leader gate
//! is needed — a request is drained ONLY by the node whose `<self>` dir it lands
//! in, so exactly-once is structural. Every failure path degrades to a typed
//! result + a log line — the worker never panics, mirroring `lifecycle_exec`.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mackes_mesh_types::device_control::{
    authorization_target, claim_pending_cancellation, take_cancellations, take_requests,
    write_result, DeviceControlCancellation, DeviceControlOp, DeviceControlRequest,
    DeviceControlResult, DeviceTarget, DEVICE_CONTROL_AUTH_VERB, DEVICE_CONTROL_CANCEL_AUTH_VERB,
    DEVICE_CONTROL_SCHEMA_VERSION,
};
use mackes_mesh_types::device_inventory;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

/// Request poll cadence — an op lands within ~3 s of replication (as `lifecycle_exec`).
pub const POLL: Duration = Duration::from_secs(3);

/// A fixed device-control helper must not be able to wedge this worker forever.
/// In particular, a stuck `rmmod`/`modprobe` would otherwise prevent later
/// cancellations and recovery actions from being observed.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// The Bus lane a device-op FAILURE alert rides.
///
/// Folded by the `chat` worker ([`super::chat::ALERT_LANE_PREFIXES`] carries
/// `event/notify/`) into this node's `alert:<self>` conversation, so a failed
/// hardware op reaches the Chat feed (design #21), mirroring `node_grade`.
pub const NOTIFY_TOPIC: &str = "event/notify/device-control";

/// The stable `source` token on the published alert body (the Chat card badge).
pub const NOTIFY_SOURCE: &str = "device-control";

/// One concrete execution step the plan runs (§9 — a FIXED sysfs write or a FIXED
/// binary, never a shell/command string). A single op maps to one or more steps
/// (module reload = `rmmod` then `modprobe`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecStep {
    /// Write `contents` to a sysfs control file (driver bind/unbind, USB
    /// `authorized`, a bus `rescan`).
    SysfsWrite {
        /// The sysfs control file to write.
        path: PathBuf,
        /// The bytes to write (a BDF for bind/unbind, `0`/`1`, `1` for rescan).
        contents: String,
    },
    /// Run a FIXED binary with a closed arg vector (`ip`/`rmmod`/`modprobe`) — the
    /// binary is a literal, the only variable args are validated device fields.
    Command {
        /// The literal binary name (§9 — never a request-supplied string).
        bin: &'static str,
        /// The closed arg vector.
        args: Vec<String>,
    },
}

impl ExecStep {
    /// A fixed-binary step from `&str` args (mapped to owned).
    fn command(bin: &'static str, args: &[&str]) -> Self {
        Self::Command {
            bin,
            args: args.iter().map(|a| (*a).to_string()).collect(),
        }
    }
}

/// A parsed device sysfs anchor — the pieces the seams derive from. `prefix` is
/// everything up to (not incl.) the `bus` segment, kept so the driver/rescan
/// sibling paths resolve under the SAME sysfs root (real `/sys`, or a tempdir in
/// tests) rather than a hardcoded `/sys`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Anchor {
    /// `<prefix>/bus/<bus>/devices/<bdf>` — the general bus-device form (PCI, USB,
    /// platform). Driver bind/unbind lives at `<prefix>/bus/<bus>/drivers/<drv>/…`,
    /// bus rescan at `<prefix>/bus/<bus>/rescan`.
    BusDevice {
        prefix: PathBuf,
        bus: String,
        bdf: String,
        device_path: PathBuf,
    },
    /// `<prefix>/class/net/<ifname>` — a netdev anchored directly under class/net
    /// (`ip link set <ifname> up/down`).
    NetClass { ifname: String },
}

/// Parse a device sysfs path into the anchor the seams need, or `None` for a path
/// with no recognizable bus/net anchor (a CPU/memory/thermal record — those refuse
/// enable/disable honestly). Pure over its input, so the seam mapping is tested
/// without a real `/sys`.
fn parse_anchor(sysfs_path: &str) -> Option<Anchor> {
    let path = Path::new(sysfs_path);
    let comps: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str().map(str::to_string),
            _ => None,
        })
        .collect();
    // `<…>/bus/<bus>/devices/<bdf>`
    if let Some(i) = comps.iter().position(|c| c == "bus") {
        if comps.get(i + 2).map(String::as_str) == Some("devices") {
            if let (Some(bus), Some(bdf)) = (comps.get(i + 1), comps.get(i + 3)) {
                // Rebuild the absolute prefix up to (not incl.) `bus`, preserving a
                // leading `/` (sysfs paths are absolute).
                let mut prefix = PathBuf::from("/");
                for c in &comps[..i] {
                    prefix.push(c);
                }
                return Some(Anchor::BusDevice {
                    prefix,
                    bus: bus.clone(),
                    bdf: bdf.clone(),
                    device_path: path.to_path_buf(),
                });
            }
        }
    }
    // `<…>/class/net/<ifname>`
    if let Some(i) = comps.iter().position(|c| c == "class") {
        if comps.get(i + 1).map(String::as_str) == Some("net") {
            if let Some(ifname) = comps.get(i + 2) {
                return Some(Anchor::NetClass {
                    ifname: ifname.clone(),
                });
            }
        }
    }
    None
}

/// Map a typed op + device onto the FIXED real seam(s) that run it (pure).
///
/// The worker executes what this returns. An op inapplicable to the device's kind
/// (a reload with no bound module, an enable/disable with no bus/net anchor, a
/// rescan on a bus with no rescan node) is a **typed error**, never a fabricated
/// plan (§7). §9: every step is a fixed sysfs write or a fixed binary.
///
/// # Errors
/// A human-readable reason suitable for a [`DeviceControlResult::failed`] `error`.
pub fn command_plan(op: DeviceControlOp, target: &DeviceTarget) -> Result<Vec<ExecStep>, String> {
    match op {
        DeviceControlOp::ReloadModule => {
            let module = non_empty(target.driver.as_deref()).ok_or_else(|| {
                "reload-module: the device has no bound driver/module to reload".to_string()
            })?;
            validate_driver_name(module)?;
            // rmmod then modprobe — the honest module bounce. If the module is
            // in-use rmmod fails and the executor surfaces its stderr (§7).
            Ok(vec![
                ExecStep::command("rmmod", &[module]),
                ExecStep::command("modprobe", &[module]),
            ])
        }
        DeviceControlOp::RescanBus => {
            let sysfs = non_empty(target.sysfs_path.as_deref())
                .ok_or_else(|| "rescan-bus: the device carries no sysfs anchor".to_string())?;
            match parse_anchor(sysfs) {
                Some(Anchor::BusDevice { prefix, bus, .. }) => {
                    // Only PCI exposes a bus-level rescan node (`/sys/bus/pci/rescan`);
                    // USB/platform have none — refuse honestly rather than write a
                    // path that does not exist (§7).
                    if bus != "pci" {
                        return Err(format!(
                            "rescan-bus: the `{bus}` bus exposes no rescan node (only PCI does)"
                        ));
                    }
                    let rescan = prefix.join("bus").join(&bus).join("rescan");
                    Ok(vec![ExecStep::SysfsWrite {
                        path: rescan,
                        contents: "1".to_string(),
                    }])
                }
                _ => Err(format!(
                    "rescan-bus: no PCI bus anchor in `{sysfs}` (only PCI devices rescan)"
                )),
            }
        }
        DeviceControlOp::Enable | DeviceControlOp::Disable => {
            let up = matches!(op, DeviceControlOp::Enable);
            let sysfs = non_empty(target.sysfs_path.as_deref()).ok_or_else(|| {
                format!(
                    "{}: the device carries no sysfs anchor (not applicable to this kind)",
                    op.as_str()
                )
            })?;
            let anchor = parse_anchor(sysfs).ok_or_else(|| {
                format!(
                    "{}: unrecognized sysfs anchor `{sysfs}` (no bus/net enable seam)",
                    op.as_str()
                )
            })?;
            match anchor {
                // A netdev under class/net: `ip link set <if> up|down`.
                Anchor::NetClass { ifname } => Ok(vec![ExecStep::command(
                    "ip",
                    &["link", "set", &ifname, if up { "up" } else { "down" }],
                )]),
                Anchor::BusDevice {
                    prefix,
                    bus,
                    bdf,
                    device_path,
                } => {
                    if bus == "usb" {
                        // USB: the honest enable/disable seam is the `authorized`
                        // toggle on the device node itself.
                        Ok(vec![ExecStep::SysfsWrite {
                            path: device_path.join("authorized"),
                            contents: (if up { "1" } else { "0" }).to_string(),
                        }])
                    } else {
                        // PCI/platform: driver bind/unbind (unbinding disables the
                        // device; binding needs a known driver — the record's
                        // currently/last-bound one). No driver ⇒ honest refusal.
                        let driver = non_empty(target.driver.as_deref()).ok_or_else(|| {
                            format!(
                                "{}: `{bdf}` has no bound driver — the bind/unbind seam needs one",
                                op.as_str()
                            )
                        })?;
                        validate_driver_name(driver)?;
                        let node = if up { "bind" } else { "unbind" };
                        let bind_path = prefix
                            .join("bus")
                            .join(&bus)
                            .join("drivers")
                            .join(driver)
                            .join(node);
                        Ok(vec![ExecStep::SysfsWrite {
                            path: bind_path,
                            contents: bdf,
                        }])
                    }
                }
            }
        }
    }
}

/// A trimmed non-empty view of an optional string (an all-whitespace field reads
/// as absent — an honest missing anchor, not a blank one).
fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// Admit only one bounded Linux module identifier before using provider data as
/// either a fixed-command argument or a sysfs path component. In particular,
/// leading option markers and path separators must never let a hostile provider
/// reinterpret a privileged helper invocation or escape the driver directory.
fn validate_driver_name(driver: &str) -> Result<(), String> {
    let bytes = driver.as_bytes();
    if bytes.len() > 63
        || !bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(format!(
            "driver `{driver}` is not a bounded kernel module identifier — refused before mutation"
        ));
    }
    Ok(())
}

/// The device-control executor worker.
pub struct DeviceControlExecWorker {
    /// Replicated workgroup root — the `fleet/device-control/<self>/` request dir
    /// AND the `device-inventory/<self>.json` the L9 gate reads.
    workgroup_root: PathBuf,
    /// This node's short hostname — gates which requests are drained (only those
    /// addressed to this box) + the inventory stem for the offered-gate.
    self_hostname: String,
    /// This node's id — the audit actor + the notify source.
    node_id: String,
    /// The hash-chained audit DB (the `events` table). Defaults to
    /// [`crate::default_db_path`]; tests point it at a tempdir.
    db_path: PathBuf,
    /// Override the Bus spool root for the failure notify. Tests point at a tempdir.
    bus_root_override: Option<PathBuf>,
    /// Canonical exact-body privileged-action verifier. Missing credentials
    /// install a fail-closed verifier; the worker never mints capabilities.
    authorizer: Arc<ActionAuthorizer>,
}

impl DeviceControlExecWorker {
    /// Construct with production defaults: the canonical audit DB path + the default
    /// Bus root for failure notifies.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, self_hostname: String, node_id: String) -> Self {
        Self {
            workgroup_root,
            self_hostname,
            node_id,
            db_path: crate::default_db_path(),
            bus_root_override: None,
            authorizer: Arc::new(ActionAuthorizer::production()),
        }
    }

    /// Override the audit DB path (tests point at a tempdir).
    #[must_use]
    pub fn with_db_path(mut self, p: PathBuf) -> Self {
        self.db_path = p;
        self
    }

    /// Override the notify Bus root (tests point at a tempdir).
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Inject deterministic verifier/nonce-ledger state for hostile tests.
    #[cfg(test)]
    #[must_use]
    fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// The L9 rail (as `lifecycle_exec::offered`): is this exact target actually
    /// one this box **publishes**? Every control-relevant field must match one
    /// provider-owned category/device record. A matching sysfs path alone is not
    /// enough: otherwise a requester could borrow a real device's path while
    /// substituting another category, display name, or kernel module. A host that
    /// has published no inventory yet refuses everything (conservative +
    /// self-healing once it publishes).
    fn offered(
        &self,
        target: &DeviceTarget,
        expected_published_at_ms: u64,
    ) -> Option<device_inventory::DeviceRecord> {
        let Some(inv) = device_inventory::read_inventory(&self.workgroup_root, &self.self_hostname)
        else {
            return None;
        };
        if expected_published_at_ms == 0 || inv.published_at_ms != expected_published_at_ms {
            return None;
        }
        inv.categories
            .iter()
            .filter(|category| category.key == target.category)
            .flat_map(|category| category.devices.iter())
            .find(|device| {
                device.name == target.name
                    && device.sysfs_path == target.sysfs_path
                    && device.driver == target.driver
            })
            .cloned()
    }

    /// A provider-owned record with no resolved state is not safe to mutate.
    /// This applies to every control verb: a module reload and a bus rescan are
    /// mutations too, even though they do not look like a simple enable/disable
    /// toggle. Keep known operational states (for example a network link
    /// explicitly reported `down`) actionable, while refusing every unknown
    /// device. A problem string explains the refusal; it does not establish
    /// capability or make an unresolved provider state safe.
    fn admit_control_state(
        op: DeviceControlOp,
        device: &device_inventory::DeviceRecord,
    ) -> Result<(), String> {
        if device.status == device_inventory::DeviceStatus::Unknown {
            return Err(format!(
                "{}: provider state for `{}` is unavailable — refused before mutation",
                op.as_str(),
                device.name
            ));
        }
        Ok(())
    }

    /// Re-attest the exact preview generation and device identity immediately
    /// before executing an already-authorized mutation.
    fn re_attest_authorized_request(
        &self,
        req: &DeviceControlRequest,
    ) -> Result<device_inventory::DeviceRecord, String> {
        let Some(device) = self.offered(&req.target, req.expected_inventory_published_at_ms) else {
            return Err(format!(
                "device `{}` left {}'s authorized inventory generation {} before execution — refused before mutation",
                req.target.name, self.self_hostname, req.expected_inventory_published_at_ms,
            ));
        };
        Self::admit_control_state(req.op, &device)?;
        Ok(device)
    }

    /// Handle one request: gate → plan → execute. Returns the typed result WITHOUT
    /// side-effecting the audit/notify (those wrap it in [`Self::process`]) so the
    /// gate + plan logic is unit-testable in isolation.
    async fn handle_request(&self, req: &DeviceControlRequest) -> DeviceControlResult {
        if req.schema_version != DEVICE_CONTROL_SCHEMA_VERSION {
            return DeviceControlResult::failed(
                &req.id,
                format!(
                    "unsupported device-control request schema {} (expected {}) — refused before mutation",
                    req.schema_version, DEVICE_CONTROL_SCHEMA_VERSION
                ),
            );
        }
        if req.target_host != self.self_hostname {
            return DeviceControlResult::failed(
                &req.id,
                format!(
                    "request targets `{}` but this provider owns `{}` — refused",
                    req.target_host, self.self_hostname
                ),
            );
        }
        let Some(device) = self.offered(&req.target, req.expected_inventory_published_at_ms) else {
            return DeviceControlResult::failed(
                &req.id,
                format!(
                    "device `{}` is not in {}'s expected inventory generation {} — refused (L9 rail)",
                    req.target.name,
                    self.self_hostname,
                    req.expected_inventory_published_at_ms,
                ),
            );
        };
        if let Err(reason) = Self::admit_control_state(req.op, &device) {
            return DeviceControlResult::failed(&req.id, reason);
        }
        let steps = match command_plan(req.op, &req.target) {
            Ok(s) => s,
            Err(reason) => return DeviceControlResult::failed(&req.id, reason),
        };
        let target = match authorization_target(&req.id) {
            Ok(target) => target,
            Err(reason) => return DeviceControlResult::failed(&req.id, reason),
        };
        let body = match serde_json::to_string(req) {
            Ok(body) => body,
            Err(_) => {
                return DeviceControlResult::failed(
                    &req.id,
                    "device-control request could not be authorized",
                );
            }
        };
        if let Err(reason) = self.authorizer.authorize(
            &body,
            MutationContext {
                verb: DEVICE_CONTROL_AUTH_VERB,
                node: &self.self_hostname,
                target: &target,
            },
        ) {
            return DeviceControlResult::failed(
                &req.id,
                format!("device-control authorization refused: {reason}"),
            );
        }
        // Authorization can involve durable nonce-ledger I/O. The inventory may
        // advance while that work is in flight, so the earlier preview gate is
        // not sufficient authority for the eventual hardware effect. Re-read
        // the provider publication at the last possible point and require the
        // exact previewed generation and control identity to remain actionable.
        // A consumed capability is intentionally not reusable when this gate
        // fails: the operator must preview and authorize the new generation.
        if let Err(reason) = self.re_attest_authorized_request(req) {
            return DeviceControlResult::failed(&req.id, reason);
        }
        match execute_plan(&steps).await {
            Ok(note) => DeviceControlResult::ok(
                &req.id,
                format!("{} on {}: {note}", req.op.as_str(), req.target.name),
            ),
            Err(err) => DeviceControlResult::failed(&req.id, err),
        }
    }

    /// Handle one request end-to-end: [`Self::handle_request`] → hash-chain audit
    /// (every op, success OR refusal, §8) → a failure notify (§21) → the typed
    /// result. Every request is audited and never panics.
    async fn process(&self, req: &DeviceControlRequest) -> DeviceControlResult {
        let result = self.handle_request(req).await;
        self.audit(req, &result);
        if !result.ok {
            self.notify_failure(req, &result.error);
        }
        result
    }

    /// Authorize and atomically claim one exact queued request. Once the normal
    /// drain has removed a request, this path can only return `NotPending`; it
    /// never reports cancellation for an effect that may have started.
    fn process_cancellation(&self, cancel: &DeviceControlCancellation) -> DeviceControlResult {
        // A refused/late cancellation answers the cancellation id, never the
        // original id: otherwise an unsigned or late marker could overwrite the
        // authoritative execution result. Only a successful atomic claim
        // terminates the original request id as `Cancelled`.
        let refused = |reason: String| DeviceControlResult::not_pending(&cancel.id, reason);
        let result = if cancel.schema_version != DEVICE_CONTROL_SCHEMA_VERSION {
            refused("unsupported device-control cancellation schema".into())
        } else if cancel.target_host != self.self_hostname {
            refused(format!(
                "cancellation targets `{}` but this provider owns `{}`",
                cancel.target_host, self.self_hostname
            ))
        } else {
            let capability_target = authorization_target(&cancel.id);
            let body = serde_json::to_string(cancel);
            match (capability_target, body) {
                (Ok(target), Ok(body)) => match self.authorizer.authorize(
                    &body,
                    MutationContext {
                        verb: DEVICE_CONTROL_CANCEL_AUTH_VERB,
                        node: &self.self_hostname,
                        target: &target,
                    },
                ) {
                    Ok(()) => match claim_pending_cancellation(&self.workgroup_root, cancel) {
                        Ok(request) => DeviceControlResult::cancelled(
                            &request.id,
                            format!(
                                "cancelled {} on {} before execution",
                                request.op.as_str(),
                                request.target.name
                            ),
                        ),
                        Err(reason) => refused(reason.into()),
                    },
                    Err(reason) => refused(format!(
                        "device-control cancellation authorization refused: {reason}"
                    )),
                },
                _ => refused("device-control cancellation could not be authorized".into()),
            }
        };

        let detail = serde_json::json!({
            "action": "device-control-cancel",
            "cancellation_id": cancel.id,
            "target_request_id": cancel.target_request_id,
            "result_id": result.id,
            "op": cancel.op.as_str(),
            "target_host": cancel.target_host,
            "device": cancel.target.name,
            "from": cancel.from,
            "outcome": result.outcome,
            "detail": result.detail,
            "error": result.error,
        });
        crate::events::append_and_alert(
            &self.db_path,
            &self.node_id,
            crate::events::EventKind::AdminAction,
            detail,
        );
        result
    }

    /// Write the hash-chain audit row for one op through the EXISTING audit plane
    /// (best-effort — `append_and_alert` logs + swallows a store fault, so an audit
    /// hiccup never wedges the op lane).
    fn audit(&self, req: &DeviceControlRequest, result: &DeviceControlResult) {
        let detail = serde_json::json!({
            "action": "device-control",
            "request_id": req.id,
            "result_id": result.id,
            "op": req.op.as_str(),
            "target_host": req.target_host,
            "expected_inventory_published_at_ms": req.expected_inventory_published_at_ms,
            "device": req.target.name,
            "sysfs_path": req.target.sysfs_path,
            "driver": req.target.driver,
            "from": req.from,
            "outcome": result.outcome,
            "ok": result.ok,
            "detail": result.detail,
            "error": result.error,
        });
        crate::events::append_and_alert(
            &self.db_path,
            &self.node_id,
            crate::events::EventKind::AdminAction,
            detail,
        );
    }

    /// Publish a FAILURE alert on [`NOTIFY_TOPIC`] (the `chat`-folded lane) so a
    /// failed hardware op reaches the operator's Chat feed (#21), mirroring
    /// `node_grade::emit_alert`. Best-effort (a write hiccup is logged, never fatal).
    fn notify_failure(&self, req: &DeviceControlRequest, error: &str) {
        let Some(root) = self
            .bus_root_override
            .clone()
            .or_else(mde_bus::default_data_dir)
        else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let summary = format!(
            "{} on `{}` failed on {}",
            req.op.label(),
            req.target.name,
            req.target_host
        );
        let body = serde_json::json!({
            "severity": "warning",
            "source": NOTIFY_SOURCE,
            "summary": summary,
            "host": req.target_host,
            "device": req.target.name,
            "op": req.op.as_str(),
            "error": error,
        })
        .to_string();
        if let Err(e) = persist.write(NOTIFY_TOPIC, Priority::Default, None, Some(&body)) {
            tracing::debug!(
                target: "mackesd::device_control",
                topic = NOTIFY_TOPIC,
                error = %e,
                "device-control failure notify publish failed",
            );
        }
    }

    /// Drain + execute every request addressed to this host, writing each result
    /// back for the requester to poll.
    async fn execute_pending(&self) {
        // Cancellations are considered before ordinary claims. The atomic
        // request rename in `claim_pending_cancellation` is the linearization
        // point: success proves the effect has not begun.
        for cancel in take_cancellations(&self.workgroup_root, &self.self_hostname) {
            let result = self.process_cancellation(&cancel);
            let _ = write_result(&self.workgroup_root, &self.self_hostname, &result);
        }
        for req in take_requests(&self.workgroup_root, &self.self_hostname) {
            let result = self.process(&req).await;
            tracing::info!(
                target: "mackesd::device_control",
                id = %req.id, op = %req.op.as_str(), device = %req.target.name,
                target_host = %req.target_host, ok = result.ok,
                "device-control request handled (DEVMGR-8)"
            );
            let _ = write_result(&self.workgroup_root, &self.self_hostname, &result);
        }
    }
}

/// Execute a plan step-by-step, capturing the first failure's honest reason.
/// Returns a compact success note (the seams that ran) or the typed error.
async fn execute_plan(steps: &[ExecStep]) -> Result<String, String> {
    execute_plan_with_timeout(steps, COMMAND_TIMEOUT).await
}

/// Execute a plan with an explicit command deadline. The parameter is kept in
/// this small seam so the timeout behavior can be tested without waiting for
/// the production 30-second deadline.
async fn execute_plan_with_timeout(
    steps: &[ExecStep],
    command_timeout: Duration,
) -> Result<String, String> {
    let mut notes = Vec::new();
    for step in steps {
        match step {
            ExecStep::SysfsWrite { path, contents } => {
                // A control path comes from provider-owned inventory, but the
                // replicated tree can be replaced between admission and effect.
                // Open the final component with NOFOLLOW so that replacement by
                // a symlink cannot redirect a privileged write to an arbitrary
                // file. Intermediate sysfs directory links are expected Linux
                // topology; only the actual control endpoint is constrained.
                write_sysfs_control(path, contents)?;
                notes.push(format!("wrote `{contents}` → {}", path.display()));
            }
            ExecStep::Command { bin, args } => {
                let out = tokio::time::timeout(
                    command_timeout,
                    tokio::process::Command::new(*bin)
                        .args(args)
                        .kill_on_drop(true)
                        .output(),
                )
                .await
                .map_err(|_| {
                    format!(
                        "`{bin} {}` timed out after {}s",
                        args.join(" "),
                        command_timeout.as_secs()
                    )
                })?
                .map_err(|e| format!("`{bin}` unavailable: {e}"))?;
                if !out.status.success() {
                    return Err(format!(
                        "`{bin} {}` failed: {}",
                        args.join(" "),
                        String::from_utf8_lossy(&out.stderr).trim()
                    ));
                }
                notes.push(format!("{bin} {}", args.join(" ")));
            }
        }
    }
    Ok(notes.join("; "))
}

/// Write one provider-planned sysfs control without following its final path
/// component. These are existing kernel attributes, so creation is never
/// allowed; the descriptor is also CLOEXEC because the worker may run other
/// fixed command steps in the same action.
fn write_sysfs_control(path: &Path, contents: &str) -> Result<(), String> {
    use rustix::fs::{Mode, OFlags};
    use std::io::Write as _;

    let fd = rustix::fs::open(
        path,
        OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| format!("sysfs write {} failed to open safely: {e}", path.display()))?;
    let mut file: std::fs::File = fd.into();
    file.write_all(contents.as_bytes())
        .map_err(|e| format!("sysfs write {} failed: {e}", path.display()))
}

#[async_trait::async_trait]
impl Worker for DeviceControlExecWorker {
    fn name(&self) -> &'static str {
        "device_control"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            self.execute_pending().await;
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(POLL) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer};
    use mackes_mesh_types::device_control::{
        write_cancellation, write_request, DeviceControlOutcome,
    };
    use mackes_mesh_types::device_inventory::{
        category, DeviceCategory, DeviceInventory, DeviceRecord, DeviceStatus, HostSummary,
        ToolAvailability,
    };

    const AUTH_KEY: &[u8] = b"device-control-shared-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn test_authorizer(root: &Path) -> Arc<ActionAuthorizer> {
        Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            root.join("auth"),
            AUTH_NOW,
        ))
    }

    fn authorize_request(req: &DeviceControlRequest, nonce: &str) -> DeviceControlRequest {
        let unsigned = serde_json::to_string(req).unwrap();
        let target = authorization_target(&req.id).unwrap();
        let armed = authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: DEVICE_CONTROL_AUTH_VERB,
                node: &req.target_host,
                target: &target,
            },
            nonce,
            AUTH_NOW + 30_000,
        );
        serde_json::from_str(&armed).unwrap()
    }

    fn authorize_cancellation(
        cancel: &DeviceControlCancellation,
        nonce: &str,
    ) -> DeviceControlCancellation {
        let unsigned = serde_json::to_string(cancel).unwrap();
        let target = authorization_target(&cancel.id).unwrap();
        let armed = authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: DEVICE_CONTROL_CANCEL_AUTH_VERB,
                node: &cancel.target_host,
                target: &target,
            },
            nonce,
            AUTH_NOW + 30_000,
        );
        serde_json::from_str(&armed).unwrap()
    }

    // ── command_plan: each op maps to its real command / sysfs seam ────────────

    #[test]
    fn disable_pci_device_unbinds_its_driver() {
        let target = DeviceTarget {
            name: "Intel I219-V".into(),
            category: category::NETWORK_ADAPTERS.into(),
            sysfs_path: Some("/sys/bus/pci/devices/0000:00:1f.6".into()),
            driver: Some("e1000e".into()),
        };
        let steps = command_plan(DeviceControlOp::Disable, &target).expect("planned");
        assert_eq!(
            steps,
            vec![ExecStep::SysfsWrite {
                path: PathBuf::from("/sys/bus/pci/drivers/e1000e/unbind"),
                contents: "0000:00:1f.6".into(),
            }]
        );
        // Enable binds the same driver.
        let steps = command_plan(DeviceControlOp::Enable, &target).expect("planned");
        assert_eq!(
            steps,
            vec![ExecStep::SysfsWrite {
                path: PathBuf::from("/sys/bus/pci/drivers/e1000e/bind"),
                contents: "0000:00:1f.6".into(),
            }]
        );
    }

    #[test]
    fn disable_usb_device_toggles_authorized() {
        let target = DeviceTarget {
            name: "Logitech Webcam".into(),
            category: category::INPUT.into(),
            sysfs_path: Some("/sys/bus/usb/devices/1-1".into()),
            driver: Some("uvcvideo".into()),
        };
        let steps = command_plan(DeviceControlOp::Disable, &target).expect("planned");
        assert_eq!(
            steps,
            vec![ExecStep::SysfsWrite {
                path: PathBuf::from("/sys/bus/usb/devices/1-1/authorized"),
                contents: "0".into(),
            }]
        );
        let steps = command_plan(DeviceControlOp::Enable, &target).expect("planned");
        assert_eq!(
            steps[0],
            ExecStep::SysfsWrite {
                path: PathBuf::from("/sys/bus/usb/devices/1-1/authorized"),
                contents: "1".into(),
            }
        );
    }

    #[test]
    fn disable_netclass_device_uses_ip_link() {
        let target = DeviceTarget {
            name: "eth0".into(),
            category: category::NETWORK_ADAPTERS.into(),
            sysfs_path: Some("/sys/class/net/eth0".into()),
            driver: None,
        };
        let steps = command_plan(DeviceControlOp::Disable, &target).expect("planned");
        assert_eq!(
            steps,
            vec![ExecStep::command("ip", &["link", "set", "eth0", "down"])]
        );
    }

    #[test]
    fn reload_module_bounces_the_driver() {
        let target = DeviceTarget {
            name: "GPU".into(),
            category: category::DISPLAY.into(),
            sysfs_path: Some("/sys/bus/pci/devices/0000:00:02.0".into()),
            driver: Some("i915".into()),
        };
        let steps = command_plan(DeviceControlOp::ReloadModule, &target).expect("planned");
        assert_eq!(
            steps,
            vec![
                ExecStep::command("rmmod", &["i915"]),
                ExecStep::command("modprobe", &["i915"]),
            ]
        );
    }

    #[test]
    fn rescan_bus_writes_the_pci_rescan_node() {
        let target = DeviceTarget {
            name: "SD Host Controller".into(),
            category: category::PCI_DEVICES.into(),
            sysfs_path: Some("/sys/bus/pci/devices/0000:02:00.0".into()),
            driver: None,
        };
        let steps = command_plan(DeviceControlOp::RescanBus, &target).expect("planned");
        assert_eq!(
            steps,
            vec![ExecStep::SysfsWrite {
                path: PathBuf::from("/sys/bus/pci/rescan"),
                contents: "1".into(),
            }]
        );
    }

    // ── command_plan: an inapplicable op is a TYPED ERROR, never a fake success ─

    #[test]
    fn reload_module_without_a_driver_is_a_typed_error() {
        let target = DeviceTarget::new("A thermal zone", category::SENSORS);
        let err = command_plan(DeviceControlOp::ReloadModule, &target).expect_err("no module");
        assert!(err.contains("no bound driver/module"), "{err}");
    }

    #[test]
    fn hostile_provider_driver_cannot_escape_or_reinterpret_the_control_seam() {
        let mut target = DeviceTarget {
            name: "Hostile PCI function".into(),
            category: category::PCI_DEVICES.into(),
            sysfs_path: Some("/sys/bus/pci/devices/0000:02:00.0".into()),
            driver: Some("../../tmp/forged-driver".into()),
        };
        let path_escape = command_plan(DeviceControlOp::Disable, &target)
            .expect_err("a driver path escape must not produce a sysfs write");
        assert!(path_escape.contains("bounded kernel module identifier"));

        target.driver = Some("--force".into());
        let option_injection = command_plan(DeviceControlOp::ReloadModule, &target)
            .expect_err("a helper option must not be accepted as a module name");
        assert!(option_injection.contains("bounded kernel module identifier"));
    }

    #[test]
    fn enable_disable_without_a_sysfs_anchor_is_a_typed_error() {
        // A CPU/memory record has no bus/net anchor — enable/disable is inapplicable.
        let target = DeviceTarget::new("Core i7-8650U", category::PROCESSORS);
        let err = command_plan(DeviceControlOp::Disable, &target).expect_err("no anchor");
        assert!(err.contains("no sysfs anchor"), "{err}");
    }

    #[test]
    fn disable_pci_without_a_bound_driver_is_a_typed_error() {
        // A driverless PCI function can't be unbound (no driver dir to write).
        let target = DeviceTarget {
            name: "SD Host Controller".into(),
            category: category::PCI_DEVICES.into(),
            sysfs_path: Some("/sys/bus/pci/devices/0000:02:00.0".into()),
            driver: None,
        };
        let err = command_plan(DeviceControlOp::Disable, &target).expect_err("no driver");
        assert!(err.contains("no bound driver"), "{err}");
    }

    #[test]
    fn rescan_on_a_usb_bus_is_a_typed_error() {
        let target = DeviceTarget {
            name: "USB hub".into(),
            category: category::USB_CONTROLLERS.into(),
            sysfs_path: Some("/sys/bus/usb/devices/usb1".into()),
            driver: Some("hub".into()),
        };
        let err = command_plan(DeviceControlOp::RescanBus, &target).expect_err("no usb rescan");
        assert!(err.contains("no rescan node"), "{err}");
    }

    #[tokio::test]
    async fn sysfs_control_write_refuses_a_replaced_final_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let victim = tmp.path().join("victim");
        let control = tmp.path().join("authorized");
        std::fs::write(&victim, "unchanged").unwrap();
        std::os::unix::fs::symlink(&victim, &control).unwrap();

        let result = execute_plan(&[ExecStep::SysfsWrite {
            path: control,
            contents: "0".into(),
        }])
        .await;

        assert!(
            result.is_err(),
            "a final symlink must not be a control target"
        );
        assert_eq!(std::fs::read_to_string(victim).unwrap(), "unchanged");
    }

    #[tokio::test]
    async fn command_control_step_times_out_instead_of_wedging_the_worker() {
        let result = execute_plan_with_timeout(
            &[ExecStep::Command {
                bin: "sleep",
                args: vec!["1".into()],
            }],
            Duration::from_millis(25),
        )
        .await;

        let error = result.expect_err("a command beyond its deadline must fail");
        assert!(error.contains("sleep 1"), "{error}");
        assert!(error.contains("timed out"), "{error}");
    }

    // ── the offered gate + audit fire, without touching real hardware ──────────

    fn write_inventory_at(
        root: &Path,
        host: &str,
        published_at_ms: u64,
        devices: Vec<DeviceRecord>,
    ) {
        let inv = DeviceInventory {
            host: host.to_string(),
            published_at_ms,
            summary: HostSummary::default(),
            tools: ToolAvailability::default(),
            categories: vec![DeviceCategory::new(category::NETWORK_ADAPTERS, devices)],
        };
        let dir = device_inventory::inventory_dir(root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            device_inventory::inventory_path(root, host),
            serde_json::to_string_pretty(&inv).unwrap(),
        )
        .unwrap();
    }

    fn write_inventory(root: &Path, host: &str, devices: Vec<DeviceRecord>) {
        write_inventory_at(root, host, 1, devices);
    }

    #[tokio::test]
    async fn a_device_not_in_the_local_inventory_is_refused_and_audited() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("audit.db");
        // The box publishes an inventory that does NOT contain the requested device.
        write_inventory(tmp.path(), "edge-2", vec![]);
        let w = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        )
        .with_db_path(db.clone())
        .with_bus_root(tmp.path().join("bus"));

        let req = DeviceControlRequest {
            schema_version: mackes_mesh_types::device_control::DEVICE_CONTROL_SCHEMA_VERSION,
            armed_token: None,
            id: "01REF".into(),
            op: DeviceControlOp::Disable,
            target: DeviceTarget {
                name: "Ghost NIC".into(),
                category: category::NETWORK_ADAPTERS.into(),
                sysfs_path: Some("/sys/bus/pci/devices/0000:99:99.9".into()),
                driver: Some("ghost".into()),
            },
            target_host: "edge-2".into(),
            expected_inventory_published_at_ms: 1,
            from: "peer:laptop-mm".into(),
        };
        let result = w.process(&req).await;
        assert!(!result.ok);
        assert!(result.error.contains("refused"), "{}", result.error);

        // The refusal is audited on the hash-chain (a refused op is on the chain too).
        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 1, "one audit row for the refused op");
        assert!(matches!(
            crate::audit::verify(&rows),
            crate::audit::VerifyOutcome::Intact { verified: 1, .. }
        ));
        // A failure also drops a notify on the folded lane.
        let persist = Persist::open(tmp.path().join("bus")).unwrap();
        let alerts = persist.list_since(NOTIFY_TOPIC, None).unwrap();
        assert_eq!(alerts.len(), 1, "a failed op notifies");
    }

    #[test]
    fn overlapping_controls_retain_exact_request_and_terminal_result_identity_in_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("audit.db");
        let worker = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        )
        .with_db_path(db.clone());
        let request = |id: &str| DeviceControlRequest {
            schema_version: DEVICE_CONTROL_SCHEMA_VERSION,
            armed_token: None,
            id: id.into(),
            op: DeviceControlOp::Disable,
            target: DeviceTarget::new("Shared NIC", category::NETWORK_ADAPTERS),
            target_host: "edge-2".into(),
            expected_inventory_published_at_ms: 7,
            from: "peer:laptop-mm".into(),
        };

        let first = request("control-first");
        let second = request("control-second");
        worker.audit(
            &first,
            &DeviceControlResult::ok(&first.id, "disabled Shared NIC"),
        );
        worker.audit(
            &second,
            &DeviceControlResult::failed(&second.id, "provider disappeared"),
        );

        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 2);
        assert!(matches!(
            crate::audit::verify(&rows),
            crate::audit::VerifyOutcome::Intact { verified: 2, .. }
        ));
        let first_event: crate::events::Event =
            serde_json::from_slice(&rows[0].payload).expect("first audit event");
        assert_eq!(first_event.detail["request_id"], "control-first");
        assert_eq!(first_event.detail["result_id"], "control-first");
        assert_eq!(first_event.detail["outcome"], "succeeded");
        let second_event: crate::events::Event =
            serde_json::from_slice(&rows[1].payload).expect("second audit event");
        assert_eq!(second_event.detail["request_id"], "control-second");
        assert_eq!(second_event.detail["result_id"], "control-second");
        assert_eq!(second_event.detail["outcome"], "failed");
    }

    #[tokio::test]
    async fn an_offered_usb_device_disables_for_real_through_the_sysfs_seam() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("audit.db");
        // A real sysfs-shaped device node under a tempdir (the anchor resolves the
        // `authorized` sibling under the SAME root — so the write is a real fs write).
        let dev_dir = tmp.path().join("sys/bus/usb/devices/1-1");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("authorized"), "1").unwrap();
        let sysfs_path = dev_dir.to_string_lossy().into_owned();

        write_inventory(
            tmp.path(),
            "edge-2",
            vec![DeviceRecord {
                sysfs_path: Some(sysfs_path.clone()),
                ..DeviceRecord::new("Logitech Webcam", DeviceStatus::Ok)
            }],
        );
        let w = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        )
        .with_db_path(db.clone())
        .with_bus_root(tmp.path().join("bus"))
        .with_authorizer(test_authorizer(tmp.path()));

        let req = authorize_request(
            &DeviceControlRequest {
                schema_version: mackes_mesh_types::device_control::DEVICE_CONTROL_SCHEMA_VERSION,
                armed_token: None,
                id: "01USB".into(),
                op: DeviceControlOp::Disable,
                target: DeviceTarget {
                    name: "Logitech Webcam".into(),
                    category: category::NETWORK_ADAPTERS.into(),
                    sysfs_path: Some(sysfs_path),
                    driver: None,
                },
                target_host: "edge-2".into(),
                expected_inventory_published_at_ms: 1,
                from: "peer:laptop-mm".into(),
            },
            "offered-usb-device",
        );
        let result = w.process(&req).await;
        assert!(result.ok, "{}", result.error);
        // The kernel control file was written for real.
        assert_eq!(
            std::fs::read_to_string(dev_dir.join("authorized")).unwrap(),
            "0"
        );
        // A successful op audits but does NOT notify.
        let persist = Persist::open(tmp.path().join("bus")).unwrap();
        assert!(
            persist.list_since(NOTIFY_TOPIC, None).unwrap().is_empty(),
            "a successful op raises no failure notify"
        );
    }

    #[tokio::test]
    async fn unavailable_provider_state_cannot_reach_any_control_seam() {
        let tmp = tempfile::tempdir().unwrap();
        let dev_dir = tmp.path().join("sys/bus/usb/devices/1-1");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("authorized"), "1").unwrap();
        let sysfs_path = dev_dir.to_string_lossy().into_owned();
        write_inventory(
            tmp.path(),
            "edge-2",
            vec![DeviceRecord {
                sysfs_path: Some(sysfs_path.clone()),
                status: DeviceStatus::Unknown,
                problem: None,
                ..DeviceRecord::new("Unavailable USB device", DeviceStatus::Unknown)
            }],
        );
        let worker = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        )
        .with_authorizer(test_authorizer(tmp.path()));
        let request = DeviceControlRequest {
            schema_version: DEVICE_CONTROL_SCHEMA_VERSION,
            armed_token: None,
            id: "01UNAVAILABLE".into(),
            op: DeviceControlOp::Disable,
            target: DeviceTarget {
                name: "Unavailable USB device".into(),
                category: category::NETWORK_ADAPTERS.into(),
                sysfs_path: Some(sysfs_path),
                driver: None,
            },
            target_host: "edge-2".into(),
            expected_inventory_published_at_ms: 1,
            from: "peer:laptop-mm".into(),
        };

        for op in DeviceControlOp::ALL {
            let mut request = request.clone();
            request.id = format!("01UNAVAILABLE-{}", op.as_str());
            request.op = op;
            let result = worker.handle_request(&request).await;
            assert!(
                !result.ok,
                "unavailable provider state must refuse {}",
                op.as_str()
            );
            assert!(
                result.error.contains("provider state"),
                "{} unexpectedly escaped provider admission: {}",
                op.as_str(),
                result.error
            );
        }
        assert_eq!(
            std::fs::read_to_string(dev_dir.join("authorized")).unwrap(),
            "1",
            "unavailable state must be refused before the sysfs seam"
        );
    }

    #[test]
    fn superseded_inventory_generation_is_re_attested_before_hardware() {
        let tmp = tempfile::tempdir().unwrap();
        let dev_dir = tmp.path().join("sys/bus/usb/devices/1-1");
        std::fs::create_dir_all(&dev_dir).unwrap();
        let authorized = dev_dir.join("authorized");
        std::fs::write(&authorized, "1").unwrap();
        let sysfs_path = dev_dir.to_string_lossy().into_owned();
        let device = DeviceRecord {
            sysfs_path: Some(sysfs_path.clone()),
            ..DeviceRecord::new("Generation-bound webcam", DeviceStatus::Ok)
        };
        write_inventory_at(tmp.path(), "edge-2", 41, vec![device.clone()]);
        let worker = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        )
        .with_authorizer(test_authorizer(tmp.path()));
        let request = authorize_request(
            &DeviceControlRequest {
                schema_version: DEVICE_CONTROL_SCHEMA_VERSION,
                armed_token: None,
                id: "01GENERATION-REATTEST".into(),
                op: DeviceControlOp::Disable,
                target: DeviceTarget {
                    name: "Generation-bound webcam".into(),
                    category: category::NETWORK_ADAPTERS.into(),
                    sysfs_path: Some(sysfs_path),
                    driver: None,
                },
                target_host: "edge-2".into(),
                expected_inventory_published_at_ms: 41,
                from: "peer:operator-seat".into(),
            },
            "generation-re-attestation",
        );

        assert!(
            worker
                .offered(&request.target, request.expected_inventory_published_at_ms)
                .is_some(),
            "the preview generation must initially be offered"
        );
        // Simulate a provider publication superseding that successful preview
        // while durable authorization is in flight, then exercise the final
        // gate directly so this regression cannot pass on the initial gate.
        write_inventory_at(tmp.path(), "edge-2", 42, vec![device]);
        let result = worker.re_attest_authorized_request(&request);

        let error = result.expect_err("a superseded preview must fail final attestation");
        assert!(
            error.contains("authorized inventory generation 41"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(&authorized).unwrap(),
            "1",
            "a superseded preview generation must never reach sysfs"
        );
    }

    #[tokio::test]
    async fn unknown_provider_state_with_problem_cannot_reach_any_control_seam() {
        let tmp = tempfile::tempdir().unwrap();
        let dev_dir = tmp.path().join("sys/bus/usb/devices/1-1");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("authorized"), "1").unwrap();
        let sysfs_path = dev_dir.to_string_lossy().into_owned();
        write_inventory(
            tmp.path(),
            "edge-2",
            vec![DeviceRecord {
                sysfs_path: Some(sysfs_path.clone()),
                status: DeviceStatus::Unknown,
                problem: Some("provider probe failed".into()),
                ..DeviceRecord::new("Unresolved USB device", DeviceStatus::Unknown)
            }],
        );
        let worker = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        );
        let request = DeviceControlRequest {
            schema_version: DEVICE_CONTROL_SCHEMA_VERSION,
            armed_token: None,
            id: "01UNKNOWN-PROBLEM".into(),
            op: DeviceControlOp::Disable,
            target: DeviceTarget {
                name: "Unresolved USB device".into(),
                category: category::NETWORK_ADAPTERS.into(),
                sysfs_path: Some(sysfs_path),
                driver: None,
            },
            target_host: "edge-2".into(),
            expected_inventory_published_at_ms: 1,
            from: "peer:laptop-mm".into(),
        };

        for op in DeviceControlOp::ALL {
            let mut request = request.clone();
            request.id = format!("01UNKNOWN-PROBLEM-{}", op.as_str());
            request.op = op;
            let result = worker.handle_request(&request).await;
            assert!(
                !result.ok,
                "unknown provider state must refuse {}",
                op.as_str()
            );
            assert!(
                result.error.contains("provider state"),
                "{} escaped admission: {}",
                op.as_str(),
                result.error
            );
        }
        assert_eq!(
            std::fs::read_to_string(dev_dir.join("authorized")).unwrap(),
            "1"
        );
    }

    #[tokio::test]
    async fn missing_altered_and_replayed_authority_never_reaches_hardware() {
        let tmp = tempfile::tempdir().unwrap();
        let dev_dir = tmp.path().join("sys/bus/usb/devices/1-1");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("authorized"), "1").unwrap();
        let sysfs_path = dev_dir.to_string_lossy().into_owned();
        write_inventory(
            tmp.path(),
            "edge-2",
            vec![DeviceRecord {
                sysfs_path: Some(sysfs_path.clone()),
                ..DeviceRecord::new("Logitech Webcam", DeviceStatus::Ok)
            }],
        );
        let worker = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        )
        .with_authorizer(test_authorizer(tmp.path()));
        let unsigned = DeviceControlRequest {
            schema_version: mackes_mesh_types::device_control::DEVICE_CONTROL_SCHEMA_VERSION,
            armed_token: None,
            id: "01AUTH".into(),
            op: DeviceControlOp::Disable,
            target: DeviceTarget {
                name: "Logitech Webcam".into(),
                category: category::NETWORK_ADAPTERS.into(),
                sysfs_path: Some(sysfs_path),
                driver: None,
            },
            target_host: "edge-2".into(),
            expected_inventory_published_at_ms: 1,
            from: "peer:laptop-mm".into(),
        };

        let missing = worker.handle_request(&unsigned).await;
        assert!(!missing.ok);
        assert!(missing.error.contains("authorization refused"));
        assert_eq!(
            std::fs::read_to_string(dev_dir.join("authorized")).unwrap(),
            "1",
            "missing authority cannot mutate"
        );

        let armed = authorize_request(&unsigned, "exact-body-device-control");
        let mut altered = armed.clone();
        altered.from = "peer:forged-seat".into();
        let refused = worker.handle_request(&altered).await;
        assert!(!refused.ok);
        assert!(refused.error.contains("authorization refused"));
        assert_eq!(
            std::fs::read_to_string(dev_dir.join("authorized")).unwrap(),
            "1",
            "an altered signed body cannot mutate"
        );

        let applied = worker.handle_request(&armed).await;
        assert!(applied.ok, "{}", applied.error);
        assert_eq!(
            std::fs::read_to_string(dev_dir.join("authorized")).unwrap(),
            "0"
        );
        std::fs::write(dev_dir.join("authorized"), "1").unwrap();
        let replayed = worker.handle_request(&armed).await;
        assert!(!replayed.ok);
        assert!(replayed.error.contains("already used"));
        assert_eq!(
            std::fs::read_to_string(dev_dir.join("authorized")).unwrap(),
            "1",
            "a replayed capability cannot mutate"
        );
    }

    #[tokio::test]
    async fn signed_unsupported_request_schema_never_reaches_hardware() {
        let tmp = tempfile::tempdir().unwrap();
        let dev_dir = tmp.path().join("sys/bus/usb/devices/1-1");
        std::fs::create_dir_all(&dev_dir).unwrap();
        let authorized = dev_dir.join("authorized");
        std::fs::write(&authorized, "1").unwrap();
        let sysfs_path = dev_dir.to_string_lossy().into_owned();
        write_inventory(
            tmp.path(),
            "edge-2",
            vec![DeviceRecord {
                sysfs_path: Some(sysfs_path.clone()),
                ..DeviceRecord::new("Webcam", DeviceStatus::Ok)
            }],
        );
        let worker = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        )
        .with_authorizer(test_authorizer(tmp.path()));
        let request = authorize_request(
            &DeviceControlRequest {
                schema_version: DEVICE_CONTROL_SCHEMA_VERSION + 1,
                armed_token: None,
                id: "01UNSUPPORTEDSCHEMA".into(),
                op: DeviceControlOp::Disable,
                target: DeviceTarget {
                    name: "Webcam".into(),
                    category: category::NETWORK_ADAPTERS.into(),
                    sysfs_path: Some(sysfs_path),
                    driver: None,
                },
                target_host: "edge-2".into(),
                expected_inventory_published_at_ms: 1,
                from: "peer:laptop-mm".into(),
            },
            "unsupported-schema",
        );

        let result = worker.handle_request(&request).await;

        assert!(!result.ok);
        assert!(result
            .error
            .contains("unsupported device-control request schema"));
        assert_eq!(std::fs::read_to_string(authorized).unwrap(), "1");
    }

    #[tokio::test]
    async fn superseded_provider_generation_cannot_reach_the_mutation_seam() {
        let tmp = tempfile::tempdir().unwrap();
        let dev_dir = tmp.path().join("sys/bus/usb/devices/1-1");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("authorized"), "1").unwrap();
        let sysfs_path = dev_dir.to_string_lossy().into_owned();
        let device = DeviceRecord {
            sysfs_path: Some(sysfs_path.clone()),
            ..DeviceRecord::new("Logitech Webcam", DeviceStatus::Ok)
        };

        // Generation 7 is what the seat inspected. Before its replicated request
        // arrives, the provider replaces that snapshot with generation 8. Even
        // though every device identity field still matches, the old action must
        // fail closed before the sysfs write.
        write_inventory_at(tmp.path(), "edge-2", 8, vec![device]);
        let worker = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        )
        .with_db_path(tmp.path().join("audit.db"))
        .with_bus_root(tmp.path().join("bus"));
        let stale = DeviceControlRequest {
            schema_version: mackes_mesh_types::device_control::DEVICE_CONTROL_SCHEMA_VERSION,
            armed_token: None,
            id: "01STALE".into(),
            op: DeviceControlOp::Disable,
            target: DeviceTarget {
                name: "Logitech Webcam".into(),
                category: category::NETWORK_ADAPTERS.into(),
                sysfs_path: Some(sysfs_path),
                driver: None,
            },
            target_host: "edge-2".into(),
            expected_inventory_published_at_ms: 7,
            from: "peer:laptop-mm".into(),
        };

        let result = worker.process(&stale).await;
        assert!(
            !result.ok,
            "a superseded provider generation must fail closed"
        );
        assert!(result.error.contains("expected inventory generation 7"));
        assert_eq!(
            std::fs::read_to_string(dev_dir.join("authorized")).unwrap(),
            "1",
            "stale provider state must not reach the hardware mutation seam"
        );
        let conn = crate::store::open(&tmp.path().join("audit.db")).unwrap();
        let rows = crate::store::load_audit_rows(&conn).unwrap();
        assert_eq!(rows.len(), 1, "the stale refusal remains in audit history");
        let payload = String::from_utf8_lossy(&rows[0].payload);
        assert!(
            payload.contains("expected_inventory_published_at_ms"),
            "{payload}"
        );
    }

    #[tokio::test]
    async fn forged_properties_cannot_borrow_a_provider_owned_device_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dev_dir = tmp.path().join("sys/bus/usb/devices/1-1");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("authorized"), "1").unwrap();
        let sysfs_path = dev_dir.to_string_lossy().into_owned();

        write_inventory(
            tmp.path(),
            "edge-2",
            vec![DeviceRecord {
                sysfs_path: Some(sysfs_path.clone()),
                driver: Some("uvcvideo".into()),
                ..DeviceRecord::new("Logitech Webcam", DeviceStatus::Ok)
            }],
        );
        let worker = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        );
        let hostile = DeviceControlRequest {
            schema_version: mackes_mesh_types::device_control::DEVICE_CONTROL_SCHEMA_VERSION,
            armed_token: None,
            id: "01FORGED".into(),
            op: DeviceControlOp::Disable,
            target: DeviceTarget {
                name: "Provider-owned camera renamed by requester".into(),
                category: category::DISPLAY.into(),
                sysfs_path: Some(sysfs_path),
                driver: Some("arbitrary-module".into()),
            },
            target_host: "edge-2".into(),
            expected_inventory_published_at_ms: 1,
            from: "peer:hostile-seat".into(),
        };

        let result = worker.handle_request(&hostile).await;
        assert!(!result.ok, "forged provider properties must fail closed");
        assert!(result.error.contains("expected inventory generation 1"));
        assert_eq!(
            std::fs::read_to_string(dev_dir.join("authorized")).unwrap(),
            "1",
            "a borrowed sysfs identity must not reach the mutation seam"
        );
    }

    #[tokio::test]
    async fn foreign_host_request_cannot_use_this_providers_entity() {
        let tmp = tempfile::tempdir().unwrap();
        let dev_dir = tmp.path().join("sys/bus/usb/devices/1-1");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("authorized"), "1").unwrap();
        let sysfs_path = dev_dir.to_string_lossy().into_owned();

        write_inventory(
            tmp.path(),
            "edge-2",
            vec![DeviceRecord {
                sysfs_path: Some(sysfs_path.clone()),
                driver: Some("uvcvideo".into()),
                ..DeviceRecord::new("Logitech Webcam", DeviceStatus::Ok)
            }],
        );
        let worker = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        );
        let hostile = DeviceControlRequest {
            schema_version: mackes_mesh_types::device_control::DEVICE_CONTROL_SCHEMA_VERSION,
            armed_token: None,
            id: "01FOREIGN".into(),
            op: DeviceControlOp::Disable,
            target: DeviceTarget {
                name: "Logitech Webcam".into(),
                category: category::NETWORK_ADAPTERS.into(),
                sysfs_path: Some(sysfs_path),
                driver: Some("uvcvideo".into()),
            },
            target_host: "edge-9".into(),
            expected_inventory_published_at_ms: 1,
            from: "peer:hostile-seat".into(),
        };

        let result = worker.handle_request(&hostile).await;
        assert!(!result.ok, "foreign-host actions must fail closed");
        assert!(result.error.contains("this provider owns `edge-2`"));
        assert_eq!(
            std::fs::read_to_string(dev_dir.join("authorized")).unwrap(),
            "1",
            "a foreign-host request must not reach the mutation seam"
        );
    }

    #[tokio::test]
    async fn execute_pending_drains_the_targets_dir_and_writes_a_result() {
        let tmp = tempfile::tempdir().unwrap();
        write_inventory(tmp.path(), "edge-2", vec![]);
        let w = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        )
        .with_db_path(tmp.path().join("audit.db"))
        .with_bus_root(tmp.path().join("bus"));

        // A peer dispatches a (refused — not offered) request into edge-2's dir.
        write_request(
            tmp.path(),
            &DeviceControlRequest {
                schema_version: mackes_mesh_types::device_control::DEVICE_CONTROL_SCHEMA_VERSION,
                armed_token: None,
                id: "01DR".into(),
                op: DeviceControlOp::RescanBus,
                target: DeviceTarget {
                    name: "Ghost".into(),
                    category: category::PCI_DEVICES.into(),
                    sysfs_path: Some("/sys/bus/pci/devices/0000:00:00.0".into()),
                    driver: None,
                },
                target_host: "edge-2".into(),
                expected_inventory_published_at_ms: 1,
                from: "peer:laptop-mm".into(),
            },
        )
        .unwrap();

        w.execute_pending().await;
        // The result landed for the requester + the request was consumed.
        let result = mackes_mesh_types::device_control::take_result(tmp.path(), "edge-2", "01DR")
            .expect("result written back");
        assert!(!result.ok);
        assert!(
            take_requests(tmp.path(), "edge-2").is_empty(),
            "the request was consumed"
        );
    }

    #[tokio::test]
    async fn signed_exact_cancellation_claims_only_a_still_pending_request_and_is_audited() {
        let tmp = tempfile::tempdir().unwrap();
        let request = DeviceControlRequest {
            schema_version: DEVICE_CONTROL_SCHEMA_VERSION,
            armed_token: None,
            id: "01CANCELLED".into(),
            op: DeviceControlOp::Disable,
            target: DeviceTarget::new("Queued NIC", category::NETWORK_ADAPTERS),
            target_host: "edge-2".into(),
            expected_inventory_published_at_ms: 7,
            from: "peer:laptop-mm".into(),
        };
        write_request(tmp.path(), &request).unwrap();
        let cancellation = authorize_cancellation(
            &DeviceControlCancellation {
                schema_version: DEVICE_CONTROL_SCHEMA_VERSION,
                armed_token: None,
                id: "01CANCEL".into(),
                target_request_id: request.id.clone(),
                op: request.op,
                target: request.target.clone(),
                target_host: request.target_host.clone(),
                expected_inventory_published_at_ms: request.expected_inventory_published_at_ms,
                from: request.from.clone(),
            },
            "exact-cancel",
        );
        write_cancellation(tmp.path(), &cancellation).unwrap();
        let worker = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "edge-2".into(),
            "peer:edge-2".into(),
        )
        .with_authorizer(test_authorizer(tmp.path()))
        .with_db_path(tmp.path().join("audit.db"));

        worker.execute_pending().await;

        let result =
            mackes_mesh_types::device_control::take_result(tmp.path(), "edge-2", &request.id)
                .expect("typed cancellation result");
        assert_eq!(result.outcome, DeviceControlOutcome::Cancelled);
        assert!(!result.ok, "cancellation is not execution success");
        assert!(take_requests(tmp.path(), "edge-2").is_empty());
        let conn = crate::store::open(&tmp.path().join("audit.db")).unwrap();
        let rows = crate::store::load_audit_rows(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(String::from_utf8_lossy(&rows[0].payload).contains("device-control-cancel"));
    }

    #[tokio::test]
    async fn worker_name_is_locked() {
        let w = DeviceControlExecWorker::new(
            PathBuf::from("/tmp/x"),
            "pine".into(),
            "peer:pine".into(),
        );
        assert_eq!(w.name(), "device_control");
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = DeviceControlExecWorker::new(
            tmp.path().to_path_buf(),
            "pine".into(),
            "peer:pine".into(),
        )
        .with_db_path(tmp.path().join("audit.db"));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }
}
