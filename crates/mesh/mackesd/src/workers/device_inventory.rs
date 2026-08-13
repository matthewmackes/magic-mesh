//! DEVMGR-1 — the device-inventory **enumeration engine** behind the existing
//! `hardware_probe` worker (`docs/design/about-device-manager.md`, locked
//! 2026-07-04).
//!
//! This is NOT a new worker (lock #16 — "extend an existing inventory worker,
//! not a brand-new one"): it is the enumeration + publish support the rank-0
//! [`super::hardware_probe`] worker calls on its tick, in the mold of the
//! crate-root `legacy_inventory` module. The `hardware_probe` census entry
//! (`worker_role::WORKER_REGISTRY`) is unchanged — the same worker now publishes a
//! second artifact.
//!
//! ## What it does (the producer side of §6)
//!
//! Walk the local host's Linux hardware graph **sysfs-first** — `/sys/bus/pci`,
//! `/sys/bus/usb`, `/sys/block`, `/sys/class/{input,thermal,hwmon,bluetooth,
//! power_supply}`, `/proc/{cpuinfo,meminfo,uptime}` — into the full locked
//! taxonomy (#4), naming devices from the `pci.ids`/`usb.ids` databases and
//! deriving each device's honest Linux **status + problem reason** (#11: a PCI
//! function with no driver bound is informational; PCI `enable=0` is never treated
//! as an administrative action; a corroborated dmesg error marks `degraded`.
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
//! Device faults are consumed by the System and Mesh Health authority. This
//! producer publishes only neutral inventory and never maintains a parallel
//! problem counter, transition ledger, or notification stream.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fs2::FileExt as _;
use mackes_mesh_types::device_inventory::{
    category, DeviceCategory, DeviceInventory, DeviceRecord, DeviceResources, DeviceStatus,
    HostSummary, ToolAvailability,
};

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

/// Sysfs attributes are tiny; `/proc/cpuinfo` is the largest expected input.
/// Keep even an unusually large host bounded before materializing it.
const MAX_READ_TRIM_BYTES: usize = 256 * 1024;

/// The vendor databases are larger than individual sysfs attributes, but
/// remain bounded static inputs rather than untrusted unbounded streams.
const MAX_IDS_DATABASE_BYTES: usize = 16 * 1024 * 1024;

/// Read a small sysfs/procfs file, trimmed; `None` when absent/empty/unreadable.
fn read_trim(path: &Path) -> Option<String> {
    use std::io::Read as _;

    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?
        .into()
    };

    #[cfg(not(unix))]
    let file = {
        // Keep non-Unix builds fail-soft without following an already-present
        // final symlink. Unix production targets use descriptor-level
        // NOFOLLOW + NONBLOCK above.
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if !metadata.file_type().is_file() {
            return None;
        }
        std::fs::File::open(path).ok()?
    };

    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_READ_TRIM_BYTES as u64 {
        return None;
    }

    let capacity = usize::try_from(metadata.len())
        .unwrap_or(MAX_READ_TRIM_BYTES)
        .min(MAX_READ_TRIM_BYTES)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    file.take((MAX_READ_TRIM_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_READ_TRIM_BYTES {
        return None;
    }

    let s = String::from_utf8(bytes).ok()?;
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
                if let Ok(text) = read_ids_database(Path::new(p)) {
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

fn read_ids_database(path: &Path) -> std::io::Result<String> {
    use std::io::Read as _;

    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_IDS_DATABASE_BYTES as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "hardware ID database is not a bounded regular file",
        ));
    }
    let mut text = String::new();
    std::fs::File::open(path)?
        .take((MAX_IDS_DATABASE_BYTES + 1) as u64)
        .read_to_string(&mut text)?;
    if text.len() > MAX_IDS_DATABASE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "hardware ID database exceeds its byte bound",
        ));
    }
    Ok(text)
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
/// Precedence: an explicitly policy-disabled device (`disabled=true`) wins; then a
/// driver-expecting device with no driver bound stays `unknown`; then a matched
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
        return (DeviceStatus::Unknown, None);
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
        // Linux exposes `enable=0` for many perfectly healthy PCI functions. It
        // is a power/configuration observation, not proof that an operator used
        // the platform's Disable action. Only an explicit policy/action record
        // may set `disabled`; no such record is part of this sysfs enumerator.
        let disabled = false;
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
    for dir in physical_block_children_bounded(&roots.sys.join("block")) {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let model = read_trim(&dir.join("device").join("model"));
        let vendor = read_trim(&dir.join("device").join("vendor"));
        let sectors = read_trim(&dir.join("size")).and_then(|s| s.parse::<u64>().ok());
        let size_bytes = sectors.map(|sectors| sectors.saturating_mul(512));
        let kernel_state = read_trim(&dir.join("device").join("state"))
            .filter(|state| BLOCK_DEVICE_STATES.contains(&state.as_str()));
        let dev = read_trim(&dir.join("dev")).filter(|dev| admitted_block_dev(dev));
        let read_only = read_trim(&dir.join("ro")).and_then(|value| match value.as_str() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        });
        let (status, problem) = match kernel_state.as_deref() {
            Some("running" | "live" | "active") => (DeviceStatus::Ok, None),
            Some(state @ ("offline" | "blocked" | "quiesce" | "suspended")) => (
                DeviceStatus::Degraded,
                Some(format!("kernel block state: {state}")),
            ),
            // Some block classes do not export `device/state`. A registered
            // major:minor with non-zero media is still kernel-owned evidence
            // that the provider has usable block storage; neither fact alone is
            // sufficient.
            None if dev.is_some() && sectors.is_some_and(|sectors| sectors > 0) => {
                (DeviceStatus::Ok, None)
            }
            _ => (
                DeviceStatus::Unknown,
                Some("block device readiness unavailable".to_string()),
            ),
        };
        let base = model.clone().unwrap_or(name);
        let name_disp = match size_bytes {
            Some(b) => format!("{base} ({})", human_bytes(b)),
            None => base,
        };
        let mut events = Vec::with_capacity(3);
        if let Some(state) = kernel_state {
            events.push(format!("kernel state: {state}"));
        }
        if let Some(dev) = dev {
            events.push(format!("device number: {dev}"));
        }
        if let Some(read_only) = read_only {
            events.push(format!("read-only: {read_only}"));
        }
        out.push(DeviceRecord {
            name: name_disp,
            vendor,
            model,
            ids: None,
            sysfs_path: Some(dir.to_string_lossy().into_owned()),
            driver: None,
            driver_version: None,
            status,
            problem,
            resources: DeviceResources::default(),
            events,
        });
    }
    out
}

const BLOCK_DEVICE_STATES: &[&str] = &[
    "running",
    "live",
    "active",
    "offline",
    "blocked",
    "quiesce",
    "suspended",
];

fn admitted_block_dev(value: &str) -> bool {
    let Some((major, minor)) = value.split_once(':') else {
        return false;
    };
    !major.is_empty()
        && !minor.is_empty()
        && major.len() <= 5
        && minor.len() <= 7
        && major.bytes().all(|byte| byte.is_ascii_digit())
        && minor.bytes().all(|byte| byte.is_ascii_digit())
}

/// Maximum physical disks published by one inventory generation.
///
/// `/sys/block` is kernel-owned in production, but the resulting inventory is
/// replicated fleet-wide and rendered by every Workers client. Keep that state
/// bounded even when a malformed sysfs mount exposes an excessive number of
/// entries. Virtual block devices are rejected before admission so an attacker
/// cannot hide physical disks by filling the lexicographically earliest rows
/// with `loop*`/`dm-*` names.
const MAX_BLOCK_DEVICES: usize = 256;

fn physical_block_children_bounded(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = BTreeSet::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if is_virtual_block(name) {
            continue;
        }
        out.insert(path);
        if out.len() > MAX_BLOCK_DEVICES {
            if let Some(last) = out.iter().next_back().cloned() {
                out.remove(&last);
            }
        }
    }
    out.into_iter().collect()
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
    class_named_devices_bounded(roots, class, name_file, usize::MAX)
}

fn class_named_devices_bounded(
    roots: &SysfsRoots,
    class: &str,
    name_file: Option<&str>,
    limit: usize,
) -> Vec<DeviceRecord> {
    let mut out = Vec::new();
    for dir in sorted_children_bounded(&roots.sys.join("class").join(class), limit) {
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

/// Maximum input devices retained in one inventory generation.
const MAX_INPUT_DEVICES: usize = 256;

/// Input devices (`/sys/class/input/input*/name`).
///
/// Input names are kernel-provider observations, not authority.  A missing or
/// unreadable name therefore remains a sourced row with an explicit unavailable
/// state instead of being promoted to a healthy device using its node name.
/// Admission is deterministic and bounded before any attributes are read.
#[must_use]
pub fn input_devices(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    input_children_bounded(&roots.sys.join("class").join("input"))
        .into_iter()
        .filter_map(|dir| {
            let node = dir.file_name()?.to_str()?;
            let observed_name = read_trim(&dir.join("name"));
            let inhibited =
                read_trim(&dir.join("inhibited")).and_then(|value| match value.as_str() {
                    "0" => Some(false),
                    "1" => Some(true),
                    _ => None,
                });
            let inhibit_attribute_present = dir.join("inhibited").exists();
            let (status, problem) = match (observed_name.is_some(), inhibited) {
                (_, Some(true)) => (
                    DeviceStatus::Disabled,
                    Some("input device inhibited by kernel policy".to_string()),
                ),
                (true, Some(false) | None) if !inhibit_attribute_present || inhibited.is_some() => {
                    (DeviceStatus::Ok, None)
                }
                (false, Some(false) | None)
                    if !inhibit_attribute_present || inhibited.is_some() =>
                {
                    (
                        DeviceStatus::Unknown,
                        Some("input device name unavailable".to_string()),
                    )
                }
                _ => (
                    DeviceStatus::Unknown,
                    Some("input inhibition state unavailable".to_string()),
                ),
            };
            let events = inhibited
                .map(|inhibited| {
                    vec![format!(
                        "inhibited: {}",
                        if inhibited { "yes" } else { "no" }
                    )]
                })
                .unwrap_or_default();
            Some(DeviceRecord {
                name: observed_name.unwrap_or_else(|| node.to_string()),
                sysfs_path: Some(dir.to_string_lossy().into_owned()),
                problem,
                events,
                ..DeviceRecord::new(node, status)
            })
        })
        .collect()
}

/// Select physical/logical `input*` provider rows before applying the budget;
/// sibling `event*`/`mouse*` nodes must not consume input-device capacity.
fn input_children_bounded(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut admitted = BTreeSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(node) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !node.starts_with("input") {
            continue;
        }
        admitted.insert(path);
        if admitted.len() > MAX_INPUT_DEVICES {
            if let Some(last) = admitted.iter().next_back().cloned() {
                admitted.remove(&last);
            }
        }
    }
    admitted.into_iter().collect()
}

/// Maximum physical network interfaces published by one inventory generation.
/// Linux interface names are kernel-bounded; this additional entity cap keeps a
/// hostile fixture or malformed sysfs mount from expanding the mesh artifact.
const MAX_NETWORK_INTERFACES: usize = 256;

/// Maximum physical DRM connectors retained in one inventory generation.
const MAX_DRM_CONNECTORS: usize = 128;
/// Maximum advertised modes retained for one connector.
const MAX_DRM_CONNECTOR_MODES: usize = 16;
/// Maximum thermal and hwmon entities retained in one inventory generation.
const MAX_SENSOR_DEVICES: usize = 128;
/// Maximum power-supply entities retained in one inventory generation.
///
/// Power supplies are kernel class entries, but the inventory is a bounded
/// mesh artifact.  Keep admission deterministic so a malformed or hostile
/// class tree cannot expand one host's published hardware state without limit.
const MAX_POWER_SUPPLIES: usize = 64;

/// Read at most `limit` lexicographically first children with bounded memory.
fn sorted_children_bounded(dir: &Path, limit: usize) -> Vec<PathBuf> {
    if limit == 0 {
        return Vec::new();
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = BTreeSet::new();
    for entry in rd.flatten() {
        out.insert(entry.path());
        if out.len() > limit {
            if let Some(last) = out.iter().next_back().cloned() {
                out.remove(&last);
            }
        }
    }
    out.into_iter().collect()
}

fn drm_connector_name(node: &str) -> Option<&str> {
    if node.len() > 64 {
        return None;
    }
    let (card, connector) = node.split_once('-')?;
    let card_index = card.strip_prefix("card")?;
    if card_index.is_empty()
        || !card_index.bytes().all(|byte| byte.is_ascii_digit())
        || connector.is_empty()
        || !connector
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    Some(connector)
}

fn admitted_drm_mode(mode: &str) -> Option<&str> {
    if mode.len() > 16 {
        return None;
    }
    let (width, height) = mode.split_once('x')?;
    let width = width.parse::<u16>().ok()?;
    let height = height.parse::<u16>().ok()?;
    (width > 0 && width <= 16_384 && height > 0 && height <= 16_384).then_some(mode)
}

/// Physical DRM connectors (`/sys/class/drm/card*-*`).
///
/// Only allowlisted kernel state is published. EDID blobs, display names, and
/// other potentially identifying payloads are deliberately not read. A
/// disconnected connector is normal inventory evidence, not a health fault.
#[must_use]
pub fn display_connectors(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    sorted_children_bounded(
        &roots.sys.join("class").join("drm"),
        MAX_DRM_CONNECTORS,
    )
    .into_iter()
    .filter_map(|dir| {
        let node = dir.file_name()?.to_str()?;
        let connector = drm_connector_name(node)?;
        let connection = read_trim(&dir.join("status")).filter(|value| {
            matches!(value.as_str(), "connected" | "disconnected" | "unknown")
        });
        let enabled = read_trim(&dir.join("enabled"))
            .filter(|value| matches!(value.as_str(), "enabled" | "disabled" | "unknown"));
        let modes = read_trim(&dir.join("modes"))
            .map(|body| {
                body.lines()
                    .filter_map(admitted_drm_mode)
                    .take(MAX_DRM_CONNECTOR_MODES)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let status = if connection.as_deref() == Some("connected") {
            DeviceStatus::Ok
        } else {
            DeviceStatus::Unknown
        };
        let problem = connection
            .is_none()
            .then(|| "connector state unavailable".to_string());
        let mut events = Vec::with_capacity(3);
        if let Some(connection) = connection {
            events.push(format!("connection: {connection}"));
        }
        if let Some(enabled) = enabled {
            events.push(format!("enabled: {enabled}"));
        }
        if !modes.is_empty() {
            events.push(format!("modes: {}", modes.join(", ")));
        }
        Some(DeviceRecord {
            name: format!("Display connector {connector}"),
            sysfs_path: Some(dir.to_string_lossy().into_owned()),
            driver: bound_driver(&dir.join("device")),
            driver_version: driver_version(&dir.join("device")),
            status,
            problem,
            events,
            ..DeviceRecord::new(node, status)
        })
    })
    .collect()
}

/// Physical network interfaces (`/sys/class/net/*`).
///
/// This is intentionally credential-free: it never reads addresses, SSIDs,
/// connection profiles, or NetworkManager state. The class path is the exact
/// generation identity consumed by the safe device-control executor. A
/// `device` link distinguishes hardware-backed interfaces from loopback and
/// transient virtual links; link state is observational and never interpreted
/// as an administrative disable action.
#[must_use]
pub fn network_interfaces(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    sorted_children_bounded(&roots.sys.join("class").join("net"), MAX_NETWORK_INTERFACES)
        .into_iter()
        .filter_map(|dir| {
            let node = dir.file_name()?.to_str()?;
            if node == "lo" || std::fs::symlink_metadata(dir.join("device")).is_err() {
                return None;
            }

            let wireless = dir.join("wireless").is_dir();
            let operstate = read_trim(&dir.join("operstate"));
            let carrier = read_trim(&dir.join("carrier")).and_then(|value| match value.as_str() {
                "0" => Some("absent"),
                "1" => Some("present"),
                _ => None,
            });
            let (status, problem) = match (operstate.as_deref(), carrier) {
                (Some("up"), Some("absent")) => (
                    DeviceStatus::Unknown,
                    Some("carrier absent while link reports up".to_string()),
                ),
                (Some("up"), _) => (DeviceStatus::Ok, None),
                (
                    Some(
                        state @ ("down" | "dormant" | "lowerlayerdown" | "notpresent" | "testing"
                        | "unknown"),
                    ),
                    _,
                ) => (DeviceStatus::Unknown, Some(format!("link state: {state}"))),
                _ => (
                    DeviceStatus::Unknown,
                    Some("link state unavailable".to_string()),
                ),
            };
            let mut events = Vec::with_capacity(3);
            events.push(if wireless {
                "kind: wireless".to_string()
            } else {
                "kind: wired".to_string()
            });
            if let Some(state) = operstate.filter(|state| {
                matches!(
                    state.as_str(),
                    "up" | "down"
                        | "dormant"
                        | "lowerlayerdown"
                        | "notpresent"
                        | "testing"
                        | "unknown"
                )
            }) {
                events.push(format!("link state: {state}"));
            }
            if let Some(carrier) = carrier {
                events.push(format!("carrier: {carrier}"));
            }

            Some(DeviceRecord {
                name: if wireless {
                    format!("Wi-Fi interface {node}")
                } else {
                    format!("Network interface {node}")
                },
                sysfs_path: Some(dir.to_string_lossy().into_owned()),
                driver: bound_driver(&dir.join("device")),
                driver_version: driver_version(&dir.join("device")),
                status,
                problem,
                events,
                ..DeviceRecord::new(node, status)
            })
        })
        .collect()
}

/// Sensors + thermal zones (`/sys/class/thermal/*` types + `/sys/class/hwmon/*`
/// names). A thermal zone carries its current temperature as an event line.
#[must_use]
#[allow(clippy::cast_precision_loss)] // a millidegree i64 → °C f64 is lossless in range
pub fn sensors(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    let mut out = Vec::new();
    for dir in sorted_children_bounded(&roots.sys.join("class").join("thermal"), MAX_SENSOR_DEVICES)
    {
        let node = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if !node.starts_with("thermal_zone") || out.len() >= MAX_SENSOR_DEVICES {
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
    if out.len() < MAX_SENSOR_DEVICES {
        out.extend(class_named_devices_bounded(
            roots,
            "hwmon",
            Some("name"),
            MAX_SENSOR_DEVICES - out.len(),
        ));
    }
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

const MAX_POWER_IDENTITY_BYTES: usize = 128;

fn power_identity(value: Option<String>) -> Option<String> {
    value.filter(|value| {
        value.len() <= MAX_POWER_IDENTITY_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' ')
    })
}

fn power_supply_type(value: Option<String>) -> Option<String> {
    value.filter(|value| {
        matches!(
            value.as_str(),
            "Unknown"
                | "Battery"
                | "UPS"
                | "Mains"
                | "USB"
                | "USB_DCP"
                | "USB_CDP"
                | "USB_ACA"
                | "USB_C"
                | "USB_PD"
                | "USB_PD_DRP"
                | "BrickID"
                | "Wireless"
        )
    })
}

fn power_supply_status(value: Option<String>) -> Option<String> {
    value.filter(|value| {
        matches!(
            value.as_str(),
            "Unknown" | "Charging" | "Discharging" | "Not charging" | "Full"
        )
    })
}

/// Battery / power supplies (`/sys/class/power_supply/*`).
///
/// Only the kernel ABI's bounded enumerations and numeric ranges cross the
/// provider boundary. Missing or malformed core state is published as an
/// unavailable row rather than a fabricated healthy supply.
#[must_use]
pub fn power_supplies(roots: &SysfsRoots) -> Vec<DeviceRecord> {
    let mut out = Vec::new();
    for dir in sorted_children_bounded(
        &roots.sys.join("class").join("power_supply"),
        MAX_POWER_SUPPLIES,
    ) {
        let node = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let kind = power_supply_type(read_trim(&dir.join("type")));
        let model = power_identity(read_trim(&dir.join("model_name")));
        let vendor = power_identity(read_trim(&dir.join("manufacturer")));
        let disp = model.clone().unwrap_or_else(|| {
            kind.as_deref().map_or_else(
                || format!("Power supply ({node})"),
                |kind| format!("{kind} ({node})"),
            )
        });
        let capacity = read_trim(&dir.join("capacity"))
            .and_then(|value| value.parse::<u8>().ok())
            .filter(|capacity| *capacity <= 100);
        let supply_status = power_supply_status(read_trim(&dir.join("status")));
        let online = read_trim(&dir.join("online")).and_then(|value| match value.as_str() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        });
        let state_available = match kind.as_deref() {
            Some("Battery") | Some("UPS") => {
                capacity.is_some()
                    && supply_status
                        .as_deref()
                        .is_some_and(|status| status != "Unknown")
            }
            Some("Unknown") | None => false,
            Some(_) => online.is_some(),
        };
        let status = if state_available {
            DeviceStatus::Ok
        } else {
            DeviceStatus::Unknown
        };
        let mut rec = DeviceRecord {
            vendor,
            model,
            sysfs_path: Some(dir.to_string_lossy().into_owned()),
            problem: (!state_available).then(|| "power supply state unavailable".to_string()),
            ..DeviceRecord::new(disp, status)
        };
        if let Some(kind) = kind {
            rec.events.push(format!("type: {kind}"));
        }
        if let Some(capacity) = capacity {
            rec.events.push(format!("capacity: {capacity}%"));
        }
        if let Some(supply_status) = supply_status {
            rec.events.push(format!("status: {supply_status}"));
        }
        if let Some(online) = online {
            rec.events
                .push(format!("online: {}", if online { "yes" } else { "no" }));
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

/// Reconcile independently enumerated sysfs views before publication.
///
/// Linux exposes one hardware object through several class and bus trees.  If
/// two paths resolve to the same kernel object, an exact duplicate is harmless,
/// but different category/body claims are equivocation: choosing whichever
/// provider happened to run first would publish unsupported state.  Suppress
/// only that stable identity while retaining unrelated devices.
fn suppress_conflicting_sysfs_identities(
    buckets: &mut BTreeMap<String, Vec<DeviceRecord>>,
) {
    let mut admitted: BTreeMap<PathBuf, (String, DeviceRecord)> = BTreeMap::new();
    let mut conflicted = BTreeSet::new();

    for (category, records) in buckets.iter() {
        for record in records {
            let Some(identity) = stable_sysfs_identity(record) else {
                continue;
            };
            let mut body = record.clone();
            body.sysfs_path = None;
            match admitted.get(&identity) {
                Some((admitted_category, admitted_body))
                    if admitted_category != category || admitted_body != &body =>
                {
                    conflicted.insert(identity);
                }
                Some(_) => {}
                None => {
                    admitted.insert(identity, (category.clone(), body));
                }
            }
        }
    }

    let mut published = BTreeSet::new();
    for records in buckets.values_mut() {
        records.retain(|record| {
            if record.sysfs_path.is_none() {
                return true;
            }
            let Some(identity) = stable_sysfs_identity(record) else {
                return false;
            };
            !conflicted.contains(&identity) && published.insert(identity)
        });
    }
}

fn stable_sysfs_identity(record: &DeviceRecord) -> Option<PathBuf> {
    let path = Path::new(record.sysfs_path.as_deref()?);
    // Re-attest the kernel object at reconciliation time.  A hot-unplug can
    // remove a class/bus node after its attributes were read but before this
    // generation is assembled.  Falling back to the stale textual path would
    // publish hardware that no longer exists and make that stale row look as
    // authoritative as a live provider identity until the next worker tick.
    std::fs::canonicalize(path).ok()
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
    add(&mut buckets, category::DISPLAY, display_connectors(roots));
    add(
        &mut buckets,
        category::NETWORK_ADAPTERS,
        network_interfaces(roots),
    );
    add(&mut buckets, category::INPUT, input_devices(roots));
    add(&mut buckets, category::SENSORS, sensors(roots));
    add(&mut buckets, category::BLUETOOTH, bluetooth(roots));
    add(&mut buckets, category::POWER, power_supplies(roots));

    suppress_conflicting_sysfs_identities(&mut buckets);

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
    // Bind this generation to when its probe began. A slow pre-restart census
    // must not acquire a newer generation merely because it finished last.
    let probe_started_at_ms = now_ms();
    let roots = SysfsRoots::system();
    let ids = IdsDb::load();
    let tools = tool_availability(&ids);
    let dmesg = capture_dmesg();
    let mut inv = enumerate(&roots, &ids, tools, hostname, &dmesg);
    inv.published_at_ms = probe_started_at_ms;
    let inventory_path = write_inventory(workgroup_root, &inv)?;
    if let Err(error) = publish_display_readiness(workgroup_root, hostname) {
        tracing::warn!(%error, "display-provider publication failed");
    }
    Ok(inventory_path)
}

// ── truthful physical-display provider ───────────────────────────────────────

const DISPLAY_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DISPLAY_CONNECTORS: usize = 64;
const MAX_DISPLAY_FACT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum DisplayReadiness {
    Ready,
    Disconnected,
    Disabled,
    Unknown,
}

#[derive(Debug, serde::Serialize)]
struct DisplaySnapshot<'a> {
    schema_version: u16,
    node_id: &'a str,
    observed_unix_ms: u64,
    readiness: DisplayReadiness,
    connectors: u16,
    connected_connectors: u16,
    reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrmConnectorStatus {
    Connected,
    Disconnected,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DrmConnectorFact {
    identity: String,
    card: u16,
    status: DrmConnectorStatus,
    enabled: bool,
    has_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplaySeat {
    Active(u32),
    Inactive,
}

fn parse_display_seat(raw: &str) -> Option<DisplaySeat> {
    if raw.is_empty() || raw.len() > MAX_DISPLAY_FACT_BYTES || raw.contains('\0') {
        return None;
    }
    let mut fields = BTreeMap::new();
    for line in raw.lines() {
        let (key, value) = line.split_once('=')?;
        if !matches!(key, "LoadState" | "ActiveState" | "SubState" | "MainPID")
            || fields.insert(key, value).is_some()
        {
            return None;
        }
    }
    if fields.len() != 4 || fields.get("LoadState") != Some(&"loaded") {
        return None;
    }
    match (fields.get("ActiveState"), fields.get("SubState"), fields.get("MainPID")) {
        (Some(&"active"), Some(&"running"), Some(pid)) => pid
            .parse::<u32>()
            .ok()
            .filter(|pid| *pid > 1)
            .map(DisplaySeat::Active),
        (Some(&"inactive" | &"failed"), Some(_), Some(&"0")) => Some(DisplaySeat::Inactive),
        _ => None,
    }
}

fn parse_drm_masters(raw: &str) -> Option<BTreeSet<u32>> {
    if raw.is_empty() || raw.len() > MAX_DISPLAY_FACT_BYTES || raw.contains('\0') {
        return None;
    }
    let mut lines = raw.lines();
    let header = lines.next()?.split_whitespace().collect::<Vec<_>>();
    let pid_column = header.iter().position(|field| *field == "pid")?;
    let master_column = header.iter().position(|field| *field == "master")?;
    let mut masters = BTreeSet::new();
    for line in lines.filter(|line| !line.trim().is_empty()) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != header.len() {
            return None;
        }
        let pid = fields.get(pid_column)?.parse::<u32>().ok().filter(|pid| *pid > 1)?;
        match *fields.get(master_column)? {
            "y" if masters.insert(pid) => {}
            "n" => {}
            _ => return None,
        }
    }
    Some(masters)
}

fn classify_display(
    mut connectors: Vec<DrmConnectorFact>,
    seat: Option<DisplaySeat>,
    masters: Option<BTreeMap<u16, BTreeSet<u32>>>,
) -> (DisplayReadiness, usize, usize, &'static str) {
    connectors.sort_unstable_by(|left, right| left.identity.cmp(&right.identity));
    let malformed = connectors.len() > MAX_DISPLAY_CONNECTORS
        || connectors.windows(2).any(|pair| pair[0].identity == pair[1].identity)
        || connectors.iter().any(|fact| {
            fact.identity.is_empty()
                || fact.identity.len() > 128
                || (fact.status == DrmConnectorStatus::Disconnected && (fact.enabled || fact.has_mode))
                || (fact.status == DrmConnectorStatus::Connected && (!fact.enabled || !fact.has_mode))
        });
    if malformed {
        return (DisplayReadiness::Unknown, 0, 0, "DRM connector facts are malformed or contradictory");
    }
    let connected = connectors.iter().filter(|fact| fact.status == DrmConnectorStatus::Connected).count();
    if connectors.iter().any(|fact| fact.status == DrmConnectorStatus::Unknown) {
        return (DisplayReadiness::Unknown, connectors.len(), connected, "DRM connector state is unknown");
    }
    let (Some(seat), Some(masters)) = (seat, masters) else {
        return (DisplayReadiness::Unknown, connectors.len(), connected, "seat or DRM-master facts are unavailable");
    };
    let cards = connectors.iter().map(|fact| fact.card).collect::<BTreeSet<_>>();
    if masters.keys().any(|card| !cards.contains(card)) || masters.values().any(|set| set.len() > 1) {
        return (DisplayReadiness::Unknown, connectors.len(), connected, "DRM-master identity is substituted or ambiguous");
    }
    match seat {
        DisplaySeat::Inactive if masters.values().any(|set| !set.is_empty()) => (DisplayReadiness::Unknown, connectors.len(), connected, "disabled seat contradicts a live DRM master"),
        DisplaySeat::Inactive => (DisplayReadiness::Disabled, connectors.len(), connected, "Construct seat service is disabled"),
        DisplaySeat::Active(_) if connected == 0 && masters.values().all(BTreeSet::is_empty) => (DisplayReadiness::Disconnected, connectors.len(), 0, "no physical display is connected"),
        DisplaySeat::Active(pid) => {
            let exact_owner = connectors.iter().filter(|fact| fact.status == DrmConnectorStatus::Connected).all(|fact| {
                masters.get(&fact.card).is_some_and(|set| set.len() == 1 && set.contains(&pid))
            }) && masters.values().all(|set| set.is_empty() || set.contains(&pid));
            if exact_owner {
                (DisplayReadiness::Ready, connectors.len(), connected, "connected displays are owned by the active Construct seat")
            } else {
                (DisplayReadiness::Unknown, connectors.len(), connected, "DRM master does not match the active Construct seat")
            }
        }
    }
}

fn display_file(path: &Path) -> Option<String> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_DISPLAY_FACT_BYTES as u64 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    (bytes.len() <= MAX_DISPLAY_FACT_BYTES).then(|| String::from_utf8(bytes).ok()).flatten()
}

fn gather_drm_connectors(root: &Path) -> Option<Vec<DrmConnectorFact>> {
    let mut entries = std::fs::read_dir(root).ok()?.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_unstable_by_key(std::fs::DirEntry::file_name);
    let mut facts = Vec::new();
    for entry in entries {
        let identity = entry.file_name().into_string().ok()?;
        let Some((card, _)) = identity
            .strip_prefix("card")
            .and_then(|name| name.split_once('-'))
        else {
            continue;
        };
        let card = card.parse().ok()?;
        let status = match display_file(&entry.path().join("status"))?.trim() {
            "connected" => DrmConnectorStatus::Connected,
            "disconnected" => DrmConnectorStatus::Disconnected,
            "unknown" => DrmConnectorStatus::Unknown,
            _ => return None,
        };
        let enabled = match display_file(&entry.path().join("enabled"))?.trim() {
            "enabled" => true,
            "disabled" => false,
            _ => return None,
        };
        facts.push(DrmConnectorFact {
            identity,
            card,
            status,
            enabled,
            has_mode: !display_file(&entry.path().join("modes"))?.trim().is_empty(),
        });
        if facts.len() > MAX_DISPLAY_CONNECTORS { return None; }
    }
    Some(facts)
}

fn gather_display_seat() -> Option<DisplaySeat> {
    let mut command = std::process::Command::new("systemctl");
    command.args(["show", "mde-shell-egui.service", "--property=LoadState,ActiveState,SubState,MainPID"]);
    let output = super::proc::output_with_timeout(command, DISPLAY_COMMAND_TIMEOUT).ok()?;
    output.status.success().then(|| String::from_utf8(output.stdout).ok()).flatten().as_deref().and_then(parse_display_seat)
}

fn gather_drm_masters(root: &Path) -> Option<BTreeMap<u16, BTreeSet<u32>>> {
    let mut facts = BTreeMap::new();
    for entry in std::fs::read_dir(root).ok()?.filter_map(Result::ok) {
        let card = entry.file_name().into_string().ok()?.parse::<u16>().ok()?;
        let clients = parse_drm_masters(&display_file(&entry.path().join("clients"))?)?;
        if facts.insert(card, clients).is_some() { return None; }
    }
    Some(facts)
}

fn publish_display_readiness(workgroup_root: &Path, hostname: &str) -> std::io::Result<PathBuf> {
    if hostname.is_empty() || hostname.len() > 128 || !hostname.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')) {
        return Err(std::io::Error::other("invalid display-provider node identity"));
    }
    let (readiness, connectors, connected_connectors, reason) = classify_display(
        gather_drm_connectors(Path::new("/sys/class/drm")).unwrap_or_default(),
        gather_display_seat(),
        gather_drm_masters(Path::new("/sys/kernel/debug/dri")),
    );
    let snapshot = DisplaySnapshot {
        schema_version: 1,
        node_id: hostname,
        observed_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().try_into().unwrap_or(u64::MAX),
        readiness,
        connectors: connectors.try_into().unwrap_or(u16::MAX),
        connected_connectors: connected_connectors.try_into().unwrap_or(u16::MAX),
        reason,
    };
    let dir = workgroup_root.join("display-provider");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{hostname}.json"));
    let temporary = dir.join(format!(".{hostname}.json.tmp"));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&snapshot).map_err(std::io::Error::other)?)?;
    std::fs::rename(&temporary, &path)?;
    Ok(path)
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
    let lock_path = dir.join(format!(".{}.lock", inv.host));
    let lock = open_inventory_lock(&lock_path)?;
    lock.lock_exclusive()?;

    if let Some(current) = mackes_mesh_types::device_inventory::read_inventory(
        workgroup_root,
        &inv.host,
    ) {
        if current.published_at_ms > inv.published_at_ms
            || (current.published_at_ms == inv.published_at_ms && current != *inv)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "refusing stale or equivocated device inventory generation {} behind {}",
                    inv.published_at_ms, current.published_at_ms
                ),
            ));
        }
    }

    let body = serde_json::to_string_pretty(inv)?;
    let tmp = dir.join(format!(".{}.json.tmp", inv.host));
    write_inventory_temp(&tmp, body.as_bytes())?;
    std::fs::rename(&tmp, &path)?;
    // The rename makes the row visible atomically, but durability also needs
    // the staged file and its containing directory flushed before reporting
    // publication success. Otherwise a crash can lose the new generation
    // while callers already hold a successful result.
    let published = File::open(&path)?;
    published.sync_all()?;
    File::open(&dir)?.sync_all()?;
    Ok(path)
}

/// Replace the fixed per-host staging row without following a pre-planted
/// symlink. The host lock makes reuse of this deterministic path safe, while
/// an exclusive descriptor open keeps publication confined to the inventory
/// directory even if stale or hostile substrate state occupies the temp name.
/// Only an unlinked, single-link regular residue may be reclaimed.
fn write_inventory_temp(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::MetadataExt as _;
    use rustix::fs::{Mode, OFlags};

    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && metadata.nlink() == 1 => {
            std::fs::remove_file(path)?;
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "inventory staging row is not a private regular file",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let fd = rustix::fs::open(
        path,
        OFlags::CREATE
            | OFlags::EXCL
            | OFlags::WRONLY
            | OFlags::CLOEXEC
            | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )?;
    let mut file = File::from(fd);
    file.write_all(body)?;
    file.sync_all()
}

/// Open the stable per-host publication lock without following a substituted
/// final symlink. The lock serializes the generation check with the rename.
fn open_inventory_lock(path: &Path) -> std::io::Result<File> {
    use rustix::fs::{Mode, OFlags};

    let fd = rustix::fs::open(
        path,
        OFlags::CREATE | OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )?;
    Ok(fd.into())
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
        put(&sys.join("block/nvme0n1/dev"), "259:0\n");
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
        put(&sys.join("class/drm/card0-eDP-1/status"), "connected\n");
        put(&sys.join("class/drm/card0-eDP-1/enabled"), "enabled\n");
        put(
            &sys.join("class/drm/card0-eDP-1/modes"),
            "1920x1080\n1280x720\n",
        );
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
    fn oversized_ids_database_is_rejected_before_parse() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pci.ids");
        std::fs::write(&path, vec![b'x'; MAX_IDS_DATABASE_BYTES + 1]).unwrap();
        let error = read_ids_database(&path).expect_err("oversized ID database must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn read_trim_preserves_small_ids_and_rejects_hostile_values() {
        let tmp = tempfile::tempdir().unwrap();

        let valid = tmp.path().join("valid");
        put(&valid, "\n 0x8086 \n");
        assert_eq!(read_trim(&valid).as_deref(), Some("0x8086"));
        assert_eq!(
            read_trim(&valid).and_then(|s| parse_hex_id(&s)),
            Some(0x8086)
        );

        let empty = tmp.path().join("empty");
        put(&empty, " \n\t");
        assert!(read_trim(&empty).is_none());

        let invalid_utf8 = tmp.path().join("invalid-utf8");
        fs::write(&invalid_utf8, [0xff, 0xfe, 0xfd]).unwrap();
        assert!(read_trim(&invalid_utf8).is_none());

        let oversized = tmp.path().join("oversized");
        fs::write(
            &oversized,
            vec![b'x'; MAX_READ_TRIM_BYTES.saturating_add(1)],
        )
        .unwrap();
        assert!(read_trim(&oversized).is_none());

        let directory = tmp.path().join("directory");
        fs::create_dir(&directory).unwrap();
        assert!(read_trim(&directory).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn read_trim_rejects_final_symlinks_and_special_files() {
        use std::os::unix::fs::symlink;
        use std::process::Command;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        put(&outside, "0xffff\n");

        let symlinked = tmp.path().join("symlinked");
        symlink(&outside, &symlinked).unwrap();
        assert!(read_trim(&symlinked).is_none());

        let fifo = tmp.path().join("fifo");
        if Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|status| status.success())
        {
            assert!(read_trim(&fifo).is_none());
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
        // A missing driver binding is evidence, not an administrative fault.
        let (s, p) = derive_status(false, true, None, None);
        assert_eq!(s, DeviceStatus::Unknown);
        assert_eq!(p, None);
        // A bound device → ok, no phantom problem.
        assert_eq!(
            derive_status(false, true, Some("i915"), None).0,
            DeviceStatus::Ok
        );
        // A bridge (doesn't bind a driver) with no driver → ok, not flagged.
        assert_eq!(derive_status(false, false, None, None).0, DeviceStatus::Ok);
        // An explicit platform policy record (the boolean input) is authoritative.
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
        assert!(
            by_cat
                .values()
                .flatten()
                .all(|device| device.status != DeviceStatus::Disabled),
            "PCI sysfs enable=0 alone must never classify a device as disabled"
        );
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
        // The driverless card reader remains neutral inventory evidence.
        let pci = &by_cat[category::PCI_DEVICES];
        let reader = pci
            .iter()
            .find(|d| d.ids.as_deref() == Some("10ec:5227"))
            .unwrap();
        assert_eq!(reader.status, DeviceStatus::Unknown);
        assert_eq!(reader.problem, None);
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
    fn input_provider_is_bounded_and_reports_unavailable_names_truthfully() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        let input = roots.sys.join("class/input");
        for index in 0..MAX_INPUT_DEVICES.saturating_add(16) {
            let device = input.join(format!("input{index:04}"));
            if index != 0 {
                put(&device.join("name"), &format!("Input device {index}\n"));
            } else {
                fs::create_dir_all(&device).unwrap();
            }
        }
        put(&input.join("input0001/inhibited"), "1\n");
        put(&input.join("input0002/inhibited"), "invalid\n");
        put(&input.join("input0003/inhibited"), "0\n");
        put(&input.join("event0000/dev"), "13:64\n");

        let records = input_devices(&roots);
        assert_eq!(records.len(), MAX_INPUT_DEVICES);
        assert_eq!(records[0].name, "input0000");
        assert_eq!(records[0].status, DeviceStatus::Unknown);
        assert_eq!(
            records[0].problem.as_deref(),
            Some("input device name unavailable")
        );
        assert_eq!(records[1].status, DeviceStatus::Disabled);
        assert_eq!(
            records[1].problem.as_deref(),
            Some("input device inhibited by kernel policy")
        );
        assert_eq!(records[1].events, ["inhibited: yes"]);
        assert_eq!(records[2].status, DeviceStatus::Unknown);
        assert_eq!(
            records[2].problem.as_deref(),
            Some("input inhibition state unavailable")
        );
        assert_eq!(records[3].status, DeviceStatus::Ok);
        assert_eq!(records[3].events, ["inhibited: no"]);
        assert_eq!(records.last().unwrap().name, "Input device 255");
        assert!(records
            .iter()
            .all(|record| !record.name.starts_with("event")));
    }

    #[test]
    fn sensors_are_bounded_and_do_not_publish_untrusted_payloads() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        for index in 0..MAX_SENSOR_DEVICES.saturating_add(16) {
            let zone = roots
                .sys
                .join("class/thermal")
                .join(format!("thermal_zone{index:03}"));
            put(&zone.join("type"), &format!("sensor-{index}\n"));
            put(&zone.join("temp"), "42000\n");
        }
        put(
            &roots.sys.join("class/hwmon/hwmon999/name"),
            "credential-like-sensor-payload\n",
        );

        let records = sensors(&roots);
        assert_eq!(records.len(), MAX_SENSOR_DEVICES);
        assert_eq!(records[0].name, "Thermal zone: sensor-0");
        assert!(!serde_json::to_string(&records)
            .unwrap()
            .contains("credential-like"));
    }

    #[test]
    fn power_supplies_are_bounded_and_deterministically_admitted() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        let power = roots.sys.join("class/power_supply");
        for index in 0..MAX_POWER_SUPPLIES.saturating_add(16) {
            let supply = power.join(format!("supply-{index:03}"));
            put(&supply.join("type"), "Battery\n");
            put(&supply.join("model_name"), &format!("Model {index}\n"));
            put(&supply.join("capacity"), "82\n");
            put(&supply.join("status"), "Discharging\n");
        }

        let records = power_supplies(&roots);
        assert_eq!(records.len(), MAX_POWER_SUPPLIES);
        assert_eq!(records[0].name, "Model 0");
        assert_eq!(records.last().unwrap().name, "Model 63");
    }

    #[test]
    fn power_supply_provider_reports_unavailable_and_filters_hostile_state() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        let power = roots.sys.join("class/power_supply");

        let battery = power.join("BAT0");
        put(&battery.join("type"), "Battery\n");
        put(&battery.join("model_name"), "Honest Battery\n");
        put(&battery.join("manufacturer"), "ACME\n");
        put(&battery.join("capacity"), "73\n");
        put(&battery.join("status"), "Discharging\n");

        let unavailable = power.join("BAT1");
        put(&unavailable.join("type"), "Battery\n");
        put(&unavailable.join("capacity"), "101\n");
        put(&unavailable.join("status"), "credential=do-not-publish\n");
        put(
            &unavailable.join("manufacturer"),
            &format!("{}\n", "x".repeat(MAX_POWER_IDENTITY_BYTES + 1)),
        );

        let records = power_supplies(&roots);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].status, DeviceStatus::Ok);
        assert_eq!(
            records[0].events,
            ["type: Battery", "capacity: 73%", "status: Discharging"]
        );
        assert_eq!(records[1].status, DeviceStatus::Unknown);
        assert_eq!(
            records[1].problem.as_deref(),
            Some("power supply state unavailable")
        );
        let published = serde_json::to_string(&records[1]).unwrap();
        assert!(!published.contains("credential"));
        assert!(!published.contains(&"x".repeat(MAX_POWER_IDENTITY_BYTES + 1)));
        assert!(!published.contains("101%"));
    }

    #[test]
    fn physical_block_provider_is_bounded_after_virtual_device_filtering() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        let block = roots.sys.join("block");

        // These sort before the physical rows but must not consume the physical
        // provider's budget.
        for index in 0..(MAX_BLOCK_DEVICES + 32) {
            fs::create_dir_all(block.join(format!("loop{index:04}"))).unwrap();
        }
        for index in 0..(MAX_BLOCK_DEVICES + 8) {
            let disk = block.join(format!("nvme{index:04}"));
            put(
                &disk.join("device").join("model"),
                &format!("Disk {index:04}\n"),
            );
            put(&disk.join("size"), "2048\n");
            put(&disk.join("dev"), &format!("259:{index}\n"));
        }

        let records = block_devices(&roots);
        assert_eq!(records.len(), MAX_BLOCK_DEVICES);
        assert_eq!(records.first().unwrap().model.as_deref(), Some("Disk 0000"));
        assert_eq!(records.last().unwrap().model.as_deref(), Some("Disk 0255"));
        assert!(records.iter().all(|record| {
            record
                .sysfs_path
                .as_deref()
                .is_some_and(|path| !path.contains("loop"))
        }));
    }

    #[test]
    fn physical_block_provider_reports_kernel_readiness_and_refuses_invented_health() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        let block = roots.sys.join("block");

        let healthy = block.join("nvme0n1");
        put(&healthy.join("size"), "2048\n");
        put(&healthy.join("dev"), "259:0\n");
        put(&healthy.join("device/state"), "live\n");
        put(&healthy.join("ro"), "0\n");

        let blocked = block.join("sda");
        put(&blocked.join("size"), "4096\n");
        put(&blocked.join("dev"), "8:0\n");
        put(&blocked.join("device/state"), "blocked\n");
        put(&blocked.join("ro"), "1\n");

        let unresolved = block.join("sdb");
        put(&unresolved.join("size"), "0\n");
        put(&unresolved.join("dev"), "credential-shaped-device-number\n");
        put(
            &unresolved.join("device/state"),
            "credential-shaped-state\n",
        );

        let records = block_devices(&roots);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].status, DeviceStatus::Ok);
        assert_eq!(
            records[0].events,
            [
                "kernel state: live",
                "device number: 259:0",
                "read-only: false"
            ]
        );
        assert_eq!(records[1].status, DeviceStatus::Degraded);
        assert_eq!(
            records[1].problem.as_deref(),
            Some("kernel block state: blocked")
        );
        assert!(records[1]
            .events
            .iter()
            .any(|event| event == "read-only: true"));
        assert_eq!(records[2].status, DeviceStatus::Unknown);
        assert_eq!(
            records[2].problem.as_deref(),
            Some("block device readiness unavailable")
        );
        let published = serde_json::to_string(&records).unwrap();
        assert!(!published.contains("credential-shaped"));
    }

    #[test]
    fn physical_network_interfaces_are_bounded_sourced_and_credential_free() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        let net = roots.sys.join("class/net");
        let wired = net.join("eno1");
        put(&wired.join("operstate"), "down\n");
        put(&wired.join("carrier"), "0\n");
        put(
            &wired.join("address"),
            "credential-like-address-must-not-publish\n",
        );
        fs::create_dir_all(wired.join("device")).unwrap();
        let wifi = net.join("wlan0");
        put(&wifi.join("operstate"), "up\n");
        put(&wifi.join("carrier"), "1\n");
        put(
            &wifi.join("ssid"),
            "credential-like-ssid-must-not-publish\n",
        );
        fs::create_dir_all(wifi.join("device")).unwrap();
        fs::create_dir_all(wifi.join("wireless")).unwrap();
        put(&net.join("lo/operstate"), "unknown\n");
        put(&net.join("veth0/operstate"), "up\n");

        let records = network_interfaces(&roots);
        assert_eq!(records.len(), 2);
        let eno1 = records
            .iter()
            .find(|record| record.name.contains("eno1"))
            .unwrap();
        assert_eq!(eno1.status, DeviceStatus::Unknown);
        assert_eq!(eno1.problem.as_deref(), Some("link state: down"));
        assert_eq!(
            Path::new(eno1.sysfs_path.as_deref().unwrap()),
            wired.as_path()
        );
        let wlan0 = records
            .iter()
            .find(|record| record.name.contains("wlan0"))
            .unwrap();
        assert_eq!(wlan0.status, DeviceStatus::Ok);
        assert!(wlan0.events.iter().any(|event| event == "kind: wireless"));

        let published = serde_json::to_string(&records).unwrap();
        assert!(!published.contains("credential-like"));
        assert!(!published.contains("veth0"));
        assert!(!published.contains("\"lo\""));
    }

    #[test]
    fn physical_display_connectors_are_bounded_sourced_and_credential_free() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        let drm = roots.sys.join("class/drm");
        put(&drm.join("card0-eDP-1/status"), "connected\n");
        put(&drm.join("card0-eDP-1/enabled"), "enabled\n");
        put(
            &drm.join("card0-eDP-1/modes"),
            "1920x1080\n1280x720\ninvalid\n99999x1\n",
        );
        put(
            &drm.join("card0-eDP-1/edid"),
            "credential-like-display-identity-must-not-publish\n",
        );
        put(&drm.join("card0-HDMI-A-1/status"), "disconnected\n");
        put(&drm.join("card0-HDMI-A-1/enabled"), "disabled\n");
        put(&drm.join("card0/status"), "connected\n");
        put(&drm.join("renderD128/status"), "connected\n");

        let records = display_connectors(&roots);
        assert_eq!(records.len(), 2);
        let embedded = records
            .iter()
            .find(|record| record.name.contains("eDP-1"))
            .unwrap();
        assert_eq!(embedded.status, DeviceStatus::Ok);
        assert_eq!(
            embedded.events,
            [
                "connection: connected",
                "enabled: enabled",
                "modes: 1920x1080, 1280x720",
            ]
        );
        let hdmi = records
            .iter()
            .find(|record| record.name.contains("HDMI-A-1"))
            .unwrap();
        assert_eq!(hdmi.status, DeviceStatus::Unknown);
        assert_eq!(hdmi.problem, None);

        let published = serde_json::to_string(&records).unwrap();
        assert!(!published.contains("credential-like"));
        assert!(!published.contains("invalid"));
        assert!(!published.contains("99999x1"));
        assert!(!published.contains("renderD128"));
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
    }

    #[cfg(unix)]
    #[test]
    fn conflicting_sysfs_sources_suppress_only_the_equivocated_hardware_identity() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        let physical = roots.sys.join("devices/pci0000:00/0000:00:1f.6");
        put(&physical.join("vendor"), "0x8086\n");
        put(&physical.join("device"), "0x15d8\n");
        put(&physical.join("class"), "0x020000\n");
        put(&physical.join("operstate"), "up\n");
        put(&physical.join("carrier"), "1\n");

        let pci_view = roots.sys.join("bus/pci/devices/0000:00:1f.6");
        let net_view = roots.sys.join("class/net/eno1");
        fs::create_dir_all(pci_view.parent().unwrap()).unwrap();
        fs::create_dir_all(net_view.parent().unwrap()).unwrap();
        symlink(&physical, &pci_view).unwrap();
        symlink(&physical, &net_view).unwrap();

        // A distinct interface must survive even though the aliased physical
        // object above produces incompatible PCI and class-net bodies.
        let unrelated = roots.sys.join("class/net/eno2");
        put(&unrelated.join("operstate"), "up\n");
        put(&unrelated.join("carrier"), "1\n");
        fs::create_dir_all(unrelated.join("device")).unwrap();

        let inv = enumerate(
            &roots,
            &IdsDb::default(),
            ToolAvailability::default(),
            "test-box",
            &[],
        );
        let network = inv
            .categories
            .iter()
            .find(|candidate| candidate.key == category::NETWORK_ADAPTERS)
            .expect("unrelated network hardware remains published");
        assert_eq!(network.devices.len(), 1);
        assert!(network.devices[0].name.contains("eno2"));
        let published = serde_json::to_string(&inv).unwrap();
        assert!(!published.contains("eno1"));
        assert!(!published.contains("0000:00:1f.6"));

        // The other side of the admission rule is equally important: two
        // independently enumerated paths carrying the exact same declaration
        // for one kernel object collapse instead of suppressing that object.
        let exact_physical = roots.sys.join("devices/platform/exact0");
        fs::create_dir_all(&exact_physical).unwrap();
        let exact_alias_a = roots.sys.join("fixture-aliases/exact-a");
        let exact_alias_b = roots.sys.join("fixture-aliases/exact-b");
        fs::create_dir_all(exact_alias_a.parent().unwrap()).unwrap();
        symlink(&exact_physical, &exact_alias_a).unwrap();
        symlink(&exact_physical, &exact_alias_b).unwrap();

        let exact_record = DeviceRecord {
            sysfs_path: Some(exact_alias_a.to_string_lossy().into_owned()),
            ..DeviceRecord::new("Exact device", DeviceStatus::Ok)
        };
        let mut exact_alias_record = exact_record.clone();
        exact_alias_record.sysfs_path = Some(exact_alias_b.to_string_lossy().into_owned());
        let mut exact_buckets = BTreeMap::from([(
            category::INPUT.to_string(),
            vec![exact_record, exact_alias_record],
        )]);

        suppress_conflicting_sysfs_identities(&mut exact_buckets);
        assert_eq!(
            exact_buckets[category::INPUT].len(),
            1,
            "exact aliases must deduplicate without suppressing their identity"
        );
    }

    #[test]
    fn hot_unplugged_sysfs_identity_is_revoked_before_publication() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = SysfsRoots::under(tmp.path());
        let removed = roots.sys.join("class/input/input0");
        let present = roots.sys.join("class/input/input1");
        put(&removed.join("name"), "Removed keyboard\n");
        put(&present.join("name"), "Present keyboard\n");

        let records = input_devices(&roots);
        assert_eq!(
            records.len(),
            2,
            "both providers existed during enumeration"
        );

        // Model a physical unplug after provider attributes were captured but
        // before the inventory generation is reconciled for publication.
        fs::remove_dir_all(&removed).unwrap();
        let mut buckets = BTreeMap::from([(category::INPUT.to_string(), records)]);
        suppress_conflicting_sysfs_identities(&mut buckets);

        let admitted = &buckets[category::INPUT];
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].name, "Present keyboard");
        assert_eq!(
            admitted[0].sysfs_path.as_deref(),
            Some(present.to_string_lossy().as_ref())
        );
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
    fn delayed_pre_restart_inventory_cannot_replace_a_newer_hardware_generation() {
        let store = tempfile::tempdir().unwrap();
        let mut current = DeviceInventory::fixture();
        current.host = "restart-host".into();
        current.published_at_ms = 2;
        write_inventory(store.path(), &current).unwrap();

        // This census began before restart and observed an empty/healthy-looking
        // provider tree, but did not finish until the replacement worker had
        // already committed generation 2.
        let mut delayed = current.clone();
        delayed.published_at_ms = 1;
        delayed.categories.clear();
        let error = write_inventory(store.path(), &delayed)
            .expect_err("a delayed census must lose generation arbitration");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(
            mackes_mesh_types::device_inventory::read_inventory(
                store.path(),
                "restart-host"
            ),
            Some(current)
        );
    }

    #[cfg(unix)]
    #[test]
    fn inventory_publish_cannot_follow_a_substituted_staging_row() {
        use std::os::unix::fs::symlink;

        let store = tempfile::tempdir().unwrap();
        let dir = mackes_mesh_types::device_inventory::inventory_dir(store.path());
        fs::create_dir_all(&dir).unwrap();
        let outside = store.path().join("outside-authority");
        fs::write(&outside, "operator-owned\n").unwrap();
        symlink(&outside, dir.join(".host-a.json.tmp")).unwrap();

        let mut inv = DeviceInventory::fixture();
        inv.host = "host-a".into();
        let error = write_inventory(store.path(), &inv)
            .expect_err("a substituted staging row must fail closed");

        assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(fs::read_to_string(&outside).unwrap(), "operator-owned\n");
        assert!(!mackes_mesh_types::device_inventory::inventory_path(store.path(), "host-a")
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

    fn display_connector(
        identity: &str,
        card: u16,
        status: DrmConnectorStatus,
    ) -> DrmConnectorFact {
        let connected = status == DrmConnectorStatus::Connected;
        DrmConnectorFact {
            identity: identity.into(),
            card,
            status,
            enabled: connected,
            has_mode: connected,
        }
    }

    #[test]
    fn hostile_display_provider_facts_fail_unknown_without_identity_leakage() {
        let active = Some(DisplaySeat::Active(4242));
        let owned = Some(BTreeMap::from([(0, BTreeSet::from([4242]))]));
        let cases = [
            classify_display(
                vec![
                    display_connector("card0-DP-1", 0, DrmConnectorStatus::Connected),
                    display_connector("card0-DP-1", 0, DrmConnectorStatus::Connected),
                ],
                active,
                owned.clone(),
            ),
            classify_display(
                vec![DrmConnectorFact {
                    enabled: true,
                    ..display_connector("card0-DP-1", 0, DrmConnectorStatus::Disconnected)
                }],
                active,
                owned.clone(),
            ),
            classify_display(
                vec![display_connector("card0-DP-1", 0, DrmConnectorStatus::Connected)],
                active,
                Some(BTreeMap::from([(0, BTreeSet::from([9999]))])),
            ),
            classify_display(
                vec![display_connector("card0-DP-1", 0, DrmConnectorStatus::Connected)],
                active,
                Some(BTreeMap::from([(1, BTreeSet::from([4242]))])),
            ),
            classify_display(
                vec![display_connector("card0-DP-1", 0, DrmConnectorStatus::Unknown)],
                active,
                owned,
            ),
        ];
        for (readiness, count, connected, reason) in cases {
            assert_eq!(readiness, DisplayReadiness::Unknown);
            assert!(!reason.contains("DP-1"));
            assert!(count <= MAX_DISPLAY_CONNECTORS);
            assert!(connected <= MAX_DISPLAY_CONNECTORS);
        }
        assert!(parse_display_seat("LoadState=loaded\nActiveState=active\nActiveState=active\nSubState=running\nMainPID=4242\n").is_none());
        assert!(parse_drm_masters("command pid dev master\nshell 4242 0 maybe\n").is_none());
    }

    #[test]
    fn coherent_display_provider_states_remain_distinct() {
        let ready = classify_display(
            vec![display_connector("card0-DP-1", 0, DrmConnectorStatus::Connected)],
            Some(DisplaySeat::Active(42)),
            Some(BTreeMap::from([(0, BTreeSet::from([42]))])),
        );
        assert_eq!(ready.0, DisplayReadiness::Ready);
        let disconnected = classify_display(
            vec![display_connector("card0-DP-1", 0, DrmConnectorStatus::Disconnected)],
            Some(DisplaySeat::Active(42)),
            Some(BTreeMap::from([(0, BTreeSet::new())])),
        );
        assert_eq!(disconnected.0, DisplayReadiness::Disconnected);
        let disabled = classify_display(
            vec![display_connector("card0-DP-1", 0, DrmConnectorStatus::Disconnected)],
            Some(DisplaySeat::Inactive),
            Some(BTreeMap::from([(0, BTreeSet::new())])),
        );
        assert_eq!(disabled.0, DisplayReadiness::Disabled);
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
