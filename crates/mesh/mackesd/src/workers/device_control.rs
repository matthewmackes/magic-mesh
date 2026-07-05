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
use std::time::Duration;

use mackes_mesh_types::device_control::{
    take_requests, write_result, DeviceControlOp, DeviceControlRequest, DeviceControlResult,
    DeviceTarget,
};
use mackes_mesh_types::device_inventory;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// Request poll cadence — an op lands within ~3 s of replication (as `lifecycle_exec`).
pub const POLL: Duration = Duration::from_secs(3);

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

    /// The L9 rail (as `lifecycle_exec::offered`): is this device actually one this
    /// box **publishes**? A request naming a device absent from this node's own
    /// `device-inventory/<self>.json` (a stale/misrouted/spoofed request) is refused
    /// before any privileged write. A host that has published no inventory yet
    /// refuses everything (conservative + self-healing once it publishes).
    fn offered(&self, target: &DeviceTarget) -> bool {
        let Some(inv) = device_inventory::read_inventory(&self.workgroup_root, &self.self_hostname)
        else {
            return false;
        };
        inv.categories.iter().flat_map(|c| &c.devices).any(|d| {
            match (target.sysfs_path.as_deref(), d.sysfs_path.as_deref()) {
                // The sysfs path is the strong key when both carry one.
                (Some(a), Some(b)) => a == b,
                // Otherwise fall back to the device name.
                _ => d.name == target.name,
            }
        })
    }

    /// Handle one request: gate → plan → execute. Returns the typed result WITHOUT
    /// side-effecting the audit/notify (those wrap it in [`Self::process`]) so the
    /// gate + plan logic is unit-testable in isolation.
    async fn handle_request(&self, req: &DeviceControlRequest) -> DeviceControlResult {
        if !self.offered(&req.target) {
            return DeviceControlResult::failed(
                &req.id,
                format!(
                    "device `{}` is not in {}'s published inventory — refused (L9 rail)",
                    req.target.name, self.self_hostname
                ),
            );
        }
        let steps = match command_plan(req.op, &req.target) {
            Ok(s) => s,
            Err(reason) => return DeviceControlResult::failed(&req.id, reason),
        };
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

    /// Write the hash-chain audit row for one op through the EXISTING audit plane
    /// (best-effort — `append_and_alert` logs + swallows a store fault, so an audit
    /// hiccup never wedges the op lane).
    fn audit(&self, req: &DeviceControlRequest, result: &DeviceControlResult) {
        let detail = serde_json::json!({
            "action": "device-control",
            "op": req.op.as_str(),
            "target_host": req.target_host,
            "device": req.target.name,
            "sysfs_path": req.target.sysfs_path,
            "driver": req.target.driver,
            "from": req.from,
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
    let mut notes = Vec::new();
    for step in steps {
        match step {
            ExecStep::SysfsWrite { path, contents } => {
                // A real sysfs write IS `std::fs::write` — the kernel control file.
                std::fs::write(path, contents)
                    .map_err(|e| format!("sysfs write {} failed: {e}", path.display()))?;
                notes.push(format!("wrote `{contents}` → {}", path.display()));
            }
            ExecStep::Command { bin, args } => {
                let out = tokio::process::Command::new(*bin)
                    .args(args)
                    .output()
                    .await
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
    use mackes_mesh_types::device_control::write_request;
    use mackes_mesh_types::device_inventory::{
        category, DeviceCategory, DeviceInventory, DeviceRecord, DeviceStatus, HostSummary,
        ToolAvailability,
    };

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

    // ── the offered gate + audit fire, without touching real hardware ──────────

    fn write_inventory(root: &Path, host: &str, devices: Vec<DeviceRecord>) {
        let inv = DeviceInventory {
            host: host.to_string(),
            published_at_ms: 1,
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
            id: "01REF".into(),
            op: DeviceControlOp::Disable,
            target: DeviceTarget {
                name: "Ghost NIC".into(),
                category: category::NETWORK_ADAPTERS.into(),
                sysfs_path: Some("/sys/bus/pci/devices/0000:99:99.9".into()),
                driver: Some("ghost".into()),
            },
            target_host: "edge-2".into(),
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
        .with_bus_root(tmp.path().join("bus"));

        let req = DeviceControlRequest {
            id: "01USB".into(),
            op: DeviceControlOp::Disable,
            target: DeviceTarget {
                name: "Logitech Webcam".into(),
                category: category::NETWORK_ADAPTERS.into(),
                sysfs_path: Some(sysfs_path),
                driver: Some("uvcvideo".into()),
            },
            target_host: "edge-2".into(),
            from: "peer:laptop-mm".into(),
        };
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
                id: "01DR".into(),
                op: DeviceControlOp::RescanBus,
                target: DeviceTarget {
                    name: "Ghost".into(),
                    category: category::PCI_DEVICES.into(),
                    sysfs_path: Some("/sys/bus/pci/devices/0000:00:00.0".into()),
                    driver: None,
                },
                target_host: "edge-2".into(),
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
