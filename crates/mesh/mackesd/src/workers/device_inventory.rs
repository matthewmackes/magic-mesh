//! DEVMGR-1 — the device-inventory **enumeration engine** behind the existing
//! `hardware_probe` worker (`docs/design/about-device-manager.md`, locked
//! 2026-07-04).
//!
//! This is NOT a new worker (lock #16 — "extend an existing inventory worker,
//! not a brand-new one"): it is the enumeration + publish support the rank-0
//! [`super::hardware_probe`] worker calls on its tick, in the mold of the
//! crate-root `legacy_inventory` module. The `hardware_probe` census entry
//! (`worker_role::WORKER_TIERS`) is unchanged — the same worker now publishes a
//! second artifact.
//!
//! ## What it does (the producer side of §6)
//!
//! Walk the local host's Linux hardware graph **sysfs-first** — `/sys/bus/pci`,
//! `/sys/bus/usb`, `/sys/block`, `/sys/class/{input,thermal,hwmon,bluetooth,
//! power_supply}`, `/proc/{cpuinfo,meminfo,uptime}` — into the full locked
//! taxonomy (#4), naming devices from the `pci.ids`/`usb.ids` databases and
//! deriving each device's honest Linux **status + problem reason** (#11: a PCI
//! function with no driver bound is `no-driver`; `enable=0` / a de-authorized USB
//! device is `disabled`; a dmesg error line marks `degraded`). `lshw`/`dmidecode`
//! are consulted only for tool-availability flags here and stay **best-effort**
//! (#15 — an absent tool degrades honestly, never fails). The assembled
//! [`mackes_mesh_types::device_inventory::DeviceInventory`] is published to
//! `<workgroup_root>/device-inventory/<hostname>.json` (the SEC-5 atomic
//! temp+rename own-row idiom `node_grade` uses), so every peer reads every host's
//! tree.
//!
//! The whole engine takes an injectable [`SysfsRoots`] (production points at
//! `/sys` + `/proc`; tests point at a fixture tree), so the taxonomy build, the
//! status derivation, and the publish path are all headless-testable.
//!
//! ## DEVMGR-9 — fleet-wide device-fault notify (#21)
//!
//! On each publish the engine also diffs the NEW snapshot against the host's
//! PREVIOUS published snapshot ([`fault_transitions`]): a device transitioning
//! **into** a problem state (a driver drop → `no-driver`, disk I/O errors →
//! `degraded`, a NIC set down → `disabled`) is an edge event. The
//! `hardware_probe` worker runs each edge through the per-device
//! [`DeviceFaultGate`] (the flapping guard — mirror of `node_grade`'s
//! `AlertGate` cooldown, #20) and publishes one debounced alert on
//! [`NOTIFY_TOPIC`] (`event/notify/device-fault`) — a lane the `chat` worker
//! folds into this node's `alert:<self>` conversation (CHAT-FIX-2), so the
//! fault reaches Chat + the phone. Honest by construction: a fault that merely
//! *persists* appears in both snapshots and never re-fires; an
//! [`Unknown`](DeviceStatus::Unknown) state is an honest unknown, not a fault,
//! and never alerts (§7 — no fabricated failures).

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mackes_mesh_types::device_inventory::{
    category, DeviceCategory, DeviceInventory, DeviceRecord, DeviceResources, DeviceStatus,
    HostSummary, ToolAvailability,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

// ── injectable roots ─────────────────────────────────────────────────────────

/// The filesystem roots the enumeration reads. Production is [`Self::system`]
/// (`/sys` + `/proc`); tests inject a fixture tree.
#[derive(Debug, Clone)]
pub struct SysfsRoots {
    /// The sysfs mount (`/sys`).
    pub sys: PathBuf,
    /// The procfs mount (`/proc`).
    pub proc: PathBuf,
}

impl SysfsRoots {
    /// The real host roots.
    #[must_use]
    pub fn system() -> Self {
        Self {
            sys: PathBuf::from("/sys"),
            proc: PathBuf::from("/proc"),
        }
    }

    /// Point both roots under one fixture directory (`<root>/sys`, `<root>/proc`).
    #[must_use]
    pub fn under(root: &Path) -> Self {
        Self {
            sys: root.join("sys"),
            proc: root.join("proc"),
        }
    }
}

// ── small sysfs read helpers ─────────────────────────────────────────────────

/// Read a small sysfs/procfs file, trimmed; `None` when absent/empty/unreadable.
fn read_trim(path: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// The child directory names of `dir`, sorted for a stable render order. Empty
/// (never an error) when the directory is absent — an honest degrade for a host
/// / bus that has no such class.
fn sorted_children(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    out.sort();
    out
}

/// Parse a `0x`-prefixed (or bare) hex id into a `u16` (`8086` → `0x8086`).
#[must_use]
pub fn parse_hex_id(s: &str) -> Option<u16> {
    let t = s.trim().trim_start_matches("0x");
    u16::from_str_radix(t, 16).ok()
}

/// The bound-module name of a bus device: `readlink <dir>/driver` → its basename.
fn bound_driver(dir: &Path) -> Option<String> {
    let target = std::fs::read_link(dir.join("driver")).ok()?;
    target
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
}

/// The bound module's exported version (`<dir>/driver/module/version`), if any.
fn driver_version(dir: &Path) -> Option<String> {
    read_trim(&dir.join("driver").join("module").join("version"))
}

// ── the pci.ids / usb.ids database ───────────────────────────────────────────

/// A parsed `*.ids` database: `vendor-id → (vendor-name, {device-id → name})`.
type IdsMap = BTreeMap<u16, (String, BTreeMap<u16, String>)>;

/// The `pci.ids` + `usb.ids` name databases (#15 — human vendor/model names).
/// Empty maps are the honest degrade when the databases aren't installed.
#[derive(Debug, Default, Clone)]
pub struct IdsDb {
    /// PCI vendor/device names.
    pub pci: IdsMap,
    /// USB vendor/product names.
    pub usb: IdsMap,
}

/// Parse the shared `*.ids` grammar.
///
/// A vendor line (`8086  Intel …`) sits at column 0, its device lines indented
/// one tab (`\t5916  HD Graphics 620`); deeper (subsystem) lines and
/// comments/blank lines are ignored.
#[must_use]
pub fn parse_ids(text: &str) -> IdsMap {
    let mut map: IdsMap = BTreeMap::new();
    let mut cur: Option<u16> = None;
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('\t') {
            // A device line (exactly one leading tab). Two tabs = subsystem, skip.
            if rest.starts_with('\t') {
                continue;
            }
            let (Some(vendor), Some((id, name))) = (cur, split_id_name(rest)) else {
                continue;
            };
            if let Some(entry) = map.get_mut(&vendor) {
                entry.1.insert(id, name);
            }
        } else if let Some((id, name)) = split_id_name(line) {
            cur = Some(id);
            map.entry(id).or_insert_with(|| (name, BTreeMap::new()));
        }
    }
    map
}

/// Split an `<hex-id><spaces><name>` row into its id + name.
fn split_id_name(s: &str) -> Option<(u16, String)> {
    let s = s.trim_start_matches('\t');
    let (id_str, name) = s.split_once("  ").or_else(|| s.split_once(' '))?;
    let id = parse_hex_id(id_str)?;
    Some((id, name.trim().to_string()))
}

impl IdsDb {
    /// Load from the standard database locations, best-effort.
    ///
    /// An absent file leaves that map empty. [`Self::has_pci`] / [`Self::has_usb`]
    /// then feed the [`ToolAvailability`] flags.
    #[must_use]
    pub fn load() -> Self {
        const PCI_PATHS: &[&str] = &["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"];
        const USB_PATHS: &[&str] = &[
            "/usr/share/hwdata/usb.ids",
            "/usr/share/misc/usb.ids",
            "/var/lib/usbutils/usb.ids",
        ];
        let first = |paths: &[&str]| -> IdsMap {
            for p in paths {
                if let Ok(text) = std::fs::read_to_string(p) {
                    return parse_ids(&text);
                }
            }
            IdsMap::new()
        };
        Self {
            pci: first(PCI_PATHS),
            usb: first(USB_PATHS),
        }
    }

    /// Whether the PCI database resolved any names.
    #[must_use]
    pub fn has_pci(&self) -> bool {
        !self.pci.is_empty()
    }

    /// Whether the USB database resolved any names.
    #[must_use]
    pub fn has_usb(&self) -> bool {
        !self.usb.is_empty()
    }

    /// Resolve `(vendor-name, device-name)` from a map, either possibly absent.
    fn name(map: &IdsMap, vendor: u16, device: u16) -> (Option<String>, Option<String>) {
        map.get(&vendor).map_or((None, None), |(vname, devs)| {
            (Some(vname.clone()), devs.get(&device).cloned())
        })
    }

    /// PCI `(vendor, model)` names for a `vendor:device` pair.
    #[must_use]
    pub fn pci_name(&self, vendor: u16, device: u16) -> (Option<String>, Option<String>) {
        Self::name(&self.pci, vendor, device)
    }

    /// USB `(vendor, product)` names for a `vendor:product` pair.
    #[must_use]
    pub fn usb_name(&self, vendor: u16, product: u16) -> (Option<String>, Option<String>) {
        Self::name(&self.usb, vendor, product)
    }
}

// ── PCI class → category + resource parsing ──────────────────────────────────

/// Parse a sysfs PCI `class` value (`0x030000`) into `(class, subclass)` bytes.
#[must_use]
pub fn parse_pci_class(s: &str) -> Option<(u8, u8)> {
    let v = u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()?;
    #[allow(clippy::cast_possible_truncation)]
    let class = ((v >> 16) & 0xff) as u8;
    #[allow(clippy::cast_possible_truncation)]
    let subclass = ((v >> 8) & 0xff) as u8;
    Some((class, subclass))
}

/// Map a PCI `(class, subclass)` to a taxonomy category key (#4).
#[must_use]
pub const fn pci_category(class: u8, subclass: u8) -> &'static str {
    match class {
        0x01 => category::STORAGE_CONTROLLERS,
        0x02 | 0x0d => category::NETWORK_ADAPTERS, // network + wireless
        0x03 => category::DISPLAY,
        0x04 => category::AUDIO, // multimedia (audio/video)
        0x0c if subclass == 0x03 => category::USB_CONTROLLERS,
        _ => category::PCI_DEVICES,
    }
}

/// Whether a PCI device of this class is expected to bind a kernel driver.
/// Bridges (class 0x06) are routinely driverless and MUST NOT read as a problem.
const fn pci_binds_driver(class: u8) -> bool {
    class != 0x06
}

/// Parse a sysfs `resource` file into I/O-port + memory windows.
///
/// Each line is `<start> <end> <flags>`; an all-zero line is an unused BAR.
/// `flags & 0x100` (`IORESOURCE_IO`) is a port window, otherwise a memory window.
#[must_use]
pub fn parse_resources(text: &str) -> (Vec<String>, Vec<String>) {
    let mut io = Vec::new();
    let mut mem = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(start), Some(end), Some(flags)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let parse = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok();
        let (Some(s), Some(e), Some(f)) = (parse(start), parse(end), parse(flags)) else {
            continue;
        };
        if s == 0 && e == 0 {
            continue; // unused BAR
        }
        let range = format!("0x{s:x}-0x{e:x}");
        if f & 0x100 != 0 {
            io.push(range);
        } else {
            mem.push(range);
        }
    }
    (io, mem)
}

// ── status derivation ────────────────────────────────────────────────────────

/// Derive the honest `(status, problem)` for a bus device.
///
/// Precedence: an administratively-disabled device (`disabled=true`) wins; then a
/// driver-expecting device with no driver bound is `no-driver`; then a matched
/// dmesg error line marks `degraded` (carrying the real line as the reason);
/// otherwise `ok`. Keeping the real Linux reason beside the state keeps the
/// synthetic MDM problem code honest (design "Risks").
#[must_use]
pub fn derive_status(
    disabled: bool,
    binds_driver: bool,
    driver: Option<&str>,
    dmesg_error: Option<&str>,
) -> (DeviceStatus, Option<String>) {
    if disabled {
        return (
            DeviceStatus::Disabled,
            Some("device administratively disabled".into()),
        );
    }
    if binds_driver && driver.is_none() {
        return (
            DeviceStatus::NoDriver,
            Some("no kernel driver bound".into()),
        );
    }
    if let Some(line) = dmesg_error {
        return (DeviceStatus::Degraded, Some(line.to_string()));
    }
    (DeviceStatus::Ok, None)
}

/// Whether a dmesg line looks like an error worth marking a device `degraded`.
fn is_error_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    ["error", "failed", "fault", "timeout", "i/o error", "hang"]
        .iter()
        .any(|k| l.contains(k))
}

/// The most recent dmesg line mentioning `token` (a PCI address / device name),
/// plus whether it is error-level. `None` when nothing mentions it.
fn dmesg_match<'a>(dmesg: &'a [String], token: &str) -> Option<(&'a str, bool)> {
    dmesg
        .iter()
        .rev()
        .find(|l| l.contains(token))
        .map(|l| (l.as_str(), is_error_line(l)))
}

// ── category enumerators ─────────────────────────────────────────────────────

/// Enumerate `/sys/bus/pci/devices` → categorized [`DeviceRecord`]s keyed by
/// their taxonomy category.
///
/// Each device carries ids, id-db names, sysfs path, bound driver + version,
/// derived status, and IRQ/mem/io resources.
#[must_use]
pub fn pci_devices(
    roots: &SysfsRoots,
    ids: &IdsDb,
    dmesg: &[String],
) -> BTreeMap<String, Vec<DeviceRecord>> {
    let mut by_cat: BTreeMap<String, Vec<DeviceRecord>> = BTreeMap::new();
    for dir in sorted_children(&roots.sys.join("bus").join("pci").join("devices")) {
        let addr = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let vendor = read_trim(&dir.join("vendor")).and_then(|s| parse_hex_id(&s));
        let device = read_trim(&dir.join("device")).and_then(|s| parse_hex_id(&s));
        let (class, subclass) = read_trim(&dir.join("class"))
            .and_then(|s| parse_pci_class(&s))
            .unwrap_or((0xff, 0xff));
        let (vname, mname) = match (vendor, device) {
            (Some(v), Some(d)) => ids.pci_name(v, d),
            _ => (None, None),
        };
        let ids_str = match (vendor, device) {
            (Some(v), Some(d)) => Some(format!("{v:04x}:{d:04x}")),
            _ => None,
        };
        let driver = bound_driver(&dir);
        let disabled = read_trim(&dir.join("enable")).as_deref() == Some("0");
        let (dmesg_line, dmesg_err) =
            dmesg_match(dmesg, &addr).map_or((None, false), |(l, e)| (Some(l), e));
        let (status, problem) = derive_status(
            disabled,
            pci_binds_driver(class),
            driver.as_deref(),
            dmesg_line.filter(|_| dmesg_err),
        );
        let irq = read_trim(&dir.join("irq")).and_then(|s| s.parse::<u32>().ok());
        let (io_ports, memory) = read_trim(&dir.join("resource"))
            .map_or((Vec::new(), Vec::new()), |t| parse_resources(&t));
        let name = display_name(
            vname.as_deref(),
            mname.as_deref(),
            ids_str.as_deref(),
            &addr,
        );
        // Any dmesg line mentioning this device is an Event; only an error-level
        // one (folded into `derive_status` above) also degrades it.
        let events = dmesg_line.map(|l| vec![l.to_string()]).unwrap_or_default();
        let rec = DeviceRecord {
            name,
            vendor: vname,
            model: mname,
            ids: ids_str,
            sysfs_path: Some(dir.to_string_lossy().into_owned()),
            driver,
            driver_version: driver_version(&dir),
            status,
            problem,
            resources: DeviceResources {
                irq,
                io_ports,
                memory,
                dma: Vec::new(),
            },
            events,
        };
        by_cat
            .entry(pci_category(class, subclass).to_string())
            .or_default()
            .push(rec);
    }
    by_cat
}

/// Compose the best available display name for a device.
fn display_name(
    vendor: Option<&str>,
    model: Option<&str>,
    ids: Option<&str>,
    fallback: &str,
) -> String {
    match (vendor, model) {
        (Some(v), Some(m)) => format!("{v} {m}"),
        (Some(v), None) => v.to_string(),
        (None, Some(m)) => m.to_string(),
        (None, None) => ids.unwrap_or(fallback).to_string(),
    }
}

/// USB category routing from the device class / first-interface class (#4).
fn usb_category(dir: &Path, name: &str, b_device_class: Option<&str>) -> &'static str {
    if name.starts_with("usb") || b_device_class == Some("09") {
        return category::USB_CONTROLLERS; // root hubs + hubs
    }
    // bDeviceClass is often 00 (per-interface); consult the first interface.
    let iface_class = sorted_children(dir)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(name) && n.contains(':'))
        })
        .find_map(|p| read_trim(&p.join("bInterfaceClass")));
    let cls = b_device_class
        .filter(|c| *c != "00")
        .map(str::to_string)
        .or(iface_class);
    match cls.as_deref() {
        Some("03") => category::INPUT,            // HID
        Some("01") => category::AUDIO,            // audio
        Some("e0" | "E0") => category::BLUETOOTH, // wireless (BT) controller
        _ => category::USB_CONTROLLERS,
    }
}

/// Enumerate `/sys/bus/usb/devices` device nodes (those exposing `idVendor`).
#[must_use]
pub fn usb_devices(roots: &SysfsRoots, ids: &IdsDb) -> BTreeMap<String, Vec<DeviceRecord>> {
    let mut by_cat: BTreeMap<String, Vec<DeviceRecord>> = BTreeMap::new();
    for dir in sorted_children(&roots.sys.join("bus").join("usb").join("devices")) {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        // Only real device nodes carry idVendor; interface nodes (`1-1:1.0`) don't.
        let Some(vendor) = read_trim(&dir.join("idVendor")).and_then(|s| parse_hex_id(&s)) else {
            continue;
        };
        let product = read_trim(&dir.join("idProduct")).and_then(|s| parse_hex_id(&s));
        let (db_v, db_p) = product.map_or((None, None), |p| ids.usb_name(vendor, p));
        // Prefer the device's own manufacturer/product strings over the db.
        let vname = read_trim(&dir.join("manufacturer")).or(db_v);
        let mname = read_trim(&dir.join("product")).or(db_p);
        let ids_str = product.map(|p| format!("{vendor:04x}:{p:04x}"));
        let b_class = read_trim(&dir.join("bDeviceClass"));
        let disabled = read_trim(&dir.join("authorized")).as_deref() == Some("0");
        let (status, problem) = if disabled {
            (DeviceStatus::Disabled, Some("device de-authorized".into()))
        } else {
            (DeviceStatus::Ok, None)
        };
        let name_disp = display_name(
            vname.as_deref(),
            mname.as_deref(),
            ids_str.as_deref(),
            &name,
        );
        let cat = usb_category(&dir, &name, b_class.as_deref());
        let rec = DeviceRecord {
            name: name_disp,
            vendor: vname,
            model: mname,
            ids: ids_str,
            sysfs_path: Some(dir.to_string_lossy().into_owned()),
            driver: bound_driver(&dir),
            driver_version: driver_version(&dir),
            status,
            problem,
            resources: DeviceResources::default(),
            events: Vec::new(),
        };
        by_cat.entry(cat.to_string()).or_default().push(rec);
    }
    by_cat
}

/// Enumerate physical block devices under `/sys/block` (Disk drives, #4).
/// Virtual devices (loop/ram/dm/zram) are skipped — they are not hardware.
#[must_use]
pub fn block_devices(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    let mut out = Vec::new();
    for dir in sorted_children(&roots.sys.join("block")) {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if is_virtual_block(&name) {
            continue;
        }
        let model = read_trim(&dir.join("device").join("model"));
        let vendor = read_trim(&dir.join("device").join("vendor"));
        let size_bytes = read_trim(&dir.join("size"))
            .and_then(|s| s.parse::<u64>().ok())
            .map(|sectors| sectors.saturating_mul(512));
        let base = model.clone().unwrap_or(name);
        let name_disp = match size_bytes {
            Some(b) => format!("{base} ({})", human_bytes(b)),
            None => base,
        };
        out.push(DeviceRecord {
            name: name_disp,
            vendor,
            model,
            ids: None,
            sysfs_path: Some(dir.to_string_lossy().into_owned()),
            driver: None,
            driver_version: None,
            status: DeviceStatus::Ok,
            problem: None,
            resources: DeviceResources::default(),
            events: Vec::new(),
        });
    }
    out
}

/// Whether a `/sys/block` name is a virtual (non-hardware) device.
fn is_virtual_block(name: &str) -> bool {
    ["loop", "ram", "dm-", "zram", "md", "sr"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// Format a byte count as a compact human string (`931.5 GB`).
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let mut v = bytes as f64;
    for unit in ["B", "KB", "MB", "GB", "TB", "PB"] {
        if v < 1024.0 {
            return format!("{v:.1} {unit}");
        }
        v /= 1024.0;
    }
    format!("{v:.1} EB")
}

/// Parse `/proc/cpuinfo` → the first `model name` + the logical-processor count
/// (one per `processor:` line — the MDM-faithful logical-CPU count).
#[must_use]
pub fn parse_cpuinfo(text: &str) -> (Option<String>, u32) {
    let mut model = None;
    let mut count = 0u32;
    for line in text.lines() {
        if line.starts_with("processor") && line.contains(':') {
            count += 1;
        } else if model.is_none() {
            if let Some(v) = line.strip_prefix("model name") {
                model = v.split_once(':').map(|(_, m)| m.trim().to_string());
            }
        }
    }
    (model, count)
}

/// Processors category: one record per logical CPU (MDM-faithful), named the
/// model. Falls back to the sysfs `cpu*` count when cpuinfo has no model line.
#[must_use]
pub fn processors(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    let (model, count) =
        read_trim(&roots.proc.join("cpuinfo")).map_or((None, 0), |t| parse_cpuinfo(&t));
    let count = if count > 0 {
        count
    } else {
        u32::try_from(
            sorted_children(&roots.sys.join("devices").join("system").join("cpu"))
                .iter()
                .filter(|p| {
                    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                        n.len() > 3
                            && n.starts_with("cpu")
                            && n[3..].bytes().all(|b| b.is_ascii_digit())
                    })
                })
                .count(),
        )
        .unwrap_or(0)
    };
    let label = model.unwrap_or_else(|| "Processor".to_string());
    (0..count)
        .map(|_| DeviceRecord::new(label.clone(), DeviceStatus::Ok))
        .collect()
}

/// Parse `MemTotal` (kB) from `/proc/meminfo`.
#[must_use]
pub fn parse_meminfo_total_kb(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse::<u64>().ok())
}

/// Memory category: a single system-RAM record from `/proc/meminfo`.
#[must_use]
pub fn memory(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    read_trim(&roots.proc.join("meminfo"))
        .and_then(|t| parse_meminfo_total_kb(&t))
        .map_or_else(Vec::new, |kb| {
            vec![DeviceRecord::new(
                format!("System memory ({})", human_bytes(kb.saturating_mul(1024))),
                DeviceStatus::Ok,
            )]
        })
}

/// Enumerate a simple `/sys/class/<class>` set where each child exposes a `name`
/// (or is itself the display name). Used for input / bluetooth / thermal / hwmon.
fn class_named_devices(
    roots: &SysfsRoots,
    class: &str,
    name_file: Option<&str>,
) -> Vec<DeviceRecord> {
    let mut out = Vec::new();
    for dir in sorted_children(&roots.sys.join("class").join(class)) {
        let node = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let display = name_file
            .and_then(|nf| read_trim(&dir.join(nf)))
            .unwrap_or(node);
        out.push(DeviceRecord {
            sysfs_path: Some(dir.to_string_lossy().into_owned()),
            ..DeviceRecord::new(display, DeviceStatus::Ok)
        });
    }
    out
}

/// Input devices (`/sys/class/input/input*/name`).
#[must_use]
pub fn input_devices(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    class_named_devices(roots, "input", Some("name"))
        .into_iter()
        // Keep only the `input*` device nodes (skip the `event*`/`mouse*` children
        // sysfs also lists under class/input).
        .filter(|r| {
            r.sysfs_path
                .as_deref()
                .and_then(|p| Path::new(p).file_name().and_then(|n| n.to_str()))
                .is_some_and(|n| n.starts_with("input"))
        })
        .collect()
}

/// Sensors + thermal zones (`/sys/class/thermal/*` types + `/sys/class/hwmon/*`
/// names). A thermal zone carries its current temperature as an event line.
#[must_use]
#[allow(clippy::cast_precision_loss)] // a millidegree i64 → °C f64 is lossless in range
pub fn sensors(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    let mut out = Vec::new();
    for dir in sorted_children(&roots.sys.join("class").join("thermal")) {
        let node = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if !node.starts_with("thermal_zone") {
            continue;
        }
        let kind = read_trim(&dir.join("type")).unwrap_or_else(|| node.to_string());
        let mut rec = DeviceRecord {
            sysfs_path: Some(dir.to_string_lossy().into_owned()),
            ..DeviceRecord::new(format!("Thermal zone: {kind}"), DeviceStatus::Ok)
        };
        if let Some(milli) = read_trim(&dir.join("temp")).and_then(|s| s.parse::<i64>().ok()) {
            rec.events.push(format!("{:.1} °C", milli as f64 / 1000.0));
        }
        out.push(rec);
    }
    out.extend(class_named_devices(roots, "hwmon", Some("name")));
    out
}

/// Bluetooth radios (`/sys/class/bluetooth/hci*`).
#[must_use]
pub fn bluetooth(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    class_named_devices(roots, "bluetooth", None)
        .into_iter()
        .filter(|r| {
            r.sysfs_path
                .as_deref()
                .and_then(|p| Path::new(p).file_name().and_then(|n| n.to_str()))
                .is_some_and(|n| n.starts_with("hci"))
        })
        .map(|r| DeviceRecord {
            name: format!("Bluetooth {}", short_node(r.sysfs_path.as_deref())),
            ..r
        })
        .collect()
}

/// The sysfs node basename for a display fallback.
fn short_node(path: Option<&str>) -> String {
    path.and_then(|p| Path::new(p).file_name().and_then(|n| n.to_str()))
        .unwrap_or("device")
        .to_string()
}

/// Battery / power supplies (`/sys/class/power_supply/*`), each carrying its
/// type + charge as an event line.
#[must_use]
pub fn power_supplies(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    let mut out = Vec::new();
    for dir in sorted_children(&roots.sys.join("class").join("power_supply")) {
        let node = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let kind = read_trim(&dir.join("type")).unwrap_or_else(|| "Power".to_string());
        let model = read_trim(&dir.join("model_name"));
        let vendor = read_trim(&dir.join("manufacturer"));
        let disp = model.clone().unwrap_or_else(|| format!("{kind} ({node})"));
        let mut rec = DeviceRecord {
            vendor,
            model,
            sysfs_path: Some(dir.to_string_lossy().into_owned()),
            ..DeviceRecord::new(disp, DeviceStatus::Ok)
        };
        if let Some(cap) = read_trim(&dir.join("capacity")) {
            let st = read_trim(&dir.join("status")).unwrap_or_default();
            rec.events.push(format!("{cap}% {st}").trim().to_string());
        }
        out.push(rec);
    }
    out
}

// ── host summary + tool availability ─────────────────────────────────────────

/// Parse the first (uptime) field of `/proc/uptime`.
#[must_use]
pub fn parse_uptime_secs(text: &str) -> Option<u64> {
    text.split_whitespace()
        .next()?
        .split('.')
        .next()?
        .parse::<u64>()
        .ok()
}

/// Read `PRETTY_NAME=` from an `/etc/os-release` body.
#[must_use]
pub fn parse_os_pretty(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_string())
}

/// Assemble the header-card [`HostSummary`] from `/proc` + `/etc/os-release`.
#[must_use]
pub fn host_summary(roots: &SysfsRoots) -> HostSummary {
    let (cpu_model, cpu_count) =
        read_trim(&roots.proc.join("cpuinfo")).map_or((None, 0), |t| parse_cpuinfo(&t));
    HostSummary {
        // /etc/os-release isn't under the injected roots; read the real file
        // best-effort (a fixture test simply gets None here).
        os: read_trim(Path::new("/etc/os-release")).and_then(|t| parse_os_pretty(&t)),
        kernel: read_trim(&roots.proc.join("sys").join("kernel").join("osrelease")),
        uptime_secs: read_trim(&roots.proc.join("uptime")).and_then(|t| parse_uptime_secs(&t)),
        cpu_model,
        cpu_count: (cpu_count > 0).then_some(cpu_count),
        mem_total_kb: read_trim(&roots.proc.join("meminfo"))
            .and_then(|t| parse_meminfo_total_kb(&t)),
    }
}

/// Whether an executable named `bin` is on `PATH` (a pure lookup — no spawn, so
/// it's fast + can't hang). Feeds the [`ToolAvailability`] flags.
#[must_use]
pub fn tool_present(bin: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(bin);
        std::fs::metadata(&candidate).is_ok_and(|m| m.is_file())
    })
}

/// The tool-availability record (#15).
#[must_use]
pub fn tool_availability(ids: &IdsDb) -> ToolAvailability {
    ToolAvailability {
        lshw: tool_present("lshw"),
        dmidecode: tool_present("dmidecode"),
        pci_ids: ids.has_pci(),
        usb_ids: ids.has_usb(),
    }
}

// ── assemble + publish ───────────────────────────────────────────────────────

/// Now in wall-clock ms since the epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Append `recs` under `key`, but only when non-empty — an empty category is
/// never materialized (#22).
fn add(buckets: &mut BTreeMap<String, Vec<DeviceRecord>>, key: &str, recs: Vec<DeviceRecord>) {
    if !recs.is_empty() {
        buckets.entry(key.to_string()).or_default().extend(recs);
    }
}

/// Build the full [`DeviceInventory`] for `hostname` from the injected roots +
/// databases + a captured dmesg buffer.
///
/// The taxonomy is emitted in [`category::ORDER`], and **empty categories are
/// dropped** (#22 — a non-PC / shallow host carries only the categories it
/// actually has).
#[must_use]
pub fn enumerate(
    roots: &SysfsRoots,
    ids: &IdsDb,
    tools: ToolAvailability,
    hostname: &str,
    dmesg: &[String],
) -> DeviceInventory {
    // Merge the PCI + USB category buckets into one map, then layer the rest.
    let mut buckets: BTreeMap<String, Vec<DeviceRecord>> = BTreeMap::new();
    for (k, v) in pci_devices(roots, ids, dmesg) {
        buckets.entry(k).or_default().extend(v);
    }
    for (k, v) in usb_devices(roots, ids) {
        buckets.entry(k).or_default().extend(v);
    }
    add(&mut buckets, category::PROCESSORS, processors(roots));
    add(&mut buckets, category::MEMORY, memory(roots));
    add(&mut buckets, category::DISK_DRIVES, block_devices(roots));
    add(&mut buckets, category::INPUT, input_devices(roots));
    add(&mut buckets, category::SENSORS, sensors(roots));
    add(&mut buckets, category::BLUETOOTH, bluetooth(roots));
    add(&mut buckets, category::POWER, power_supplies(roots));

    // Emit in the canonical order, dropping empties (#22).
    let categories = category::ORDER
        .iter()
        .filter_map(|key| {
            buckets
                .remove(*key)
                .filter(|v| !v.is_empty())
                .map(|devices| DeviceCategory::new(key, devices))
        })
        .collect();

    DeviceInventory {
        host: hostname.to_string(),
        published_at_ms: now_ms(),
        summary: host_summary(roots),
        tools,
        categories,
    }
}

/// Capture a bounded dmesg buffer, best-effort (empty on any failure /
/// `dmesg_restrict`). Kept small — only recent lines matter for the Events tab.
#[must_use]
pub fn capture_dmesg() -> Vec<String> {
    let mut cmd = std::process::Command::new("dmesg");
    cmd.args(["--level=err,warn", "--notime"]);
    let Ok(out) = super::proc::output_with_timeout(cmd, super::proc::DEFAULT_CMD_TIMEOUT) else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .rev()
        .take(200)
        .map(str::to_string)
        .collect()
}

/// Assemble the current host's inventory from the real system + publish it.
///
/// Writes to `<workgroup_root>/device-inventory/<hostname>.json` (atomic
/// temp+rename, the SEC-5 own-row idiom). The `hardware_probe` worker calls this
/// on its tick.
///
/// # Errors
/// Directory-create / write / rename / serialization failures.
pub fn publish_system(workgroup_root: &Path, hostname: &str) -> std::io::Result<PathBuf> {
    publish_system_observing(workgroup_root, hostname).map(|(path, _)| path)
}

/// [`publish_system`], additionally returning the DEVMGR-9 fault edges.
///
/// The edges are the devices that entered a problem state SINCE the previous
/// published snapshot ([`fault_transitions`]). The `hardware_probe` worker
/// debounces + publishes them on the notify lane; the CLI-parity
/// [`publish_system`] wrapper drops them.
///
/// # Errors
/// Directory-create / write / rename / serialization failures.
pub fn publish_system_observing(
    workgroup_root: &Path,
    hostname: &str,
) -> std::io::Result<(PathBuf, Vec<FaultTransition>)> {
    let roots = SysfsRoots::system();
    let ids = IdsDb::load();
    let tools = tool_availability(&ids);
    let dmesg = capture_dmesg();
    let inv = enumerate(&roots, &ids, tools, hostname, &dmesg);
    write_inventory_observing(workgroup_root, &inv)
}

/// [`write_inventory`] with the DEVMGR-9 observation.
///
/// Reads the host's PREVIOUS published snapshot first, writes the new one, and
/// returns the fault-transition edges between them. Split out so the
/// fault-notify path is exercised end to end with fixture-built trees (no real
/// `/sys` needed).
///
/// # Errors
/// Directory-create / write / rename / serialization failures.
pub fn write_inventory_observing(
    workgroup_root: &Path,
    inv: &DeviceInventory,
) -> std::io::Result<(PathBuf, Vec<FaultTransition>)> {
    let prev = mackes_mesh_types::device_inventory::read_inventory(workgroup_root, &inv.host);
    let path = write_inventory(workgroup_root, inv)?;
    Ok((path, fault_transitions(prev.as_ref(), inv)))
}

/// Write a prebuilt inventory to the substrate (atomic temp+rename). Split out so
/// tests exercise the publish path with a fixture-built tree.
///
/// # Errors
/// Directory-create / write / rename / serialization failures.
pub fn write_inventory(workgroup_root: &Path, inv: &DeviceInventory) -> std::io::Result<PathBuf> {
    let dir = mackes_mesh_types::device_inventory::inventory_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    let path = mackes_mesh_types::device_inventory::inventory_path(workgroup_root, &inv.host);
    let body = serde_json::to_string_pretty(inv)?;
    let tmp = dir.join(format!(".{}.json.tmp", inv.host));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

// ── DEVMGR-9: the fleet-wide device-fault notify (#21) ──────────────────────

/// The Bus lane a device-fault alert rides.
///
/// The `chat` worker's `ALERT_LANE_PREFIXES` carries `event/notify/`, so each
/// alert folds into this node's `alert:<self>` conversation (CHAT-FIX-2) and
/// reaches Chat + the phone.
pub const NOTIFY_TOPIC: &str = "event/notify/device-fault";

/// The stable `source` token on the published alert body (the Chat card badge).
pub const NOTIFY_SOURCE: &str = "device-fault";

/// Per-device debounce window — the flapping guard (#21).
///
/// A mirror of `node_grade::ALERT_COOLDOWN` scaled to the `hardware_probe`
/// 5-minute tick: a device that bounces ok↔faulted across ticks alerts once per
/// window, not on every re-entry.
pub const FAULT_COOLDOWN: Duration = Duration::from_secs(1800);

/// Whether a state is a **fault** worth notifying.
///
/// A fault is a real problem the producer derived from Linux facts.
/// [`DeviceStatus::Unknown`] is an honest could-not-determine, NOT a fault —
/// alerting on it would fabricate a failure out of a missing read (§7).
#[must_use]
pub const fn is_fault(status: DeviceStatus) -> bool {
    matches!(
        status,
        DeviceStatus::NoDriver | DeviceStatus::Disabled | DeviceStatus::Degraded
    )
}

/// The alert `severity` for a fault state (the Chat card colour).
///
/// A device actively reporting errors (`degraded` — I/O faults, dmesg errors)
/// is critical; a dropped driver / an administratively-downed device is a
/// warning.
#[must_use]
pub const fn fault_severity(status: DeviceStatus) -> &'static str {
    match status {
        DeviceStatus::Degraded => "critical",
        _ => "warning",
    }
}

/// One device-fault edge: a device observed transitioning **into** a problem
/// state between two published snapshots (DEVMGR-9). Carries what the alert
/// body + the debounce key need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultTransition {
    /// The stable per-device debounce key (`sysfs_path`, or `<category>/<name>`
    /// for a record without one).
    pub key: String,
    /// The category the device sits under (context for the alert card).
    pub category: String,
    /// The device's display name.
    pub name: String,
    /// The problem state it entered.
    pub status: DeviceStatus,
    /// The honest Linux reason ([`DeviceRecord::problem`]), when the producer
    /// derived one.
    pub reason: Option<String>,
}

/// The stable identity key for a device (the debounce + diff key): its sysfs
/// path when it has one, else `<category>/<name>` (CPU/memory records carry no
/// sysfs node).
#[must_use]
pub fn device_key(category_key: &str, dev: &DeviceRecord) -> String {
    dev.sysfs_path
        .clone()
        .unwrap_or_else(|| format!("{category_key}/{}", dev.name))
}

/// Diff two snapshots for devices transitioning **into** a fault state (#21).
///
/// A device faulted in `next` fires only when the previous snapshot shows it
/// non-faulted — or doesn't show it at all (a device arriving already broken,
/// or the host's very first publish, mirroring `AlertGate`'s prev-`None`
/// entering semantics). A fault that persists across snapshots never re-fires
/// (edge-triggered); a recovery fires nothing.
#[must_use]
pub fn fault_transitions(
    prev: Option<&DeviceInventory>,
    next: &DeviceInventory,
) -> Vec<FaultTransition> {
    let prev_status: BTreeMap<String, DeviceStatus> = prev
        .map(|inv| {
            inv.categories
                .iter()
                .flat_map(|c| c.devices.iter().map(|d| (device_key(&c.key, d), d.status)))
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    for cat in &next.categories {
        for dev in &cat.devices {
            if !is_fault(dev.status) {
                continue;
            }
            let key = device_key(&cat.key, dev);
            let entering = prev_status.get(&key).is_none_or(|s| !is_fault(*s));
            if entering {
                out.push(FaultTransition {
                    key,
                    category: cat.key.clone(),
                    name: dev.name.clone(),
                    status: dev.status,
                    reason: dev.problem.clone(),
                });
            }
        }
    }
    out
}

/// The per-device debounce gate (#21) — the flapping guard.
///
/// Sits over the edge-triggered [`fault_transitions`], mirroring
/// `node_grade::AlertGate`'s cooldown. [`fault_transitions`] already
/// edge-triggers (a persisting fault never re-fires); this gate additionally
/// suppresses a device that *re-enters* a fault state inside
/// [`FAULT_COOLDOWN`] (ok↔faulted flapping).
#[derive(Debug, Default)]
pub struct DeviceFaultGate {
    /// Last admitted alert per device key.
    last_alert: BTreeMap<String, Instant>,
}

impl DeviceFaultGate {
    /// Whether an alert for `key` may fire at `now`. Admitting records the
    /// timestamp; a repeat inside the cooldown is suppressed.
    pub fn admit(&mut self, key: &str, now: Instant) -> bool {
        let cooled = self
            .last_alert
            .get(key)
            .is_none_or(|t| now.duration_since(*t) >= FAULT_COOLDOWN);
        if cooled {
            self.last_alert.insert(key.to_string(), now);
        }
        cooled
    }
}

/// Serialize + publish one device-fault alert on [`NOTIFY_TOPIC`].
///
/// Mirrors `node_grade::emit_alert`. Every field is a string so
/// `mde_chat::fold_alert` keeps it (it preserves string fields only).
/// Best-effort — a write failure is logged, never fatal.
pub fn emit_fault_alert(persist: &Persist, host: &str, t: &FaultTransition) {
    let summary = format!(
        "device `{}` on {host} entered state {}{}",
        t.name,
        t.status.as_str(),
        t.reason
            .as_deref()
            .map(|r| format!(" \u{2014} {r}"))
            .unwrap_or_default()
    );
    let body = serde_json::json!({
        "severity": fault_severity(t.status),
        "source": NOTIFY_SOURCE,
        "summary": summary,
        "host": host,
        "device": t.name,
        "category": t.category,
        "status": t.status.as_str(),
        "reason": t.reason.as_deref().unwrap_or(""),
    })
    .to_string();
    if let Err(e) = persist.write(NOTIFY_TOPIC, Priority::Default, None, Some(&body)) {
        tracing::debug!(
            target: "mackesd::device_inventory",
            topic = NOTIFY_TOPIC,
            error = %e,
            "device-fault notify publish failed",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Write `content` to `path`, creating parents.
    fn put(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// Build a small fixture `/sys` + `/proc` tree under `root`.
    fn fixture_tree(root: &Path) {
        let sys = root.join("sys");
        let proc = root.join("proc");
        // A bound display GPU (class 0x0300, driver i915, an IRQ + a mem BAR).
        let gpu = sys.join("bus/pci/devices/0000:00:02.0");
        put(&gpu.join("vendor"), "0x8086\n");
        put(&gpu.join("device"), "0x5917\n");
        put(&gpu.join("class"), "0x030000\n");
        put(&gpu.join("irq"), "131\n");
        put(
            &gpu.join("resource"),
            "0x00000000dd000000 0x00000000ddffffff 0x0000000000040200\n\
             0x0000000000000000 0x0000000000000000 0x0000000000000000\n\
             0x000000000000f000 0x000000000000f03f 0x0000000000040101\n",
        );
        // Its driver symlink → a drivers dir carrying module/version.
        let drv = sys.join("bus/pci/drivers/i915");
        put(&drv.join("module/version"), "1.0.0\n");
        std::os::unix::fs::symlink(&drv, gpu.join("driver")).unwrap();
        // A driverless PCI function (an SD host controller, no `driver` link).
        let sd = sys.join("bus/pci/devices/0000:02:00.0");
        put(&sd.join("vendor"), "0x10ec\n");
        put(&sd.join("device"), "0x5227\n");
        put(&sd.join("class"), "0xff0000\n"); // unclassified → pci-devices
                                              // A PCI bridge with no driver — must NOT flag no-driver.
        let br = sys.join("bus/pci/devices/0000:00:1c.0");
        put(&br.join("vendor"), "0x8086\n");
        put(&br.join("device"), "0x9d10\n");
        put(&br.join("class"), "0x060400\n");
        // A USB HID mouse (interface class 03) under a root hub.
        let hub = sys.join("bus/usb/devices/usb1");
        put(&hub.join("idVendor"), "1d6b\n");
        put(&hub.join("idProduct"), "0002\n");
        put(&hub.join("bDeviceClass"), "09\n");
        let mouse = sys.join("bus/usb/devices/1-1");
        put(&mouse.join("idVendor"), "046d\n");
        put(&mouse.join("idProduct"), "c52b\n");
        put(&mouse.join("bDeviceClass"), "00\n");
        put(&mouse.join("manufacturer"), "Logitech\n");
        put(&mouse.join("product"), "USB Receiver\n");
        // The interface node is a CHILD of the device dir (as it is in real sysfs
        // once the flat `bus/usb/devices/1-1` symlink is followed).
        put(&mouse.join("1-1:1.0").join("bInterfaceClass"), "03\n");
        // A physical NVMe drive + a virtual loop device (skipped).
        put(&sys.join("block/nvme0n1/size"), "1000215216\n");
        put(&sys.join("block/nvme0n1/device/model"), "Samsung SSD 970\n");
        put(&sys.join("block/loop0/size"), "0\n");
        // CPU + memory + uptime.
        put(
            &proc.join("cpuinfo"),
            "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i7-8650U\nprocessor\t: 1\nmodel name\t: Intel(R) Core(TM) i7-8650U\n",
        );
        put(
            &proc.join("meminfo"),
            "MemTotal:       16072192 kB\nMemFree: 100 kB\n",
        );
        put(&proc.join("uptime"), "48120.42 100000.00\n");
        put(
            &proc.join("sys/kernel/osrelease"),
            "7.0.8-200.fc44.x86_64\n",
        );
        // Input + thermal + power classes.
        put(
            &sys.join("class/input/input3/name"),
            "AT Translated Set 2 keyboard\n",
        );
        put(&sys.join("class/input/event3/dev"), "13:67\n"); // a child, filtered out
        put(
            &sys.join("class/thermal/thermal_zone0/type"),
            "x86_pkg_temp\n",
        );
        put(&sys.join("class/thermal/thermal_zone0/temp"), "42000\n");
        put(
            &sys.join("class/bluetooth/hci0/address"),
            "AA:BB:CC:DD:EE:FF\n",
        );
        put(&sys.join("class/power_supply/BAT0/type"), "Battery\n");
        put(&sys.join("class/power_supply/BAT0/capacity"), "82\n");
        put(&sys.join("class/power_supply/BAT0/status"), "Discharging\n");
    }

    fn fixture_ids() -> IdsDb {
        IdsDb {
            pci: parse_ids(
                "8086  Intel Corporation\n\t5917  UHD Graphics 620\n10ec  Realtek Semiconductor Co., Ltd.\n\t5227  RTS5227 PCI Express Card Reader\n",
            ),
            usb: parse_ids("046d  Logitech, Inc.\n\tc52b  Unifying Receiver\n1d6b  Linux Foundation\n\t0002  2.0 root hub\n"),
        }
    }

    #[test]
    fn hex_and_class_parsers() {
        assert_eq!(parse_hex_id("0x8086"), Some(0x8086));
        assert_eq!(parse_hex_id("5916\n"), Some(0x5916));
        assert_eq!(parse_hex_id("zzzz"), None);
        assert_eq!(parse_pci_class("0x030000"), Some((0x03, 0x00)));
        assert_eq!(parse_pci_class("0x0c0330"), Some((0x0c, 0x03)));
        assert_eq!(pci_category(0x03, 0x00), category::DISPLAY);
        assert_eq!(pci_category(0x02, 0x00), category::NETWORK_ADAPTERS);
        assert_eq!(pci_category(0x0c, 0x03), category::USB_CONTROLLERS);
        assert_eq!(pci_category(0x0c, 0x05), category::PCI_DEVICES); // SMBus
        assert_eq!(pci_category(0x01, 0x06), category::STORAGE_CONTROLLERS);
    }

    #[test]
    fn resource_parser_buckets_io_and_mem() {
        let (io, mem) = parse_resources(
            "0x00000000dd000000 0x00000000ddffffff 0x0000000000040200\n\
             0x0000000000000000 0x0000000000000000 0x0000000000000000\n\
             0x000000000000f000 0x000000000000f03f 0x0000000000040101\n",
        );
        assert_eq!(mem, vec!["0xdd000000-0xddffffff"]);
        assert_eq!(io, vec!["0xf000-0xf03f"]);
    }

    #[test]
    fn ids_parser_reads_vendor_and_device() {
        let db = parse_ids(
            "8086  Intel Corporation\n\t5916  HD Graphics 620\n\t\t1234 5678  A subsystem\n",
        );
        let (v, d) = IdsDb::name(&db, 0x8086, 0x5916);
        assert_eq!(v.as_deref(), Some("Intel Corporation"));
        assert_eq!(d.as_deref(), Some("HD Graphics 620"));
        // Subsystem (two-tab) lines are ignored, unknown device → None model.
        assert_eq!(IdsDb::name(&db, 0x8086, 0x9999).1, None);
        assert_eq!(IdsDb::name(&db, 0xffff, 0x0).0, None);
    }

    #[test]
    fn status_derivation_is_honest() {
        // A driverless driver-expecting device → no-driver + the real reason.
        let (s, p) = derive_status(false, true, None, None);
        assert_eq!(s, DeviceStatus::NoDriver);
        assert_eq!(p.as_deref(), Some("no kernel driver bound"));
        // A bound device → ok, no phantom problem.
        assert_eq!(
            derive_status(false, true, Some("i915"), None).0,
            DeviceStatus::Ok
        );
        // A bridge (doesn't bind a driver) with no driver → ok, not flagged.
        assert_eq!(derive_status(false, false, None, None).0, DeviceStatus::Ok);
        // enable=0 wins even over a bound driver.
        assert_eq!(
            derive_status(true, true, Some("x"), None).0,
            DeviceStatus::Disabled
        );
        // A dmesg error line → degraded, carrying the line.
        let (s, p) = derive_status(false, true, Some("nvme"), Some("nvme0: I/O error, reset"));
        assert_eq!(s, DeviceStatus::Degraded);
        assert_eq!(p.as_deref(), Some("nvme0: I/O error, reset"));
        assert!(is_error_line("blk_update_request: I/O error"));
        assert!(!is_error_line("usb 1-1: new high-speed USB device"));
    }

    #[test]
    fn pci_walk_builds_categorized_records() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_tree(tmp.path());
        let roots = SysfsRoots::under(tmp.path());
        let ids = fixture_ids();
        // A dmesg error mentioning the GPU addr should degrade it — but it has a
        // driver, so the ordering is: driver present, dmesg error → degraded.
        let dmesg = vec!["0000:00:02.0: firmware load failed".to_string()];
        let by_cat = pci_devices(&roots, &ids, &dmesg);
        let display = &by_cat[category::DISPLAY];
        assert_eq!(display.len(), 1);
        let gpu = &display[0];
        assert_eq!(gpu.name, "Intel Corporation UHD Graphics 620");
        assert_eq!(gpu.ids.as_deref(), Some("8086:5917"));
        assert_eq!(gpu.driver.as_deref(), Some("i915"));
        assert_eq!(gpu.driver_version.as_deref(), Some("1.0.0"));
        assert_eq!(gpu.resources.irq, Some(131));
        assert_eq!(gpu.resources.memory, vec!["0xdd000000-0xddffffff"]);
        assert_eq!(gpu.resources.io_ports, vec!["0xf000-0xf03f"]);
        assert_eq!(
            gpu.status,
            DeviceStatus::Degraded,
            "dmesg error degrades it"
        );
        assert!(!gpu.events.is_empty(), "the dmesg line is attached");
        // The driverless card reader lands in pci-devices with a no-driver status.
        let pci = &by_cat[category::PCI_DEVICES];
        let reader = pci
            .iter()
            .find(|d| d.ids.as_deref() == Some("10ec:5227"))
            .unwrap();
        assert_eq!(reader.status, DeviceStatus::NoDriver);
        assert_eq!(reader.problem.as_deref(), Some("no kernel driver bound"));
        // The bridge is present but NOT flagged (bridges are driverless normally).
        let bridge = pci
            .iter()
            .find(|d| d.ids.as_deref() == Some("8086:9d10"))
            .unwrap();
        assert_eq!(bridge.status, DeviceStatus::Ok);
    }

    #[test]
    fn usb_walk_routes_by_interface_class() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_tree(tmp.path());
        let roots = SysfsRoots::under(tmp.path());
        let by_cat = usb_devices(&roots, &fixture_ids());
        // The root hub → usb-controllers; the HID mouse → input.
        assert!(by_cat[category::USB_CONTROLLERS]
            .iter()
            .any(|d| d.ids.as_deref() == Some("1d6b:0002")));
        let input = &by_cat[category::INPUT];
        let mouse = input
            .iter()
            .find(|d| d.ids.as_deref() == Some("046d:c52b"))
            .unwrap();
        // The device's own manufacturer/product strings win over the db.
        assert_eq!(mouse.name, "Logitech USB Receiver");
        assert_eq!(mouse.status, DeviceStatus::Ok);
    }

    #[test]
    fn other_categories_enumerate() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_tree(tmp.path());
        let roots = SysfsRoots::under(tmp.path());
        // Two logical CPUs from cpuinfo.
        let cpus = processors(&roots);
        assert_eq!(cpus.len(), 2);
        assert!(cpus[0].name.contains("i7-8650U"));
        // One RAM record.
        let mem = memory(&roots);
        assert_eq!(mem.len(), 1);
        assert!(mem[0].name.contains("GB"));
        // The NVMe drive; the loop device is skipped.
        let disks = block_devices(&roots);
        assert_eq!(disks.len(), 1);
        assert!(disks[0].name.contains("Samsung SSD 970"));
        // Input keeps input3, drops the event3 child.
        let inputs = input_devices(&roots);
        assert_eq!(inputs.len(), 1);
        assert!(inputs[0].name.contains("keyboard"));
        // Thermal zone carries a temperature event.
        let s = sensors(&roots);
        assert!(s
            .iter()
            .any(|r| r.name.contains("x86_pkg_temp") && !r.events.is_empty()));
        // Bluetooth + power.
        assert!(bluetooth(&roots).iter().any(|r| r.name.contains("hci0")));
        assert!(power_supplies(&roots)
            .iter()
            .any(|r| r.name.contains("Battery")));
    }

    #[test]
    fn enumerate_assembles_ordered_tree_and_drops_empties() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_tree(tmp.path());
        let roots = SysfsRoots::under(tmp.path());
        let ids = fixture_ids();
        let tools = tool_availability(&ids);
        assert!(tools.pci_ids && tools.usb_ids);
        let inv = enumerate(&roots, &ids, tools, "test-box", &[]);
        assert_eq!(inv.host, "test-box");
        // Categories are a subset of the canonical order, none empty.
        let keys: Vec<&str> = inv.categories.iter().map(|c| c.key.as_str()).collect();
        assert!(keys.contains(&category::PROCESSORS));
        assert!(keys.contains(&category::DISPLAY));
        assert!(keys.contains(&category::DISK_DRIVES));
        for c in &inv.categories {
            assert!(!c.devices.is_empty(), "no empty category emitted (#22)");
        }
        // Emitted in canonical order.
        let order: Vec<usize> = keys
            .iter()
            .map(|k| category::ORDER.iter().position(|o| o == k).unwrap())
            .collect();
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(order, sorted, "categories follow category::ORDER");
        // The header summary is populated.
        assert_eq!(inv.summary.cpu_count, Some(2));
        assert_eq!(inv.summary.mem_total_kb, Some(16_072_192));
        assert!(inv.summary.kernel.as_deref().unwrap().contains("fc44"));
        // At least one problem device (the driverless reader).
        assert!(inv.problem_count() >= 1);
    }

    #[test]
    fn publish_writes_to_the_substrate_path_and_reads_back() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_tree(tmp.path());
        let store = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        let ids = fixture_ids();
        let tools = tool_availability(&ids);
        let inv = enumerate(&roots, &ids, tools, "test-box", &[]);
        let path = write_inventory(store.path(), &inv).unwrap();
        assert_eq!(
            path,
            mackes_mesh_types::device_inventory::inventory_path(store.path(), "test-box")
        );
        // A peer reads it straight off the substrate via the shared read helper.
        let read =
            mackes_mesh_types::device_inventory::read_inventory(store.path(), "test-box").unwrap();
        assert_eq!(read, inv);
        // No leftover temp file.
        assert!(!store
            .path()
            .join("device-inventory")
            .join(".test-box.json.tmp")
            .exists());
    }

    #[test]
    fn degrades_honestly_on_an_empty_host() {
        // No sysfs/proc at all → an empty-but-valid inventory, never a panic.
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        let inv = enumerate(
            &roots,
            &IdsDb::default(),
            ToolAvailability::default(),
            "bare",
            &[],
        );
        assert_eq!(inv.host, "bare");
        assert!(inv.categories.is_empty());
        assert_eq!(inv.device_count(), 0);
    }

    /// A one-device inventory for `host`, the device in `status` (DEVMGR-9
    /// transition fixtures).
    fn nic_inventory(host: &str, status: DeviceStatus, problem: Option<&str>) -> DeviceInventory {
        let dev = DeviceRecord {
            sysfs_path: Some("/sys/bus/pci/devices/0000:03:00.0".into()),
            problem: problem.map(str::to_string),
            ..DeviceRecord::new("Intel I219-LM Ethernet", status)
        };
        DeviceInventory {
            host: host.to_string(),
            published_at_ms: 1,
            summary: HostSummary::default(),
            tools: ToolAvailability::default(),
            categories: vec![DeviceCategory::new(category::NETWORK_ADAPTERS, vec![dev])],
        }
    }

    #[test]
    fn fault_transitions_fire_only_on_entry_into_a_problem_state() {
        let clean = nic_inventory("edge-1", DeviceStatus::Ok, None);
        let faulted = nic_inventory("edge-1", DeviceStatus::Degraded, Some("nic: I/O error"));
        // ok → degraded is an edge.
        let t = fault_transitions(Some(&clean), &faulted);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].name, "Intel I219-LM Ethernet");
        assert_eq!(t[0].status, DeviceStatus::Degraded);
        assert_eq!(t[0].reason.as_deref(), Some("nic: I/O error"));
        assert_eq!(t[0].key, "/sys/bus/pci/devices/0000:03:00.0");
        // A persisting fault is NOT a new edge (edge-triggered, never a re-fire).
        assert!(fault_transitions(Some(&faulted), &faulted).is_empty());
        // A recovery fires nothing.
        assert!(fault_transitions(Some(&faulted), &clean).is_empty());
        // A device arriving already broken (absent from prev) is an entry; so is
        // the host's very first publish (prev None — AlertGate semantics).
        let empty = DeviceInventory {
            categories: vec![],
            ..clean.clone()
        };
        assert_eq!(fault_transitions(Some(&empty), &faulted).len(), 1);
        assert_eq!(fault_transitions(None, &faulted).len(), 1);
        // An honest Unknown is NOT a fault — never an alert (§7).
        let unknown = nic_inventory("edge-1", DeviceStatus::Unknown, None);
        assert!(fault_transitions(Some(&clean), &unknown).is_empty());
        // …and ok→unknown→degraded still reads as an entry when it lands.
        assert_eq!(fault_transitions(Some(&unknown), &faulted).len(), 1);
    }

    #[test]
    fn fault_gate_debounces_flapping_per_device() {
        let mut gate = DeviceFaultGate::default();
        let t0 = std::time::Instant::now();
        assert!(gate.admit("dev-a", t0), "first entry fires");
        assert!(
            !gate.admit("dev-a", t0 + Duration::from_secs(60)),
            "a re-entry inside the cooldown is suppressed (flapping)"
        );
        assert!(
            gate.admit("dev-b", t0 + Duration::from_secs(60)),
            "an unrelated device is independent"
        );
        assert!(
            gate.admit("dev-a", t0 + FAULT_COOLDOWN),
            "after the cooldown the device may alert again"
        );
    }

    #[test]
    fn severity_and_fault_classification_are_honest() {
        assert!(is_fault(DeviceStatus::NoDriver));
        assert!(is_fault(DeviceStatus::Disabled));
        assert!(is_fault(DeviceStatus::Degraded));
        assert!(!is_fault(DeviceStatus::Ok));
        assert!(!is_fault(DeviceStatus::Unknown), "unknown is not a fault");
        assert_eq!(fault_severity(DeviceStatus::Degraded), "critical");
        assert_eq!(fault_severity(DeviceStatus::NoDriver), "warning");
        assert_eq!(fault_severity(DeviceStatus::Disabled), "warning");
    }

    #[test]
    fn a_simulated_fault_fires_one_debounced_notify_and_flapping_does_not_spam() {
        // The DEVMGR-9 acceptance, end to end over the real publish + Bus paths:
        // clean → faulted fires exactly one alert on the folded lane; the device
        // then flapping ok↔faulted inside the cooldown adds nothing.
        let store = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let mut gate = DeviceFaultGate::default();
        let host = "edge-1";
        let clean = nic_inventory(host, DeviceStatus::Ok, None);
        let faulted = nic_inventory(host, DeviceStatus::Degraded, Some("nic: I/O error"));

        let mut drive = |inv: &DeviceInventory, at: Instant| {
            let (_, transitions) = write_inventory_observing(store.path(), inv).unwrap();
            for t in &transitions {
                if gate.admit(&t.key, at) {
                    emit_fault_alert(&persist, host, t);
                }
            }
        };

        let t0 = Instant::now();
        drive(&clean, t0); // baseline publish — nothing to alert
        drive(&faulted, t0 + Duration::from_secs(300)); // the fault edge → ONE notify
        drive(&clean, t0 + Duration::from_secs(600)); // recovery — nothing
        drive(&faulted, t0 + Duration::from_secs(900)); // flap re-entry — debounced

        let alerts = persist.list_since(NOTIFY_TOPIC, None).unwrap();
        assert_eq!(alerts.len(), 1, "exactly one debounced fault notify");
        let body: serde_json::Value =
            serde_json::from_str(alerts[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["severity"], "critical");
        assert_eq!(body["source"], NOTIFY_SOURCE);
        assert_eq!(body["host"], host);
        assert_eq!(body["device"], "Intel I219-LM Ethernet");
        assert_eq!(body["reason"], "nic: I/O error");
        assert!(body["summary"]
            .as_str()
            .unwrap()
            .contains("entered state degraded"));
    }

    #[test]
    fn small_parsers() {
        assert_eq!(parse_uptime_secs("48120.42 100000.0\n"), Some(48120));
        assert_eq!(
            parse_os_pretty("NAME=Fedora\nPRETTY_NAME=\"Fedora Linux 44\"\n").as_deref(),
            Some("Fedora Linux 44")
        );
        assert_eq!(
            parse_meminfo_total_kb("MemTotal:  16072192 kB\n"),
            Some(16_072_192)
        );
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(1024), "1.0 KB");
        let (m, n) =
            parse_cpuinfo("model name\t: X\nprocessor\t: 0\nmodel name\t: X\nprocessor\t: 1\n");
        assert_eq!(m.as_deref(), Some("X"));
        assert_eq!(n, 2);
    }
}
