//! SUBAUDIT-D2 — the missing **producer** for the Hardware panel.
//!
//! Each node publishes its own [`PeerProbe`] (PCI/USB trees, kernel,
//! power, descriptors) into the replicated directory at
//! `<workgroup_root>/<node_id>/mackesd/probe.json`, so every peer's
//! Workbench Hardware panel can render the fleet's hardware. The schema
//! (`mackes_mesh_types::peer_probe`) + the consumer (the panel) shipped
//! long ago, but nothing ever gathered + wrote the probe — the panel was
//! permanently "No hardware probes yet". This worker closes that gap.
//!
//! Gather is best-effort + degrades cleanly: a missing tool (`lspci`,
//! `lsusb`, `sensors`) yields an empty section, never a failure. The
//! connection-specific bus fields (`rtt`/`nat`/`ice`/`mesh_path`) describe a
//! *link to a peer*; for a node's self-probe they carry honest local
//! defaults (rtt 0, `Lan`, self path).
//!
//! DEVMGR-1 folds a SECOND artifact into this same rank-0 worker (lock #16 —
//! "extend an existing inventory worker, not a new one"): on each tick it also
//! calls [`super::device_inventory::publish_system_observing`], which walks the
//! full Linux hardware taxonomy sysfs-first and publishes
//! `<workgroup_root>/device-inventory/<hostname>.json` for the About →
//! Device-Manager surface. The worker's census entry is unchanged.
//!
//! DEVMGR-9 (#21) rides the same tick: the publish also returns the
//! fault-transition edges against the previous snapshot (a device entering
//! `no-driver` / `disabled` / `degraded`), and this worker debounces each
//! through the per-device [`device_inventory::DeviceFaultGate`] (the
//! `node_grade` flapping-guard idiom) before publishing one alert on
//! `event/notify/device-fault` — the CHAT-FIX-2 lane the `chat` worker folds
//! into Chat + the phone. Still NOT a new worker: the same census entry
//! produces a third, event-shaped artifact.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mackes_mesh_types::peer_probe::{
    BusTopology, Descriptors, KernelDriver, NatClass, PeerProbe, PowerThermal,
};
use mde_bus::persist::Persist;

use super::{device_inventory, ShutdownToken, Worker};

/// Re-gather + publish cadence. Hardware changes slowly; a 5-minute
/// refresh keeps the directory current without churn.
pub const TICK: Duration = Duration::from_secs(300);

/// Run one command, returning trimmed stdout lines (empty on any failure).
fn cmd_lines(bin: &str, args: &[&str]) -> Vec<String> {
    std::process::Command::new(bin)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim_end)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Single trimmed line from a command (empty string on failure).
fn cmd_line(bin: &str, args: &[&str]) -> String {
    cmd_lines(bin, args).into_iter().next().unwrap_or_default()
}

/// Parse `PRETTY_NAME=...` out of /etc/os-release (`Fedora 44` etc.).
#[must_use]
pub fn parse_distro(os_release: &str) -> String {
    os_release
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap_or_default()
}

/// First `vendor:product` from `lspci -n` (`00:02.0 0300: 8086:5916`) →
/// `("8086","5916")`. Empty pair when absent.
#[must_use]
pub fn parse_first_pci_id(lspci_n: &str) -> (String, String) {
    for line in lspci_n.lines() {
        if let Some(pair) = line
            .split_whitespace()
            .find(|t| t.contains(':') && t.len() == 9)
        {
            if let Some((v, p)) = pair.split_once(':') {
                return (v.to_string(), p.to_string());
            }
        }
    }
    (String::new(), String::new())
}

/// Read a `/sys/class/power_supply` integer file, if present.
fn read_sys_u8(path: &str) -> Option<u8> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Gather this node's hardware probe. Pure-ish (shells read-only tools).
#[must_use]
pub fn gather(node_id: &str) -> PeerProbe {
    let hostname = {
        let h = cmd_line("hostname", &["-s"]);
        if h.is_empty() {
            std::env::var("HOSTNAME").unwrap_or_else(|_| node_id.to_string())
        } else {
            h
        }
    };
    let distro = parse_distro(&std::fs::read_to_string("/etc/os-release").unwrap_or_default());
    let pci_tree = cmd_lines("lspci", &["-tv"]);
    let usb_tree = cmd_lines("lsusb", &["-t"]);
    let (vendor_id, product_id) = parse_first_pci_id(&cmd_lines("lspci", &["-n"]).join("\n"));

    // Power: best-effort sysfs read (laptop) — None on a server/desktop.
    let battery_pct = read_sys_u8("/sys/class/power_supply/BAT0/capacity")
        .or_else(|| read_sys_u8("/sys/class/power_supply/BAT1/capacity"));
    let on_ac = read_sys_u8("/sys/class/power_supply/AC/online")
        .or_else(|| read_sys_u8("/sys/class/power_supply/ACAD/online"))
        .map_or_else(|| battery_pct.is_none(), |v| v == 1);

    let sysfs_classes = std::fs::read_dir("/sys/class")
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    PeerProbe {
        peer_id: node_id.to_string(),
        hostname,
        vendor_id,
        product_id,
        distro,
        bus: BusTopology {
            // Self-probe: connection-specific fields carry local defaults.
            mesh_path: vec![],
            rtt_ms: 0,
            nat_class: NatClass::Lan,
            ice_candidate: String::new(),
            pci_tree,
            usb_tree,
        },
        kernel: KernelDriver {
            uname: cmd_line("uname", &["-a"]),
            transport_module: "nebula".to_string(),
            mded_version: env!("CARGO_PKG_VERSION").to_string(),
            dmesg_tail: vec![],
        },
        power: PowerThermal {
            battery_pct,
            on_ac,
            cpu_pkg_c: None,
            fan_rpm: None,
        },
        descriptors: Descriptors {
            mesh_services: vec![],
            sysfs_classes,
            usb_descriptors: vec![],
        },
    }
}

/// `<workgroup_root>/<node_id>/mackesd/probe.json` — the replicated
/// path the Hardware panel reads per peer.
#[must_use]
pub fn probe_path(workgroup_root: &Path, node_id: &str) -> PathBuf {
    workgroup_root
        .join(node_id)
        .join("mackesd")
        .join("probe.json")
}

/// Gather + write this node's probe into the replicated directory.
fn publish(workgroup_root: &Path, node_id: &str) {
    let probe = gather(node_id);
    let path = probe_path(workgroup_root, node_id);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(target: "mackesd::hardware_probe", error = %e, "mkdir failed");
            return;
        }
    }
    match serde_json::to_vec_pretty(&probe) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&path, &bytes) {
                tracing::warn!(target: "mackesd::hardware_probe", error = %e, path = %path.display(), "probe write failed");
            } else {
                tracing::debug!(target: "mackesd::hardware_probe", path = %path.display(), "published hardware probe");
            }
        }
        Err(e) => {
            tracing::warn!(target: "mackesd::hardware_probe", error = %e, "probe serialize failed");
        }
    }
}

/// The default Bus root (persisted message tree), matching every other worker.
fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// The hardware-probe producer worker.
pub struct HardwareProbeWorker {
    workgroup_root: PathBuf,
    node_id: String,
    tick: Duration,
    /// The Bus spool the DEVMGR-9 fault notify publishes on. Tests point it at a
    /// tempdir; `None` degrades to publish-only (no notify — an honest no-Bus seat).
    bus_root: Option<PathBuf>,
    /// The per-device fault debounce (DEVMGR-9, #21) — held across ticks so a
    /// flapping device alerts once per [`device_inventory::FAULT_COOLDOWN`].
    fault_gate: device_inventory::DeviceFaultGate,
}

impl HardwareProbeWorker {
    /// A production worker over `workgroup_root`, publishing as `node_id`, with
    /// the default Bus root for the DEVMGR-9 fault notify.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            workgroup_root,
            node_id,
            tick: TICK,
            bus_root: default_bus_root(),
            fault_gate: device_inventory::DeviceFaultGate::default(),
        }
    }

    /// Override the notify Bus root (tests point at a tempdir).
    #[must_use]
    pub fn with_bus_root(mut self, bus_root: Option<PathBuf>) -> Self {
        self.bus_root = bus_root;
        self
    }
}

#[async_trait::async_trait]
impl Worker for HardwareProbeWorker {
    fn name(&self) -> &'static str {
        "hardware_probe"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let persist = self
            .bus_root
            .clone()
            .and_then(|root| Persist::open(root).ok());
        loop {
            let root = self.workgroup_root.clone();
            let node = self.node_id.clone();
            // DEVMGR-1 — the rail/By-node views key on the SAME stem node_grade
            // publishes under (node_id with the `peer:` prefix stripped), so a
            // host's grade + device tree line up in the shell.
            let host = node.strip_prefix("peer:").unwrap_or(&node).to_string();
            let host_for_alert = host.clone();
            // Gather shells read-only tools + walks sysfs — keep it off the
            // scheduler. One blocking task publishes BOTH the PeerProbe
            // (SUBAUDIT-D2) and the full device-inventory tree (DEVMGR-1),
            // returning the DEVMGR-9 fault edges against the previous snapshot.
            let outcome = tokio::task::spawn_blocking(move || {
                publish(&root, &node);
                device_inventory::publish_system_observing(&root, &host)
            })
            .await;
            match outcome {
                Ok(Ok((_, transitions))) => {
                    // DEVMGR-9 — one debounced notify per device entering a fault
                    // state (#21). The diff already edge-triggers; the gate guards
                    // ok↔faulted flapping across ticks.
                    let now = Instant::now();
                    for t in &transitions {
                        if self.fault_gate.admit(&t.key, now) {
                            if let Some(p) = persist.as_ref() {
                                device_inventory::emit_fault_alert(p, &host_for_alert, t);
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        target: "mackesd::hardware_probe",
                        error = %e,
                        "device-inventory publish failed",
                    );
                }
                Err(_) => {} // join error — the blocking task was cancelled
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.tick) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_distro_extracts_pretty_name() {
        let os = "NAME=Fedora\nPRETTY_NAME=\"Fedora Linux 44 (Server Edition)\"\nVERSION_ID=44\n";
        assert_eq!(parse_distro(os), "Fedora Linux 44 (Server Edition)");
        assert_eq!(parse_distro("NAME=Foo\n"), "");
    }

    #[test]
    fn parse_first_pci_id_pulls_vendor_product() {
        let lspci = "00:00.0 0600: 8086:1234\n00:02.0 0300: 8086:5916 (rev 02)\n";
        assert_eq!(
            parse_first_pci_id(lspci),
            ("8086".to_string(), "1234".to_string())
        );
        assert_eq!(parse_first_pci_id(""), (String::new(), String::new()));
    }

    #[test]
    fn probe_path_is_under_the_node_mackesd_dir() {
        let p = probe_path(Path::new("/mnt/mesh-storage"), "peer:fedora");
        assert_eq!(
            p,
            Path::new("/mnt/mesh-storage/peer:fedora/mackesd/probe.json")
        );
    }

    #[test]
    fn gather_fills_identity_and_is_serializable() {
        let probe = gather("peer:test-node");
        assert_eq!(probe.peer_id, "peer:test-node");
        assert_eq!(probe.kernel.transport_module, "nebula");
        assert!(!probe.kernel.mded_version.is_empty());
        // Round-trips through the on-disk JSON shape the panel reads.
        let json = serde_json::to_string(&probe).expect("serialize");
        let back: PeerProbe = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.peer_id, "peer:test-node");
    }
}
