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
//! calls [`super::device_inventory::publish_system`], which walks the
//! full Linux hardware taxonomy sysfs-first and publishes
//! `<workgroup_root>/device-inventory/<hostname>.json` for the About →
//! Device-Manager surface. Device faults are evaluated and notified by the
//! System and Mesh Health authority, so this inventory worker has no parallel
//! problem ledger or alert path. The worker's census entry is unchanged.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{device_inventory, wifi_provider, ShutdownToken, Worker};
use mackes_mesh_types::peer_probe::{
    BusTopology, Descriptors, KernelDriver, NatClass, PeerProbe, PowerThermal,
};

/// Re-gather + publish cadence. Hardware changes slowly; a 5-minute
/// refresh keeps the directory current without churn.
pub const TICK: Duration = Duration::from_secs(300);

const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_POWER_SUPPLIES: usize = 64;
const MAX_SYSFS_CLASSES: usize = 256;

/// Run one command, returning trimmed stdout lines (empty on any failure).
fn cmd_lines(bin: &str, args: &[&str]) -> Vec<String> {
    let mut command = std::process::Command::new(bin);
    command.args(args);
    crate::workers::proc::output_with_timeout(command, COMMAND_TIMEOUT)
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
fn read_sys_u8(path: &Path) -> Option<u8> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Discover power state by the kernel's typed power-supply interface rather
/// than assuming firmware-specific `BAT0`/`AC` names.
///
/// Entries are sorted and bounded before reads so a hostile or broken sysfs
/// mount cannot make this credential-free observation provider unbounded. The
/// first valid battery is the stable summary source; any online non-battery
/// supply establishes external power.
fn gather_power(power_supply_root: &Path) -> (Option<u8>, bool) {
    let mut entries = std::fs::read_dir(power_supply_root)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    entry
                        .file_name()
                        .into_string()
                        .ok()
                        .map(|name| (name, entry.path()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    entries.truncate(MAX_POWER_SUPPLIES);

    let mut battery_pct = None;
    let mut saw_battery = false;
    let mut on_ac = false;
    for (_, path) in entries {
        let supply_type = std::fs::read_to_string(path.join("type")).unwrap_or_default();
        if supply_type.trim() == "Battery" {
            saw_battery = true;
            if battery_pct.is_none() {
                battery_pct = read_sys_u8(&path.join("capacity")).filter(|value| *value <= 100);
            }
        } else if read_sys_u8(&path.join("online")) == Some(1) {
            on_ac = true;
        }
    }

    // A machine with no battery is conventionally externally powered. When a
    // battery exists, absence of an online supply must not fabricate AC power.
    (battery_pct, on_ac || !saw_battery)
}

/// Publish a deterministic, bounded summary of the kernel's device classes.
///
/// `/sys/class` is provider-owned input.  Sort before applying the bound so a
/// noisy or compromised mount cannot make the visible subset depend on
/// directory iteration order, and ignore aliases that are not directories.
fn gather_sysfs_classes(sysfs_class_root: &Path) -> Vec<String> {
    let mut classes = std::fs::read_dir(sysfs_class_root)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry
                        .file_type()
                        .map(|kind| kind.is_dir() || kind.is_symlink())
                        .unwrap_or(false)
                })
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    classes.sort_unstable();
    classes.dedup();
    classes.truncate(MAX_SYSFS_CLASSES);
    classes
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
    let (battery_pct, on_ac) = gather_power(Path::new("/sys/class/power_supply"));

    let sysfs_classes = gather_sysfs_classes(Path::new("/sys/class"));

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
    match serde_json::to_vec_pretty(&probe) {
        Ok(bytes) => {
            if let Err(e) = write_probe(&path, &bytes) {
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

/// Atomically publish one probe without following a substituted staging row.
///
/// The replicated directory is writable substrate state, so the fixed staging
/// name must never be opened through a symlink or hard-link alias. A private
/// regular residue from an interrupted prior write may be reclaimed; every
/// other occupant fails closed and leaves the prior `probe.json` untouched.
fn write_probe(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_probe_with_finalize(path, bytes, || {})
}

/// Descriptor-anchored probe publication with an injected pre-finalize seam for
/// hostile replacement tests.
fn write_probe_with_finalize(
    path: &Path,
    bytes: &[u8],
    before_finalize: impl FnOnce(),
) -> std::io::Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags};
    use std::io::Write as _;
    use std::os::unix::fs::MetadataExt as _;

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "hardware probe path has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let directory_fd = rustix::fs::open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let directory: std::fs::File = directory_fd.into();
    let pinned_parent = directory.metadata()?;
    match rustix::fs::statat(&directory, ".probe.json.tmp", AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata)
            if rustix::fs::FileType::from_raw_mode(metadata.st_mode)
                == rustix::fs::FileType::RegularFile
                && metadata.st_nlink == 1 =>
        {
            rustix::fs::unlinkat(&directory, ".probe.json.tmp", AtFlags::empty())?;
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "hardware probe staging row is not a private regular file",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let fd = rustix::fs::openat(
        &directory,
        ".probe.json.tmp",
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )?;
    let mut file = std::fs::File::from(fd);
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    before_finalize();

    // The path must still resolve to the exact directory we pinned before the
    // transaction. A replacement cannot redirect the final name, and a
    // detached old directory cannot be reported as a successful publication.
    let current_parent = std::fs::symlink_metadata(parent);
    let parent_is_current = current_parent.as_ref().is_ok_and(|metadata| {
        metadata.file_type().is_dir()
            && metadata.dev() == pinned_parent.dev()
            && metadata.ino() == pinned_parent.ino()
    });
    if !parent_is_current {
        let _ = rustix::fs::unlinkat(&directory, ".probe.json.tmp", AtFlags::empty());
        return Err(std::io::Error::other(
            "hardware probe publication directory changed during transaction",
        ));
    }

    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "hardware probe path has no file name",
        )
    })?;
    if let Err(error) = rustix::fs::renameat(&directory, ".probe.json.tmp", &directory, file_name) {
        let _ = rustix::fs::unlinkat(&directory, ".probe.json.tmp", AtFlags::empty());
        return Err(error.into());
    }
    directory.sync_all()
}

/// The default Bus root (persisted message tree), matching every other worker.
/// The hardware-probe producer worker.
pub struct HardwareProbeWorker {
    workgroup_root: PathBuf,
    node_id: String,
    tick: Duration,
}

impl HardwareProbeWorker {
    /// A production worker over `workgroup_root`, publishing as `node_id`, with
    /// the neutral device-inventory publisher.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            workgroup_root,
            node_id,
            tick: TICK,
        }
    }
}

#[async_trait::async_trait]
impl Worker for HardwareProbeWorker {
    fn name(&self) -> &'static str {
        "hardware_probe"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            let root = self.workgroup_root.clone();
            let node = self.node_id.clone();
            // DEVMGR-1 — the rail/By-node views key on the SAME stem node_grade
            // publishes under (node_id with the `peer:` prefix stripped), so a
            // host's grade + device tree line up in the shell.
            let host = node.strip_prefix("peer:").unwrap_or(&node).to_string();
            // Gather shells read-only tools + walks sysfs — keep it off the
            // scheduler. One blocking task publishes BOTH the PeerProbe
            // (SUBAUDIT-D2) and the full device-inventory tree (DEVMGR-1),
            // publishing both neutral inventory artifacts.
            let outcome = tokio::task::spawn_blocking(move || {
                publish(&root, &node);
                let inventory = device_inventory::publish_system(&root, &host);
                if let Err(error) = wifi_provider::publish_system(&root, &host) {
                    tracing::warn!(
                        target: "mackesd::hardware_probe",
                        %error,
                        "Wi-Fi provider publish failed",
                    );
                }
                inventory
            })
            .await;
            match outcome {
                Ok(Ok(_)) => {}
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

    #[test]
    fn power_probe_is_typed_deterministic_and_bounded() {
        let sysfs = tempfile::tempdir().unwrap();
        for index in 0..MAX_POWER_SUPPLIES + 8 {
            let supply = sysfs.path().join(format!("z-noise-{index:03}"));
            std::fs::create_dir(&supply).unwrap();
            std::fs::write(supply.join("type"), "USB\n").unwrap();
            std::fs::write(supply.join("online"), "0\n").unwrap();
        }
        let overflow_source = sysfs.path().join("zz-overflow-source");
        std::fs::create_dir(&overflow_source).unwrap();
        std::fs::write(overflow_source.join("type"), "USB_C\n").unwrap();
        std::fs::write(overflow_source.join("online"), "1\n").unwrap();

        let battery = sysfs.path().join("a-surface-battery");
        std::fs::create_dir(&battery).unwrap();
        std::fs::write(battery.join("type"), "Battery\n").unwrap();
        std::fs::write(battery.join("capacity"), "73\n").unwrap();
        let adapter = sysfs.path().join("b-usbc-source");
        std::fs::create_dir(&adapter).unwrap();
        std::fs::write(adapter.join("type"), "USB_C\n").unwrap();
        std::fs::write(adapter.join("online"), "1\n").unwrap();

        assert_eq!(gather_power(sysfs.path()), (Some(73), true));

        std::fs::write(adapter.join("online"), "0\n").unwrap();
        assert_eq!(gather_power(sysfs.path()), (Some(73), false));
        std::fs::write(battery.join("capacity"), "255\n").unwrap();
        assert_eq!(gather_power(sysfs.path()), (None, false));
    }

    #[test]
    fn sysfs_class_projection_is_typed_deterministic_and_bounded() {
        let sysfs = tempfile::tempdir().unwrap();
        for index in (0..MAX_SYSFS_CLASSES + 16).rev() {
            std::fs::create_dir(sysfs.path().join(format!("class-{index:03}"))).unwrap();
        }
        std::fs::write(sysfs.path().join("not-a-class"), b"noise").unwrap();

        let classes = gather_sysfs_classes(sysfs.path());

        assert_eq!(classes.len(), MAX_SYSFS_CLASSES);
        assert_eq!(classes.first().map(String::as_str), Some("class-000"));
        assert_eq!(classes.last().map(String::as_str), Some("class-255"));
        assert!(!classes.iter().any(|name| name == "not-a-class"));
    }

    #[cfg(unix)]
    #[test]
    fn command_probe_times_out_a_hung_child() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let started = std::time::Instant::now();
        let result = crate::workers::proc::output_with_timeout(command, Duration::from_millis(150));
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn substituted_probe_staging_row_cannot_redirect_hardware_publication() {
        use std::os::unix::fs::symlink;

        let store = tempfile::tempdir().unwrap();
        let path = probe_path(store.path(), "peer:node-a");
        let parent = path.parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        let outside = store.path().join("outside-authority");
        std::fs::write(&outside, b"operator-owned\n").unwrap();
        symlink(&outside, parent.join(".probe.json.tmp")).unwrap();

        let error = write_probe(&path, br#"{"peer_id":"peer:node-a"}"#)
            .expect_err("a substituted staging row must fail closed");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read(&outside).unwrap(), b"operator-owned\n");
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn replaced_probe_directory_cannot_capture_hardware_publication() {
        let store = tempfile::tempdir().unwrap();
        let path = probe_path(store.path(), "peer:node-a");
        let parent = path.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&parent).unwrap();
        let detached = store.path().join("detached-mackesd");

        let error = write_probe_with_finalize(
            &path,
            br#"{"peer_id":"peer:node-a","trusted":true}"#,
            || {
                std::fs::rename(&parent, &detached).unwrap();
                std::fs::create_dir(&parent).unwrap();
                std::fs::write(parent.join(".probe.json.tmp"), b"foreign authority\n").unwrap();
            },
        )
        .expect_err("a replaced publication directory must fail closed");

        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert_eq!(
            std::fs::read(parent.join(".probe.json.tmp")).unwrap(),
            b"foreign authority\n"
        );
        assert!(!path.exists());
        assert!(!detached.join(".probe.json.tmp").exists());
        assert!(!detached.join("probe.json").exists());
    }
}
