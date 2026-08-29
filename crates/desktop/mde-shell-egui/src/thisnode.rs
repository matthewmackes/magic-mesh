//! Workbench · This Node — live local-node status (WB-ThisNode).
//!
//! The first Workbench plane, wired off the SAME world-readable mesh-status
//! snapshot the chrome bar folds (`/run/mde/mesh-status.json`, written every ~30s
//! by the root `mesh-status.timer`). The desktop user can't read the root-only
//! replicated peer directory, so this JSON is the desktop tier's read path — the
//! shell leans on no `mackesd` IPC (§6). Every field here is real, live-updating
//! node reality; nothing is a stand-in (§7):
//!
//! * **Identity** — this node's hostname (the snapshot's own `self` marker), its
//!   pinned `role`, its Nebula `overlay_ip`, and the tunnel `cipher`.
//! * **Presence + heartbeat** — the node's directory `presence` tier
//!   (online/idle/offline) and the freshness of its last heartbeat, measured
//!   against the snapshot's own `generated_ms` clock (no desktop-clock skew).
//! * **Version** — the installed `mde-core` version and whether a newer one is
//!   live on the mesh (the snapshot's fleet-wide `latest_version` fold).
//! * **Node services** — this node's own daemon health (mackesd / Nebula /
//!   Syncthing / Bus / DNS / Voice / Music / KDE-Connect / Workbench), the
//!   `services` map each node publishes into its `shell-status.json`.
//! * **Mesh context** — the live peer count (online / total) and the elected mesh
//!   leader.
//!
//! Resource telemetry is intentionally aggregate and bounded: the panel can
//! show a short live history of CPU load, memory, and root-storage usage while
//! keeping process names, mount paths, and device identity out of the shell.
//!
//! `project` is pure (no IO, no egui, no GPU), so it's unit-tested directly; the
//! only IO is the snapshot read in [`ThisNodeState::poll`].

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::thread;
use std::time::SystemTime;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::{DenseList, Style};

use crate::this_node_catalog::{page_index, PageEntry, Section, SectionGroup};

use serde_json::Value;

/// The world-readable mesh-status snapshot — the same source the chrome bar reads
/// (the desktop user can't read the root-only replicated peer directory).
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// Poll cadence — a heartbeat, a service flip, or a role change surfaces within
/// this window. Matches the chrome bar + the Fleet datacenter poll; the read is a
/// cheap local file scan, so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

/// The snapshot writer normally runs every ~30 seconds. Treat a readable but
/// older snapshot as degraded too: a provider that stopped updating must not
/// look current merely because the last file remains parseable.
const MAX_SNAPSHOT_AGE_MS: u64 = 90_000;

/// Keep the world-readable mesh snapshot bounded before `serde_json` walks its
/// peer directory and service maps. The writer is local, but the desktop tier
/// treats this filesystem boundary as hostile and fails soft.
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024;

/// Keep the chart useful at the five-second poll cadence without turning the
/// This Node state into an unbounded telemetry store.
const MAX_TELEMETRY_SAMPLES: usize = 60;

const SERVICES_REFRESH: Duration = Duration::from_secs(15);
const MAX_FAILED_SERVICES: usize = 32;
const MAX_SERVICE_NAME_CHARS: usize = 128;
const MAX_LOCAL_PRINTERS: usize = 16;
const SECURITY_REFRESH: Duration = Duration::from_secs(15);
const LOCAL_PROVIDER_MAX_AGE: Duration = Duration::from_secs(45);
const MAX_DEVICE_MAPPINGS: usize = 32;
const MAX_BACKUP_BUNDLE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_APP_MIRROR_BYTES: usize = 256 * 1024;
const MAX_APP_ROWS: usize = 256;
const MAX_VENDOR_PACKS: usize = 8;
const MAX_VENDOR_CAPABILITIES: usize = 8;
const MAX_VENDOR_TEXT_CHARS: usize = 96;

#[derive(Default)]
struct LocalFreshness {
    last_success: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LocalFreshnessSummary {
    fresh: u8,
    stale: u8,
    awaiting: u8,
}

impl LocalFreshnessSummary {
    fn add(&mut self, freshness: &LocalFreshness) {
        match freshness.last_success {
            Some(last) if last.elapsed() <= LOCAL_PROVIDER_MAX_AGE => self.fresh += 1,
            Some(_) => self.stale += 1,
            None => self.awaiting += 1,
        }
    }
}

struct LocalServiceProvider {
    failed: Vec<String>,
    state: Result<(), &'static str>,
    freshness: LocalFreshness,
    last_poll: Option<Instant>,
    receiver: Option<Receiver<Result<Vec<String>, &'static str>>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LocalLocaleFacts {
    language: Option<String>,
    locale: Option<String>,
    timezone: Option<String>,
    keyboard_region: Option<String>,
    time_synchronized: Option<bool>,
}

struct LocalLocaleProvider {
    facts: LocalLocaleFacts,
    state: Result<(), &'static str>,
    freshness: LocalFreshness,
    last_poll: Option<Instant>,
    receiver: Option<Receiver<Result<LocalLocaleFacts, &'static str>>>,
}

impl Default for LocalLocaleProvider {
    fn default() -> Self {
        Self {
            facts: LocalLocaleFacts::default(),
            state: Err("The local locale provider has not returned yet."),
            freshness: LocalFreshness::default(),
            last_poll: None,
            receiver: None,
        }
    }
}

impl LocalLocaleProvider {
    fn poll(&mut self) {
        if let Some(receiver) = self.receiver.take() {
            match receiver.try_recv() {
                Ok(Ok(facts)) => {
                    self.facts = facts;
                    self.state = Ok(());
                    self.freshness.last_success = Some(Instant::now());
                }
                Ok(Err(error)) => {
                    self.facts = LocalLocaleFacts::default();
                    self.state = Err(error);
                }
                Err(TryRecvError::Empty) => self.receiver = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    self.facts = LocalLocaleFacts::default();
                    self.state =
                        Err("The local locale provider stopped before returning evidence.");
                }
            }
        }
        if self.receiver.is_none()
            && self
                .last_poll
                .is_none_or(|last| last.elapsed() >= SERVICES_REFRESH)
        {
            self.last_poll = Some(Instant::now());
            let (sender, receiver) = channel();
            self.receiver = Some(receiver);
            thread::spawn(move || {
                let _ = sender.send(read_local_locale());
            });
        }
    }
}

fn bounded_locale_value(value: &str) -> Option<String> {
    let value = value.trim().trim_matches(['"', '\'']);
    if value.is_empty()
        || value.len() > 96
        || value.chars().any(char::is_control)
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'@' | b'/' | b':')
        })
    {
        return None;
    }
    Some(value.to_owned())
}

fn read_local_locale() -> Result<LocalLocaleFacts, &'static str> {
    let mut language = None;
    let mut locale = None;
    let mut keyboard_region = None;
    for path in [
        "/etc/locale.conf",
        "/etc/default/locale",
        "/etc/default/keyboard",
        "/etc/vconsole.conf",
    ] {
        let Ok(contents) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = bounded_locale_value(value);
            match key.trim() {
                "LANGUAGE" if language.is_none() => language = value,
                "LANG" | "LC_ALL" if locale.is_none() => locale = value,
                "XKBLAYOUT" | "KEYMAP" if keyboard_region.is_none() => keyboard_region = value,
                _ => {}
            }
        }
    }
    let timezone = std::fs::read_to_string("/etc/timezone")
        .ok()
        .and_then(|value| bounded_locale_value(&value))
        .or_else(|| {
            std::fs::read_link("/etc/localtime")
                .ok()
                .and_then(|path| path.to_str().map(str::to_owned))
                .and_then(|path| path.strip_prefix("/usr/share/zoneinfo/").map(str::to_owned))
                .and_then(|value| bounded_locale_value(&value))
        });
    let time_synchronized = Command::new("timedatectl")
        .args(["show", "--property=NTPSynchronized", "--value"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(
            |output| match String::from_utf8_lossy(&output.stdout).trim() {
                "yes" => Some(true),
                "no" => Some(false),
                _ => None,
            },
        );
    if language.is_none()
        && locale.is_none()
        && timezone.is_none()
        && keyboard_region.is_none()
        && time_synchronized.is_none()
    {
        return Err("The fixed local locale and time-zone providers returned no usable facts.");
    }
    Ok(LocalLocaleFacts {
        language,
        locale,
        timezone,
        keyboard_region,
        time_synchronized,
    })
}

impl Default for LocalServiceProvider {
    fn default() -> Self {
        Self {
            failed: Vec::new(),
            state: Err("The local service provider has not returned yet."),
            freshness: LocalFreshness::default(),
            last_poll: None,
            receiver: None,
        }
    }
}

impl LocalServiceProvider {
    fn poll(&mut self) {
        if let Some(receiver) = self.receiver.take() {
            match receiver.try_recv() {
                Ok(Ok(failed)) => {
                    self.failed = failed;
                    self.state = Ok(());
                    self.freshness.last_success = Some(Instant::now());
                }
                Ok(Err(error)) => {
                    self.failed.clear();
                    self.state = Err(error);
                }
                Err(TryRecvError::Empty) => self.receiver = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    self.failed.clear();
                    self.state =
                        Err("The local service provider stopped before returning evidence.");
                }
            }
        }
        if self.receiver.is_none()
            && self
                .last_poll
                .is_none_or(|last| last.elapsed() >= SERVICES_REFRESH)
        {
            self.last_poll = Some(Instant::now());
            let (sender, receiver) = channel();
            self.receiver = Some(receiver);
            thread::spawn(move || {
                let _ = sender.send(read_failed_services());
            });
        }
    }
}

fn read_failed_services() -> Result<Vec<String>, &'static str> {
    let output = Command::new("systemctl")
        .args([
            "--failed",
            "--plain",
            "--no-legend",
            "--no-pager",
            "--type=service",
        ])
        .output()
        .map_err(|_| "The local systemd service provider is unavailable.")?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err("The local systemd service provider refused the fixed read-only query.");
    }
    let mut failed = Vec::new();
    for raw in String::from_utf8_lossy(&output.stdout).lines() {
        let name = raw.split_whitespace().next().unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let name: String = name.chars().take(MAX_SERVICE_NAME_CHARS).collect();
        failed.push(name);
        if failed.len() == MAX_FAILED_SERVICES {
            break;
        }
    }
    Ok(failed)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LocalPrinter {
    name: String,
    state: String,
}

struct LocalPrinterProvider {
    printers: Vec<LocalPrinter>,
    default: Option<String>,
    state: Result<(), &'static str>,
    freshness: LocalFreshness,
    last_poll: Option<Instant>,
    receiver: Option<Receiver<Result<(Vec<LocalPrinter>, Option<String>), &'static str>>>,
}

impl Default for LocalPrinterProvider {
    fn default() -> Self {
        Self {
            printers: Vec::new(),
            default: None,
            state: Err("The local printer provider has not returned yet."),
            freshness: LocalFreshness::default(),
            last_poll: None,
            receiver: None,
        }
    }
}

impl LocalPrinterProvider {
    fn poll(&mut self) {
        if let Some(receiver) = self.receiver.take() {
            match receiver.try_recv() {
                Ok(Ok((printers, default))) => {
                    self.printers = printers;
                    self.default = default;
                    self.state = Ok(());
                    self.freshness.last_success = Some(Instant::now());
                }
                Ok(Err(error)) => {
                    self.printers.clear();
                    self.default = None;
                    self.state = Err(error);
                }
                Err(TryRecvError::Empty) => self.receiver = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    self.printers.clear();
                    self.default = None;
                    self.state =
                        Err("The local printer provider stopped before returning evidence.");
                }
            }
        }
        if self.receiver.is_none()
            && self
                .last_poll
                .is_none_or(|last| last.elapsed() >= SERVICES_REFRESH)
        {
            self.last_poll = Some(Instant::now());
            let (sender, receiver) = channel();
            self.receiver = Some(receiver);
            thread::spawn(move || {
                let _ = sender.send(read_local_printers());
            });
        }
    }
}

fn read_local_printers() -> Result<(Vec<LocalPrinter>, Option<String>), &'static str> {
    let output = Command::new("lpstat")
        .args(["-p", "-d"])
        .output()
        .map_err(|_| "The local CUPS printer provider is unavailable.")?;
    if !output.status.success() {
        return Err("The local CUPS printer provider refused the fixed read-only query.");
    }
    Ok(parse_local_printers(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn parse_local_printers(output: &str) -> (Vec<LocalPrinter>, Option<String>) {
    let mut printers = Vec::new();
    let mut default = None;
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.first() == Some(&"printer") && fields.len() >= 2 {
            let state = line
                .split_once(" is ")
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .unwrap_or("unknown");
            printers.push(LocalPrinter {
                name: fields[1].chars().take(MAX_SERVICE_NAME_CHARS).collect(),
                state: state.chars().take(32).collect(),
            });
            if printers.len() == MAX_LOCAL_PRINTERS {
                break;
            }
        } else if fields.first() == Some(&"system")
            && fields.get(1) == Some(&"default")
            && fields.get(2) == Some(&"destination:")
        {
            default = fields
                .get(3)
                .map(|name| name.chars().take(MAX_SERVICE_NAME_CHARS).collect());
        }
    }
    (printers, default)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FirewallState {
    Running,
    NotRunning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EncryptionState {
    Observed { mappings: usize, encrypted: usize },
    NoMappings,
}

struct LocalSecurityProvider {
    firewall: Result<FirewallState, &'static str>,
    encryption: Result<EncryptionState, &'static str>,
    last_poll: Option<Instant>,
    encryption_last_poll: Option<Instant>,
    firewall_last_success: Option<Instant>,
    encryption_last_success: Option<Instant>,
    receiver: Option<Receiver<Result<FirewallState, &'static str>>>,
    encryption_receiver: Option<Receiver<Result<EncryptionState, &'static str>>>,
}

impl Default for LocalSecurityProvider {
    fn default() -> Self {
        Self {
            firewall: Err("The local firewalld provider has not returned yet."),
            encryption: Err("The local encryption provider has not returned yet."),
            last_poll: None,
            encryption_last_poll: None,
            firewall_last_success: None,
            encryption_last_success: None,
            receiver: None,
            encryption_receiver: None,
        }
    }
}

impl LocalSecurityProvider {
    fn poll(&mut self) {
        if let Some(receiver) = self.receiver.take() {
            match receiver.try_recv() {
                Ok(result) => {
                    if result.is_ok() {
                        self.firewall_last_success = Some(Instant::now());
                    }
                    self.firewall = result;
                }
                Err(TryRecvError::Empty) => self.receiver = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    self.firewall =
                        Err("The local firewalld provider stopped before returning evidence.");
                }
            }
        }
        if let Some(receiver) = self.encryption_receiver.take() {
            match receiver.try_recv() {
                Ok(result) => {
                    if result.is_ok() {
                        self.encryption_last_success = Some(Instant::now());
                    }
                    self.encryption = result;
                }
                Err(TryRecvError::Empty) => self.encryption_receiver = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    self.encryption =
                        Err("The local encryption provider stopped before returning evidence.");
                }
            }
        }
        if self.receiver.is_none()
            && self
                .last_poll
                .is_none_or(|last| last.elapsed() >= SECURITY_REFRESH)
        {
            self.last_poll = Some(Instant::now());
            let (sender, receiver) = channel();
            self.receiver = Some(receiver);
            thread::spawn(move || {
                let _ = sender.send(read_firewall_state());
            });
        }
        if self.encryption_receiver.is_none()
            && self
                .encryption_last_poll
                .is_none_or(|last| last.elapsed() >= SECURITY_REFRESH)
        {
            self.encryption_last_poll = Some(Instant::now());
            let (sender, receiver) = channel();
            self.encryption_receiver = Some(receiver);
            thread::spawn(move || {
                let _ = sender.send(read_encryption_state(Path::new("/sys/class/block")));
            });
        }
    }
}

fn read_firewall_state() -> Result<FirewallState, &'static str> {
    let output = Command::new("firewall-cmd")
        .args(["--state"])
        .output()
        .map_err(|_| "The local firewalld provider is unavailable.")?;
    parse_firewall_state(String::from_utf8_lossy(&output.stdout).trim())
}

fn parse_firewall_state(state: &str) -> Result<FirewallState, &'static str> {
    if state.eq_ignore_ascii_case("running") {
        Ok(FirewallState::Running)
    } else if state.eq_ignore_ascii_case("not running") {
        Ok(FirewallState::NotRunning)
    } else {
        Err("The local firewalld provider returned an unknown state.")
    }
}

fn show_local_freshness(ui: &mut egui::Ui, last_success: Option<Instant>) {
    match last_success {
        Some(last) if last.elapsed() <= LOCAL_PROVIDER_MAX_AGE => {
            ui.colored_label(
                Style::OK,
                format!("Fresh • {}s ago", last.elapsed().as_secs()),
            );
        }
        Some(_) => {
            ui.colored_label(Style::WARN, "Stale • refresh local provider");
        }
        None => {
            ui.colored_label(Style::TEXT_DIM, "Awaiting local provider");
        }
    }
}

fn show_local_timestamp_freshness(ui: &mut egui::Ui, observed_at_ms: u64) {
    if observed_at_ms == 0 {
        ui.colored_label(Style::TEXT_DIM, "Awaiting provider timestamp");
        return;
    }
    let age_ms = unix_epoch_ms().saturating_sub(observed_at_ms);
    if age_ms <= LOCAL_PROVIDER_MAX_AGE.as_millis() as u64 {
        ui.colored_label(Style::OK, format!("Fresh • {}s ago", age_ms / 1000));
    } else {
        ui.colored_label(Style::WARN, "Stale • refresh local hardware provider");
    }
}

fn read_encryption_state(root: &Path) -> Result<EncryptionState, &'static str> {
    let entries =
        std::fs::read_dir(root).map_err(|_| "The local encryption provider is unavailable.")?;
    let mut mappings = 0;
    let mut encrypted = 0;
    for entry in entries {
        let entry =
            entry.map_err(|_| "The local encryption provider could not inspect mappings.")?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("dm-") {
            continue;
        }
        mappings += 1;
        if mappings > MAX_DEVICE_MAPPINGS {
            return Err("The local encryption provider exceeded its mapping bound.");
        }
        let uuid = entry.path().join("dm/uuid");
        let uuid = std::fs::read_to_string(uuid)
            .map_err(|_| "The local encryption provider could not read mapping state.")?;
        if uuid.trim_start().starts_with("CRYPT-LUKS") {
            encrypted += 1;
        }
    }
    if mappings == 0 {
        Ok(EncryptionState::NoMappings)
    } else {
        Ok(EncryptionState::Observed {
            mappings,
            encrypted,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackupState {
    Present { bytes: u64, modified_ms: u64 },
    Missing,
}

struct LocalBackupProvider {
    path: PathBuf,
    state: Result<BackupState, &'static str>,
    freshness: LocalFreshness,
    last_poll: Option<Instant>,
    receiver: Option<Receiver<Result<BackupState, &'static str>>>,
}

impl LocalBackupProvider {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            state: Err("The local backup provider has not returned yet."),
            freshness: LocalFreshness::default(),
            last_poll: None,
            receiver: None,
        }
    }

    fn poll(&mut self) {
        if let Some(receiver) = self.receiver.take() {
            match receiver.try_recv() {
                Ok(result) => {
                    if result.is_ok() {
                        self.freshness.last_success = Some(Instant::now());
                    }
                    self.state = result;
                }
                Err(TryRecvError::Empty) => self.receiver = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    self.state =
                        Err("The local backup provider stopped before returning evidence.");
                }
            }
        }
        if self.receiver.is_none()
            && self
                .last_poll
                .is_none_or(|last| last.elapsed() >= SECURITY_REFRESH)
        {
            self.last_poll = Some(Instant::now());
            let path = self.path.clone();
            let (sender, receiver) = channel();
            self.receiver = Some(receiver);
            thread::spawn(move || {
                let _ = sender.send(read_backup_metadata(&path));
            });
        }
    }
}

fn read_backup_metadata(path: &Path) -> Result<BackupState, &'static str> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BackupState::Missing)
        }
        Err(_) => return Err("The local backup artifact could not be inspected."),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("The local backup artifact is not a regular file.");
    }
    if metadata.len() == 0 || metadata.len() > MAX_BACKUP_BUNDLE_BYTES {
        return Err("The local backup artifact has an invalid bounded size.");
    }
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .ok_or("The local backup artifact has no usable modification time.")?;
    Ok(BackupState::Present {
        bytes: metadata.len(),
        modified_ms,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ApplicationFacts {
    installed: Option<usize>,
    running: Option<usize>,
}

struct LocalApplicationProvider {
    installed_path: PathBuf,
    running_path: PathBuf,
    facts: Result<ApplicationFacts, &'static str>,
    freshness: LocalFreshness,
    last_poll: Option<Instant>,
    receiver: Option<Receiver<Result<ApplicationFacts, &'static str>>>,
}

impl LocalApplicationProvider {
    fn new(installed_path: PathBuf, running_path: PathBuf) -> Self {
        Self {
            installed_path,
            running_path,
            facts: Err("The local application mirror has not returned yet."),
            freshness: LocalFreshness::default(),
            last_poll: None,
            receiver: None,
        }
    }

    fn poll(&mut self) {
        if let Some(receiver) = self.receiver.take() {
            match receiver.try_recv() {
                Ok(result) => {
                    if result.is_ok() {
                        self.freshness.last_success = Some(Instant::now());
                    }
                    self.facts = result;
                }
                Err(TryRecvError::Empty) => self.receiver = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    self.facts =
                        Err("The local application mirror stopped before returning evidence.");
                }
            }
        }
        if self.receiver.is_none()
            && self
                .last_poll
                .is_none_or(|last| last.elapsed() >= SERVICES_REFRESH)
        {
            self.last_poll = Some(Instant::now());
            let installed = self.installed_path.clone();
            let running = self.running_path.clone();
            let (sender, receiver) = channel();
            self.receiver = Some(receiver);
            thread::spawn(move || {
                let _ = sender.send(read_application_facts(&installed, &running));
            });
        }
    }
}

fn read_application_facts(
    installed_path: &Path,
    running_path: &Path,
) -> Result<ApplicationFacts, &'static str> {
    Ok(ApplicationFacts {
        installed: read_app_array_count(installed_path, "entries")?,
        running: read_app_array_count(running_path, "ids")?,
    })
}

fn read_app_array_count(path: &Path, key: &str) -> Result<Option<usize>, &'static str> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("The local application mirror could not be inspected."),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("The local application mirror is not a regular file.");
    }
    if metadata.len() > MAX_APP_MIRROR_BYTES as u64 {
        return Err("The local application mirror exceeds its bounded size.");
    }
    let body =
        std::fs::read(path).map_err(|_| "The local application mirror could not be read.")?;
    if body.len() > MAX_APP_MIRROR_BYTES {
        return Err("The local application mirror grew beyond its bounded size.");
    }
    let value: Value =
        serde_json::from_slice(&body).map_err(|_| "The local application mirror is malformed.")?;
    let count = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or("The local application mirror omitted its bounded collection.")?
        .len();
    (count <= MAX_APP_ROWS)
        .then_some(Some(count))
        .ok_or("The local application mirror exceeded its row bound.")
}

/// Read one mesh-status snapshot through the descriptor that is consumed.
/// Reject the final symlink, special descriptors, oversized input, and files
/// whose size changes while they are being read before JSON materialization.
fn read_bounded_snapshot(path: &Path) -> Option<String> {
    use std::io::Read as _;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400000 | 0o4000); // O_NOFOLLOW | O_NONBLOCK
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100 | 0x4); // O_NOFOLLOW | O_NONBLOCK
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        if !std::fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false)
        {
            return None;
        }
    }
    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
    {
        return None;
    }

    let file = options.open(path).ok()?;
    let before = file.metadata().ok()?;
    if !before.file_type().is_file() || before.len() > MAX_SNAPSHOT_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(before.len())
            .unwrap_or(MAX_SNAPSHOT_BYTES)
            .saturating_add(1),
    );
    (&file)
        .take((MAX_SNAPSHOT_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return None;
    }
    let after = file.metadata().ok()?;
    if !after.file_type().is_file()
        || after.len() != before.len()
        || after.len() != u64::try_from(bytes.len()).ok()?
    {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn snapshot_age_ms(generated_ms: u64, now_ms: u64) -> Option<u64> {
    (generated_ms > 0).then(|| now_ms.saturating_sub(generated_ms))
}

/// A filled-circle status dot — the shared glyph the datacenter rows / chrome pip
/// use, so a service dot reads one `Style` size + colour.
const DOT: &str = "\u{25CF}";

/// This node's daemon catalog: the `services` map key each node publishes into its
/// `shell-status.json`, paired with the label the plane renders. Fixed order so the
/// health list is stable frame-to-frame; a key absent from the snapshot is simply
/// not listed (never rendered as a false "down").
const SERVICE_CATALOG: [(&str, &str); 9] = [
    ("mackesd", "Mesh daemon"),
    ("nebula", "Overlay (Nebula)"),
    ("sync", "Sync (Syncthing)"),
    ("bus", "Mesh Bus"),
    ("dns", "Mesh DNS"),
    ("voice", "Voice HUD"),
    ("music", "Music"),
    ("kdc", "KDE Connect"),
    ("workbench", "Workbench"),
];

/// Keep list-valued connectivity facts small even when a faulty or newer
/// snapshot carries more entries than this surface can render usefully.
const MAX_CONNECTIVITY_FACTS: usize = 8;

const CONNECTIVITY_PROVIDER_CATALOG: [ConnectivityProvider; 5] = [
    ConnectivityProvider::Wifi,
    ConnectivityProvider::Ethernet,
    ConnectivityProvider::Cellular,
    ConnectivityProvider::Mesh,
    ConnectivityProvider::DnsLighthouse,
];

// ──────────────────────────── projected view ────────────────────────────

/// This node's live status, folded from the mesh-status snapshot. Pure data
/// (parsed without egui/IO/GPU), so it's unit-tested directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NodeStatus {
    /// `true` once a snapshot has been parsed — distinguishes "no snapshot yet"
    /// (the connecting state) from a parsed one.
    seen: bool,
    /// `true` when this node's OWN row was found in the snapshot's directory
    /// (`nodes[]`). `false` when the snapshot is readable but this node hasn't
    /// published a heartbeat record yet — the per-node fields then render honest
    /// "not yet in the peer directory", never a fabricated value.
    in_directory: bool,
    /// This node's hostname — the snapshot's `self` marker (local hostname when the
    /// snapshot omits it).
    hostname: String,
    /// Pinned deployment role (`lighthouse` / `server` / `workstation`), when known.
    role: Option<String>,
    /// This node's Nebula overlay IP, when known.
    overlay_ip: Option<String>,
    /// Directory presence tier: `online` / `idle` / `offline`, when known.
    presence: Option<String>,
    /// Wall-clock ms of this node's last heartbeat (`0` when never reported).
    last_seen_ms: u64,
    /// When the snapshot was generated — the reference clock for heartbeat age (so
    /// freshness can't skew against the desktop's own clock).
    generated_ms: u64,
    /// Installed `mde-core` version, when known.
    version: Option<String>,
    /// `true` when a newer version than this node's is live on the mesh.
    update_available: bool,
    /// The newest version seen across the mesh (for the update hint).
    latest_version: Option<String>,
    /// This node's own daemon health, in catalog order (label, up).
    services: Vec<(&'static str, bool)>,
    /// The directory's explicit `(online, total)` peer counts.
    ///
    /// This stays absent when either field is missing or when the writer emits
    /// an impossible pair. A missing pair must not become a fabricated `0/0`
    /// live count in the hardware center.
    peer_counts: Option<(u64, u64)>,
    /// The elected mesh leader's hostname, when one holds the lease.
    leader: Option<String>,
    /// Whether the last valid projection is retained after a provider read
    /// failure. Retained values remain diagnostic-only until refreshed.
    stale: bool,
    /// Bounded explanation for the retained stale projection.
    stale_reason: Option<String>,
    /// The Nebula tunnel cipher label, when nebula is up.
    cipher: Option<String>,
    /// Read-only interface, route, lighthouse, and resolver facts published by
    /// the network section of mesh-status.
    connectivity: ConnectivityFacts,
    /// Credential-free power-profile observation from the root snapshot writer.
    power_profile: PowerProfileFacts,
    /// Aggregate kernel power-source facts; supply names and sysfs paths never
    /// cross the world-readable snapshot boundary.
    power_source: PowerSourceFacts,
    /// Aggregate DRM/backlight observation with connector identity removed.
    display: DisplayFacts,
    /// Aggregate evdev observation with device identity removed.
    input: InputFacts,
    /// Aggregate local account posture. Usernames, home paths, shells, group
    /// membership, and credentials never cross the snapshot boundary.
    users: UsersFacts,
    /// Aggregate storage/thermal/fan observations with hardware identity
    /// removed from the shared snapshot.
    hardware: HardwareFacts,
    /// Credential-free PipeWire/Pulse/WirePlumber observation from the node
    /// status writer. Missing fields remain unknown rather than becoming a
    /// fabricated healthy audio stack.
    audio: AudioFacts,
    /// Credential-free camera/microphone privacy observations. A missing
    /// privacy provider remains unknown; device presence is not permission.
    privacy: PrivacyFacts,
    /// Bounded BlueZ observation; names, addresses, pairing keys, and trust
    /// material never cross the world-readable snapshot boundary.
    bluetooth: BluetoothFacts,
    /// Bounded aggregate resource telemetry. Process, mount, and device
    /// identity are intentionally absent from the world-readable projection.
    telemetry: TelemetryFacts,
    /// Versioned vendor-pack metadata. This is a manifest boundary only: packs
    /// cannot create routes or executable actions from snapshot data.
    vendor_packs: Vec<VendorPackFacts>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PowerProfileFacts {
    active: Option<String>,
    available: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PowerSourceFacts {
    battery_count: Option<u64>,
    battery_percent: Option<u64>,
    battery_status: Option<String>,
    ac_online: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DisplayFacts {
    connectors: Option<u64>,
    connected: Option<u64>,
    modes: Option<u64>,
    backlights: Option<u64>,
    backlight_percent: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct InputFacts {
    event_devices: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsersFacts {
    provider: bool,
    account_count: Option<u64>,
    login_count: Option<u64>,
    admin_groups: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct HardwareFacts {
    storage_devices: Option<u64>,
    storage_total_bytes: Option<u64>,
    storage_removable: Option<u64>,
    thermal_zones: Option<u64>,
    thermal_max_milli_c: Option<u64>,
    fan_devices: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AudioFacts {
    pulse_available: Option<bool>,
    pipewire_graph: Option<bool>,
    wireplumber_policy: Option<bool>,
    alsa_devices: Option<u64>,
    playback: Option<bool>,
    capture: Option<bool>,
    recovery: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PrivacyFacts {
    microphone_muted: Option<bool>,
    camera_devices: Option<u64>,
    camera_privacy: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BluetoothFacts {
    adapters: Option<u64>,
    powered: Option<bool>,
    devices: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TelemetryFacts {
    cpu_cores: Option<u64>,
    load_1m_milli: Option<u64>,
    memory_total_bytes: Option<u64>,
    memory_available_bytes: Option<u64>,
    root_total_bytes: Option<u64>,
    root_used_bytes: Option<u64>,
    root_available_bytes: Option<u64>,
    root_used_percent: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VendorPackFacts {
    name: String,
    version: String,
    status: &'static str,
    capabilities: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct TelemetrySample {
    load_percent: Option<f32>,
    memory_percent: Option<f32>,
    root_percent: Option<f32>,
}

fn telemetry_sample(facts: &TelemetryFacts) -> TelemetrySample {
    let load_percent = facts
        .load_1m_milli
        .zip(facts.cpu_cores)
        .filter(|(_, cores)| *cores > 0)
        .map(|(load, cores)| (load as f32 / (cores as f32 * 1000.0) * 100.0).clamp(0.0, 100.0));
    let memory_percent = facts
        .memory_total_bytes
        .zip(facts.memory_available_bytes)
        .filter(|(total, _)| *total > 0)
        .map(|(total, available)| {
            (total.saturating_sub(available) as f32 / total as f32 * 100.0).clamp(0.0, 100.0)
        });
    let root_percent = facts
        .root_used_percent
        .map(|percent| percent as f32)
        .or_else(|| {
            facts
                .root_total_bytes
                .zip(facts.root_used_bytes)
                .filter(|(total, _)| *total > 0)
                .map(|(total, used)| (used as f32 / total as f32 * 100.0).clamp(0.0, 100.0))
        });
    TelemetrySample {
        load_percent,
        memory_percent,
        root_percent,
    }
}

fn bluetooth_facts(value: Option<&Value>) -> BluetoothFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return BluetoothFacts::default();
    };
    let bounded_count = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_u64)
            .filter(|count| *count <= 64)
    };
    BluetoothFacts {
        adapters: bounded_count("adapters"),
        powered: object.get("powered").and_then(Value::as_bool),
        devices: bounded_count("devices"),
    }
}

fn telemetry_facts(value: Option<&Value>) -> TelemetryFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return TelemetryFacts::default();
    };
    let bounded = |key: &str, maximum| {
        object
            .get(key)
            .and_then(Value::as_u64)
            .filter(|value| *value <= maximum)
    };
    let load_1m_milli = object
        .get("load_1m_milli")
        .and_then(Value::as_u64)
        .filter(|value| *value <= 256_000);
    TelemetryFacts {
        cpu_cores: bounded("cpu_cores", 256),
        load_1m_milli,
        memory_total_bytes: bounded("memory_total_bytes", 1_u64 << 44),
        memory_available_bytes: bounded("memory_available_bytes", 1_u64 << 44),
        root_total_bytes: bounded("root_total_bytes", 1_u64 << 44),
        root_used_bytes: bounded("root_used_bytes", 1_u64 << 44),
        root_available_bytes: bounded("root_available_bytes", 1_u64 << 44),
        root_used_percent: bounded("root_used_percent", 100),
    }
}

fn power_source_facts(value: Option<&Value>) -> PowerSourceFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return PowerSourceFacts::default();
    };
    let bounded = |key: &str, maximum| {
        object
            .get(key)
            .and_then(Value::as_u64)
            .filter(|value| *value <= maximum)
    };
    let battery_status = object
        .get("battery_status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 32)
        .filter(|value| value.chars().all(|ch| ch.is_ascii_alphabetic()))
        .map(str::to_owned);
    PowerSourceFacts {
        battery_count: bounded("battery_count", 16),
        battery_percent: bounded("battery_percent", 100),
        battery_status,
        ac_online: object.get("ac_online").and_then(Value::as_bool),
    }
}

fn display_facts(value: Option<&Value>) -> DisplayFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return DisplayFacts::default();
    };
    let bounded = |key: &str, maximum| {
        object
            .get(key)
            .and_then(Value::as_u64)
            .filter(|value| *value <= maximum)
    };
    DisplayFacts {
        connectors: bounded("connectors", 64),
        connected: bounded("connected", 64),
        modes: bounded("modes", 512),
        backlights: bounded("backlights", 16),
        backlight_percent: bounded("backlight_percent", 100),
    }
}

fn input_facts(value: Option<&Value>) -> InputFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return InputFacts::default();
    };
    InputFacts {
        event_devices: object
            .get("event_devices")
            .and_then(Value::as_u64)
            .filter(|value| *value <= 64),
    }
}

fn hardware_facts(value: Option<&Value>) -> HardwareFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return HardwareFacts::default();
    };
    let bounded = |key: &str, maximum| {
        object
            .get(key)
            .and_then(Value::as_u64)
            .filter(|value| *value <= maximum)
    };
    HardwareFacts {
        storage_devices: bounded("storage_devices", 128),
        storage_total_bytes: bounded("storage_total_bytes", 1_u64 << 44),
        storage_removable: bounded("storage_removable", 32),
        thermal_zones: bounded("thermal_zones", 128),
        thermal_max_milli_c: bounded("thermal_max_milli_c", 200_000),
        fan_devices: bounded("fan_devices", 32),
    }
}

fn privacy_facts(value: Option<&Value>) -> PrivacyFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return PrivacyFacts::default();
    };
    PrivacyFacts {
        microphone_muted: object.get("microphone_muted").and_then(Value::as_bool),
        camera_devices: object
            .get("camera_devices")
            .and_then(Value::as_u64)
            .filter(|count| *count <= 64),
        camera_privacy: object.get("camera_privacy").and_then(Value::as_bool),
    }
}

fn audio_facts(value: Option<&Value>) -> AudioFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return AudioFacts::default();
    };
    let component_available = |flat_key: &str, typed_key: &str| {
        object.get(flat_key).and_then(Value::as_bool).or_else(|| {
            object
                .get(typed_key)
                .and_then(Value::as_object)
                .and_then(|component| component.get("availability"))
                .and_then(Value::as_str)
                .map(|state| state == "available")
        })
    };
    let typed_pulse = object
        .get("pulse_audio_compatibility")
        .and_then(Value::as_object);
    let recovery = object
        .get("recovery")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 160)
        .map(str::to_owned);
    let recovery = recovery.or_else(|| {
        object
            .get("recovery")
            .and_then(Value::as_object)
            .and_then(|component| component.get("availability"))
            .and_then(Value::as_str)
            .filter(|state| *state != "available")
            .map(|_| "Audio recovery provider is unavailable; refresh the snapshot.".to_owned())
    });
    AudioFacts {
        pulse_available: object
            .get("pulse_available")
            .and_then(Value::as_bool)
            .or_else(|| {
                typed_pulse
                    .and_then(|pulse| pulse.get("compatibility"))
                    .and_then(Value::as_str)
                    .map(|compatibility| compatibility == "compatible")
            }),
        pipewire_graph: component_available("pipewire_graph", "pipewire_graph"),
        wireplumber_policy: component_available("wireplumber_policy", "wireplumber_policy"),
        alsa_devices: object
            .get("alsa_devices")
            .and_then(Value::as_u64)
            .or_else(|| {
                object
                    .get("alsa_ucm_discovery")
                    .and_then(Value::as_object)
                    .and_then(|component| component.get("observed_items"))
                    .and_then(Value::as_u64)
            })
            .filter(|value| *value <= 256),
        playback: component_available("playback", "playback"),
        capture: component_available("capture", "capture"),
        recovery,
    }
}

/// Read a non-empty string field off a JSON object, or `None`.
fn nonempty(val: &Value, key: &str) -> Option<String> {
    val.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse the `services` map into the catalog-ordered (label, up) rows actually
/// present. A missing map (an older writer / a node with no `shell-status.json`)
/// yields an empty list → the view says "not yet reported" rather than a false
/// all-down.
fn parse_services(services: Option<&Value>) -> Vec<(&'static str, bool)> {
    let Some(obj) = services.and_then(Value::as_object) else {
        return Vec::new();
    };
    SERVICE_CATALOG
        .iter()
        .filter_map(|(key, label)| {
            obj.get(*key)
                .and_then(Value::as_bool)
                .map(|up| (*label, up))
        })
        .collect()
}

fn power_profile_facts(value: Option<&Value>) -> PowerProfileFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return PowerProfileFacts::default();
    };
    let active = object
        .get("active")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 32)
        .filter(|value| {
            value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        })
        .map(str::to_owned);
    let mut available = object
        .get("available")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 32)
        .filter(|value| {
            value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    available.sort();
    available.dedup();
    PowerProfileFacts { active, available }
}

fn users_facts(value: Option<&Value>) -> UsersFacts {
    let Some(object) = value.and_then(Value::as_object) else {
        return UsersFacts::default();
    };
    let bounded = |key: &str, maximum| {
        object
            .get(key)
            .and_then(Value::as_u64)
            .filter(|value| *value <= maximum)
    };
    UsersFacts {
        provider: object
            .get("provider")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        account_count: bounded("account_count", 4096),
        login_count: bounded("login_count", 4096),
        admin_groups: bounded("admin_groups", 16),
    }
}

/// The read-only connectivity facts this node can prove from mesh-status.
/// Empty fields stay empty so the renderer can say exactly which observation
/// is unavailable instead of filling a gap with local guesses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ConnectivityFacts {
    interface: Option<String>,
    cidr: Option<String>,
    default_route: Option<String>,
    lighthouses: Vec<String>,
    dns_servers: Vec<String>,
    /// Explicit underlay observations are optional because the current
    /// mesh-status writer only publishes the overlay. Never infer a provider
    /// from an interface prefix or from the presence of a default route.
    interfaces: [Option<InterfaceProviderFacts>; 3],
}

impl ConnectivityFacts {
    fn from_network(network: Option<&Value>) -> Self {
        let interface = first_network_string(network, &["overlay_if", "interface", "ifname"])
            .or_else(|| first_interface_entry_string(network, &["name", "interface", "ifname"]));
        let cidr = first_network_string(network, &["overlay_cidr", "cidr"])
            .or_else(|| first_interface_entry_string(network, &["cidr", "ip_cidr"]));
        let default_route =
            first_network_string(network, &["default_gw", "default_route", "default_gateway"]);

        Self {
            interface,
            cidr,
            default_route,
            lighthouses: network_fact_list(network, &["lighthouse_ips", "lighthouses"]),
            dns_servers: network_fact_list(
                network,
                &["dns_servers", "nameservers", "resolvers", "dns"],
            ),
            interfaces: interface_provider_facts(network),
        }
    }

    fn is_empty(&self) -> bool {
        self.interface.is_none()
            && self.cidr.is_none()
            && self.default_route.is_none()
            && self.lighthouses.is_empty()
            && self.dns_servers.is_empty()
            && self.interfaces.iter().all(Option::is_none)
    }

    fn has_underlay_observation(&self) -> bool {
        self.interfaces.iter().any(Option::is_some)
    }

    fn provider_projection(&self) -> [ConnectivityProviderProjection; 5] {
        CONNECTIVITY_PROVIDER_CATALOG.map(|provider| match provider {
            ConnectivityProvider::Wifi => {
                interface_provider_projection(provider, self.interfaces[0].as_ref())
            }
            ConnectivityProvider::Ethernet => {
                interface_provider_projection(provider, self.interfaces[1].as_ref())
            }
            ConnectivityProvider::Cellular => {
                interface_provider_projection(provider, self.interfaces[2].as_ref())
            }
            ConnectivityProvider::Mesh => ConnectivityProviderProjection {
                provider,
                availability: self.mesh_provider_availability(),
                recovery: match self.mesh_provider_availability() {
                    ConnectivityAvailability::Available(_) => ProviderRecovery::None,
                    ConnectivityAvailability::Degraded(_) => ProviderRecovery::RefreshSnapshot,
                    ConnectivityAvailability::Unavailable(_) => ProviderRecovery::AwaitProvider,
                },
                interface: self.interface.clone(),
                cidr: self.cidr.clone(),
            },
            ConnectivityProvider::DnsLighthouse => ConnectivityProviderProjection {
                provider,
                availability: self.dns_lighthouse_availability(),
                recovery: match self.dns_lighthouse_availability() {
                    ConnectivityAvailability::Available(_) => ProviderRecovery::None,
                    ConnectivityAvailability::Degraded(_) => ProviderRecovery::RefreshSnapshot,
                    ConnectivityAvailability::Unavailable(_) => ProviderRecovery::AwaitProvider,
                },
                interface: None,
                cidr: None,
            },
        })
    }

    fn mesh_provider_availability(&self) -> ConnectivityAvailability {
        if self.interface.is_none() && self.cidr.is_none() {
            return ConnectivityAvailability::Unavailable(
                "No explicit mesh overlay interface or CIDR is published.",
            );
        }
        if self.interface.is_some()
            && self.cidr.is_some()
            && (!self.lighthouses.is_empty() || !self.dns_servers.is_empty())
        {
            ConnectivityAvailability::Available(
                "Mesh interface, CIDR, and reachability are published.",
            )
        } else {
            ConnectivityAvailability::Degraded(
                "Mesh overlay facts are partial; interface, CIDR, and reachability are not all published.",
            )
        }
    }

    fn dns_lighthouse_availability(&self) -> ConnectivityAvailability {
        match (self.dns_servers.is_empty(), self.lighthouses.is_empty()) {
            (false, false) => ConnectivityAvailability::Available(
                "Mesh DNS and lighthouse endpoints are published by the snapshot.",
            ),
            (false, true) => ConnectivityAvailability::Available(
                "Mesh DNS resolvers are published; lighthouse endpoints are not published.",
            ),
            (true, false) => ConnectivityAvailability::Available(
                "Lighthouse endpoints are published; mesh DNS resolvers are not published.",
            ),
            (true, true) => ConnectivityAvailability::Unavailable(
                "No DNS resolver or lighthouse endpoint is published.",
            ),
        }
    }
}

/// A connectivity card's state is separate from the broader capability list:
/// a readable snapshot can still have no published node-local network facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectivityAvailability {
    Available(&'static str),
    Degraded(&'static str),
    Unavailable(&'static str),
}

impl ConnectivityAvailability {
    const fn tone(self) -> Color32 {
        match self {
            Self::Available(_) => Style::OK,
            Self::Degraded(_) => Style::WARN,
            Self::Unavailable(_) => Style::TEXT_DIM,
        }
    }

    const fn word(self) -> &'static str {
        match self {
            Self::Available(_) => "available",
            Self::Degraded(_) => "degraded",
            Self::Unavailable(_) => "unavailable",
        }
    }

    const fn detail(self) -> &'static str {
        match self {
            Self::Available(detail) | Self::Degraded(detail) | Self::Unavailable(detail) => detail,
        }
    }
}

/// Provider kinds are accepted only when the snapshot names them explicitly.
/// In particular, `wlan*`, `en*`, and `wwan*` prefixes are not evidence of a
/// backend and therefore never select a provider here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectivityProvider {
    Wifi,
    Ethernet,
    Cellular,
    Mesh,
    DnsLighthouse,
}

impl ConnectivityProvider {
    const fn label(self) -> &'static str {
        match self {
            Self::Wifi => "Wi-Fi",
            Self::Ethernet => "Ethernet",
            Self::Cellular => "Cellular",
            Self::Mesh => "Mesh overlay",
            Self::DnsLighthouse => "DNS / lighthouse",
        }
    }

    const fn index(self) -> Option<usize> {
        match self {
            Self::Wifi => Some(0),
            Self::Ethernet => Some(1),
            Self::Cellular => Some(2),
            Self::Mesh | Self::DnsLighthouse => None,
        }
    }
}

/// The only underlay state admitted into the read model. Raw NetworkManager /
/// ModemManager payloads, SSIDs, APNs, passwords, and PSKs are intentionally not
/// represented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderLinkState {
    Connected,
    Degraded,
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InterfaceProviderFacts {
    state: ProviderLinkState,
    interface: Option<String>,
    cidr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectivityProviderProjection {
    provider: ConnectivityProvider,
    availability: ConnectivityAvailability,
    recovery: ProviderRecovery,
    interface: Option<String>,
    cidr: Option<String>,
}

/// The only recovery actions this read-only provider boundary may advertise.
///
/// These are recovery guidance, not mutation verbs: the snapshot can request a
/// bounded re-read, but it cannot authorize reconnecting a link, changing a
/// profile, or supplying credentials. Keeping the distinction typed prevents a
/// future provider row from turning an unavailable observation into an implicit
/// network write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderRecovery {
    None,
    RefreshSnapshot,
    AwaitProvider,
}

impl ProviderRecovery {
    const fn label(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::RefreshSnapshot => Some("Recovery: refresh provider snapshot"),
            Self::AwaitProvider => Some("Recovery: await provider publication"),
        }
    }
}

fn first_network_string(network: Option<&Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| network.and_then(|value| nonempty(value, key)))
}

fn first_interface_entry_string(network: Option<&Value>, keys: &[&str]) -> Option<String> {
    network
        .and_then(|value| value.get("interfaces"))
        .and_then(Value::as_array)
        .and_then(|interfaces| {
            interfaces
                .iter()
                .find_map(|interface| keys.iter().find_map(|key| nonempty(interface, key)))
        })
}

fn bounded_strings(value: &Value) -> Vec<String> {
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .take(MAX_CONNECTIVITY_FACTS)
            .map(str::to_string)
            .collect(),
        Value::String(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .take(MAX_CONNECTIVITY_FACTS)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn network_fact_list(network: Option<&Value>, keys: &[&str]) -> Vec<String> {
    for key in keys {
        let Some(value) = network.and_then(|network| network.get(*key)) else {
            continue;
        };
        let direct = bounded_strings(value);
        if !direct.is_empty() {
            return direct;
        }
        if let Some(object) = value.as_object() {
            for nested_key in ["servers", "nameservers", "resolvers", "ips"] {
                let nested = object
                    .get(nested_key)
                    .map(bounded_strings)
                    .unwrap_or_default();
                if !nested.is_empty() {
                    return nested;
                }
            }
        }
    }
    Vec::new()
}

/// Read the optional typed underlay observations from `network.interfaces[]`.
/// Only the provider kind, link state, interface name, and CIDR cross the
/// snapshot boundary. The array is bounded and duplicate provider entries are
/// ignored deterministically, so a newer writer cannot create an unbounded UI
/// surface or smuggle credentials into the read model.
fn interface_provider_facts(network: Option<&Value>) -> [Option<InterfaceProviderFacts>; 3] {
    let mut facts = [None, None, None];
    let Some(interfaces) = network
        .and_then(|network| network.get("interfaces"))
        .and_then(Value::as_array)
    else {
        return facts;
    };

    for interface in interfaces.iter().take(MAX_CONNECTIVITY_FACTS) {
        let Some(provider) = explicit_interface_provider(interface) else {
            continue;
        };
        let Some(index) = provider.index() else {
            continue;
        };
        if facts[index].is_some() {
            continue;
        }
        facts[index] = Some(InterfaceProviderFacts {
            state: interface_link_state(interface),
            interface: ["name", "interface", "ifname"]
                .iter()
                .find_map(|key| nonempty(interface, key)),
            cidr: ["cidr", "ip_cidr"]
                .iter()
                .find_map(|key| nonempty(interface, key)),
        });
    }
    facts
}

fn explicit_interface_provider(interface: &Value) -> Option<ConnectivityProvider> {
    ["provider", "kind", "type", "technology", "transport"]
        .iter()
        .filter_map(|key| nonempty(interface, key))
        .find_map(|value| match value.to_ascii_lowercase().as_str() {
            "wifi" | "wi-fi" | "wireless" => Some(ConnectivityProvider::Wifi),
            "ethernet" | "wired" => Some(ConnectivityProvider::Ethernet),
            "cellular" | "mobile" | "wwan" => Some(ConnectivityProvider::Cellular),
            _ => None,
        })
}

fn interface_link_state(interface: &Value) -> ProviderLinkState {
    if let Some(connected) = interface.get("connected").and_then(Value::as_bool) {
        return if connected {
            ProviderLinkState::Connected
        } else {
            ProviderLinkState::Disconnected
        };
    }
    let Some(state) = ["state", "status", "operstate"]
        .iter()
        .find_map(|key| nonempty(interface, key))
    else {
        return ProviderLinkState::Degraded;
    };
    match state.to_ascii_lowercase().as_str() {
        "connected" | "up" | "online" | "activated" | "ready" => ProviderLinkState::Connected,
        "disconnected" | "down" | "offline" | "unavailable" | "disabled" => {
            ProviderLinkState::Disconnected
        }
        _ => ProviderLinkState::Degraded,
    }
}

fn interface_provider_projection(
    provider: ConnectivityProvider,
    facts: Option<&InterfaceProviderFacts>,
) -> ConnectivityProviderProjection {
    let (availability, interface, cidr, recovery) = match facts {
        None => (
            match provider {
                ConnectivityProvider::Wifi => ConnectivityAvailability::Unavailable(
                    "No explicit Wi-Fi provider observation is published.",
                ),
                ConnectivityProvider::Ethernet => ConnectivityAvailability::Unavailable(
                    "No explicit Ethernet provider observation is published.",
                ),
                ConnectivityProvider::Cellular => ConnectivityAvailability::Unavailable(
                    "No explicit cellular provider observation is published.",
                ),
                ConnectivityProvider::Mesh | ConnectivityProvider::DnsLighthouse => {
                    ConnectivityAvailability::Unavailable(
                        "This provider is projected from the mesh-status overlay facts.",
                    )
                }
            },
            None,
            None,
            ProviderRecovery::AwaitProvider,
        ),
        Some(facts) => (
            match (provider, facts.state) {
                (ConnectivityProvider::Wifi, ProviderLinkState::Connected) => {
                    ConnectivityAvailability::Available(
                        "A typed Wi-Fi observation reports a connected link.",
                    )
                }
                (ConnectivityProvider::Wifi, ProviderLinkState::Degraded) => {
                    ConnectivityAvailability::Degraded(
                        "A typed Wi-Fi observation is present, but link state is incomplete or degraded.",
                    )
                }
                (ConnectivityProvider::Wifi, ProviderLinkState::Disconnected) => {
                    ConnectivityAvailability::Unavailable(
                        "A typed Wi-Fi observation reports no connected link.",
                    )
                }
                (ConnectivityProvider::Ethernet, ProviderLinkState::Connected) => {
                    ConnectivityAvailability::Available(
                        "A typed Ethernet observation reports a connected link.",
                    )
                }
                (ConnectivityProvider::Ethernet, ProviderLinkState::Degraded) => {
                    ConnectivityAvailability::Degraded(
                        "A typed Ethernet observation is present, but link state is incomplete or degraded.",
                    )
                }
                (ConnectivityProvider::Ethernet, ProviderLinkState::Disconnected) => {
                    ConnectivityAvailability::Unavailable(
                        "A typed Ethernet observation reports no connected link.",
                    )
                }
                (ConnectivityProvider::Cellular, ProviderLinkState::Connected) => {
                    ConnectivityAvailability::Available(
                        "A typed cellular observation reports a connected link.",
                    )
                }
                (ConnectivityProvider::Cellular, ProviderLinkState::Degraded) => {
                    ConnectivityAvailability::Degraded(
                        "A typed cellular observation is present, but link state is incomplete or degraded.",
                    )
                }
                (ConnectivityProvider::Cellular, ProviderLinkState::Disconnected) => {
                    ConnectivityAvailability::Unavailable(
                        "A typed cellular observation reports no connected link.",
                    )
                }
                (ConnectivityProvider::Mesh | ConnectivityProvider::DnsLighthouse, _) => {
                    ConnectivityAvailability::Degraded(
                        "This provider is projected from the mesh-status overlay facts.",
                    )
                }
            },
            facts.interface.clone(),
            facts.cidr.clone(),
            match facts.state {
                ProviderLinkState::Connected => ProviderRecovery::None,
                ProviderLinkState::Degraded
                | ProviderLinkState::Disconnected => ProviderRecovery::RefreshSnapshot,
            },
        ),
    };
    ConnectivityProviderProjection {
        provider,
        availability,
        recovery,
        interface,
        cidr,
    }
}

impl NodeStatus {
    /// Fold the mesh-status snapshot into this node's status. `fallback_host` is the
    /// locally-resolved hostname, used only when the snapshot omits its `self`
    /// marker. A missing / garbage / non-mesh snapshot yields the honest unseen
    /// status (drives the connecting state), never a panic — mirroring the chrome
    /// bar's tolerance.
    fn project(snapshot: &str, fallback_host: &str) -> Self {
        let Ok(v) = serde_json::from_str::<Value>(snapshot) else {
            return Self::default();
        };
        let self_host = nonempty(&v, "self");
        let nodes = v.get("nodes").and_then(Value::as_array);
        // A real snapshot names at least `self` or a `nodes` array; anything else
        // (an empty object, an array, a fragment) reads as unseen.
        if self_host.is_none() && nodes.is_none() {
            return Self::default();
        }

        let hostname = self_host.unwrap_or_else(|| fallback_host.to_string());
        let network = v.get("network");
        let peer_counts = match (
            v.get("online").and_then(Value::as_u64),
            v.get("total").and_then(Value::as_u64),
        ) {
            (Some(online), Some(total)) if online <= total => Some((online, total)),
            _ => None,
        };
        let own = nodes.and_then(|arr| {
            arr.iter()
                .find(|n| n.get("hostname").and_then(Value::as_str) == Some(hostname.as_str()))
        });

        Self {
            seen: true,
            in_directory: own.is_some(),
            // Prefer this node's own directory-row overlay IP; fall back to the
            // network overview's locally-probed overlay address.
            overlay_ip: own
                .and_then(|n| nonempty(n, "overlay_ip"))
                .or_else(|| network.and_then(|n| nonempty(n, "overlay_ip"))),
            role: own.and_then(|n| nonempty(n, "role")),
            presence: own.and_then(|n| nonempty(n, "presence")),
            last_seen_ms: own
                .and_then(|n| n.get("last_seen_ms").and_then(Value::as_u64))
                .unwrap_or(0),
            version: own.and_then(|n| nonempty(n, "version")),
            update_available: own
                .and_then(|n| n.get("update").and_then(Value::as_bool))
                .unwrap_or(false),
            services: parse_services(own.and_then(|n| n.get("services"))),
            generated_ms: v.get("generated_ms").and_then(Value::as_u64).unwrap_or(0),
            latest_version: nonempty(&v, "latest_version"),
            peer_counts,
            leader: network.and_then(|n| nonempty(n, "leader")),
            stale: false,
            stale_reason: None,
            cipher: network.and_then(|n| nonempty(n, "cipher")),
            connectivity: ConnectivityFacts::from_network(network),
            power_profile: power_profile_facts(v.get("power_profile")),
            power_source: power_source_facts(v.get("power_source")),
            display: display_facts(v.get("display")),
            input: input_facts(v.get("input")),
            users: users_facts(v.get("users")),
            hardware: hardware_facts(v.get("hardware")),
            audio: audio_facts(v.get("audio")),
            privacy: privacy_facts(v.get("privacy")),
            bluetooth: bluetooth_facts(v.get("bluetooth")),
            telemetry: telemetry_facts(v.get("telemetry")),
            vendor_packs: vendor_pack_facts(v.get("vendor_packs")),
            hostname,
        }
    }

    fn mark_stale(&mut self, reason: impl Into<String>) {
        self.stale = true;
        self.stale_reason = Some(reason.into());
    }

    fn connectivity_availability(&self) -> ConnectivityAvailability {
        if !self.seen {
            return ConnectivityAvailability::Unavailable(
                "Connectivity facts are unavailable until the mesh-status snapshot is read.",
            );
        }
        if self.stale {
            return ConnectivityAvailability::Degraded(
                "Connectivity facts are retained from a stale snapshot; refresh before relying on them.",
            );
        }
        if self.connectivity.is_empty() {
            return ConnectivityAvailability::Unavailable(
                "No interface, route, provider, lighthouse, or DNS facts are published by mesh-status.",
            );
        }

        let providers = self.provider_projection();
        let mesh_ready = matches!(
            providers[3].availability,
            ConnectivityAvailability::Available(_)
        );
        let underlay_ready = providers[..3].iter().any(|projection| {
            matches!(
                projection.availability,
                ConnectivityAvailability::Available(_)
            )
        }) && self.connectivity.default_route.is_some()
            && (!self.connectivity.lighthouses.is_empty()
                || !self.connectivity.dns_servers.is_empty());
        if mesh_ready || underlay_ready {
            return ConnectivityAvailability::Available(
                "Typed provider, mesh reachability, or DNS facts are published.",
            );
        }
        ConnectivityAvailability::Degraded(
            "Only partial connectivity/provider facts are published; missing values are not inferred.",
        )
    }

    /// Apply snapshot freshness to each provider row. A retained projection is
    /// never allowed to look freshly actionable merely because its last known
    /// link state was connected.
    fn provider_projection(&self) -> [ConnectivityProviderProjection; 5] {
        let mut projections = self.connectivity.provider_projection();
        if self.stale {
            for projection in &mut projections {
                projection.availability = stale_provider_availability(projection.availability);
                projection.recovery = ProviderRecovery::RefreshSnapshot;
            }
        }
        projections
    }

    /// `true` when this node holds the mesh leader lease.
    fn is_leader(&self) -> bool {
        self.leader.as_deref() == Some(self.hostname.as_str())
    }

    /// A human "N ago" freshness for this node's last heartbeat, measured against
    /// the snapshot's own `generated_ms` clock. `None` when no heartbeat has been
    /// recorded yet.
    fn heartbeat_label(&self) -> Option<String> {
        if self.last_seen_ms == 0 {
            return None;
        }
        let secs = self.generated_ms.saturating_sub(self.last_seen_ms) / 1000;
        Some(if secs < 5 {
            "just now".to_string()
        } else if secs < 90 {
            format!("{secs}s ago")
        } else if secs < 90 * 60 {
            format!("{}m ago", secs / 60)
        } else {
            format!("{}h ago", secs / 3600)
        })
    }

    /// Project the bounded, read-only capabilities this snapshot can support.
    ///
    /// The snapshot is an observation boundary, not a provider registry. A
    /// capability is therefore only `Available` when the corresponding fact is
    /// actually present; missing node-local providers are represented as typed
    /// unavailable states instead of becoming speculative controls.
    fn capability_projection(&self) -> [CapabilityProjection; CAPABILITY_CATALOG.len()] {
        CAPABILITY_CATALOG.map(|capability| CapabilityProjection {
            capability,
            availability: self.capability_availability(capability),
        })
    }

    fn capability_availability(&self, capability: NodeCapability) -> CapabilityAvailability {
        if self.stale {
            return CapabilityAvailability::Degraded(
                "The last valid provider projection is stale; refresh before relying on this state.",
            );
        }
        match capability {
            NodeCapability::MeshSnapshot => {
                if self.seen {
                    CapabilityAvailability::Available(
                        "Live world-readable mesh-status snapshot is present.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "The mesh-status snapshot has not arrived or is unreadable.",
                    )
                }
            }
            NodeCapability::NodeIdentity => {
                if !self.seen {
                    CapabilityAvailability::Unavailable(
                        "Identity is unavailable until the mesh-status snapshot is read.",
                    )
                } else if self.in_directory {
                    CapabilityAvailability::Available(
                        "Hostname, role, overlay address, and presence are live snapshot facts.",
                    )
                } else {
                    CapabilityAvailability::Degraded(
                        "The snapshot names this node, but its peer-directory row is not present.",
                    )
                }
            }
            NodeCapability::ServiceHealth => {
                if !self.seen {
                    CapabilityAvailability::Unavailable(
                        "Service health is unavailable until the mesh-status snapshot is read.",
                    )
                } else if !self.services.is_empty() {
                    CapabilityAvailability::Available(
                        "Published daemon health is available for the reported services.",
                    )
                } else if self.in_directory {
                    CapabilityAvailability::Degraded(
                        "This node is in the directory, but it has not reported service health.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "This node has no directory row from which to read service health.",
                    )
                }
            }
            NodeCapability::MeshContext => {
                if !self.seen {
                    CapabilityAvailability::Unavailable(
                        "Mesh context is unavailable until the mesh-status snapshot is read.",
                    )
                } else if self.peer_counts.is_some() && self.leader.is_some() {
                    CapabilityAvailability::Available(
                        "Peer counts and leader state are read from the live snapshot.",
                    )
                } else if self.peer_counts.is_some() || self.leader.is_some() {
                    CapabilityAvailability::Degraded(
                        "The snapshot exposes only part of mesh context; missing facts remain unavailable.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "The snapshot has no peer counts or elected leader to report.",
                    )
                }
            }
            NodeCapability::ConnectivityProviders => {
                if !self.seen {
                    CapabilityAvailability::Unavailable(
                        "Connectivity providers are unavailable until the mesh-status snapshot is read.",
                    )
                } else {
                    let providers = self.provider_projection();
                    if providers.iter().any(|projection| {
                        matches!(
                            projection.availability,
                            ConnectivityAvailability::Available(_)
                        )
                    }) {
                        CapabilityAvailability::Available(
                            "Typed Wi-Fi, Ethernet, cellular, mesh, and DNS/lighthouse observations are projected read-only.",
                        )
                    } else if providers.iter().any(|projection| {
                        matches!(
                            projection.availability,
                            ConnectivityAvailability::Degraded(_)
                        )
                    }) {
                        CapabilityAvailability::Degraded(
                            "Connectivity providers are partially observed; missing backend facts remain unavailable.",
                        )
                    } else {
                        CapabilityAvailability::Unavailable(
                            "No typed connectivity provider observation is published by mesh-status.",
                        )
                    }
                }
            }
            NodeCapability::UpdateStatus => {
                if !self.seen {
                    CapabilityAvailability::Unavailable(
                        "Version posture is unavailable until the mesh-status snapshot is read.",
                    )
                } else if self.version.is_some() {
                    CapabilityAvailability::Available(
                        "Installed version and the mesh update target are read-only snapshot facts.",
                    )
                } else {
                    CapabilityAvailability::Degraded(
                        "The snapshot has no installed version for this node.",
                    )
                }
            }
            NodeCapability::LocalTelemetry => {
                let telemetry = &self.telemetry;
                if telemetry.cpu_cores.is_some()
                    || telemetry.load_1m_milli.is_some()
                    || telemetry.memory_total_bytes.is_some()
                    || telemetry.root_total_bytes.is_some()
                {
                    CapabilityAvailability::Available(
                        "Bounded aggregate CPU, memory, and root-storage telemetry is published read-only.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "CPU, memory, and disk telemetry is not published to this snapshot surface.",
                    )
                }
            }
            NodeCapability::MutationProviders => CapabilityAvailability::Unavailable(
                "No typed node-local mutation provider is advertised by mesh-status.",
            ),
        }
    }

    /// Project every mutation/provider action through the same fail-closed
    /// boundary. The snapshot can expose an update target or service health,
    /// but it cannot authorize a write and it does not name a provider that can
    /// execute one. Keeping these as typed rows makes that boundary visible and
    /// prevents a future button from silently turning a read model into a writer.
    fn action_projection(&self) -> [ActionProjection; ACTION_CATALOG.len()] {
        ACTION_CATALOG.map(|action| ActionProjection {
            action,
            availability: self.action_availability(action),
        })
    }

    fn action_availability(&self, action: ThisNodeAction) -> CapabilityAvailability {
        if self.stale {
            return CapabilityAvailability::Degraded(
                "The provider projection is stale; refresh before requesting an action.",
            );
        }
        match action {
            ThisNodeAction::RestartService => {
                if self.services.is_empty() {
                    CapabilityAvailability::Unavailable(
                        "No reported service target is available for a restart request.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "Service health is read-only here; no typed service-control provider is connected.",
                    )
                }
            }
            ThisNodeAction::ApplyUpdate => {
                if self.update_available {
                    CapabilityAvailability::Unavailable(
                        "An update target is visible, but no typed update provider is connected.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "No pending update action is advertised by the live snapshot.",
                    )
                }
            }
            ThisNodeAction::ChangeConnectivity => {
                if self.connectivity.has_underlay_observation() {
                    CapabilityAvailability::Unavailable(
                        "Connectivity provider state is visible, but no typed NetworkManager/ModemManager mutation provider is connected.",
                    )
                } else {
                    CapabilityAvailability::Unavailable(
                        "NetworkManager/ModemManager observation and mutation providers are not connected to This Node.",
                    )
                }
            }
            ThisNodeAction::InspectConnectivityProfiles => CapabilityAvailability::Unavailable(
                "NetworkManager profile inventory is read-only until a typed SecretAgent activation provider is connected.",
            ),
            ThisNodeAction::ToggleBluetooth => CapabilityAvailability::Unavailable(
                "Bluetooth observations are published, but the typed local BlueZ action requires a fresh System provider target.",
            ),
            ThisNodeAction::ManageBluetoothDevices => CapabilityAvailability::Unavailable(
                "Bluetooth devices are observed, but pairing and device management require a fresh typed BlueZ provider target.",
            ),
            ThisNodeAction::PowerSession => CapabilityAvailability::Unavailable(
                "Power capabilities are observed, but a fresh typed logind provider is required before a session action can be offered.",
            ),
            ThisNodeAction::ToggleWifi => CapabilityAvailability::Unavailable(
                "Wi-Fi observations are published, but a live typed NetworkManager radio target is required before a safe toggle can be offered.",
            ),
            ThisNodeAction::DisconnectNetworkLink => CapabilityAvailability::Unavailable(
                "Connectivity observations are published, but a live typed NetworkManager device target is required before a safe disconnect can be offered.",
            ),
            ThisNodeAction::ToggleDisplayOutput => CapabilityAvailability::Unavailable(
                "Display observations are published, but a typed local seat output target is required before a safe toggle can be offered.",
            ),
            ThisNodeAction::SetDisplayMode => CapabilityAvailability::Unavailable(
                "Display observations are published, but a live typed connector mode target is required before a mode change can be offered.",
            ),
            ThisNodeAction::ArrangeDisplays => CapabilityAvailability::Unavailable(
                "Display observations are published, but a fresh typed display arrangement is required before outputs can be reordered.",
            ),
            ThisNodeAction::AdjustDisplayBrightness => CapabilityAvailability::Unavailable(
                "Display observations are published, but a live typed backlight or DDC target is required before a safe brightness step can be offered.",
            ),
            ThisNodeAction::ToggleAudioMute => CapabilityAvailability::Unavailable(
                "Audio observations are published, but a live typed mixer target is required before a safe mute can be offered.",
            ),
            ThisNodeAction::AdjustAudioVolume => CapabilityAvailability::Unavailable(
                "Audio observations are published, but a live typed master mixer target is required before a safe volume step can be offered.",
            ),
            ThisNodeAction::ToggleMicrophoneMute => CapabilityAvailability::Unavailable(
                "Audio observations are published, but a live typed capture target is required before microphone mute can be offered.",
            ),
            ThisNodeAction::AdjustChargeLimit => CapabilityAvailability::Unavailable(
                "Power observations are published, but a live typed charge-threshold target is required before a safe cap change can be offered.",
            ),
            ThisNodeAction::AdjustKeyboardBrightness => CapabilityAvailability::Unavailable(
                "Input observations are published, but a live typed keyboard-LED target is required before a safe brightness step can be offered.",
            ),
            ThisNodeAction::AdjustPointerSpeed => CapabilityAvailability::Unavailable(
                "Input observations are published, but a live typed direct-seat input policy is required before a pointer change can be offered.",
            ),
            ThisNodeAction::ToggleTapToClick => CapabilityAvailability::Unavailable(
                "Input observations are published, but a live typed direct-seat input policy is required before tap-to-click can be changed.",
            ),
            ThisNodeAction::ConfigureTouchGestures => CapabilityAvailability::Unavailable(
                "Input observations are published, but a live typed direct-seat touch/gesture policy is required before these controls can be changed.",
            ),
            ThisNodeAction::ChangePowerProfile => {
                CapabilityAvailability::Unavailable(if self.power_profile.available.is_empty() {
                    "No power-profile provider observation is published by mesh-status."
                } else {
                    "Power-profile state is observed read-only; typed local mutation still requires the System provider authorization path."
                })
            }
            ThisNodeAction::ChangePlatformProfile => CapabilityAvailability::Unavailable(
                "Kernel platform-profile choices are observed locally; action availability requires a fresh trusted hardware provider target.",
            ),
            ThisNodeAction::ConfigureHardware => CapabilityAvailability::Unavailable(
                "Hardware/OEM mutation is not connected to a typed, bounded provider.",
            ),
        }
    }
}

fn vendor_pack_facts(value: Option<&Value>) -> Vec<VendorPackFacts> {
    let Some(packs) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    packs
        .iter()
        .take(MAX_VENDOR_PACKS)
        .filter_map(|pack| {
            let name = bounded_vendor_text(pack.get("name")?.as_str()?)?;
            let version = bounded_vendor_text(
                pack.get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
            )
            .unwrap_or_else(|| "unknown".to_owned());
            let status = match pack
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unavailable")
                .to_ascii_lowercase()
                .as_str()
            {
                "installed" | "current" => "installed",
                "outdated" => "outdated",
                _ => "unavailable",
            };
            let capabilities = pack
                .get("capabilities")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(MAX_VENDOR_CAPABILITIES)
                .filter_map(Value::as_str)
                .filter_map(bounded_vendor_text)
                .collect();
            Some(VendorPackFacts {
                name,
                version,
                status,
                capabilities,
            })
        })
        .collect()
}

fn bounded_vendor_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.chars().count() <= MAX_VENDOR_TEXT_CHARS
        && !value.chars().any(char::is_control))
    .then(|| value.to_owned())
}

/// Retained provider facts must not continue to announce current availability
/// after the source snapshot goes stale. Preserve an actually-unavailable state
/// (there is still no observation to degrade), but make every observed state
/// visibly stale in the row itself rather than relying on the banner above it.
fn stale_provider_availability(availability: ConnectivityAvailability) -> ConnectivityAvailability {
    match availability {
        ConnectivityAvailability::Available(_) | ConnectivityAvailability::Degraded(_) => {
            ConnectivityAvailability::Degraded(
                "The last provider observation is stale; refresh before relying on it.",
            )
        }
        ConnectivityAvailability::Unavailable(detail) => {
            ConnectivityAvailability::Unavailable(detail)
        }
    }
}

/// Fixed capability identifiers for the This Node read model. Keep this catalog
/// finite: a remote snapshot may describe services, but it cannot create an
/// unbounded set of UI capabilities or privileged operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeCapability {
    MeshSnapshot,
    NodeIdentity,
    ServiceHealth,
    MeshContext,
    ConnectivityProviders,
    UpdateStatus,
    LocalTelemetry,
    MutationProviders,
}

impl NodeCapability {
    const fn label(self) -> &'static str {
        match self {
            Self::MeshSnapshot => "Mesh status snapshot",
            Self::NodeIdentity => "Node identity",
            Self::ServiceHealth => "Service health",
            Self::MeshContext => "Mesh context",
            Self::ConnectivityProviders => "Connectivity providers",
            Self::UpdateStatus => "Version posture",
            Self::LocalTelemetry => "Node telemetry",
            Self::MutationProviders => "Mutation providers",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::MeshSnapshot => "Bounded source for this surface",
            Self::NodeIdentity => "Hostname, role, overlay, and presence",
            Self::ServiceHealth => "Published daemon health rows",
            Self::MeshContext => "Peer count and elected leader",
            Self::ConnectivityProviders => {
                "Wi-Fi, Ethernet, cellular, mesh, and DNS/lighthouse state"
            }
            Self::UpdateStatus => "Installed version and update target",
            Self::LocalTelemetry => "CPU, memory, and disk readings",
            Self::MutationProviders => "Typed local control backends",
        }
    }
}

const CAPABILITY_CATALOG: [NodeCapability; 8] = [
    NodeCapability::MeshSnapshot,
    NodeCapability::NodeIdentity,
    NodeCapability::ServiceHealth,
    NodeCapability::MeshContext,
    NodeCapability::ConnectivityProviders,
    NodeCapability::UpdateStatus,
    NodeCapability::LocalTelemetry,
    NodeCapability::MutationProviders,
];

/// A capability's honest state and the reason the UI can show to an operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapabilityAvailability {
    Available(&'static str),
    Degraded(&'static str),
    Unavailable(&'static str),
}

impl CapabilityAvailability {
    const fn tone(self) -> Color32 {
        match self {
            Self::Available(_) => Style::OK,
            Self::Degraded(_) => Style::WARN,
            Self::Unavailable(_) => Style::TEXT_DIM,
        }
    }

    const fn word(self) -> &'static str {
        match self {
            Self::Available(_) => "available",
            Self::Degraded(_) => "degraded",
            Self::Unavailable(_) => "unavailable",
        }
    }

    const fn detail(self) -> &'static str {
        match self {
            Self::Available(detail) | Self::Degraded(detail) | Self::Unavailable(detail) => detail,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapabilityProjection {
    capability: NodeCapability,
    availability: CapabilityAvailability,
}

/// Typed actions that this read-only snapshot may describe, but not execute.
/// The fixed list is intentionally small and provider-neutral; arbitrary verbs,
/// paths, shell commands, and guessed targets never enter the UI model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThisNodeAction {
    RestartService,
    ApplyUpdate,
    ChangeConnectivity,
    InspectConnectivityProfiles,
    ToggleBluetooth,
    ManageBluetoothDevices,
    PowerSession,
    ToggleWifi,
    DisconnectNetworkLink,
    ToggleDisplayOutput,
    SetDisplayMode,
    ArrangeDisplays,
    AdjustDisplayBrightness,
    ToggleAudioMute,
    AdjustAudioVolume,
    ToggleMicrophoneMute,
    AdjustChargeLimit,
    AdjustKeyboardBrightness,
    AdjustPointerSpeed,
    ToggleTapToClick,
    ConfigureTouchGestures,
    ChangePowerProfile,
    ChangePlatformProfile,
    ConfigureHardware,
}

impl ThisNodeAction {
    const fn label(self) -> &'static str {
        match self {
            Self::RestartService => "Restart a service",
            Self::ApplyUpdate => "Apply node update",
            Self::ChangeConnectivity => "Change connectivity",
            Self::InspectConnectivityProfiles => "Inspect connectivity profiles",
            Self::ToggleBluetooth => "Toggle Bluetooth power",
            Self::ManageBluetoothDevices => "Manage Bluetooth devices",
            Self::PowerSession => "Power & session",
            Self::ToggleWifi => "Toggle Wi-Fi power",
            Self::DisconnectNetworkLink => "Disconnect a network link",
            Self::ToggleDisplayOutput => "Toggle display output",
            Self::SetDisplayMode => "Set display mode",
            Self::ArrangeDisplays => "Arrange displays",
            Self::AdjustDisplayBrightness => "Adjust display brightness",
            Self::ToggleAudioMute => "Toggle master audio mute",
            Self::AdjustAudioVolume => "Adjust master audio volume",
            Self::ToggleMicrophoneMute => "Toggle microphone mute",
            Self::AdjustChargeLimit => "Adjust charge limit",
            Self::AdjustKeyboardBrightness => "Adjust keyboard brightness",
            Self::AdjustPointerSpeed => "Adjust pointer speed",
            Self::ToggleTapToClick => "Toggle tap-to-click",
            Self::ConfigureTouchGestures => "Configure touch & gestures",
            Self::ChangePowerProfile => "Change power profile",
            Self::ChangePlatformProfile => "Change kernel platform profile",
            Self::ConfigureHardware => "Configure hardware",
        }
    }

    const fn contract(self) -> ActionContract {
        match self {
            Self::RestartService => ActionContract {
                impact: "Interrupts the selected service and may interrupt dependent workloads.",
                confirmation: "Name the service and confirm the interruption before dispatch.",
                authorization: "Privileged trusted session on the target node.",
                audit: "Operator, target service, timestamp, request, and provider outcome.",
                recovery: "Recheck service health; restore the last known-good service state if restart fails.",
            },
            Self::ApplyUpdate => ActionContract {
                impact: "May restart the node and temporarily interrupt local and mesh services.",
                confirmation: "Show version, restart requirement, maintenance window, and confirm.",
                authorization: "Privileged trusted session with an update provider.",
                audit: "Operator, source/target version, timestamp, confirmation, and outcome.",
                recovery: "Use the provider's bounded rollback or safe recovery profile; never shell-compose an update.",
            },
            Self::ChangeConnectivity => ActionContract {
                impact: "Can disconnect underlay links or alter routes, DNS, or mesh reachability.",
                confirmation: "Show affected interface and reachability impact before confirming.",
                authorization: "Privileged trusted session through typed NetworkManager/ModemManager.",
                audit: "Operator, typed provider request, target, timestamp, and resulting link state.",
                recovery: "Preserve nebula1/lighthouse reachability and restore the prior bounded profile on failure.",
            },
            Self::InspectConnectivityProfiles => ActionContract {
                impact: "Inspection is read-only; optional profile activation can interrupt underlay links and mesh reachability.",
                confirmation: "Profile activation requires a visible second click and an interactive SecretAgent credential prompt.",
                authorization: "Fresh typed NetworkManager Settings.Connection observation through the trusted local seat.",
                audit: "Provider observation timestamp, selected bounded profile, operator confirmation, and activation outcome.",
                recovery: "Preserve nebula1 and lighthouse reachability; surface provider refusal and refresh before retrying.",
            },
            Self::ToggleBluetooth => ActionContract {
                impact: "Turns the local Bluetooth radio off or on and may disconnect peripherals.",
                confirmation: "Show the current radio state and affected peripheral impact before confirming.",
                authorization: "Privileged trusted session through the typed local BlueZ provider.",
                audit: "Operator, adapter target, old/new power state, timestamp, and provider outcome.",
                recovery: "Restore the prior radio state if the provider reports a failed transition.",
            },
            Self::ManageBluetoothDevices => ActionContract {
                impact: "Pairs, connects, disconnects, trusts, or forgets a selected Bluetooth peripheral.",
                confirmation: "Show the device, current bond/link state, and expected peripheral impact before confirming.",
                authorization: "Privileged trusted session through the typed local BlueZ device provider and pairing agent.",
                audit: "Operator, device target, verb, old/new bond/link/trust state, timestamp, and provider outcome.",
                recovery: "Retain the last confirmed device state; surface pairing prompts or provider refusal without guessing.",
            },
            Self::PowerSession => ActionContract {
                impact: "Locks, suspends, hibernates, reboots, or powers off this node and may interrupt all workloads.",
                confirmation: "Show logind policy availability, expected interruption, and confirm host-down verbs twice.",
                authorization: "Trusted local session through the typed logind provider and its capability policy.",
                audit: "Operator, power verb, capability result, timestamp, confirmation, and provider outcome.",
                recovery: "Surface refusal without claiming a transition; provider-owned recovery handles an accepted host-down action.",
            },
            Self::ToggleWifi => ActionContract {
                impact: "Turns the NetworkManager Wi-Fi radio off or on and may disconnect wireless links.",
                confirmation: "Show the current radio state and preserve mesh/underlay reachability warnings before confirming.",
                authorization: "Privileged trusted session through the typed local NetworkManager provider.",
                audit: "Operator, radio state, affected link, timestamp, and provider outcome.",
                recovery: "Retain the provider's last confirmed state and surface refusal; never rewrite profiles or credentials.",
            },
            Self::DisconnectNetworkLink => ActionContract {
                impact: "Disconnects the selected underlay link and may interrupt local or mesh-dependent traffic.",
                confirmation: "Show the interface and mesh-reachability warning before confirming.",
                authorization: "Privileged trusted session through the typed local NetworkManager device provider.",
                audit: "Operator, interface target, prior state, timestamp, and provider outcome.",
                recovery: "Retain the provider-confirmed disconnected state; reconnect only through a separate typed profile workflow.",
            },
            Self::ToggleDisplayOutput => ActionContract {
                impact: "Turns a connected display output off or on and may change the active workspace layout.",
                confirmation: "Show the affected display and preserve the last-console interlock before confirming.",
                authorization: "Privileged trusted session through the typed local seat display provider.",
                audit: "Operator, display target, old/new enabled state, timestamp, and provider outcome.",
                recovery: "Keep one active console and restore the prior output state if the provider refuses the change.",
            },
            Self::SetDisplayMode => ActionContract {
                impact: "Changes the selected display's desired resolution/refresh and may reflow the workspace.",
                confirmation: "Show the display, current mode, requested mode, and live-apply limitation before confirming.",
                authorization: "Privileged trusted session through the typed local DRM connector/layout provider.",
                audit: "Operator, display target, old/new mode, timestamp, and provider outcome.",
                recovery: "Retain the last confirmed layout intent; live DRM application remains bounded by the runner.",
            },
            Self::ArrangeDisplays => ActionContract {
                impact: "Reorders enabled display outputs in the saved left-to-right workspace arrangement.",
                confirmation: "Show the display, direction, resulting order, and live-apply limitation before confirming.",
                authorization: "Privileged trusted session through the typed local DRM connector/layout provider.",
                audit: "Operator, display target, direction, old/new order, timestamp, and provider outcome.",
                recovery: "Retain the last confirmed arrangement intent; live re-apply remains bounded by the DRM runner.",
            },
            Self::AdjustDisplayBrightness => ActionContract {
                impact: "Changes the active panel or external display brightness by a bounded ten-percent step.",
                confirmation: "Show the display target, current level, and next level before confirming.",
                authorization: "Privileged trusted session through the typed local backlight or DDC provider.",
                audit: "Operator, provider target, old/new brightness, timestamp, and provider outcome.",
                recovery: "Keep the last confirmed brightness and surface an out-of-range or provider refusal without guessing.",
            },
            Self::ToggleAudioMute => ActionContract {
                impact: "Mutes or unmutes the master playback route for this node.",
                confirmation: "Show the current playback state before confirming the change.",
                authorization: "Privileged trusted session through the typed local mixer provider.",
                audit: "Operator, master route, old/new mute state, timestamp, and provider outcome.",
                recovery: "Retain the provider's last confirmed mute state and surface refusal without guessing.",
            },
            Self::AdjustAudioVolume => ActionContract {
                impact: "Changes the master playback volume by a bounded ten-percent step.",
                confirmation: "Show the current volume and next volume before confirming.",
                authorization: "Privileged trusted session through the typed local PipeWire mixer provider.",
                audit: "Operator, master route, old/new volume, timestamp, and provider outcome.",
                recovery: "Retain the provider's last confirmed volume and surface refusal without guessing.",
            },
            Self::ToggleMicrophoneMute => ActionContract {
                impact: "Mutes or unmutes the selected local capture route for this node.",
                confirmation: "Show the capture target and current mute state before confirming.",
                authorization: "Privileged trusted session through the typed local PipeWire capture provider.",
                audit: "Operator, capture route, old/new mute state, timestamp, and provider outcome.",
                recovery: "Retain the provider's last confirmed mute state and surface refusal without guessing.",
            },
            Self::AdjustChargeLimit => ActionContract {
                impact: "Changes the battery charge-stop cap by a bounded five-percent step.",
                confirmation: "Show the current cap, next cap, and charging-life impact before confirming.",
                authorization: "Privileged trusted session through the typed local charge-threshold provider.",
                audit: "Operator, battery target, old/new cap, timestamp, and provider outcome.",
                recovery: "Retain the provider's last confirmed cap and surface refusal without guessing.",
            },
            Self::AdjustKeyboardBrightness => ActionContract {
                impact: "Changes the selected keyboard backlight by a bounded ten-percent step.",
                confirmation: "Show the keyboard LED target, current level, and next level before confirming.",
                authorization: "Privileged trusted session through the typed local keyboard-LED provider.",
                audit: "Operator, keyboard LED target, old/new brightness, timestamp, and provider outcome.",
                recovery: "Retain the provider's last confirmed level and surface refusal without guessing.",
            },
            Self::AdjustPointerSpeed => ActionContract {
                impact: "Changes native pointer sensitivity by a bounded ten-percent step.",
                confirmation: "Show the current policy and next speed before confirming.",
                authorization: "Privileged trusted session through the typed direct-seat input-policy provider.",
                audit: "Operator, pointer policy, old/new speed, timestamp, and provider outcome.",
                recovery: "Retain the last confirmed policy and restore it if the direct-seat handoff refuses.",
            },
            Self::ToggleTapToClick => ActionContract {
                impact: "Enables or disables touchpad tap-to-click for the local seat.",
                confirmation: "Show the current touchpad policy and requested change before confirming.",
                authorization: "Privileged trusted session through the typed direct-seat input-policy provider.",
                audit: "Operator, touchpad policy, old/new state, timestamp, and provider outcome.",
                recovery: "Retain the last confirmed policy and surface any direct-seat refusal without guessing.",
            },
            Self::ConfigureTouchGestures => ActionContract {
                impact: "Changes touchscreen, two-finger-scroll, or edge-gesture behavior on the local seat.",
                confirmation: "Show the selected policy, current state, and requested state before confirming.",
                authorization: "Privileged trusted session through the typed direct-seat input-policy provider.",
                audit: "Operator, gesture policy, old/new state, timestamp, and provider outcome.",
                recovery: "Retain the last confirmed policy and surface any direct-seat refusal without guessing.",
            },
            Self::ChangePowerProfile => ActionContract {
                impact: "Changes performance, thermals, battery life, or charging behavior.",
                confirmation: "Show profile, thermal/power impact, and restart requirement if any.",
                authorization: "Privileged trusted session through the typed System power provider.",
                audit: "Operator, old/new profile, timestamp, limits, and provider outcome.",
                recovery: "Fall back to the provider's safe profile and retain thermal watchdog protection.",
            },
            Self::ChangePlatformProfile => ActionContract {
                impact: "Changes the kernel-advertised performance/thermal policy and may affect battery life or fan behavior.",
                confirmation: "Show the current and requested kernel profile before confirming the change.",
                authorization: "Privileged trusted session through the typed fixed-root hardware provider.",
                audit: "Operator, old/new platform profile, timestamp, provider limits, and outcome.",
                recovery: "Retain the last confirmed profile; surface refusal and require a fresh provider read before retrying.",
            },
            Self::ConfigureHardware => ActionContract {
                impact: "May change firmware, dock, fan, device, or OEM hardware behavior.",
                confirmation: "Require explicit arming and show bounded safety limits before dispatch.",
                authorization: "Privileged trusted session plus capability-detected vendor/provider authorization.",
                audit: "Operator, typed capability, requested bounds, timestamp, and outcome.",
                recovery: "Watchdog to a safe profile; no raw sysfs, MSR, SMI, /dev/mem, or shell fallback.",
            },
        }
    }
}

const ACTION_CATALOG: [ThisNodeAction; 24] = [
    ThisNodeAction::RestartService,
    ThisNodeAction::ApplyUpdate,
    ThisNodeAction::ChangeConnectivity,
    ThisNodeAction::InspectConnectivityProfiles,
    ThisNodeAction::ToggleBluetooth,
    ThisNodeAction::ManageBluetoothDevices,
    ThisNodeAction::PowerSession,
    ThisNodeAction::ToggleWifi,
    ThisNodeAction::DisconnectNetworkLink,
    ThisNodeAction::ToggleDisplayOutput,
    ThisNodeAction::SetDisplayMode,
    ThisNodeAction::ArrangeDisplays,
    ThisNodeAction::AdjustDisplayBrightness,
    ThisNodeAction::ToggleAudioMute,
    ThisNodeAction::AdjustAudioVolume,
    ThisNodeAction::ToggleMicrophoneMute,
    ThisNodeAction::AdjustChargeLimit,
    ThisNodeAction::AdjustKeyboardBrightness,
    ThisNodeAction::AdjustPointerSpeed,
    ThisNodeAction::ToggleTapToClick,
    ThisNodeAction::ConfigureTouchGestures,
    ThisNodeAction::ChangePowerProfile,
    ThisNodeAction::ChangePlatformProfile,
    ThisNodeAction::ConfigureHardware,
];

/// Safety metadata travels with the finite action identifier. It is displayed
/// even when the provider is absent, so enabling a future writer cannot omit
/// the confirmation, trusted-session, audit, or recovery contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActionContract {
    impact: &'static str,
    confirmation: &'static str,
    authorization: &'static str,
    audit: &'static str,
    recovery: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActionProjection {
    action: ThisNodeAction,
    availability: CapabilityAvailability,
}

/// Directory presence tier → tone: online is healthy, idle warns, offline is a
/// danger, anything else reads dim.
fn presence_tone(presence: &str) -> Color32 {
    match presence {
        "online" => Style::OK,
        "idle" => Style::WARN,
        "offline" => Style::DANGER,
        _ => Style::TEXT_DIM,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ThisNodeView {
    #[default]
    Inventory,
    Detail(PageEntry),
    Actions,
}

// ──────────────────────────── the ThisNode state ────────────────────────────

/// The This Node plane's live state: the projected status plus the small IO
/// context to refresh it on the shared cadence.
pub(crate) struct ThisNodeState {
    /// The world-readable snapshot path (resolved once).
    snapshot_path: PathBuf,
    /// This node's locally-resolved hostname — the fallback `self` when the
    /// snapshot omits it (resolved once).
    local_host: String,
    /// The latest projection. Unseen until the first snapshot lands (drives the
    /// connecting state).
    status: NodeStatus,
    /// When the snapshot was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
    /// Bounded local history for the Performance detail chart. This is a
    /// presentation cache, not a second health or telemetry authority.
    telemetry_history: Vec<TelemetrySample>,
    /// Redacted provider outcomes consumed by the shared Events/Audit detail
    /// panes. This is a presentation cache; execution remains owned by System.
    action_audit: Vec<crate::system::ActionAuditRecord>,
    /// Off-thread fixed local systemd failure provider for Services continuity.
    services: LocalServiceProvider,
    /// Off-thread fixed locale/time-zone provider for OS-management continuity.
    locale: LocalLocaleProvider,
    /// Off-thread fixed CUPS printer inventory for Peripherals continuity.
    printers: LocalPrinterProvider,
    /// Off-thread fixed firewalld posture provider for Security continuity.
    security: LocalSecurityProvider,
    /// Off-thread metadata-only state-backup posture provider.
    backup: LocalBackupProvider,
    /// Off-thread bounded installed/running application mirror provider.
    applications: LocalApplicationProvider,
    /// The explicit top-level workflow selected in the This Node plane.
    view: ThisNodeView,
    /// A This Node power-profile request awaiting the required second click.
    /// The actual write is delegated to the typed System provider only after
    /// this local confirmation and a fresh provider-side offer check.
    pending_power_profile: Option<String>,
    /// A kernel platform-profile request awaiting visible confirmation.
    pending_platform_profile: Option<String>,
    /// The next Bluetooth radio state awaiting confirmation in Actions.
    pending_bluetooth_power: Option<bool>,
    /// The selected Bluetooth device verb awaiting confirmation.
    pending_bluetooth_device: Option<(String, crate::system::BluetoothDeviceAction)>,
    /// The host power/session verb awaiting confirmation.
    pending_power_session: Option<mde_seat::PowerVerb>,
    /// The next Wi-Fi radio state awaiting confirmation.
    pending_wifi_power: Option<bool>,
    /// The next connected-display enabled state awaiting confirmation.
    pending_display_output: Option<bool>,
    /// The provider path of a NetworkManager profile awaiting activation
    /// confirmation.
    pending_network_profile: Option<String>,
    /// The failed systemd unit awaiting restart confirmation.
    pending_service: Option<String>,
    /// The next display brightness percentage awaiting confirmation.
    pending_display_brightness: Option<u8>,
    /// The next master playback mute state awaiting confirmation.
    pending_audio_mute: Option<bool>,
    /// The next master playback volume awaiting confirmation.
    pending_audio_volume: Option<u8>,
    /// The next microphone mute state awaiting confirmation.
    pending_microphone_mute: Option<bool>,
    /// The next battery charge-stop cap awaiting confirmation.
    pending_charge_limit: Option<u8>,
    /// The next keyboard-backlight percentage awaiting confirmation.
    pending_keyboard_brightness: Option<u8>,
    /// The next pointer-speed policy awaiting confirmation.
    pending_pointer_speed: Option<i16>,
    /// The next touchpad tap-to-click state awaiting confirmation.
    pending_tap_to_click: Option<bool>,
    /// The provider path of the underlay link awaiting disconnect confirmation.
    pending_network_disconnect: Option<String>,
    /// The selected display mode awaiting confirmation, keyed by monitor id.
    pending_display_mode: Option<(String, mde_seat::DisplayMode)>,
    /// The selected display arrangement nudge awaiting confirmation.
    pending_display_arrangement: Option<(String, bool)>,
    /// The touch/gesture policy field and next state awaiting confirmation.
    pending_touch_gesture: Option<(crate::system::TouchGesturePolicy, bool)>,
}

impl Default for ThisNodeState {
    fn default() -> Self {
        Self {
            snapshot_path: PathBuf::from(SNAPSHOT_PATH),
            local_host: local_hostname(),
            status: NodeStatus::default(),
            last_poll: None,
            telemetry_history: Vec::new(),
            action_audit: Vec::new(),
            services: LocalServiceProvider::default(),
            locale: LocalLocaleProvider::default(),
            printers: LocalPrinterProvider::default(),
            security: LocalSecurityProvider::default(),
            backup: LocalBackupProvider::new(
                mackes_mesh_types::peers::default_workgroup_root()
                    .join(local_hostname())
                    .join("mackesd")
                    .join("state-backup.enc"),
            ),
            applications: LocalApplicationProvider::new(
                mackes_mesh_types::peers::default_workgroup_root()
                    .join(local_hostname())
                    .join("apps-installed.json"),
                mackes_mesh_types::peers::default_workgroup_root()
                    .join(local_hostname())
                    .join("running-apps.json"),
            ),
            view: ThisNodeView::Inventory,
            pending_power_profile: None,
            pending_platform_profile: None,
            pending_bluetooth_power: None,
            pending_bluetooth_device: None,
            pending_power_session: None,
            pending_wifi_power: None,
            pending_display_output: None,
            pending_network_profile: None,
            pending_service: None,
            pending_display_brightness: None,
            pending_audio_mute: None,
            pending_audio_volume: None,
            pending_microphone_mute: None,
            pending_charge_limit: None,
            pending_keyboard_brightness: None,
            pending_pointer_speed: None,
            pending_tap_to_click: None,
            pending_network_disconnect: None,
            pending_display_mode: None,
            pending_display_arrangement: None,
            pending_touch_gesture: None,
        }
    }
}

impl ThisNodeState {
    fn record_telemetry_sample(&mut self) {
        let sample = telemetry_sample(&self.status.telemetry);
        if sample.load_percent.is_some()
            || sample.memory_percent.is_some()
            || sample.root_percent.is_some()
        {
            if self.telemetry_history.len() == MAX_TELEMETRY_SAMPLES {
                self.telemetry_history.remove(0);
            }
            self.telemetry_history.push(sample);
        }
    }

    /// Whether the dedicated Actions workflow is selected. The unified shell
    /// uses the same durable state as the Workbench route so navigation cannot
    /// create a second action model.
    pub(crate) fn actions_selected(&self) -> bool {
        self.view == ThisNodeView::Actions
    }

    /// Select the dedicated Actions workflow or return to inventory.
    pub(crate) fn set_actions_selected(&mut self, selected: bool) {
        self.view = if selected {
            ThisNodeView::Actions
        } else {
            ThisNodeView::Inventory
        };
    }

    /// Select a catalog child route from the shell's hierarchy. The route is
    /// kept in the This Node plane so child pages render through the same
    /// snapshot, stale-state, and unavailable-provider authority as the
    /// inventory landing view.
    pub(crate) fn set_detail_page(&mut self, page: PageEntry) {
        self.view = ThisNodeView::Detail(page);
    }

    /// Render the dedicated Actions workflow from the unified shell using the
    /// live typed System provider.
    pub(crate) fn show_actions_with_system(
        &mut self,
        ui: &mut egui::Ui,
        system: &mut crate::system::SystemState,
    ) {
        show_actions_workflow_with_system(
            ui,
            &self.status,
            system,
            &self.services,
            &mut self.pending_power_profile,
            &mut self.pending_platform_profile,
            &mut self.pending_bluetooth_power,
            &mut self.pending_bluetooth_device,
            &mut self.pending_power_session,
            &mut self.pending_wifi_power,
            &mut self.pending_display_output,
            &mut self.pending_network_profile,
            &mut self.pending_service,
            &mut self.pending_display_brightness,
            &mut self.pending_audio_mute,
            &mut self.pending_audio_volume,
            &mut self.pending_microphone_mute,
            &mut self.pending_charge_limit,
            &mut self.pending_keyboard_brightness,
            &mut self.pending_pointer_speed,
            &mut self.pending_tap_to_click,
            &mut self.pending_network_disconnect,
            &mut self.pending_display_mode,
            &mut self.pending_display_arrangement,
            &mut self.pending_touch_gesture,
        );
        self.action_audit.extend(system.take_action_audit());
        const MAX_ACTION_AUDIT: usize = 32;
        if self.action_audit.len() > MAX_ACTION_AUDIT {
            let excess = self.action_audit.len() - MAX_ACTION_AUDIT;
            self.action_audit.drain(..excess);
        }
    }

    /// The poll seam: refresh the projection from the snapshot when the cadence has
    /// elapsed, then keep the repaint heartbeat alive so a heartbeat / service flip
    /// surfaces without input. Cheap enough to call every frame — it self-gates. A
    /// missing / unreadable snapshot retains a previously valid projection as
    /// stale; before the first valid snapshot it yields the unseen status.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        self.services.poll();
        self.locale.poll();
        self.printers.poll();
        self.security.poll();
        self.backup.poll();
        self.applications.poll();
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            match read_bounded_snapshot(&self.snapshot_path) {
                Some(snapshot) => {
                    let projected = NodeStatus::project(&snapshot, &self.local_host);
                    if projected.seen || !self.status.seen {
                        self.status = projected;
                        self.record_telemetry_sample();
                        if let Some(age_ms) =
                            snapshot_age_ms(self.status.generated_ms, unix_epoch_ms())
                        {
                            if age_ms > MAX_SNAPSHOT_AGE_MS {
                                self.status.mark_stale(format!(
                                    "The mesh-status snapshot is {} seconds old; retained values may be outdated.",
                                    age_ms / 1_000
                                ));
                            }
                        }
                    } else {
                        self.status.mark_stale(
                            "The latest mesh-status snapshot was malformed; retained values are stale.",
                        );
                    }
                }
                None if self.status.seen => self.status.mark_stale(
                    "The mesh-status provider is unavailable; retained values are stale.",
                ),
                None => self.status = NodeStatus::default(),
            }
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Render the plane's live content into `ui`.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        self.show_with_system(ui, None);
    }

    /// Render the plane with the live System provider available. The Workbench
    /// and the unified shell both use this seam; callers without the provider
    /// retain the honest read-only rendering above.
    pub(crate) fn show_with_system(
        &mut self,
        ui: &mut egui::Ui,
        mut system: Option<&mut crate::system::SystemState>,
    ) {
        let mut view = self.view;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Provider snapshot")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.separator();
            if ui
                .selectable_label(view == ThisNodeView::Inventory, "Inventory")
                .clicked()
            {
                view = ThisNodeView::Inventory;
            }
            if ui
                .selectable_label(view == ThisNodeView::Actions, "Actions")
                .clicked()
            {
                view = ThisNodeView::Actions;
            }
            ui.menu_button("Open detail", |ui| {
                for page in page_index().iter().copied() {
                    if ui
                        .selectable_label(view == ThisNodeView::Detail(page), page.label)
                        .clicked()
                    {
                        view = ThisNodeView::Detail(page);
                        ui.close_menu();
                    }
                }
            });
            if ui.button("Refresh now").clicked() {
                self.last_poll = None;
                self.poll(ui.ctx());
            }
        });
        ui.add_space(Style::SP_XS);
        self.view = view;
        match view {
            ThisNodeView::Inventory => {
                if let Some(page) = show_status(ui, &self.status, self.local_freshness_summary()) {
                    self.view = ThisNodeView::Detail(page);
                }
            }
            ThisNodeView::Detail(page) => show_section_detail(
                ui,
                &self.status,
                page,
                system.as_deref_mut(),
                &self.telemetry_history,
                &self.action_audit,
                &self.services,
                &self.locale,
                &self.printers,
                &self.security,
                &self.backup,
                &self.applications,
            ),
            ThisNodeView::Actions => match system {
                Some(system) => self.show_actions_with_system(ui, system),
                None => show_actions_workflow(ui, &self.status),
            },
        }
    }

    fn local_freshness_summary(&self) -> LocalFreshnessSummary {
        let mut summary = LocalFreshnessSummary::default();
        summary.add(&self.services.freshness);
        summary.add(&self.locale.freshness);
        summary.add(&self.printers.freshness);
        summary.add(&self.backup.freshness);
        summary.add(&self.applications.freshness);
        summary.add(&LocalFreshness {
            last_success: self.security.firewall_last_success,
        });
        summary.add(&LocalFreshness {
            last_success: self.security.encryption_last_success,
        });
        summary
    }
}

/// The local hostname — `$HOSTNAME` → `/proc/sys/kernel/hostname` (what the
/// snapshot generator stamps as `self`) → `/etc/hostname` → `"localhost"`. Only a
/// fallback: the snapshot's own `self` marker is preferred.
fn local_hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(h) = std::fs::read_to_string(path) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    "localhost".to_string()
}

// ──────────────────────────── render ────────────────────────────

/// Render this node's live status: a compact inventory-first landing surface with
/// progressive disclosure into the governed provider detail routes.
fn show_status(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    local_freshness: LocalFreshnessSummary,
) -> Option<PageEntry> {
    let mut selected = None;
    if !status.seen {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(Style::SP_S);
                ui.colored_label(Style::TEXT_DIM, "Reading this node's status…");
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(
                        "This node's role, overlay address, and daemon health fold from the \
                         world-readable mesh-status snapshot.",
                    )
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
                );
                selected = show_inventory_summary(ui, status, local_freshness);
                ui.add_space(Style::SP_S);
                selected = selected.or_else(|| show_section_hierarchy(ui, status));
                show_capability_surface(ui, status);
            });
        return selected;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if status.stale {
                mde_egui::card().show(ui, |ui| {
                    ui.colored_label(Style::WARN, "This Node status is stale");
                    ui.label(
                        RichText::new(
                            status
                                .stale_reason
                                .as_deref()
                                .unwrap_or("The provider did not return a fresh snapshot."),
                        )
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                    );
                });
                ui.add_space(Style::SP_S);
            }
            selected = show_inventory_summary(ui, status, local_freshness);
            ui.add_space(Style::SP_S);
            selected = selected.or_else(|| show_section_hierarchy(ui, status));
            ui.add_space(Style::SP_S);
            mde_egui::muted_note(
                ui,
                "Select a row or use Open detail for the full provider view. The landing page stays compact and inventory-first at every size.",
            );
            show_capability_surface(ui, status);
    });
    selected
}

/// Full-page detail view for one governed section. The detail route is a view
/// over the same inventory snapshot authority; it does not create a second
/// provider model.
fn show_section_detail(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    page: PageEntry,
    system: Option<&mut crate::system::SystemState>,
    telemetry_history: &[TelemetrySample],
    action_audit: &[crate::system::ActionAuditRecord],
    services: &LocalServiceProvider,
    locale: &LocalLocaleProvider,
    printers: &LocalPrinterProvider,
    security: &LocalSecurityProvider,
    backup: &LocalBackupProvider,
    applications: &LocalApplicationProvider,
) {
    let section = page.section;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            mde_egui::card().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(page.label).strong());
                    if status.stale {
                        ui.colored_label(Style::WARN, "refresh required");
                    }
                });
                ui.label(
                    RichText::new(section.description())
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                if let Some(reason) = page.unavailable_reason() {
                    ui.label(
                        RichText::new(reason)
                            .color(Style::TEXT_DIM)
                            .size(Style::SMALL),
                    );
                }
                ui.label(
                    RichText::new(page.route)
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
            });
            ui.add_space(Style::SP_S);
            if page.route == "this-node/recovery-reset" {
                show_recovery_reset_boundary(ui);
                return;
            }
            if page.unavailable_reason().is_some() {
                show_unavailable_page(ui, page);
                return;
            }
            match page.route {
                "this-node/services" => {
                    mde_egui::card().show(ui, |ui| show_services(ui, status));
                    ui.add_space(Style::SP_S);
                    mde_egui::card().show(ui, |ui| show_local_services(ui, services));
                    ui.add_space(Style::SP_S);
                    mde_egui::card().show(ui, |ui| show_local_applications(ui, applications));
                    mde_egui::muted_note(
                        ui,
                        "Service rows are read-only snapshot facts; restart remains behind the typed Actions provider boundary.",
                    );
                }
                "this-node/storage" => {
                    mde_egui::card().show(ui, |ui| {
                        show_storage_detail(ui, status, system.as_deref());
                    });
                }
                "this-node/peripherals" => {
                    mde_egui::card().show(ui, |ui| show_local_printers(ui, printers));
                    if let Some(system) = system.as_deref() {
                        ui.add_space(Style::SP_S);
                        mde_egui::card().show(ui, |ui| show_local_hardware_inventory(ui, system));
                    }
                    mde_egui::muted_note(
                        ui,
                        "Printer jobs, queue changes, USB authorization, and dock writes remain behind typed providers with confirmation, audit, and recovery.",
                    );
                }
                "this-node/updates" => {
                    let session = crate::lifecycle_session::load_lifecycle_session();
                    mde_egui::card().show(ui, |ui| {
                        crate::lifecycle_session::show_lifecycle_session(ui, session.as_ref());
                    });
                    ui.add_space(Style::SP_S);
                    mde_egui::card().show(ui, |ui| show_update_posture(ui, status));
                    mde_egui::muted_note(
                        ui,
                        "Version and update posture are visible from the snapshot; applying an update still requires a typed provider, confirmation, audit, and recovery path.",
                    );
                }
                "this-node/virtualization" => {
                    if let Some(system) = system.as_deref() {
                        mde_egui::card().show(ui, |ui| show_remote_access_posture(ui, system));
                    } else {
                        show_provider_continuity_unavailable(
                            ui,
                            "Virtualization & Remote Access",
                            "The durable System provider is not mounted in this view.",
                        );
                    }
                }
                "this-node/backup-restore" => {
                    mde_egui::card().show(ui, |ui| show_local_backup_posture(ui, backup));
                    mde_egui::muted_note(
                        ui,
                        "This Node reads only backup metadata. Verification and restore remain privileged, passphrase-gated mackesd operations and are not executed from the UI.",
                    );
                }
                "this-node/users" => {
                    mde_egui::card().show(ui, |ui| show_users(ui, status));
                    mde_egui::muted_note(
                        ui,
                        "Account and role changes remain unavailable until a typed, admin-authorized identity provider supplies confirmation, audit, and recovery contracts.",
                    );
                }
                "this-node/security-privacy" => {
                    mde_egui::card().show(ui, |ui| show_security_privacy(ui, status));
                    ui.add_space(Style::SP_S);
                    mde_egui::card().show(ui, |ui| show_local_security_posture(ui, security));
                    if let Some(system) = system.as_deref() {
                        ui.add_space(Style::SP_S);
                        mde_egui::card().show(ui, |ui| show_local_privacy_inventory(ui, system));
                    }
                }
                "this-node/accessibility" => {
                    if let Some(system) = system.as_deref() {
                        mde_egui::card().show(ui, |ui| show_accessibility(ui, system));
                    } else {
                        show_provider_continuity_unavailable(
                            ui,
                            "Accessibility preferences",
                            "The durable System provider is not mounted in this view.",
                        );
                    }
                }
                "this-node/time-language-region" => {
                    if let Some(system) = system.as_deref() {
                        mde_egui::card().show(ui, |ui| {
                            show_time_language_region(ui, system, locale);
                        });
                    } else {
                        show_provider_continuity_unavailable(
                            ui,
                            "Time, language & region",
                            "The durable System provider is not mounted in this view.",
                        );
                    }
                }
                "this-node/system" => {
                    mde_egui::card().show(ui, |ui| show_mesh(ui, status));
                    ui.add_space(Style::SP_S);
                    mde_egui::card().show(ui, |ui| show_power_profile(ui, status));
                }
                _ => match section {
                Section::Overview => {
                    mde_egui::card().show(ui, |ui| show_identity(ui, status));
                    ui.add_space(Style::SP_S);
                    mde_egui::card().show(ui, |ui| show_services(ui, status));
                    ui.add_space(Style::SP_S);
                    mde_egui::card().show(ui, |ui| show_mesh(ui, status));
                }
                Section::Connectivity => {
                    mde_egui::card().show(ui, |ui| show_connectivity(ui, status));
                    if let Some(system) = system.as_deref() {
                        ui.add_space(Style::SP_S);
                        mde_egui::card().show(ui, |ui| show_local_network_inventory(ui, system));
                        ui.add_space(Style::SP_S);
                        mde_egui::card().show(ui, |ui| show_local_bluetooth_inventory(ui, system));
                    }
                }
                Section::DisplaySound => {
                    mde_egui::card().show(ui, |ui| {
                        show_display(ui, status);
                        ui.add_space(Style::SP_S);
                        show_audio(ui, status);
                    });
                    if let Some(system) = system.as_deref() {
                        ui.add_space(Style::SP_S);
                        mde_egui::card().show(ui, |ui| show_local_display_inventory(ui, system));
                        ui.add_space(Style::SP_S);
                        mde_egui::card().show(ui, |ui| show_local_mixer_inventory(ui, system));
                    }
                }
                Section::PowerPerformance => {
                    mde_egui::card().show(ui, |ui| show_power_profile(ui, status));
                    ui.add_space(Style::SP_S);
                    if let Some(system) = system.as_deref() {
                        mde_egui::card().show(ui, |ui| show_local_power_telemetry(ui, system));
                        ui.add_space(Style::SP_S);
                    }
                    mde_egui::card().show(ui, |ui| {
                        show_telemetry(ui, status);
                        ui.add_space(Style::SP_S);
                        show_telemetry_chart(ui, telemetry_history);
                    });
                    ui.add_space(Style::SP_S);
                    mde_egui::card().show(ui, |ui| show_hardware(ui, status));
                }
                Section::Hardware => {
                    mde_egui::card().show(ui, |ui| show_telemetry(ui, status));
                    ui.add_space(Style::SP_S);
                    mde_egui::card().show(ui, |ui| show_hardware(ui, status));
                    if let Some(system) = system.as_deref() {
                        ui.add_space(Style::SP_S);
                        mde_egui::card().show(ui, |ui| {
                            ui.label(RichText::new("Trusted local device probes").strong());
                            ui.label(
                                RichText::new(
                                    "These inventories come from the typed seat provider. Names and modes are observations only; no raw sysfs paths or firmware controls are exposed here.",
                                )
                                .color(Style::TEXT_DIM)
                                .size(Style::SMALL),
                            );
                            ui.add_space(Style::SP_XS);
                            show_local_display_inventory(ui, system);
                            ui.add_space(Style::SP_S);
                            show_local_keyboard_inventory(ui, system);
                            ui.add_space(Style::SP_S);
                            show_local_hardware_inventory(ui, system);
                        });
                    } else {
                        show_provider_continuity_unavailable(
                            ui,
                            "Trusted local device probes",
                            "The System provider is not mounted in this view; local connector and input inventory remain unavailable.",
                        );
                    }
                    ui.add_space(Style::SP_S);
                    show_capability_surface(ui, status);
                    mde_egui::muted_note(
                        ui,
                        "Thermal zones, fans, firmware, docks, and non-root storage inventory remain unavailable until bounded typed providers publish facts. Hardware actions remain fail-closed until authorization, limits, audit, and recovery are connected.",
                    );
                }
                Section::Input => {
                    mde_egui::card().show(ui, |ui| show_input(ui, status));
                    if let Some(system) = system.as_deref() {
                        ui.add_space(Style::SP_S);
                        mde_egui::card().show(ui, |ui| show_local_keyboard_inventory(ui, system));
                        ui.add_space(Style::SP_S);
                        mde_egui::card().show(ui, |ui| show_local_input_policy(ui, system));
                    }
                    mde_egui::muted_note(
                        ui,
                        "Input policy mutations remain unavailable until a typed direct-seat provider publishes authorization, OSD, and recovery contracts.",
                    );
                }
                Section::Personalization => {
                    mde_egui::card().show(ui, |ui| {
                        ui.label(RichText::new("Personalization provider").strong());
                        ui.label(
                            RichText::new(
                                "Theme, wallpaper, layout, text scale, motion, and clock preferences are owned by the typed System provider and remain durable across This Node navigation.",
                            )
                            .color(Style::TEXT_DIM)
                            .size(Style::SMALL),
                        );
                        ui.label(
                            RichText::new(
                                "Select Personalization in the This Node hierarchy to open the existing provider-backed controls.",
                            )
                            .color(Style::TEXT_DIM)
                            .size(Style::SMALL),
                        );
                    });
                }
                Section::MeshSystem => {
                    mde_egui::card().show(ui, |ui| show_mesh(ui, status));
                    ui.add_space(Style::SP_S);
                    mde_egui::card().show(ui, |ui| show_power_profile(ui, status));
                }
                },
            }
            show_detail_anatomy(ui, status, page, telemetry_history, action_audit);
        });
}

/// Shared detail anatomy for every governed page. These disclosures keep the
/// operator's mental model consistent without inventing event, audit, or
/// vendor-pack records when their providers have not published them.
fn show_detail_anatomy(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    page: PageEntry,
    telemetry_history: &[TelemetrySample],
    action_audit: &[crate::system::ActionAuditRecord],
) {
    ui.add_space(Style::SP_S);
    ui.label(RichText::new("Detail surfaces").strong());

    egui::CollapsingHeader::new("Actions")
        .id_salt(("this-node-detail-actions", page.route))
        .default_open(false)
        .show(ui, |ui| {
            ui.label(
                RichText::new(
                    "Use the dedicated Actions workspace for typed, confirmation-gated mutations.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            if status.stale {
                ui.colored_label(
                    Style::WARN,
                    "Actions remain unavailable while this detail is stale.",
                );
            }
        });

    egui::CollapsingHeader::new("Events")
        .id_salt(("this-node-detail-events", page.route))
        .default_open(false)
        .show(ui, |ui| {
            show_action_events(ui, action_audit);
        });

    egui::CollapsingHeader::new("Performance")
        .id_salt(("this-node-detail-performance", page.route))
        .default_open(false)
        .show(ui, |ui| {
            show_telemetry(ui, status);
            ui.add_space(Style::SP_S);
            show_telemetry_chart(ui, telemetry_history);
        });

    egui::CollapsingHeader::new("Audit")
        .id_salt(("this-node-detail-audit", page.route))
        .default_open(false)
        .show(ui, |ui| {
            show_action_audit(ui, action_audit);
        });

    egui::CollapsingHeader::new("Vendor packs")
        .id_salt(("this-node-detail-vendor-packs", page.route))
        .default_open(false)
        .show(ui, |ui| {
            show_vendor_packs(ui, status);
        });
}

fn show_vendor_packs(ui: &mut egui::Ui, status: &NodeStatus) {
    if status.vendor_packs.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "No vendor-pack manifests published");
        ui.label(
            RichText::new(
                "Vendor-specific controls remain separate from standard hardware controls and require a versioned manifest, capability detection, trusted authorization, safety limits, audit, and recovery contract.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }
    ui.colored_label(Style::TEXT_DIM, "Published vendor-pack manifests");
    for pack in &status.vendor_packs {
        egui::CollapsingHeader::new(format!("{} · {}", pack.name, pack.version))
            .id_salt(("this-node-vendor-pack", &pack.name))
            .default_open(false)
            .show(ui, |ui| {
                let tone = match pack.status {
                    "installed" => Style::OK,
                    "outdated" => Style::WARN,
                    _ => Style::TEXT_DIM,
                };
                ui.colored_label(tone, pack.status);
                if pack.capabilities.is_empty() {
                    ui.colored_label(Style::TEXT_DIM, "No capabilities published");
                } else {
                    ui.label(format!("Capabilities: {}", pack.capabilities.join(", ")));
                }
                ui.label(
                    RichText::new(
                        "Manifest metadata does not create executable routes or actions; vendor writes use the same typed authorization, safety, audit, and recovery boundary as platform controls.",
                    )
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
                );
            });
    }
}

fn show_action_events(ui: &mut egui::Ui, records: &[crate::system::ActionAuditRecord]) {
    if records.is_empty() {
        ui.colored_label(
            Style::TEXT_DIM,
            "No provider outcomes recorded in this visit",
        );
        ui.label(
            RichText::new(
                "Events will remain empty until a typed action or bounded event provider publishes an outcome.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }
    ui.colored_label(Style::TEXT_DIM, "Recent typed-provider outcomes");
    for record in records.iter().rev().take(12) {
        egui::CollapsingHeader::new(format!("{} · {}", record.action, record.outcome))
            .id_salt(("this-node-event", record.occurred_ms))
            .default_open(false)
            .show(ui, |ui| {
                mde_egui::field(
                    ui,
                    "Timestamp",
                    &format_audit_timestamp(record.occurred_ms),
                    Style::TEXT,
                );
                mde_egui::field(ui, "Outcome", record.outcome, Style::TEXT);
            });
    }
}

fn show_action_audit(ui: &mut egui::Ui, records: &[crate::system::ActionAuditRecord]) {
    if records.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "No local action audit records yet");
        ui.label(
            RichText::new(
                "Typed actions record operator scope, timestamp, and provider outcome after a real dispatch.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }
    ui.colored_label(Style::TEXT_DIM, "Redacted local action history");
    if ui.button("Export redacted history").clicked() {
        match export_redacted_audit(records) {
            Ok(path) => ui.colored_label(
                Style::OK,
                format!("Exported audit evidence to {}", path.display()),
            ),
            Err(error) => ui.colored_label(Style::WARN, format!("Audit export failed: {error}")),
        };
    }
    for record in records.iter().rev().take(12) {
        egui::CollapsingHeader::new(record.action)
            .id_salt(("this-node-audit", record.occurred_ms))
            .default_open(false)
            .show(ui, |ui| {
                mde_egui::field(ui, "Operator", "local trusted session", Style::TEXT);
                mde_egui::field(ui, "Timestamp", &format_audit_timestamp(record.occurred_ms), Style::TEXT);
                mde_egui::field(ui, "Outcome", record.outcome, Style::TEXT);
                ui.label(
                    RichText::new("Provider paths, device identities, credentials, and raw errors are intentionally redacted.")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
            });
    }
}

/// Write the bounded audit history to a deterministic user-state location.
/// The payload contains only the action label, accepted/refused outcome, and
/// timestamp; provider paths, device identities, credentials, and raw errors
/// are never serialized.
fn export_redacted_audit(records: &[crate::system::ActionAuditRecord]) -> std::io::Result<PathBuf> {
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local").join("state"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let directory = root.join("mde").join("this-node");
    std::fs::create_dir_all(&directory)?;
    let stamp = unix_epoch_ms();
    let path = directory.join(format!("audit-{stamp}.json"));
    let temporary = directory.join(format!(".audit-{stamp}.json.tmp"));
    let payload = redacted_audit_payload(records);
    let encoded = serde_json::to_vec_pretty(&payload).map_err(std::io::Error::other)?;
    std::fs::write(&temporary, encoded)?;
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}

fn redacted_audit_payload(records: &[crate::system::ActionAuditRecord]) -> serde_json::Value {
    serde_json::json!({
        "schema": "mde.this-node.audit.v1",
        "operator": "local trusted session",
        "records": records.iter().map(|record| serde_json::json!({
            "action": record.action,
            "outcome": record.outcome,
            "occurred_ms": record.occurred_ms,
        })).collect::<Vec<_>>(),
    })
}

fn format_audit_timestamp(milliseconds: u64) -> String {
    format!("{milliseconds} ms since Unix epoch (UTC)")
}

fn show_unavailable_page(ui: &mut egui::Ui, page: PageEntry) {
    mde_egui::card().show(ui, |ui| {
        ui.label(RichText::new(format!("{} provider", page.label)).strong());
        ui.colored_label(
            Style::TEXT_DIM,
            page.unavailable_reason()
                .unwrap_or("This provider is not connected to This Node."),
        );
        ui.label(
            RichText::new(
                "No control is reported as successful until a typed provider publishes its capability, authorization, and recovery contract.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
    });
}

fn show_recovery_reset_boundary(ui: &mut egui::Ui) {
    mde_egui::card().show(ui, |ui| {
        ui.label(RichText::new("Recovery & Reset provider").strong());
        ui.colored_label(Style::WARN, "Provider unavailable");
        ui.label(
            RichText::new(
                "This Node does not expose reset, recovery-environment, rollback, or destructive restoration controls without a privileged typed provider.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        ui.label(
            RichText::new(
                "Existing encrypted backup metadata remains visible in Backup & Restore; verification and restore stay passphrase-gated mackesd operations. No recovery action is presented as successful here.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        ui.colored_label(Style::TEXT_DIM, "Recovery guidance: use the approved privileged provider or corrected-forward re-enrollment path.");
    });
}

fn show_local_services(ui: &mut egui::Ui, services: &LocalServiceProvider) {
    ui.label(RichText::new("Local service provider").strong());
    show_local_freshness(ui, services.freshness.last_success);
    match services.state {
        Err(error) => {
            ui.colored_label(Style::WARN, error);
            ui.label(
                RichText::new(
                    "No service state is inferred when the fixed provider is absent or stale.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
        }
        Ok(()) if services.failed.is_empty() => {
            ui.colored_label(Style::OK, "No failed system services reported");
            ui.label(RichText::new("Read-only systemd evidence is bounded to 32 service names and refreshed independently from mesh health.").color(Style::TEXT_DIM).size(Style::SMALL));
        }
        Ok(()) => {
            ui.colored_label(
                Style::WARN,
                format!("{} failed system service(s)", services.failed.len()),
            );
            for name in &services.failed {
                ui.label(RichText::new(name).size(Style::SMALL));
            }
            ui.label(RichText::new("Restart remains behind the typed Actions provider with confirmation, audit, and recovery contracts.").color(Style::TEXT_DIM).size(Style::SMALL));
        }
    }
}

fn show_local_applications(ui: &mut egui::Ui, applications: &LocalApplicationProvider) {
    ui.label(RichText::new("Local application mirror").strong());
    show_local_freshness(ui, applications.freshness.last_success);
    match applications.facts {
        Err(error) => {
            ui.colored_label(Style::WARN, error);
            ui.label(RichText::new("Installed and running application counts remain unknown until the bounded mackesd mirror responds.").color(Style::TEXT_DIM).size(Style::SMALL));
        }
        Ok(ApplicationFacts {
            installed: None,
            running: None,
        }) => {
            ui.colored_label(Style::TEXT_DIM, "Application mirror not published");
            ui.label(
                RichText::new(
                    "No installed or running application facts are inferred from an absent mirror.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
        }
        Ok(ApplicationFacts { installed, running }) => {
            mde_egui::field(
                ui,
                "Installed launchable apps",
                &installed.map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                Style::TEXT,
            );
            mde_egui::field(
                ui,
                "Running app ids",
                &running.map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                Style::TEXT,
            );
            ui.label(RichText::new("Counts come from the local mackesd mirror; names, launch targets, and app actions remain in the existing Front Door authority.").color(Style::TEXT_DIM).size(Style::SMALL));
        }
    }
}

fn show_local_printers(ui: &mut egui::Ui, printers: &LocalPrinterProvider) {
    ui.label(RichText::new("Local printers").strong());
    show_local_freshness(ui, printers.freshness.last_success);
    match printers.state {
        Err(error) => {
            ui.colored_label(Style::WARN, error);
            ui.label(
                RichText::new(
                    "No printer is inferred when the fixed CUPS provider is absent or stale.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
        }
        Ok(()) if printers.printers.is_empty() => {
            ui.colored_label(Style::TEXT_DIM, "No local printers reported");
            ui.label(
                RichText::new("CUPS answered successfully; printer inventory is empty.")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        }
        Ok(()) => {
            for printer in &printers.printers {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&printer.name).strong().size(Style::SMALL));
                    ui.colored_label(Style::TEXT_DIM, &printer.state);
                    if printers.default.as_deref() == Some(printer.name.as_str()) {
                        ui.colored_label(Style::ACCENT, "default");
                    }
                });
            }
            ui.label(RichText::new("Names and status stay on the trusted local seat; no printer URI, job content, or credentials cross the mesh snapshot.").color(Style::TEXT_DIM).size(Style::SMALL));
        }
    }
}

fn show_local_security_posture(ui: &mut egui::Ui, security: &LocalSecurityProvider) {
    ui.label(RichText::new("Trusted local firewall posture").strong());
    show_local_freshness(ui, security.firewall_last_success);
    match security.firewall {
        Ok(FirewallState::Running) => {
            ui.colored_label(Style::OK, "firewalld running");
            ui.label(RichText::new("This is a read-only firewalld state probe; zone/rule details and writes remain provider-gated.").color(Style::TEXT_DIM).size(Style::SMALL));
        }
        Ok(FirewallState::NotRunning) => {
            ui.colored_label(Style::WARN, "firewalld not running");
            ui.label(
                RichText::new("No broader firewall posture is inferred from this single provider.")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        }
        Err(error) => {
            ui.colored_label(Style::WARN, error);
            ui.label(
                RichText::new(
                    "Firewall state remains unknown until the fixed local provider responds.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
        }
    }
    ui.separator();
    ui.label(RichText::new("Observed encryption mappings").strong());
    show_local_freshness(ui, security.encryption_last_success);
    match security.encryption {
        Ok(EncryptionState::Observed {
            mappings,
            encrypted,
        }) => {
            let tone = if encrypted == mappings {
                Style::OK
            } else {
                Style::WARN
            };
            ui.colored_label(
                tone,
                format!("{encrypted} of {mappings} device-mapper mapping(s) identify as LUKS"),
            );
            ui.label(RichText::new("This bounded observation does not prove full-disk coverage, unlocked state, or passphrase validity; no mapping names or keys leave the local seat.").color(Style::TEXT_DIM).size(Style::SMALL));
        }
        Ok(EncryptionState::NoMappings) => {
            ui.colored_label(Style::TEXT_DIM, "No device-mapper mappings observed");
            ui.label(
                RichText::new(
                    "No encryption posture is inferred when no local mapping is published.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
        }
        Err(error) => {
            ui.colored_label(Style::WARN, error);
            ui.label(
                RichText::new(
                    "Encryption posture remains unknown until the fixed local provider responds.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
        }
    }
}

fn show_remote_access_posture(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    ui.label(RichText::new("Remote Proofing policy").strong());
    for fact in system.remote_proofing_summary() {
        let tone = if fact.starts_with("Warning:") {
            Style::WARN
        } else {
            Style::TEXT
        };
        ui.colored_label(tone, RichText::new(fact).size(Style::SMALL));
    }
    ui.label(
        RichText::new(
            "This Node reuses the persisted System policy and derived service plan. Pairing, lifecycle, trusted-session approval, and remote input remain owned by the existing System/VDI providers; no second remote-access control surface is created here.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_local_backup_posture(ui: &mut egui::Ui, backup: &LocalBackupProvider) {
    ui.label(RichText::new("Encrypted state-backup posture").strong());
    show_local_freshness(ui, backup.freshness.last_success);
    match backup.state {
        Ok(BackupState::Present { bytes, modified_ms }) => {
            ui.colored_label(Style::OK, "Encrypted backup artifact present");
            ui.horizontal_wrapped(|ui| {
                mde_egui::field(ui, "Bundle size", &format!("{bytes} bytes"), Style::TEXT);
                mde_egui::field(
                    ui,
                    "Modified",
                    &format_audit_timestamp(modified_ms),
                    Style::TEXT,
                );
            });
            ui.label(RichText::new("Contents remain opaque to the desktop; presence does not prove passphrase validity or restore readiness.").color(Style::TEXT_DIM).size(Style::SMALL));
        }
        Ok(BackupState::Missing) => {
            ui.colored_label(Style::WARN, "No encrypted backup artifact found");
            ui.label(RichText::new("The mackesd backup worker may be disabled, unmounted, or not yet scheduled; no restore action is offered.").color(Style::TEXT_DIM).size(Style::SMALL));
        }
        Err(error) => {
            ui.colored_label(Style::WARN, error);
            ui.label(RichText::new("Backup posture remains unknown; the UI does not inspect encrypted contents or follow symlinks.").color(Style::TEXT_DIM).size(Style::SMALL));
        }
    }
}

/// Inventory-first landing surface for WL-UX-011. This is intentionally a
/// compact read-only index: each row names a governed node area and summarizes
/// only inventory/configuration facts already present in the bounded projection.
/// Detail views remain below this summary.
fn show_inventory_summary(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    local_freshness: LocalFreshnessSummary,
) -> Option<PageEntry> {
    let mut selected = None;
    mde_egui::card().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Node inventory").strong());
            if status.stale {
                ui.colored_label(Style::WARN, "refresh required");
            }
        });
        ui.label(
            RichText::new(
                "Select a governed area below to inspect its provider state and available controls.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Local providers").strong().size(Style::SMALL));
            ui.colored_label(
                Style::OK,
                format!("{} fresh", local_freshness.fresh),
            );
            if local_freshness.stale > 0 {
                ui.colored_label(
                    Style::WARN,
                    format!("{} stale", local_freshness.stale),
                );
            }
            if local_freshness.awaiting > 0 {
                ui.colored_label(
                    Style::TEXT_DIM,
                    format!("{} awaiting provider", local_freshness.awaiting),
                );
            }
        });

        let mut rows = DenseList::new();
        for section in Section::ALL {
            rows.row(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .selectable_label(false, RichText::new(section.label()).strong().size(Style::SMALL))
                        .clicked()
                    {
                        selected = page_for_section(section);
                    }
                    ui.label(
                        RichText::new(inventory_summary(section, status))
                            .color(Style::TEXT_DIM)
                            .size(Style::SMALL),
                    );
                });
            });
        }
    });
    selected
}

fn page_for_section(section: Section) -> Option<PageEntry> {
    page_index()
        .iter()
        .copied()
        .find(|page| page.section == section)
}

fn inventory_summary(section: Section, status: &NodeStatus) -> String {
    match section {
        Section::Overview => {
            let role = status.role.as_deref().unwrap_or("role unavailable");
            let presence = status.presence.as_deref().unwrap_or("presence unavailable");
            format!("{} · {}", role, presence)
        }
        Section::Connectivity => status
            .connectivity
            .interface
            .as_deref()
            .map(|interface| format!("interface {interface}"))
            .unwrap_or_else(|| "provider facts unavailable".to_owned()),
        Section::DisplaySound => {
            if status.display.connected.is_some() || status.display.backlight_percent.is_some() {
                let connected = status
                    .display
                    .connected
                    .map_or_else(|| "?".to_owned(), |value| value.to_string());
                format!("{connected} display(s) connected · audio/provider facts below")
            } else if status.audio == AudioFacts::default() {
                "display and audio provider facts unavailable".to_owned()
            } else {
                let playback = status
                    .audio
                    .playback
                    .map(|available| if available { "ready" } else { "unavailable" })
                    .unwrap_or("unknown");
                let capture = status
                    .audio
                    .capture
                    .map(|available| if available { "ready" } else { "unavailable" })
                    .unwrap_or("unknown");
                format!("playback {playback} · capture {capture}")
            }
        }
        Section::Input => status
            .input
            .event_devices
            .map(|count| format!("{count} event device(s) observed · policy read-only"))
            .unwrap_or_else(|| "direct-seat input inventory unavailable".to_owned()),
        Section::PowerPerformance => {
            let profile = status
                .power_profile
                .active
                .as_deref()
                .map(|profile| format!("profile {profile}"));
            let telemetry = status
                .telemetry
                .load_1m_milli
                .map(|load| format!("load {}.{:02}", load / 1000, (load % 1000) / 10));
            match (profile, telemetry) {
                (Some(profile), Some(telemetry)) => format!("{profile} · {telemetry}"),
                (Some(profile), None) => profile,
                (None, Some(telemetry)) => telemetry,
                (None, None) => "power and resource telemetry unavailable".to_owned(),
            }
        }
        Section::Hardware => {
            if let Some(percent) = status.telemetry.root_used_percent {
                format!("root storage {percent}% used · other inventory unavailable")
            } else {
                "device, firmware, storage, and dock inventory unavailable".to_owned()
            }
        }
        Section::Personalization => section
            .unavailable_reason()
            .unwrap_or("appearance and local preferences")
            .to_owned(),
        Section::MeshSystem => match (&status.peer_counts, &status.leader) {
            (Some((online, total)), Some(leader)) => {
                format!("{online}/{total} peers · leader {leader}")
            }
            (Some((online, total)), None) => format!("{online}/{total} peers · leader unavailable"),
            _ => "mesh context unavailable".to_owned(),
        },
    }
}

fn show_section_hierarchy(ui: &mut egui::Ui, _status: &NodeStatus) -> Option<PageEntry> {
    let mut selected = None;
    ui.label(RichText::new("This Node hierarchy").strong());
    ui.label(
        RichText::new(
            "Expand a governed section to inspect its current inventory and configuration providers.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
    ui.add_space(Style::SP_XS);

    for group in SectionGroup::ALL {
        egui::CollapsingHeader::new(group.label())
            .id_salt(("this-node-hierarchy-v2", group.label()))
            .default_open(false)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(group.description())
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                let mut rows = DenseList::new();
                for section in group.sections() {
                    rows.row(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .selectable_label(
                                    false,
                                    RichText::new(section.label()).strong().size(Style::SMALL),
                                )
                                .clicked()
                            {
                                selected = page_for_section(*section);
                            }
                            ui.label(
                                RichText::new(section.description())
                                    .color(Style::TEXT_DIM)
                                    .size(Style::SMALL),
                            );
                        });
                    });
                }
            });
    }
    selected
}

fn show_power_profile(ui: &mut egui::Ui, status: &NodeStatus) {
    let source = &status.power_source;
    if source != &PowerSourceFacts::default() {
        ui.label(RichText::new("Power source").strong());
        ui.horizontal_wrapped(|ui| {
            ui.label("Battery");
            match (source.battery_percent, source.battery_count) {
                (Some(percent), Some(count)) => {
                    ui.colored_label(Style::TEXT, format!("{percent}% · {count} source(s)"));
                }
                (Some(percent), None) => {
                    ui.colored_label(Style::TEXT, format!("{percent}%"));
                }
                (_, Some(count)) => {
                    ui.colored_label(Style::TEXT, format!("{count} source(s)"));
                }
                (None, None) => {
                    ui.colored_label(Style::TEXT_DIM, "unknown");
                }
            };
            ui.add_space(Style::SP_S);
            ui.label("AC");
            match source.ac_online {
                Some(true) => ui.colored_label(Style::OK, "online"),
                Some(false) => ui.colored_label(Style::WARN, "offline"),
                None => ui.colored_label(Style::TEXT_DIM, "unknown"),
            };
        });
        if let Some(state) = source.battery_status.as_deref() {
            ui.label(
                RichText::new(format!("Battery state: {state}"))
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        }
    }
    if status.power_profile.available.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "Power-profile provider not observed");
        ui.label(
            RichText::new("No profile names crossed the credential-free mesh-status boundary.")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Active:");
        ui.colored_label(
            if status.stale { Style::WARN } else { Style::OK },
            status.power_profile.active.as_deref().unwrap_or("unknown"),
        );
    });
    ui.label(
        RichText::new(format!(
            "Advertised profiles: {}",
            status.power_profile.available.join(", ")
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

/// Detailed local power facts from the trusted UPower seat provider. The
/// world-readable mesh snapshot intentionally exposes only aggregate battery
/// facts, so this card is shown only when This Node is connected to SystemState.
fn show_local_power_telemetry(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    ui.label(RichText::new("Local UPower detail").strong());
    show_local_power_provider_state(ui, system);
    let Some(batteries) = system.battery_targets() else {
        ui.colored_label(Style::TEXT_DIM, "UPower provider unavailable");
        ui.label(
            RichText::new(
                "Battery identity, charge estimates, and power rate remain unknown until the trusted local provider responds.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    };

    match system.ac_power_target() {
        Some(Some(true)) => ui.colored_label(Style::OK, "External power: online"),
        Some(Some(false)) => ui.colored_label(Style::WARN, "External power: offline"),
        Some(None) => ui.colored_label(Style::TEXT_DIM, "External power: unknown"),
        None => ui.colored_label(Style::TEXT_DIM, "External power: unavailable"),
    };

    if batteries.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "No battery devices reported");
        return;
    }
    let mut rows = DenseList::new();
    for battery in batteries {
        rows.row(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&battery.model).strong().size(Style::SMALL));
                ui.label(format!(
                    "{:.0}% · {}",
                    battery.percentage,
                    battery.state.label()
                ));
                ui.colored_label(Style::TEXT_DIM, battery.kind.label());
            });
            let mut estimates = Vec::new();
            if let Some(time) = battery.time_to_empty {
                estimates.push(format!("{} to empty", format_duration(time)));
            }
            if let Some(time) = battery.time_to_full {
                estimates.push(format!("{} to full", format_duration(time)));
            }
            if let Some(rate) = battery.energy_rate {
                estimates.push(format!("{rate:.1} W"));
            }
            if estimates.is_empty() {
                ui.colored_label(Style::TEXT_DIM, "estimate/rate unavailable");
            } else {
                ui.colored_label(Style::TEXT_DIM, estimates.join(" · "));
            }
        });
    }
}

/// Show the local power-profile and charge-limit provider state even when the
/// battery provider is absent. Each probe is independent, so one unavailable
/// backend must not hide the other truthful local facts.
fn show_local_power_provider_state(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    let Some(snapshot) = system.snapshot() else {
        ui.colored_label(
            Style::TEXT_DIM,
            "Local power providers have not produced a snapshot.",
        );
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.label("Power profile");
        match &snapshot.power_profile {
            mde_seat::Probe::Present(profile) => {
                ui.colored_label(Style::OK, profile.active.as_str());
                if !profile.available.is_empty() {
                    ui.colored_label(
                        Style::TEXT_DIM,
                        format!("offers {}", profile.available.join(", ")),
                    );
                }
            }
            mde_seat::Probe::Absent { .. } => {
                ui.colored_label(Style::TEXT_DIM, "unavailable");
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Charge limit");
        match &snapshot.charge_limit {
            mde_seat::Probe::Present(Some(percent)) => {
                ui.colored_label(Style::TEXT, format!("stop at {percent}%"));
            }
            mde_seat::Probe::Present(None) => {
                ui.colored_label(Style::TEXT_DIM, "not advertised by this battery");
            }
            mde_seat::Probe::Absent { .. } => {
                ui.colored_label(Style::TEXT_DIM, "unavailable");
            }
        }
    });
    ui.label(
        RichText::new(
            "Profile and charge-limit changes remain confirmation-gated through Actions; provider refusal is retained as an explicit failure.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL)
        .italics(),
    );
}

fn format_duration(duration: std::time::Duration) -> String {
    let minutes = duration.as_secs() / 60;
    if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

fn show_telemetry(ui: &mut egui::Ui, status: &NodeStatus) {
    let facts = &status.telemetry;
    if facts == &TelemetryFacts::default() {
        ui.colored_label(Style::TEXT_DIM, "Resource telemetry provider not observed");
        ui.label(
            RichText::new(
                "CPU, memory, and root-storage aggregates are unavailable until the node snapshot publishes them.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("CPU");
        ui.colored_label(
            Style::TEXT,
            facts.cpu_cores.map_or_else(
                || "unknown cores".to_owned(),
                |cores| format!("{cores} cores"),
            ),
        );
        ui.add_space(Style::SP_S);
        ui.label("1m load");
        ui.colored_label(
            Style::TEXT,
            facts.load_1m_milli.map_or_else(
                || "unknown".to_owned(),
                |load| format!("{}.{:02}", load / 1000, (load % 1000) / 10),
            ),
        );
    });

    if let (Some(total), Some(available)) = (facts.memory_total_bytes, facts.memory_available_bytes)
    {
        let used = total.saturating_sub(available);
        let ratio = if total == 0 {
            0.0
        } else {
            (used as f32 / total as f32).clamp(0.0, 1.0)
        };
        ui.label(format!(
            "Memory {} used of {}",
            format_bytes(used),
            format_bytes(total)
        ));
        ui.add(
            egui::ProgressBar::new(ratio)
                .desired_width(ui.available_width())
                .text(format!("{:.0}%", ratio * 100.0)),
        );
    } else {
        ui.label(
            RichText::new("Memory capacity: not published")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    }

    if let Some(total) = facts.root_total_bytes {
        let used = facts.root_used_bytes.unwrap_or_default();
        let ratio = facts
            .root_used_percent
            .map(|percent| percent as f32 / 100.0)
            .unwrap_or_else(|| {
                if total == 0 {
                    0.0
                } else {
                    (used as f32 / total as f32).clamp(0.0, 1.0)
                }
            });
        ui.label(format!(
            "Root storage {} used of {}",
            format_bytes(used),
            format_bytes(total)
        ));
        ui.add(
            egui::ProgressBar::new(ratio)
                .desired_width(ui.available_width())
                .text(format!("{:.0}%", ratio * 100.0)),
        );
        if let Some(available) = facts.root_available_bytes {
            ui.label(
                RichText::new(format!("Available: {}", format_bytes(available)))
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        }
    } else {
        ui.label(
            RichText::new("Root storage capacity: not published")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    }
    if status.stale {
        ui.colored_label(Style::WARN, "Telemetry is retained from a stale snapshot");
    }
}

/// Compact live history for the Performance disclosure. The chart is fed only
/// by the bounded aggregate snapshot and intentionally has no hover-dependent
/// meaning: the legend and current values remain readable to keyboard and
/// assistive-technology users even when the plot itself is unavailable.
fn show_telemetry_chart(ui: &mut egui::Ui, history: &[TelemetrySample]) {
    ui.label(RichText::new("Live resource history").strong());
    ui.horizontal_wrapped(|ui| {
        for (label, color) in [
            ("CPU load", Style::ACCENT),
            ("Memory", Style::OK),
            ("Root storage", Style::WARN),
        ] {
            ui.colored_label(color, RichText::new(label).size(Style::SMALL));
        }
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(format!("{} samples · 5s cadence", history.len())).size(Style::SMALL),
        );
    });
    if history.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "Waiting for a second telemetry sample");
        return;
    }

    let width = ui.available_width().max(180.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 112.0), egui::Sense::hover());
    let plot = rect.shrink2(egui::vec2(8.0, 16.0));
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, Style::SURFACE);
    for fraction in [0.0_f32, 0.5, 1.0] {
        let y = egui::lerp(plot.bottom()..=plot.top(), fraction);
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0, Style::BORDER),
        );
        painter.text(
            egui::pos2(plot.right() - 2.0, y - 1.0),
            egui::Align2::RIGHT_CENTER,
            format!("{}%", (fraction * 100.0).round() as u8),
            egui::FontId::proportional(Style::SMALL),
            Style::TEXT_DIM,
        );
    }

    let draw_series = |value: fn(TelemetrySample) -> Option<f32>, color: Color32| {
        let denominator = (history.len().saturating_sub(1)).max(1) as f32;
        let mut previous = None;
        for (index, sample) in history.iter().copied().enumerate() {
            let Some(value) = value(sample) else {
                previous = None;
                continue;
            };
            let point = egui::pos2(
                egui::lerp(plot.left()..=plot.right(), index as f32 / denominator),
                egui::lerp(plot.bottom()..=plot.top(), (value / 100.0).clamp(0.0, 1.0)),
            );
            if let Some(previous) = previous {
                painter.line_segment([previous, point], egui::Stroke::new(2.0, color));
            }
            previous = Some(point);
        }
    };
    draw_series(|sample| sample.load_percent, Style::ACCENT);
    draw_series(|sample| sample.memory_percent, Style::OK);
    draw_series(|sample| sample.root_percent, Style::WARN);
    let _response = mde_egui::widgets::hover_text(
        response,
        "Aggregate CPU, memory, and root-storage history from This Node snapshots.",
    );
}

fn show_hardware(ui: &mut egui::Ui, status: &NodeStatus) {
    let facts = &status.hardware;
    ui.label(RichText::new("Hardware sensors & storage").strong());
    if facts == &HardwareFacts::default() {
        ui.colored_label(Style::TEXT_DIM, "Hardware inventory provider not observed");
        ui.label(
            RichText::new(
                "Block-device capacity, thermal-zone, and fan observations are unavailable until a node provider publishes them.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Storage devices");
        ui.colored_label(
            Style::TEXT,
            facts.storage_devices.map_or_else(
                || "unknown".to_owned(),
                |count| {
                    let removable = facts.storage_removable.unwrap_or(0);
                    format!("{count} · {removable} removable")
                },
            ),
        );
    });
    if let Some(total) = facts.storage_total_bytes {
        ui.label(format!("Aggregate block capacity: {}", format_bytes(total)));
    } else {
        ui.label(
            RichText::new("Aggregate block capacity: not published")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Thermals");
        match (facts.thermal_zones, facts.thermal_max_milli_c) {
            (Some(zones), Some(max_milli_c)) => ui.label(format!(
                "{zones} zone(s) · peak {:.1} °C",
                max_milli_c as f32 / 1000.0
            )),
            (Some(zones), None) => ui.label(format!("{zones} zone(s) · temperature unknown")),
            _ => ui.colored_label(Style::TEXT_DIM, "not published"),
        };
        ui.add_space(Style::SP_S);
        ui.label("Fans");
        ui.colored_label(
            Style::TEXT,
            facts
                .fan_devices
                .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
        );
    });
    ui.label(
        RichText::new(
            "Thermal and storage writes remain unavailable until a typed, bounded hardware provider supplies authorization, limits, audit, and recovery.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

/// Storage detail anatomy for the Device Manager route. The snapshot only
/// publishes aggregate, credential-free capacity facts, so the table never
/// invents disk identities or health details that the provider did not expose.
fn show_storage_detail(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: Option<&crate::system::SystemState>,
) {
    ui.label(RichText::new("Storage capacity").strong());
    let telemetry = &status.telemetry;
    let hardware = &status.hardware;
    if let Some(total) = telemetry.root_total_bytes {
        let used = telemetry.root_used_bytes.unwrap_or_default().min(total);
        let available = telemetry
            .root_available_bytes
            .unwrap_or_else(|| total.saturating_sub(used));
        let ratio = telemetry
            .root_used_percent
            .map_or_else(
                || used as f32 / total.max(1) as f32,
                |value| value as f32 / 100.0,
            )
            .clamp(0.0, 1.0);
        ui.label(format!(
            "Root filesystem · {} used of {}",
            format_bytes(used),
            format_bytes(total)
        ));
        ui.add(
            egui::ProgressBar::new(ratio)
                .desired_width(ui.available_width())
                .text(format!("{:.0}% used", ratio * 100.0)),
        );
        ui.colored_label(
            Style::TEXT_DIM,
            format!("Available: {}", format_bytes(available.min(total))),
        );
    } else {
        ui.colored_label(Style::TEXT_DIM, "Storage capacity provider not observed");
        ui.label(
            RichText::new(
                "Capacity bars remain unavailable until the node publishes bounded aggregate storage facts; local device inventory may still be available below.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
    }

    ui.add_space(Style::SP_S);
    ui.label(RichText::new("Published storage inventory").strong());
    let mut rows = DenseList::new();
    rows.row(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Aggregate block devices")
                    .strong()
                    .size(Style::SMALL),
            );
            ui.colored_label(
                Style::TEXT,
                hardware
                    .storage_devices
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
            );
        });
        ui.colored_label(
            Style::TEXT_DIM,
            "Device identities are available in the neutral Device Manager inventory.",
        );
    });
    rows.row(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Aggregate capacity")
                    .strong()
                    .size(Style::SMALL),
            );
            ui.colored_label(
                Style::TEXT,
                hardware
                    .storage_total_bytes
                    .map_or_else(|| "unknown".to_owned(), format_bytes),
            );
        });
    });
    rows.row(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Removable devices")
                    .strong()
                    .size(Style::SMALL),
            );
            ui.colored_label(
                Style::TEXT,
                hardware
                    .storage_removable
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
            );
        });
    });
    if let Some(system) = system {
        ui.add_space(Style::SP_S);
        show_local_storage_inventory(ui, system);
    } else {
        ui.colored_label(
            Style::TEXT_DIM,
            "Trusted local storage inventory is unavailable while the System provider is not mounted.",
        );
    }
    if status.stale {
        ui.colored_label(
            Style::WARN,
            "Storage values are retained from a stale snapshot; refresh before acting.",
        );
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Detailed connector and mode inventory from the trusted local DRM probe.
/// Modes are observations only; live modeset application remains owned by the
/// DRM runner and the typed DisplayLayout action seam.
fn show_local_display_inventory(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    ui.label(RichText::new("Local DRM connector detail").strong());
    let Some(connectors) = system.display_targets() else {
        ui.colored_label(Style::TEXT_DIM, "DRM display provider unavailable");
        ui.label(
            RichText::new(
                "Connector identity, advertised modes, and preferred mode remain unknown until the trusted local display provider responds.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    };
    if connectors.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "No display connectors reported");
        return;
    }
    let mut rows = DenseList::new();
    for connector in connectors {
        rows.row(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&connector.name).strong().size(Style::SMALL));
                let state = match connector.status {
                    mde_seat::ConnectorStatus::Connected => "connected",
                    mde_seat::ConnectorStatus::Disconnected => "disconnected",
                    mde_seat::ConnectorStatus::Unknown => "state unknown",
                };
                ui.colored_label(
                    if state == "connected" {
                        Style::OK
                    } else {
                        Style::TEXT_DIM
                    },
                    state,
                );
                if let Some((width, height)) = connector.size_mm {
                    ui.colored_label(Style::TEXT_DIM, format!("{width}×{height} mm"));
                }
            });
            if let Some(preferred) = connector.preferred_mode() {
                ui.colored_label(Style::TEXT, format!("Preferred: {}", preferred.label()));
            }
            if connector.modes.is_empty() {
                ui.colored_label(Style::TEXT_DIM, "No advertised modes");
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(Style::TEXT_DIM, "Modes:");
                    for mode in connector.modes.iter().take(32) {
                        ui.label(mode.label());
                    }
                    if connector.modes.len() > 32 {
                        ui.colored_label(Style::TEXT_DIM, "additional modes omitted");
                    }
                });
            }
        });
    }
    ui.label(
        RichText::new(
            "Mode, refresh, arrangement, scale, and rotation writes remain confirmation-gated through the typed DisplayLayout provider; live DRM apply is runner-gated.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

/// Detailed keyboard-LED inventory from the typed kernel provider.
fn show_local_keyboard_inventory(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    ui.label(RichText::new("Local keyboard backlights").strong());
    let Some(backlights) = system.keyboard_backlight_targets() else {
        ui.colored_label(Style::TEXT_DIM, "Keyboard-backlight provider unavailable");
        ui.label(
            RichText::new(
                "Per-device brightness remains unknown until the kernel LED provider responds.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    };
    if backlights.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "No keyboard backlight devices reported");
        return;
    }
    let mut rows = DenseList::new();
    for backlight in backlights {
        rows.row(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&backlight.name).strong().size(Style::SMALL));
                ui.label(format!("{}%", backlight.percent()));
                ui.colored_label(
                    Style::TEXT_DIM,
                    format!("{}/{} raw", backlight.brightness, backlight.max),
                );
            });
        });
    }
}

/// Show the durable direct-seat input policy consumed by the native libinput
/// handoff. Mutations remain in the dedicated Actions workflow so this detail
/// route is a truthful read-through of the same System authority.
fn show_local_input_policy(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    ui.label(RichText::new("Direct-seat input policy").strong());
    let (pointer_speed, tap_to_click) = system.input_policy_target();
    let (two_finger_scroll, touchscreen, edge_gestures) = system.touch_gesture_policy_target();
    let rows = [
        ("Pointer speed", format!("{pointer_speed:+}%")),
        (
            "Tap to click",
            if tap_to_click {
                "enabled".to_owned()
            } else {
                "disabled".to_owned()
            },
        ),
        (
            "Two-finger scroll",
            if two_finger_scroll {
                "enabled".to_owned()
            } else {
                "disabled".to_owned()
            },
        ),
        (
            "Touchscreen input",
            if touchscreen {
                "enabled".to_owned()
            } else {
                "disabled".to_owned()
            },
        ),
        (
            "Edge gestures",
            if edge_gestures {
                "enabled".to_owned()
            } else {
                "disabled".to_owned()
            },
        ),
    ];
    let mut dense = DenseList::new();
    for (label, value) in rows {
        dense.row(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(label).strong().size(Style::SMALL));
                ui.colored_label(Style::TEXT, value);
            });
        });
    }
    ui.label(
        RichText::new(
            "Values are persisted through the trusted System provider and handed to the native libinput path. Change them through Actions for confirmation and audit.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL)
        .italics(),
    );
}

/// Detailed bounded thermal/fan inventory from the fixed-root local provider.
/// This is observation-only; platform profiles, fan curves, and power limits
/// remain unavailable until an independently authorized hardware worker exists.
fn show_local_hardware_inventory(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    ui.label(RichText::new("Local thermal & fan sensors").strong());
    let Some(hardware) = system.hardware_targets() else {
        ui.colored_label(Style::TEXT_DIM, "Thermal/hwmon provider unavailable");
        ui.label(
            RichText::new(
                "Thermal zones and fan inputs remain unknown until the fixed kernel sensor provider responds.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    };
    show_local_timestamp_freshness(ui, hardware.observed_at_ms);
    if hardware.thermal_zones.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "No thermal zones reported");
    } else {
        let mut rows = DenseList::new();
        for zone in hardware.thermal_zones.iter().take(16) {
            rows.row(ui, |ui| {
                ui.label(RichText::new(&zone.label).strong().size(Style::SMALL));
                ui.colored_label(
                    Style::TEXT,
                    zone.temperature_milli_c.map_or_else(
                        || "temperature unknown".to_owned(),
                        |value| format!("{:.1} °C", value as f32 / 1000.0),
                    ),
                );
            });
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Fan inputs");
        ui.colored_label(Style::TEXT, hardware.fan_count.to_string());
    });
    show_local_storage_inventory(ui, system);
    show_local_firmware_inventory(ui, system);
    ui.horizontal_wrapped(|ui| {
        ui.label("Platform profile");
        ui.colored_label(
            if hardware.platform_profile.is_some() {
                Style::TEXT
            } else {
                Style::TEXT_DIM
            },
            hardware.platform_profile.as_deref().unwrap_or("unknown"),
        );
    });
    if hardware.platform_profile_choices.is_empty() {
        ui.colored_label(
            Style::TEXT_DIM,
            "No standard kernel profile choices reported",
        );
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(Style::TEXT_DIM, "Available:");
            for profile in hardware.platform_profile_choices.iter().take(8) {
                ui.label(profile);
            }
        });
    }
    ui.label(
        RichText::new(
            "Sensor and profile values are read-only observations. Profile changes, fan curves, CPU/GPU limits, and firmware writes remain unavailable.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

/// Detailed local storage inventory from the trusted fixed-root seat provider.
/// Device names and flags stay out of the world-readable mesh-status projection;
/// destructive storage actions remain unavailable without a typed provider.
fn show_local_storage_inventory(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    ui.add_space(Style::SP_S);
    ui.label(RichText::new("Local storage devices").strong());
    let Some(hardware) = system.hardware_targets() else {
        ui.colored_label(Style::TEXT_DIM, "Local storage provider unavailable");
        return;
    };
    if hardware.storage_devices.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "No local block devices reported");
        return;
    }
    let mut rows = DenseList::new();
    for device in hardware.storage_devices.iter().take(32) {
        rows.row(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&device.name).strong().size(Style::SMALL));
                ui.colored_label(
                    Style::TEXT,
                    device
                        .size_bytes
                        .map_or_else(|| "capacity unknown".to_owned(), format_bytes),
                );
                ui.colored_label(
                    Style::TEXT_DIM,
                    if device.removable {
                        "removable"
                    } else {
                        "fixed"
                    },
                );
                ui.colored_label(
                    Style::TEXT_DIM,
                    match device.rotational {
                        Some(true) => "rotational",
                        Some(false) => "solid-state",
                        None => "media type unknown",
                    },
                );
            });
        });
    }
    ui.label(
        RichText::new(
            "Identity and health are local observations only; destructive storage actions remain unavailable until a typed, confirmed provider exists.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

/// Local firmware and dock inventory from fixed kernel class roots. These are
/// observations only; firmware updates and Thunderbolt authorization remain
/// unavailable until their typed privileged providers are connected.
fn show_local_firmware_inventory(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    let Some(hardware) = system.hardware_targets() else {
        return;
    };
    ui.add_space(Style::SP_S);
    ui.label(RichText::new("Firmware & docks").strong());
    if let Some(firmware) = &hardware.firmware {
        if let Some(product) = firmware.product_name.as_deref() {
            mde_egui::field(ui, "Product", product, Style::TEXT);
        }
        if let Some(version) = firmware.bios_version.as_deref() {
            mde_egui::field(ui, "Firmware version", version, Style::TEXT);
        }
    } else {
        ui.colored_label(Style::TEXT_DIM, "Firmware identity unavailable");
    }
    if hardware.thunderbolt_devices.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "No Thunderbolt/dock devices reported");
    } else {
        let mut rows = DenseList::new();
        for device in hardware.thunderbolt_devices.iter().take(16) {
            rows.row(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&device.name).strong().size(Style::SMALL));
                    ui.colored_label(
                        match device.authorized {
                            Some(true) => Style::OK,
                            Some(false) => Style::WARN,
                            None => Style::TEXT_DIM,
                        },
                        match device.authorized {
                            Some(true) => "authorized",
                            Some(false) => "unauthorized",
                            None => "authorization unknown",
                        },
                    );
                });
            });
        }
    }
    ui.label(
        RichText::new(
            "Firmware updates and dock authorization remain unavailable until an admin-authorized provider supplies confirmation, safety limits, audit, and recovery.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

/// Detailed local PipeWire graph inventory. This is intentionally read-only;
/// volume/mute writes continue through the dedicated, confirmation-gated
/// Actions workflow and the same typed mixer provider.
fn show_local_mixer_inventory(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    ui.label(RichText::new("Local PipeWire graph").strong());
    let Some(mixer) = system.mixer_target() else {
        ui.colored_label(Style::TEXT_DIM, "PipeWire mixer provider unavailable");
        ui.label(
            RichText::new(
                "Playback, capture, application, VM, and mesh-audio strips remain unknown until the trusted local graph responds.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    };

    let origin_label = |origin: &mde_seat::StripOrigin| match origin {
        mde_seat::StripOrigin::HostSession => "host",
        mde_seat::StripOrigin::LocalVm(_) => "local VM",
        mde_seat::StripOrigin::MeshRemote(_) => "mesh peer",
    };
    let show_strip = |ui: &mut egui::Ui, strip: &mde_seat::MixerStrip, label: &str| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(label).strong().size(Style::SMALL));
            ui.label(RichText::new(&strip.name).strong().size(Style::SMALL));
            ui.colored_label(Style::TEXT_DIM, origin_label(&strip.origin));
            ui.colored_label(
                if strip.muted {
                    Style::WARN
                } else {
                    Style::TEXT
                },
                if strip.muted {
                    "muted".to_owned()
                } else {
                    format!("{}%", strip.volume)
                },
            );
        });
    };

    show_strip(ui, &mixer.master, "Master");
    if !mixer.strips.is_empty() {
        ui.label(RichText::new("Playback strips").strong().size(Style::SMALL));
        for strip in mixer.strips.iter().take(64) {
            show_strip(ui, strip, "Playback");
        }
        if mixer.strips.len() > 64 {
            ui.colored_label(Style::TEXT_DIM, "Additional playback strips omitted");
        }
    }
    if !mixer.capture.is_empty() {
        ui.label(RichText::new("Capture strips").strong().size(Style::SMALL));
        for strip in mixer.capture.iter().take(64) {
            show_strip(ui, strip, "Capture");
        }
        if mixer.capture.len() > 64 {
            ui.colored_label(Style::TEXT_DIM, "Additional capture strips omitted");
        }
    }
}

fn show_display(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.label(RichText::new("Display inventory").strong());
    let facts = &status.display;
    if facts == &DisplayFacts::default() {
        ui.colored_label(Style::TEXT_DIM, "DRM display provider not observed");
        ui.label(
            RichText::new(
                "Connector, mode, and backlight observations are unavailable until the node snapshot publishes them.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Connectors");
        ui.colored_label(
            Style::TEXT,
            facts
                .connectors
                .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
        );
        ui.add_space(Style::SP_S);
        ui.label("Connected");
        ui.colored_label(
            Style::TEXT,
            facts
                .connected
                .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
        );
        ui.add_space(Style::SP_S);
        ui.label("Modes");
        ui.colored_label(
            Style::TEXT,
            facts
                .modes
                .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
        );
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Backlight");
        match (facts.backlight_percent, facts.backlights) {
            (Some(percent), Some(count)) => {
                ui.colored_label(Style::TEXT, format!("{percent}% · {count} panel(s)"));
            }
            (Some(percent), None) => {
                ui.colored_label(Style::TEXT, format!("{percent}%"));
            }
            (_, Some(count)) => {
                ui.colored_label(Style::TEXT, format!("{count} panel(s), level unknown"));
            }
            (None, None) => {
                ui.colored_label(Style::TEXT_DIM, "not published");
            }
        }
    });
    ui.label(
        RichText::new(
            "Display mode, arrangement, rotation, and brightness writes remain behind typed seat authorization.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_input(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.label(RichText::new("Input inventory").strong());
    match status.input.event_devices {
        Some(count) => ui.colored_label(Style::TEXT, format!("{count} event device(s) observed")),
        None => ui.colored_label(Style::TEXT_DIM, "evdev provider unavailable"),
    };
    ui.label(
        RichText::new(
            "Device names and event streams stay local. Keyboard, pointer, touch, pen, gesture, and tap-to-click policy requires the direct-seat provider.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_audio(ui: &mut egui::Ui, status: &NodeStatus) {
    let facts = &status.audio;
    if facts == &AudioFacts::default() {
        ui.colored_label(Style::TEXT_DIM, "Audio provider not observed");
        ui.label(
            RichText::new(
                "PipeWire, PulseAudio compatibility, WirePlumber, and ALSA/UCM facts are not published by mesh-status.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        show_privacy(ui, status);
        return;
    }
    let rows = [
        ("PulseAudio compatibility", facts.pulse_available),
        ("PipeWire graph", facts.pipewire_graph),
        ("WirePlumber policy", facts.wireplumber_policy),
        ("Playback", facts.playback),
        ("Capture", facts.capture),
    ];
    for (label, value) in rows {
        ui.horizontal(|ui| {
            ui.label(label);
            match value {
                Some(true) => ui.colored_label(Style::OK, "available"),
                Some(false) => ui.colored_label(Style::WARN, "unavailable"),
                None => ui.colored_label(Style::TEXT_DIM, "unknown"),
            };
        });
    }
    if let Some(count) = facts.alsa_devices {
        ui.label(
            RichText::new(format!("ALSA/UCM devices discovered: {count}"))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    }
    if let Some(recovery) = &facts.recovery {
        ui.label(
            RichText::new(format!("Recovery: {recovery}"))
                .color(Style::WARN)
                .size(Style::SMALL),
        );
    }
    show_privacy(ui, status);
}

fn show_privacy(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.add_space(Style::SP_S);
    ui.label(RichText::new("Camera & microphone privacy").strong());
    let facts = &status.privacy;
    ui.horizontal_wrapped(|ui| {
        ui.label("Microphone");
        match facts.microphone_muted {
            Some(true) => ui.colored_label(Style::OK, "muted"),
            Some(false) => ui.colored_label(Style::WARN, "not muted"),
            None => ui.colored_label(Style::TEXT_DIM, "privacy state unavailable"),
        };
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Camera devices");
        match facts.camera_devices {
            Some(count) => ui.colored_label(Style::TEXT, count.to_string()),
            None => ui.colored_label(Style::TEXT_DIM, "not published"),
        };
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Camera privacy");
        match facts.camera_privacy {
            Some(true) => ui.colored_label(Style::OK, "blocked"),
            Some(false) => ui.colored_label(Style::WARN, "not blocked"),
            None => ui.colored_label(Style::TEXT_DIM, "privacy provider unavailable"),
        };
    });
    ui.label(
        RichText::new(
            "Device presence never implies permission. Camera privacy controls remain unavailable until a typed provider publishes a state.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_security_privacy(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.label(RichText::new("Security & privacy posture").strong());
    show_privacy(ui, status);
    ui.add_space(Style::SP_S);
    ui.label(RichText::new("Security posture").strong());
    ui.colored_label(
        Style::TEXT_DIM,
        "The shared mesh snapshot does not publish full encryption or firewall policy",
    );
    ui.label(
        RichText::new(
            "Trusted local-seat cards below provide bounded encryption-mapping and firewalld observations. Device presence is not permission, camera privacy remains provider-gated, and no broader policy is inferred from partial facts.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

/// Show local capture privacy from the trusted PipeWire provider. Capture
/// route names stay on the local seat; camera permission remains unavailable
/// until a dedicated typed camera-privacy provider exists.
fn show_local_privacy_inventory(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    ui.label(RichText::new("Trusted local capture privacy").strong());
    let Some(mixer) = system.mixer_target() else {
        ui.colored_label(Style::TEXT_DIM, "PipeWire privacy provider unavailable");
        ui.label(
            RichText::new("Microphone route and mute state remain unknown until the trusted local graph responds.")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        return;
    };
    if mixer.capture.is_empty() {
        ui.colored_label(Style::TEXT_DIM, "No local capture route reported");
    } else {
        let mut rows = DenseList::new();
        for strip in mixer.capture.iter().take(32) {
            rows.row(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&strip.name).strong().size(Style::SMALL));
                    ui.colored_label(
                        if strip.muted { Style::OK } else { Style::WARN },
                        if strip.muted { "muted" } else { "live" },
                    );
                });
            });
        }
        if mixer.capture.len() > 32 {
            ui.colored_label(Style::TEXT_DIM, "Additional capture routes omitted");
        }
    }
    ui.label(
        RichText::new(
            "Microphone mute changes use the typed PipeWire capture target and remain confirmation/audit-gated in Actions. Camera permission state is unavailable without a typed privacy provider.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL)
        .italics(),
    );
}

fn show_accessibility(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    let (text_scale, motion) = system.accessibility_summary();
    ui.label(RichText::new("Accessibility preferences").strong());
    ui.horizontal_wrapped(|ui| {
        ui.label("Text scale");
        ui.colored_label(Style::TEXT, text_scale);
        ui.add_space(Style::SP_S);
        ui.label("Motion");
        ui.colored_label(Style::TEXT, motion);
    });
    ui.label(
        RichText::new(
            "These values come from the durable System appearance provider and apply to the whole shell. Assistive-service, screen-reader, and device-specific accessibility providers remain unavailable until they publish typed state.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_time_language_region(
    ui: &mut egui::Ui,
    system: &crate::system::SystemState,
    locale: &LocalLocaleProvider,
) {
    ui.label(RichText::new("Time, language & region").strong());
    ui.horizontal_wrapped(|ui| {
        ui.label("Display time zone");
        ui.colored_label(Style::TEXT, system.clock_zone_label());
    });
    show_local_freshness(ui, locale.freshness.last_success);
    match locale.state {
        Ok(()) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Host locale");
                ui.colored_label(
                    Style::TEXT,
                    locale.facts.locale.as_deref().unwrap_or("not reported"),
                );
                ui.add_space(Style::SP_S);
                ui.label("Language");
                ui.colored_label(
                    Style::TEXT,
                    locale.facts.language.as_deref().unwrap_or("not reported"),
                );
                ui.add_space(Style::SP_S);
                ui.label("Host time zone");
                ui.colored_label(
                    Style::TEXT,
                    locale.facts.timezone.as_deref().unwrap_or("not reported"),
                );
                ui.add_space(Style::SP_S);
                ui.label("Keyboard region");
                ui.colored_label(
                    Style::TEXT,
                    locale
                        .facts
                        .keyboard_region
                        .as_deref()
                        .unwrap_or("not reported"),
                );
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Host time synchronization");
                match locale.facts.time_synchronized {
                    Some(true) => ui.colored_label(Style::OK, "synchronized"),
                    Some(false) => ui.colored_label(Style::WARN, "not synchronized"),
                    None => ui.colored_label(Style::TEXT_DIM, "not reported"),
                };
            });
        }
        Err(error) => {
            ui.colored_label(Style::WARN, format!("Local locale provider: {error}"));
        }
    }
    ui.label(
        RichText::new(
            "The display clock zone is durable and owned by the System provider; event, mesh, and audit timestamps remain UTC. Host locale, keyboard-region, and time-sync evidence are read-only; mutation remains provider-gated.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_provider_continuity_unavailable(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.label(RichText::new(title).strong());
    ui.colored_label(Style::TEXT_DIM, detail);
}

/// Render the bounded This Node capability surface. Capability rows are
/// projections of the live snapshot; mutation rows live in the dedicated
/// Actions workflow so observation and intervention remain separate.
fn show_capability_surface(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Capabilities")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    mde_egui::card().show(ui, |ui| {
        let mut rows = DenseList::new();
        for projection in status.capability_projection() {
            rows.row(ui, |ui| show_capability_row(ui, projection));
        }
    });
}

/// Dedicated WL-UX-011 action workflow. It is deliberately a separate view
/// from inventory/observation: the existing snapshot is read-only, so every
/// control remains visibly disabled with its typed provider/authorization
/// reason until a real writer seam is present.
fn show_actions_workflow(ui: &mut egui::Ui, status: &NodeStatus) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(RichText::new("Node actions").strong());
            ui.label(
                RichText::new(
                    "Actions are grouped here separately from observation. Each mutation will require a typed provider, trusted-session authorization, impact confirmation, and an auditable result.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            if status.stale {
                mde_egui::card().show(ui, |ui| {
                    ui.colored_label(Style::WARN, "Refresh required before actions");
                    ui.label(
                        RichText::new(
                            status
                                .stale_reason
                                .as_deref()
                                .unwrap_or("The provider projection is stale."),
                        )
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                    );
                });
                ui.add_space(Style::SP_S);
            }
            mde_egui::card().show(ui, |ui| {
                let mut rows = DenseList::new();
                for projection in status.action_projection() {
                    rows.row(ui, |ui| show_action_row(ui, projection));
                }
            });
            mde_egui::muted_note(
                ui,
                "No action is enabled from a read-only snapshot. Provider and authorization state must arrive before a mutation can be offered.",
            );
        });
}

/// Actions rendering with the real local System provider connected. Only the
/// power-profile, Bluetooth-radio, Wi-Fi-radio, display-output, and master-audio rows get live controls: each target is
/// validated by the provider, the first click arms a visible impact
/// confirmation, and the second click dispatches an existing typed System/Seat
/// action seam.
fn show_actions_workflow_with_system(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    services: &LocalServiceProvider,
    pending_power_profile: &mut Option<String>,
    pending_platform_profile: &mut Option<String>,
    pending_bluetooth_power: &mut Option<bool>,
    pending_bluetooth_device: &mut Option<(String, crate::system::BluetoothDeviceAction)>,
    pending_power_session: &mut Option<mde_seat::PowerVerb>,
    pending_wifi_power: &mut Option<bool>,
    pending_display_output: &mut Option<bool>,
    pending_network_profile: &mut Option<String>,
    pending_service: &mut Option<String>,
    pending_display_brightness: &mut Option<u8>,
    pending_audio_mute: &mut Option<bool>,
    pending_audio_volume: &mut Option<u8>,
    pending_microphone_mute: &mut Option<bool>,
    pending_charge_limit: &mut Option<u8>,
    pending_keyboard_brightness: &mut Option<u8>,
    pending_pointer_speed: &mut Option<i16>,
    pending_tap_to_click: &mut Option<bool>,
    pending_network_disconnect: &mut Option<String>,
    pending_display_mode: &mut Option<(String, mde_seat::DisplayMode)>,
    pending_display_arrangement: &mut Option<(String, bool)>,
    pending_touch_gesture: &mut Option<(crate::system::TouchGesturePolicy, bool)>,
) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(RichText::new("Node actions").strong());
            ui.label(
                RichText::new(
                    "Actions are grouped separately from observation. Risky changes require confirmation and are dispatched through the trusted local seat provider.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            if status.stale {
                mde_egui::card().show(ui, |ui| {
                    ui.colored_label(Style::WARN, "Refresh required before actions");
                    ui.label(
                        RichText::new(
                            status
                                .stale_reason
                                .as_deref()
                                .unwrap_or("The provider projection is stale."),
                        )
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                    );
                });
                ui.add_space(Style::SP_S);
            }
            mde_egui::card().show(ui, |ui| {
                    let mut rows = DenseList::new();
                    for projection in status.action_projection() {
                        rows.row(ui, |ui| {
                        if projection.action == ThisNodeAction::RestartService {
                            show_restart_service_action(
                                ui,
                                status,
                                system,
                                services,
                                pending_service,
                            );
                        } else if projection.action == ThisNodeAction::ChangePowerProfile {
                            show_power_profile_action(
                                ui,
                                status,
                                system,
                                pending_power_profile,
                            );
                        } else if projection.action == ThisNodeAction::ChangePlatformProfile {
                            show_platform_profile_action(
                                ui,
                                status,
                                system,
                                pending_platform_profile,
                            );
                        } else if projection.action == ThisNodeAction::InspectConnectivityProfiles {
                            show_connectivity_profile_inventory(
                                ui,
                                status,
                                system,
                                pending_network_profile,
                            );
                        } else if projection.action == ThisNodeAction::ToggleBluetooth {
                            show_bluetooth_power_action(
                                ui,
                                status,
                                system,
                                pending_bluetooth_power,
                            );
                        } else if projection.action == ThisNodeAction::ManageBluetoothDevices {
                            show_bluetooth_device_action(
                                ui,
                                status,
                                system,
                                pending_bluetooth_device,
                            );
                        } else if projection.action == ThisNodeAction::PowerSession {
                            show_power_session_action(ui, status, system, pending_power_session);
                        } else if projection.action == ThisNodeAction::ToggleWifi {
                            show_wifi_power_action(ui, status, system, pending_wifi_power);
                        } else if projection.action == ThisNodeAction::DisconnectNetworkLink {
                            show_disconnect_network_action(
                                ui,
                                status,
                                system,
                                pending_network_disconnect,
                            );
                        } else if projection.action == ThisNodeAction::ToggleDisplayOutput {
                            show_display_output_action(
                                ui,
                                status,
                                system,
                                pending_display_output,
                            );
                        } else if projection.action == ThisNodeAction::SetDisplayMode {
                            show_display_mode_action(ui, status, system, pending_display_mode);
                        } else if projection.action == ThisNodeAction::ArrangeDisplays {
                            show_display_arrangement_action(
                                ui,
                                status,
                                system,
                                pending_display_arrangement,
                            );
                        } else if projection.action == ThisNodeAction::AdjustDisplayBrightness {
                            show_display_brightness_action(
                                ui,
                                status,
                                system,
                                pending_display_brightness,
                            );
                        } else if projection.action == ThisNodeAction::ToggleAudioMute {
                            show_audio_mute_action(ui, status, system, pending_audio_mute);
                        } else if projection.action == ThisNodeAction::AdjustAudioVolume {
                            show_audio_volume_action(ui, status, system, pending_audio_volume);
                        } else if projection.action == ThisNodeAction::ToggleMicrophoneMute {
                            show_microphone_mute_action(
                                ui,
                                status,
                                system,
                                pending_microphone_mute,
                            );
                        } else if projection.action == ThisNodeAction::AdjustChargeLimit {
                            show_charge_limit_action(ui, status, system, pending_charge_limit);
                        } else if projection.action == ThisNodeAction::AdjustKeyboardBrightness {
                            show_keyboard_brightness_action(
                                ui,
                                status,
                                system,
                                pending_keyboard_brightness,
                            );
                        } else if projection.action == ThisNodeAction::AdjustPointerSpeed {
                            show_pointer_speed_action(ui, status, system, pending_pointer_speed);
                        } else if projection.action == ThisNodeAction::ToggleTapToClick {
                            show_tap_to_click_action(ui, status, system, pending_tap_to_click);
                        } else if projection.action == ThisNodeAction::ConfigureTouchGestures {
                            show_touch_gesture_action(ui, status, system, pending_touch_gesture);
                        } else {
                            show_action_row(ui, projection);
                        }
                    });
                }
            });
            mde_egui::muted_note(
                ui,
                "Provider failures remain visible in the System surface; no optimistic success is shown here.",
            );
        });
}

fn show_restart_service_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    services: &LocalServiceProvider,
    pending: &mut Option<String>,
) {
    let contract = ThisNodeAction::RestartService.contract();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::RestartService.label()).strong());
        if status.stale {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new("Refresh the node projection before restarting a service.")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            return;
        }
        if let Err(reason) = services.state {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(reason)
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            return;
        }
        let mut offered = 0usize;
        for unit in services
            .failed
            .iter()
            .filter(|unit| mde_seat::safe_service_unit(unit))
            .take(8)
        {
            offered += 1;
            let armed = pending.as_deref() == Some(unit.as_str());
            let label = if armed {
                format!("Confirm {unit}")
            } else {
                format!("Restart {unit}")
            };
            let response = mde_egui::widgets::hover_text(
                ui.button(RichText::new(label).size(Style::SMALL)),
                if armed {
                    "Confirm the bounded restart. Dependent workloads may be interrupted."
                } else {
                    "Arm this failed service for a visible restart confirmation."
                },
            );
            if response.clicked() {
                if armed {
                    if system.restart_service(unit) {
                        *pending = None;
                    }
                } else {
                    *pending = Some(unit.clone());
                }
            }
            if armed {
                ui.colored_label(Style::WARN, "confirmation required");
            }
        }
        if offered == 0 {
            ui.colored_label(Style::TEXT_DIM, "no failed service target");
        }
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_display_mode_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<(String, mde_seat::DisplayMode)>,
) {
    let contract = ThisNodeAction::SetDisplayMode.contract();
    let target = if status.stale || status.display == DisplayFacts::default() {
        None
    } else {
        system.display_mode_target()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::SetDisplayMode.label()).strong());
        let Some((id, connector, current, modes)) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh display observation and a live typed connector mode target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let mut offered = 0usize;
        for mode in modes.iter().copied().filter(|mode| *mode != current).take(8) {
            offered += 1;
            let armed = pending.as_ref().is_some_and(|(pending_id, pending_mode)| {
                pending_id == id.as_str() && *pending_mode == mode
            });
            let label = if armed {
                format!("Confirm {}", mode.label())
            } else {
                mode.label()
            };
            let response = mde_egui::widgets::hover_text(
                ui.button(RichText::new(label).size(Style::SMALL)),
                if armed {
                    "Confirm this bounded display mode change. Live DRM application remains runner-gated."
                } else {
                    "Arm this display mode intent for confirmation."
                },
            );
            if response.clicked() {
                if armed {
                    if system.dispatch_display_mode(&id, mode) {
                        *pending = None;
                    }
                } else {
                    *pending = Some((id.as_str().to_owned(), mode));
                }
            }
            if armed {
                ui.colored_label(Style::WARN, "confirmation required");
            }
        }
        if offered == 0 {
            ui.colored_label(Style::TEXT_DIM, "no alternate mode");
        }
        ui.label(
            RichText::new(format!("{connector} · current {}", current.label()))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_display_arrangement_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<(String, bool)>,
) {
    let contract = ThisNodeAction::ArrangeDisplays.contract();
    let targets = if status.stale || status.display.connected.unwrap_or(0) < 2 {
        None
    } else {
        system.display_arrangement_targets()
    };
    ui.vertical(|ui| {
        ui.label(RichText::new(ThisNodeAction::ArrangeDisplays.label()).strong());
        let Some(targets) = targets else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "At least two fresh connected displays and a typed arrangement target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        for (id, connector, position) in targets {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(connector).strong());
                ui.colored_label(
                    Style::TEXT_DIM,
                    format!("position {}, {}", position.0, position.1),
                );
                for (left, label) in [(true, "Move left"), (false, "Move right")] {
                    let target_id = id.to_string();
                    let armed = *pending == Some((target_id.clone(), left));
                    let button_label = if armed {
                        format!("Confirm {label}")
                    } else {
                        label.to_owned()
                    };
                    let response = mde_egui::widgets::hover_text(
                        ui.button(RichText::new(button_label).size(Style::SMALL)),
                        if armed {
                            "Confirm the saved arrangement change; live DRM re-apply remains runner-gated."
                        } else {
                            "Arm this arrangement change for confirmation."
                        },
                    );
                    if response.clicked() {
                        if armed {
                            if system.dispatch_display_nudge(&id, left) {
                                *pending = None;
                            }
                        } else {
                            *pending = Some((target_id, left));
                        }
                    }
                }
            });
        }
        ui.label(
            RichText::new(
                "Arrangement is saved as typed intent; live DRM re-apply remains integration-gated.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_display_brightness_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<u8>,
) {
    let contract = ThisNodeAction::AdjustDisplayBrightness.contract();
    let target = if status.stale || status.display == DisplayFacts::default() {
        None
    } else {
        system.display_brightness_target()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::AdjustDisplayBrightness.label()).strong());
        let Some((id, current, panel, max)) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh display observation and a live typed backlight/DDC target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let dimmed = current.saturating_sub(10);
        let brighter = current.saturating_add(10).min(100);
        let mut render_step = |ui: &mut egui::Ui, next: u8, verb: &str| {
            if next == current {
                return;
            }
            let armed = *pending == Some(next);
            let label = if armed {
                format!("Confirm {verb} to {next}%")
            } else {
                format!("{verb} to {next}%")
            };
            let response = mde_egui::widgets::hover_text(
                ui.button(RichText::new(label).size(Style::SMALL)),
                if armed {
                    "Confirm this bounded brightness change."
                } else {
                    "Arm this brightness change for confirmation."
                },
            );
            if response.clicked() {
                if armed {
                    if system.dispatch_display_brightness(&id, next, panel, max) {
                        *pending = None;
                    }
                } else {
                    *pending = Some(next);
                }
            }
            if armed {
                ui.colored_label(Style::WARN, "confirmation required");
            }
        };
        render_step(ui, dimmed, "Dim display");
        render_step(ui, brighter, "Brighten display");
        ui.label(
            RichText::new(format!("Current {current}% · {} target", if panel { "panel" } else { "DDC" }))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_wifi_power_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<bool>,
) {
    let contract = ThisNodeAction::ToggleWifi.contract();
    let target = if status.stale || !status.connectivity.has_underlay_observation() {
        None
    } else {
        system.wifi_power_target()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::ToggleWifi.label()).strong());
        let Some(enabled) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh Wi-Fi observation and a live typed NetworkManager radio target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let next = !enabled;
        let armed = *pending == Some(next);
        let label = if armed {
            format!("Confirm Wi-Fi {}", if next { "on" } else { "off" })
        } else {
            format!("Turn Wi-Fi {}", if next { "on" } else { "off" })
        };
        let response = mde_egui::widgets::hover_text(
            ui.button(RichText::new(label).size(Style::SMALL)),
            if armed {
                "Confirm the radio change; wireless links may disconnect and mesh reachability may change."
            } else {
                "Arm this radio change for confirmation."
            },
        );
        if response.clicked() {
            if armed {
                if system.dispatch_wifi_power(next) {
                    *pending = None;
                }
            } else {
                *pending = Some(next);
            }
        }
        if armed {
            ui.colored_label(Style::WARN, "confirmation required");
        }
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_connectivity_profile_inventory(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending_profile: &mut Option<String>,
) {
    let contract = ThisNodeAction::InspectConnectivityProfiles.contract();
    let profiles = if status.stale || !status.connectivity.has_underlay_observation() {
        None
    } else {
        system.network_profile_targets()
    };
    ui.vertical(|ui| {
        ui.label(RichText::new(ThisNodeAction::InspectConnectivityProfiles.label()).strong());
        let Some(profiles) = profiles else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "Fresh underlay facts and the local NetworkManager profile provider are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        if profiles.is_empty() {
            ui.colored_label(Style::TEXT_DIM, "No saved profiles reported.");
        }
        for profile in profiles {
            let armed = pending_profile.as_deref() == Some(profile.path.as_str());
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Style::TEXT, profile.kind.label());
                ui.label(RichText::new(profile.label).strong());
                if !system.network_secret_agent_ready() {
                    ui.colored_label(Style::TEXT_DIM, "activation unavailable");
                } else {
                    let label = if armed {
                        "Confirm activation"
                    } else {
                        "Activate profile"
                    };
                    if ui.button(label).clicked() {
                        if armed {
                            if system.activate_network_profile(&profile.path) {
                                *pending_profile = None;
                            } else {
                                *pending_profile = None;
                            }
                        } else {
                            *pending_profile = Some(profile.path.clone());
                        }
                    }
                    if armed {
                        ui.colored_label(Style::WARN, "confirmation required");
                    }
                }
            });
        }
        ui.label(
            RichText::new(
                "Activation uses only provider-issued targets and requests credentials through the trusted SecretAgent modal. APN/DNS/proxy edits and imported VPN mutation remain unavailable; confirm before any profile can change underlay reachability.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_disconnect_network_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<String>,
) {
    let contract = ThisNodeAction::DisconnectNetworkLink.contract();
    let target = if status.stale || !status.connectivity.has_underlay_observation() {
        None
    } else {
        system.network_disconnect_target()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::DisconnectNetworkLink.label()).strong());
        let Some((path, interface)) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh underlay observation and a live connected NetworkManager device target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let armed = pending.as_deref() == Some(path.as_str());
        let label = if armed {
            format!("Confirm disconnect {interface}")
        } else {
            format!("Disconnect {interface}")
        };
        let response = mde_egui::widgets::hover_text(
            ui.button(RichText::new(label).size(Style::SMALL)),
            if armed {
                "Confirm the underlay disconnect. Mesh reachability may be interrupted."
            } else {
                "Arm the underlay disconnect for confirmation; profiles and credentials are not changed."
            },
        );
        if response.clicked() {
            if armed {
                if system.dispatch_network_disconnect(&path, &interface) {
                    *pending = None;
                }
            } else {
                *pending = Some(path);
            }
        }
        if armed {
            ui.colored_label(Style::WARN, "confirmation required");
        }
        ui.label(
            RichText::new(format!("Target {interface} · connected"))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_audio_volume_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<u8>,
) {
    let contract = ThisNodeAction::AdjustAudioVolume.contract();
    let target = if status.stale || status.audio == AudioFacts::default() {
        None
    } else {
        system.master_volume_target()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::AdjustAudioVolume.label()).strong());
        let Some((id, current)) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh audio observation and a live typed master mixer target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let lower = current.saturating_sub(10);
        let higher = current.saturating_add(10).min(100);
        let mut render_step = |ui: &mut egui::Ui, next: u8, verb: &str| {
            if next == current {
                return;
            }
            let armed = *pending == Some(next);
            let label = if armed {
                format!("Confirm {verb} to {next}%")
            } else {
                format!("{verb} to {next}%")
            };
            let response = mde_egui::widgets::hover_text(
                ui.button(RichText::new(label).size(Style::SMALL)),
                if armed {
                    "Confirm this bounded master-volume change."
                } else {
                    "Arm this master-volume change for confirmation."
                },
            );
            if response.clicked() {
                if armed {
                    if system.dispatch_master_volume(&id, next) {
                        *pending = None;
                    }
                } else {
                    *pending = Some(next);
                }
            }
            if armed {
                ui.colored_label(Style::WARN, "confirmation required");
            }
        };
        render_step(ui, lower, "Lower volume");
        render_step(ui, higher, "Raise volume");
        ui.label(
            RichText::new(format!("Current {current}%"))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_microphone_mute_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<bool>,
) {
    let contract = ThisNodeAction::ToggleMicrophoneMute.contract();
    let target = if status.stale || status.audio == AudioFacts::default() {
        None
    } else {
        system.microphone_mute_target()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::ToggleMicrophoneMute.label()).strong());
        let Some((id, muted)) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh audio observation and a live typed capture target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let next = !muted;
        let armed = *pending == Some(next);
        let label = if armed {
            format!(
                "Confirm microphone {}",
                if next { "mute" } else { "unmute" }
            )
        } else {
            format!("{} microphone", if next { "Mute" } else { "Unmute" })
        };
        let response = mde_egui::widgets::hover_text(
            ui.button(RichText::new(label).size(Style::SMALL)),
            if armed {
                "Confirm the selected capture route change before dispatch."
            } else {
                "Arm the selected capture route change for confirmation."
            },
        );
        if response.clicked() {
            if armed {
                if system.dispatch_microphone_mute(&id, next) {
                    *pending = None;
                }
            } else {
                *pending = Some(next);
            }
        }
        if armed {
            ui.colored_label(Style::WARN, "confirmation required");
        }
        ui.label(
            RichText::new(format!(
                "Target {} · currently {}",
                id,
                if muted { "muted" } else { "live" }
            ))
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_charge_limit_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<u8>,
) {
    let contract = ThisNodeAction::AdjustChargeLimit.contract();
    let target = if status.stale || status.power_source == PowerSourceFacts::default() {
        None
    } else {
        system.charge_limit_target()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::AdjustChargeLimit.label()).strong());
        let Some(current) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh battery observation and a live typed charge-threshold target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let lower = current.saturating_sub(5).max(40);
        let higher = current.saturating_add(5).min(100);
        let mut render_step = |ui: &mut egui::Ui, next: u8, verb: &str| {
            if next == current {
                return;
            }
            let armed = *pending == Some(next);
            let label = if armed {
                format!("Confirm {verb} to {next}%")
            } else {
                format!("{verb} to {next}%")
            };
            let response = mde_egui::widgets::hover_text(
                ui.button(RichText::new(label).size(Style::SMALL)),
                if armed {
                    "Confirm this bounded battery charge-cap change."
                } else {
                    "Arm this battery charge-cap change for confirmation."
                },
            );
            if response.clicked() {
                if armed {
                    if system.dispatch_charge_limit(next) {
                        *pending = None;
                    }
                } else {
                    *pending = Some(next);
                }
            }
            if armed {
                ui.colored_label(Style::WARN, "confirmation required");
            }
        };
        render_step(ui, lower, "Lower cap");
        render_step(ui, higher, "Raise cap");
        ui.label(
            RichText::new(format!("Current cap {current}%"))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_keyboard_brightness_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<u8>,
) {
    let contract = ThisNodeAction::AdjustKeyboardBrightness.contract();
    let target = if status.stale || status.input == InputFacts::default() {
        None
    } else {
        system.keyboard_brightness_target()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::AdjustKeyboardBrightness.label()).strong());
        let Some((id, current, max)) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh input observation and a live typed keyboard-LED target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let lower = current.saturating_sub(10);
        let higher = current.saturating_add(10).min(100);
        let mut render_step = |ui: &mut egui::Ui, next: u8, verb: &str| {
            if next == current {
                return;
            }
            let armed = *pending == Some(next);
            let label = if armed {
                format!("Confirm {verb} to {next}%")
            } else {
                format!("{verb} to {next}%")
            };
            let response = mde_egui::widgets::hover_text(
                ui.button(RichText::new(label).size(Style::SMALL)),
                if armed {
                    "Confirm this bounded keyboard-backlight change."
                } else {
                    "Arm this keyboard-backlight change for confirmation."
                },
            );
            if response.clicked() {
                if armed {
                    if system.dispatch_keyboard_brightness(&id, next, max) {
                        *pending = None;
                    }
                } else {
                    *pending = Some(next);
                }
            }
            if armed {
                ui.colored_label(Style::WARN, "confirmation required");
            }
        };
        render_step(ui, lower, "Dim keys");
        render_step(ui, higher, "Brighten keys");
        ui.label(
            RichText::new(format!("Target {id} · current {current}%"))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_pointer_speed_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<i16>,
) {
    let contract = ThisNodeAction::AdjustPointerSpeed.contract();
    let target = if status.stale || status.input == InputFacts::default() {
        None
    } else {
        Some(system.input_policy_target().0)
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::AdjustPointerSpeed.label()).strong());
        let Some(current) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh input observation and a live typed direct-seat policy are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let lower = current.saturating_sub(10).max(-100);
        let higher = current.saturating_add(10).min(100);
        let mut render_step = |ui: &mut egui::Ui, next: i16, verb: &str| {
            if next == current {
                return;
            }
            let armed = *pending == Some(next);
            let label = if armed {
                format!("Confirm {verb} to {next:+}%")
            } else {
                format!("{verb} to {next:+}%")
            };
            let response = mde_egui::widgets::hover_text(
                ui.button(RichText::new(label).size(Style::SMALL)),
                if armed {
                    "Confirm this bounded direct-seat pointer policy change."
                } else {
                    "Arm this bounded pointer policy change for confirmation."
                },
            );
            if response.clicked() {
                if armed {
                    if system.dispatch_pointer_speed(ui.ctx(), next) {
                        *pending = None;
                    }
                } else {
                    *pending = Some(next);
                }
            }
            if armed {
                ui.colored_label(Style::WARN, "confirmation required");
            }
        };
        render_step(ui, lower, "Slow pointer");
        render_step(ui, higher, "Speed pointer");
        ui.label(
            RichText::new(format!("Current policy {current:+}%"))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_tap_to_click_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<bool>,
) {
    let contract = ThisNodeAction::ToggleTapToClick.contract();
    let target = if status.stale || status.input == InputFacts::default() {
        None
    } else {
        Some(system.input_policy_target().1)
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::ToggleTapToClick.label()).strong());
        let Some(current) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh input observation and a live typed direct-seat policy are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let next = !current;
        let armed = *pending == Some(next);
        let label = if armed {
            format!(
                "Confirm {} tap-to-click",
                if next { "enable" } else { "disable" }
            )
        } else {
            format!("{} tap-to-click", if next { "Enable" } else { "Disable" })
        };
        let response = mde_egui::widgets::hover_text(
            ui.button(RichText::new(label).size(Style::SMALL)),
            if armed {
                "Confirm the touchpad policy change before applying it to the direct seat."
            } else {
                "Arm the touchpad policy change for confirmation."
            },
        );
        if response.clicked() {
            if armed {
                if system.dispatch_touchpad_tap(ui.ctx(), next) {
                    *pending = None;
                }
            } else {
                *pending = Some(next);
            }
        }
        if armed {
            ui.colored_label(Style::WARN, "confirmation required");
        }
        ui.label(
            RichText::new(format!(
                "Currently {}",
                if current { "enabled" } else { "disabled" }
            ))
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_touch_gesture_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<(crate::system::TouchGesturePolicy, bool)>,
) {
    let contract = ThisNodeAction::ConfigureTouchGestures.contract();
    let target = if status.stale || status.input == InputFacts::default() {
        None
    } else {
        Some(system.touch_gesture_policy_target())
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::ConfigureTouchGestures.label()).strong());
        let Some((two_finger, touchscreen, edge)) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh input observation and a live typed direct-seat policy are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let fields = [
            (
                crate::system::TouchGesturePolicy::Touchscreen,
                "Touchscreen input",
                touchscreen,
            ),
            (
                crate::system::TouchGesturePolicy::TwoFingerScroll,
                "Two-finger scroll",
                two_finger,
            ),
            (
                crate::system::TouchGesturePolicy::EdgeGestures,
                "Edge gestures",
                edge,
            ),
        ];
        for (policy, label, current) in fields {
            let next = !current;
            let armed = *pending == Some((policy, next));
            let button_label = if armed {
                format!(
                    "Confirm {} {}",
                    if next { "enable" } else { "disable" },
                    label
                )
            } else {
                format!("{} {}", if next { "Enable" } else { "Disable" }, label)
            };
            let response = mde_egui::widgets::hover_text(
                ui.button(RichText::new(button_label).size(Style::SMALL)),
                if armed {
                    "Confirm this direct-seat touch/gesture policy change."
                } else {
                    "Arm this direct-seat touch/gesture policy change for confirmation."
                },
            );
            if response.clicked() {
                if armed {
                    if system.dispatch_touch_gesture_policy(ui.ctx(), policy, next) {
                        *pending = None;
                    }
                } else {
                    *pending = Some((policy, next));
                }
            }
            if armed {
                ui.colored_label(Style::WARN, "confirmation required");
            }
            ui.label(
                RichText::new(if current { "on" } else { "off" })
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        }
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_audio_mute_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<bool>,
) {
    let contract = ThisNodeAction::ToggleAudioMute.contract();
    let target = if status.stale || status.audio == AudioFacts::default() {
        None
    } else {
        system.master_mute_target()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::ToggleAudioMute.label()).strong());
        let Some((id, muted)) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh audio observation and a live typed master mixer target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let next = !muted;
        let armed = *pending == Some(next);
        let label = if armed {
            format!(
                "Confirm master audio {}",
                if next { "mute" } else { "unmute" }
            )
        } else {
            format!("{} master audio", if next { "Mute" } else { "Unmute" })
        };
        let response = mde_egui::widgets::hover_text(
            ui.button(RichText::new(label).size(Style::SMALL)),
            if armed {
                "Confirm the master playback change before dispatch."
            } else {
                "Arm the master playback change for confirmation."
            },
        );
        if response.clicked() {
            if armed {
                if system.dispatch_master_mute(&id, next) {
                    *pending = None;
                }
            } else {
                *pending = Some(next);
            }
        }
        if armed {
            ui.colored_label(Style::WARN, "confirmation required");
        }
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_bluetooth_power_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<bool>,
) {
    let contract = ThisNodeAction::ToggleBluetooth.contract();
    let target = if status.stale || status.bluetooth.adapters.is_none() {
        None
    } else {
        system.bluetooth_power_target()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::ToggleBluetooth.label()).strong());
        let Some((path, powered)) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh aggregate Bluetooth observation and a live typed BlueZ adapter target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let next = !powered;
        let armed = *pending == Some(next);
        let label = if armed {
            format!("Confirm Bluetooth {}", if next { "on" } else { "off" })
        } else {
            format!("Turn Bluetooth {}", if next { "on" } else { "off" })
        };
        let response = mde_egui::widgets::hover_text(
            ui.button(RichText::new(label).size(Style::SMALL)),
            if armed {
                "Confirm this radio change; connected peripherals may be interrupted."
            } else {
                "Arm this radio change for confirmation."
            },
        );
        if response.clicked() {
            if armed {
                if system.dispatch_bluetooth_power(&path, next) {
                    *pending = None;
                }
            } else {
                *pending = Some(next);
            }
        }
        if armed {
            ui.colored_label(Style::WARN, "confirmation required");
        }
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_bluetooth_device_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<(String, crate::system::BluetoothDeviceAction)>,
) {
    let contract = ThisNodeAction::ManageBluetoothDevices.contract();
    let targets = if status.stale || status.bluetooth.devices.is_none() {
        None
    } else {
        system.bluetooth_device_targets()
    };
    ui.vertical(|ui| {
        ui.label(RichText::new(ThisNodeAction::ManageBluetoothDevices.label()).strong());
        let Some((adapter, devices)) = targets else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "Fresh Bluetooth device facts and a live typed BlueZ adapter target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        if devices.is_empty() {
            ui.colored_label(Style::TEXT_DIM, "No Bluetooth devices reported.");
        }
        for device in devices {
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(device.alias).strong());
                    if device.connected {
                        ui.colored_label(Style::OK, "connected");
                    } else if device.paired {
                        ui.colored_label(Style::TEXT_DIM, "paired");
                    } else {
                        ui.colored_label(Style::WARN, "not paired");
                    }
                    if device.trusted {
                        ui.colored_label(Style::TEXT_DIM, "trusted");
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let actions = [
                        (!device.paired, crate::system::BluetoothDeviceAction::Pair, "Pair"),
                        (
                            device.paired && !device.connected,
                            crate::system::BluetoothDeviceAction::Connect,
                            "Connect",
                        ),
                        (
                            device.connected,
                            crate::system::BluetoothDeviceAction::Disconnect,
                            "Disconnect",
                        ),
                        (
                            device.paired,
                            crate::system::BluetoothDeviceAction::ToggleTrusted,
                            if device.trusted { "Untrust" } else { "Trust" },
                        ),
                        (
                            device.paired,
                            crate::system::BluetoothDeviceAction::Forget,
                            "Forget",
                        ),
                    ];
                    for (available, action, label) in actions {
                        if !available {
                            continue;
                        }
                        let armed = pending
                            .as_ref()
                            .is_some_and(|(path, pending_action)| {
                                path == &device.path && *pending_action == action
                            });
                        let button_label = if armed {
                            format!("Confirm {label}")
                        } else {
                            label.to_owned()
                        };
                        let response = mde_egui::widgets::hover_text(
                            ui.button(RichText::new(button_label).size(Style::SMALL)),
                            if armed {
                                "Confirm this Bluetooth device change; pairing or forgetting may require operator recovery."
                            } else {
                                "Arm this Bluetooth device change for confirmation."
                            },
                        );
                        if response.clicked() {
                            if armed {
                                if system.dispatch_bluetooth_device(&adapter, &device.path, action) {
                                    *pending = None;
                                }
                            } else {
                                *pending = Some((device.path.clone(), action));
                            }
                        }
                    }
                });
            });
        }
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_power_session_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<mde_seat::PowerVerb>,
) {
    let contract = ThisNodeAction::PowerSession.contract();
    let caps = (!status.stale)
        .then(|| system.power_action_caps())
        .flatten();
    ui.vertical(|ui| {
        ui.label(RichText::new(ThisNodeAction::PowerSession.label()).strong());
        let Some(caps) = caps else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh typed logind capability probe is required before session actions can be offered.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        for verb in [
            mde_seat::PowerVerb::Lock,
            mde_seat::PowerVerb::Suspend,
            mde_seat::PowerVerb::Hibernate,
            mde_seat::PowerVerb::Reboot,
            mde_seat::PowerVerb::PowerOff,
        ] {
            let availability = caps.for_verb(verb);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(verb.label()).size(Style::SMALL));
                ui.colored_label(
                    if availability.offerable() {
                        Style::TEXT
                    } else {
                        Style::TEXT_DIM
                    },
                    availability.label(),
                );
                if !availability.offerable() {
                    return;
                }
                let armed = *pending == Some(verb);
                let label = if armed || verb.needs_confirm() {
                    if armed {
                        format!("Confirm {}", verb.label())
                    } else {
                        verb.label().to_owned()
                    }
                } else {
                    verb.label().to_owned()
                };
                let response = mde_egui::widgets::hover_text(
                    ui.button(RichText::new(label).size(Style::SMALL)),
                    if armed {
                        "Confirm this session action; host-down verbs interrupt local and mesh workloads."
                    } else if verb.needs_confirm() {
                        "Arm this host-down action for confirmation."
                    } else {
                        "Lock the local session through logind."
                    },
                );
                if response.clicked() {
                    if !verb.needs_confirm() || armed {
                        if system.dispatch_power_action(verb) {
                            *pending = None;
                        }
                    } else {
                        *pending = Some(verb);
                    }
                }
            });
        }
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_display_output_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<bool>,
) {
    let contract = ThisNodeAction::ToggleDisplayOutput.contract();
    let target = if status.stale || status.display.connected.is_none() {
        None
    } else {
        system.display_output_target()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::ToggleDisplayOutput.label()).strong());
        let Some((id, connector, enabled)) = target else {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "A fresh display observation and a connected typed seat output target are required.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        };
        let next = !enabled;
        let armed = *pending == Some(next);
        let label = if armed {
            format!("Confirm {connector} {}", if next { "on" } else { "off" })
        } else {
            format!("Turn {connector} {}", if next { "on" } else { "off" })
        };
        let response = mde_egui::widgets::hover_text(
            ui.button(RichText::new(label).size(Style::SMALL)),
            if armed {
                "Confirm this output change; the last active console cannot be disabled."
            } else {
                "Arm this display output change for confirmation."
            },
        );
        if response.clicked() {
            if armed {
                if system.dispatch_display_output(&id, next) {
                    *pending = None;
                }
            } else {
                *pending = Some(next);
            }
        }
        if armed {
            ui.colored_label(Style::WARN, "confirmation required");
        }
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_power_profile_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending_power_profile: &mut Option<String>,
) {
    let contract = ThisNodeAction::ChangePowerProfile.contract();
    let offered: Vec<&str> = if status.stale {
        Vec::new()
    } else {
        status
            .power_profile
            .available
            .iter()
            .map(String::as_str)
            .filter(|name| system.can_set_power_profile(name))
            .collect()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::ChangePowerProfile.label()).strong());
        if offered.is_empty() {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "No fresh profile is simultaneously advertised by mesh-status and the trusted local System provider.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        }
        for name in offered {
            let armed = pending_power_profile.as_deref() == Some(name);
            let label = if armed {
                format!("Confirm {name}")
            } else {
                format!("Switch to {name}")
            };
            let response = mde_egui::widgets::hover_text(
                ui.button(RichText::new(label).size(Style::SMALL)),
                if armed {
                    "Confirm this performance/thermal change."
                } else {
                    "Arm this power-profile change for confirmation."
                },
            );
            if response.clicked() {
                if armed {
                    if system.dispatch_power_profile(name) {
                        *pending_power_profile = None;
                    }
                } else {
                    *pending_power_profile = Some(name.to_owned());
                }
            }
        }
        if let Some(name) = pending_power_profile.as_deref() {
            ui.colored_label(Style::WARN, format!("Confirming {name}"));
        }
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_platform_profile_action(
    ui: &mut egui::Ui,
    status: &NodeStatus,
    system: &mut crate::system::SystemState,
    pending: &mut Option<String>,
) {
    let contract = ThisNodeAction::ChangePlatformProfile.contract();
    let choices = if status.stale {
        Vec::new()
    } else {
        system.platform_profile_targets().unwrap_or_default()
    };
    let current = system
        .hardware_targets()
        .and_then(|hardware| hardware.platform_profile);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ThisNodeAction::ChangePlatformProfile.label()).strong());
        if choices.is_empty() {
            ui.colored_label(Style::TEXT_DIM, "unavailable");
            ui.label(
                RichText::new(
                    "No fresh kernel platform-profile choices are advertised by the trusted hardware provider.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        }
        for choice in choices {
            if current.as_deref() == Some(choice.as_str()) {
                ui.colored_label(Style::OK, format!("{choice} active"));
                continue;
            }
            let armed = pending.as_deref() == Some(choice.as_str());
            let label = if armed {
                format!("Confirm {choice}")
            } else {
                format!("Use {choice}")
            };
            let response = mde_egui::widgets::hover_text(
                ui.button(RichText::new(label).size(Style::SMALL)),
                if armed {
                    "Confirm this kernel performance/thermal policy change."
                } else {
                    "Arm this provider-advertised platform-profile change."
                },
            );
            if response.clicked() {
                if armed {
                    if system.dispatch_platform_profile(&choice) {
                        *pending = None;
                    }
                } else {
                    *pending = Some(choice);
                }
            }
        }
        if let Some(choice) = pending.as_deref() {
            ui.colored_label(Style::WARN, format!("Confirming {choice}"));
        }
    });
    ui.label(
        RichText::new(format!(
            "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            contract.impact,
            contract.confirmation,
            contract.authorization,
            contract.audit,
            contract.recovery,
        ))
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

/// Render the trusted local BlueZ projection in the durable This Node route.
/// Device identity stays local to the trusted seat; the world-readable mesh
/// projection continues to expose only aggregate Bluetooth facts.
fn show_local_bluetooth_inventory(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    ui.label(RichText::new("Trusted local Bluetooth").strong());
    let Some(snapshot) = system.snapshot() else {
        ui.colored_label(
            Style::TEXT_DIM,
            "BlueZ provider has not produced a snapshot.",
        );
        return;
    };
    match &snapshot.bluetooth {
        mde_seat::Probe::Absent { reason, .. } => {
            ui.colored_label(Style::WARN, "BlueZ unavailable");
            ui.label(
                RichText::new(reason)
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        }
        mde_seat::Probe::Present(bluetooth) => {
            if bluetooth.adapters.is_empty() {
                ui.colored_label(
                    Style::TEXT_DIM,
                    "No local Bluetooth adapters are published.",
                );
            } else {
                let mut rows = DenseList::new();
                for adapter in &bluetooth.adapters {
                    rows.row(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(
                                if adapter.powered {
                                    Style::OK
                                } else {
                                    Style::TEXT_DIM
                                },
                                adapter.name.as_str(),
                            );
                            ui.label(if adapter.powered { "powered" } else { "off" });
                            if adapter.discovering {
                                ui.colored_label(Style::WARN, "scanning");
                            }
                            if adapter.pairable {
                                ui.colored_label(Style::TEXT_DIM, "pairable");
                            }
                        });
                    });
                }
            }
            ui.add_space(Style::SP_XS);
            ui.label(RichText::new("Known devices").strong());
            if bluetooth.devices.is_empty() {
                ui.colored_label(Style::TEXT_DIM, "No local devices are published.");
            } else {
                let mut rows = DenseList::new();
                for device in &bluetooth.devices {
                    rows.row(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(
                                if device.connected {
                                    Style::OK
                                } else {
                                    Style::TEXT_DIM
                                },
                                device.alias.as_str(),
                            );
                            ui.label(if device.connected {
                                "connected"
                            } else if device.paired {
                                "paired"
                            } else {
                                "available"
                            });
                            if device.trusted {
                                ui.colored_label(Style::TEXT_DIM, "trusted");
                            }
                            if let Some(percent) = device.battery_percent {
                                ui.colored_label(Style::TEXT_DIM, format!("{percent}%"));
                            }
                        });
                    });
                }
            }
            ui.label(
                RichText::new(
                    "Pair, connect, trust, forget, and scan actions use the typed BlueZ provider and remain confirmation/audit-gated in Actions.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL)
                .italics(),
            );
        }
    }
}

/// Render the trusted local NetworkManager projection alongside the mesh
/// projection. This keeps the durable This Node route useful for the actual
/// workstation without copying credentials or creating a second authority.
fn show_local_network_inventory(ui: &mut egui::Ui, system: &crate::system::SystemState) {
    ui.label(RichText::new("Trusted local underlay").strong());
    let Some(snapshot) = system.snapshot() else {
        ui.colored_label(
            Style::TEXT_DIM,
            "NetworkManager provider has not produced a snapshot.",
        );
        return;
    };
    match &snapshot.network {
        mde_seat::Probe::Absent { reason, .. } => {
            ui.colored_label(Style::WARN, "NetworkManager unavailable");
            ui.label(
                RichText::new(reason)
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        }
        mde_seat::Probe::Present(network) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Wi-Fi radio");
                match network.wifi_enabled {
                    Some(true) => ui.colored_label(Style::OK, "on"),
                    Some(false) => ui.colored_label(Style::WARN, "off"),
                    None => ui.colored_label(Style::TEXT_DIM, "unknown"),
                };
                ui.add_space(Style::SP_S);
                ui.label("Links");
                ui.colored_label(Style::TEXT, network.links.len().to_string());
                ui.add_space(Style::SP_S);
                ui.label("Profiles");
                ui.colored_label(Style::TEXT, network.profiles.len().to_string());
            });

            let mut rows = DenseList::new();
            for link in &network.links {
                rows.row(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(
                            match link.state {
                                mde_seat::NetworkState::Connected => Style::OK,
                                mde_seat::NetworkState::Connecting => Style::WARN,
                                mde_seat::NetworkState::Disconnected
                                | mde_seat::NetworkState::Unavailable
                                | mde_seat::NetworkState::Unknown => Style::TEXT_DIM,
                            },
                            link.interface.as_str(),
                        );
                        ui.label(match link.kind {
                            mde_seat::NetworkKind::Ethernet => "Ethernet",
                            mde_seat::NetworkKind::Wifi => "Wi-Fi",
                            mde_seat::NetworkKind::Cellular => "Cellular",
                        });
                        ui.colored_label(
                            Style::TEXT_DIM,
                            match link.state {
                                mde_seat::NetworkState::Connected => "connected",
                                mde_seat::NetworkState::Connecting => "connecting",
                                mde_seat::NetworkState::Disconnected => "disconnected",
                                mde_seat::NetworkState::Unavailable => "unavailable",
                                mde_seat::NetworkState::Unknown => "unknown",
                            },
                        );
                    });
                });
            }
            if network.links.is_empty() {
                ui.colored_label(Style::TEXT_DIM, "No recognized local links are published.");
            }

            egui::CollapsingHeader::new("Saved profile inventory")
                .id_salt("this-node-local-network-profiles")
                .default_open(false)
                .show(ui, |ui| {
                    if network.profiles.is_empty() {
                        ui.colored_label(Style::TEXT_DIM, "No credential-free profile labels are available.");
                    } else {
                        for profile in &network.profiles {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(profile.label.as_str());
                                ui.colored_label(Style::TEXT_DIM, profile.kind.label());
                            });
                        }
                    }
                    ui.label(
                        RichText::new(
                            "Credentials, SSIDs, APNs, routes, DNS values, and UUIDs stay outside the This Node snapshot. Activation and VPN changes remain behind the typed SecretAgent boundary.",
                        )
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL)
                        .italics(),
                    );
                });
        }
    }
}

/// Render only the network facts explicitly published by mesh-status. This is
/// deliberately a read-only projection; missing interface, route, lighthouse,
/// or DNS values remain visible as unavailable instead of becoming guesses.
fn show_connectivity(ui: &mut egui::Ui, status: &NodeStatus) {
    // egui's zoom scales painted geometry after layout. Reduce the logical child
    // width by that same factor so a narrow large-text card wraps against the
    // painted viewport rather than laying out one line that later overflows it.
    let zoom = ui.ctx().zoom_factor().max(1.0);
    if zoom > 1.0 {
        let width = (ui.available_width() / zoom - Style::SP_M).max(1.0);
        ui.set_min_width(width);
        ui.set_max_width(width);
        ui.set_width(width);
    }
    let availability = status.connectivity_availability();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(DOT)
                .color(availability.tone())
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.colored_label(
            availability.tone(),
            RichText::new(availability.word()).size(Style::SMALL),
        );
    });
    ui.add_space(Style::SP_XS);
    ui.add(
        egui::Label::new(
            RichText::new(availability.detail())
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        )
        .wrap(),
    );

    let facts = &status.connectivity;
    connectivity_field(ui, "Interface", facts.interface.as_deref());
    connectivity_field(ui, "CIDR", facts.cidr.as_deref());
    egui::CollapsingHeader::new("Topology details")
        .id_salt("this-node-connectivity-topology")
        .default_open(false)
        .show(ui, |ui| {
            connectivity_field(ui, "Default route", facts.default_route.as_deref());
            let lighthouses = (!facts.lighthouses.is_empty()).then(|| facts.lighthouses.join(", "));
            connectivity_field(ui, "Lighthouses", lighthouses.as_deref());
            let dns_servers = (!facts.dns_servers.is_empty()).then(|| facts.dns_servers.join(", "));
            connectivity_field(ui, "DNS", dns_servers.as_deref());
            ui.label(
                RichText::new("Topology is read-only until a typed NetworkManager/ModemManager provider supplies safe mutation, confirmation, and recovery contracts.")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL)
                    .italics(),
            );
        });

    ui.add_space(Style::SP_XS);
    ui.label(
        RichText::new("Provider state")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    mde_egui::card().show(ui, |ui| {
        let mut rows = DenseList::new();
        for projection in status.provider_projection() {
            rows.row(ui, |ui| show_connectivity_provider_row(ui, projection));
        }
    });

    ui.add(
        egui::Label::new(
            RichText::new(
                "Wi-Fi radio: typed + confirmation-gated.\nProfiles, credentials, routes, DNS: provider-gated.",
            )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        )
        .wrap(),
    );
    show_bluetooth(ui, status);
}

fn show_bluetooth(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.add_space(Style::SP_S);
    ui.label(RichText::new("Bluetooth").strong());
    let facts = &status.bluetooth;
    ui.horizontal_wrapped(|ui| {
        ui.label("BlueZ adapters");
        match facts.adapters {
            Some(count) => ui.colored_label(Style::TEXT, count.to_string()),
            None => ui.colored_label(Style::TEXT_DIM, "provider unavailable"),
        };
        ui.add_space(Style::SP_S);
        ui.label("Power");
        match facts.powered {
            Some(true) => ui.colored_label(Style::OK, "on"),
            Some(false) => ui.colored_label(Style::WARN, "off"),
            None => ui.colored_label(Style::TEXT_DIM, "unknown"),
        };
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Observed devices");
        match facts.devices {
            Some(count) => ui.colored_label(Style::TEXT, count.to_string()),
            None => ui.colored_label(Style::TEXT_DIM, "not published"),
        };
    });
    ui.add(
        egui::Label::new(
            RichText::new("Pairing and trust require typed BlueZ authorization.")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        )
        .wrap(),
    );
}

fn show_connectivity_provider_row(ui: &mut egui::Ui, projection: ConnectivityProviderProjection) {
    let availability = projection.availability;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(DOT)
                .color(availability.tone())
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(projection.provider.label())
                .color(Style::TEXT)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(
            availability.tone(),
            RichText::new(availability.word()).size(Style::SMALL),
        );
    });
    ui.add(
        egui::Label::new(
            RichText::new(availability.detail())
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        )
        .wrap(),
    );
    if let Some(interface) = projection.interface.as_deref() {
        connectivity_field(ui, "Interface", Some(interface));
    }
    if let Some(cidr) = projection.cidr.as_deref() {
        connectivity_field(ui, "CIDR", Some(cidr));
    }
    if let Some(recovery) = projection.recovery.label() {
        connectivity_field(ui, "Next safe step", Some(recovery));
    }
}

fn connectivity_field(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    let (value, tone) = value.map_or(("not published", Style::TEXT_DIM), |value| {
        (value, Style::TEXT)
    });
    // Connectivity values are provider output, not fixed copy: DNS and
    // lighthouse lists can be long, while the unavailable state is deliberately
    // verbose. Keep the label/value relationship accessible, but let the value
    // wrap inside the card instead of allowing the shared single-line `field`
    // primitive to paint past a narrow or large-text pane.
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ui.add(egui::Label::new(RichText::new(value).color(tone).size(Style::SMALL)).wrap());
    });
}

fn show_capability_row(ui: &mut egui::Ui, projection: CapabilityProjection) {
    let availability = projection.availability;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(DOT)
                .color(availability.tone())
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(projection.capability.label())
                .color(Style::TEXT)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(
            availability.tone(),
            RichText::new(availability.word()).size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(projection.capability.description()).size(Style::SMALL),
        );
    });
    ui.label(
        RichText::new(availability.detail())
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
}

fn show_action_row(ui: &mut egui::Ui, projection: ActionProjection) {
    let availability = projection.availability;
    let contract = projection.action.contract();
    ui.horizontal_wrapped(|ui| {
        // This is intentionally disabled unconditionally. No current action has
        // a writer seam, and a future read-model change must not accidentally
        // turn `Available` into an unaudited mutation from this `&self` path.
        let response = ui.add_enabled(
            false,
            egui::Button::new(RichText::new(projection.action.label()).size(Style::SMALL)),
        );
        let response = mde_egui::widgets::hover_text(response, availability.detail());
        install_action_accessibility(
            ui.ctx(),
            response.id,
            response.rect,
            projection.action,
            availability,
        );
        ui.colored_label(
            availability.tone(),
            RichText::new(availability.word()).size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(availability.detail()).size(Style::SMALL),
        );
    });
    ui.add(
        egui::Label::new(
            RichText::new(format!(
                "Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
                contract.impact,
                contract.confirmation,
                contract.authorization,
                contract.audit,
                contract.recovery,
            ))
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        )
        .wrap(),
    );
}

/// Keep the typed action boundary visible to assistive technology as well as to
/// sighted operators. These controls remain disabled because the snapshot is a
/// read-only observation; the value carries the exact provider/authorization
/// reason instead of making a screen reader guess from a dim button.
fn install_action_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    rect: egui::Rect,
    action: ThisNodeAction,
    availability: CapabilityAvailability,
) {
    let _ = ctx.accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(action.label());
        node.set_value(format!(
            "{}: {} Impact: {} Confirmation: {} Authorization: {} Audit: {} Recovery: {}",
            availability.word(),
            availability.detail(),
            action.contract().impact,
            action.contract().confirmation,
            action.contract().authorization,
            action.contract().audit,
            action.contract().recovery,
        ));
        node.set_bounds(accesskit_rect(rect));
        node.set_disabled();
        node.clear_actions();
    });
}

fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// The identity card: hostname + role + a leader marker, then overlay IP, cipher,
/// presence + heartbeat freshness, and the installed version + update hint.
fn show_users(ui: &mut egui::Ui, status: &NodeStatus) {
    let users = &status.users;
    ui.label(RichText::new("Local account posture").strong());
    if !users.provider {
        ui.colored_label(Style::TEXT_DIM, "account provider unavailable");
        ui.label(
            RichText::new(
                "This Node could not read the local account provider; no usernames or sign-in state are inferred.",
            )
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
        );
        return;
    }
    ui.label(RichText::new("Role-first posture").strong());
    let mut roles = DenseList::new();
    roles.row(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Administrative roles")
                    .strong()
                    .size(Style::SMALL),
            );
            ui.colored_label(
                Style::TEXT,
                users.admin_groups.map_or_else(
                    || "unknown".to_owned(),
                    |count| format!("{count} policy group(s)"),
                ),
            );
        });
        ui.colored_label(
            Style::TEXT_DIM,
            "Membership and privilege names remain local to the trusted identity provider.",
        );
    });
    roles.row(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Interactive users")
                    .strong()
                    .size(Style::SMALL),
            );
            ui.colored_label(
                Style::TEXT,
                users
                    .login_count
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
            );
        });
    });
    roles.row(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("All accounts").strong().size(Style::SMALL));
            ui.colored_label(
                Style::TEXT,
                users
                    .account_count
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
            );
        });
    });
    ui.label(
        RichText::new(
            "Only aggregate counts are published. Names, home paths, shells, group membership, and credentials remain outside the shared snapshot; role changes require a typed, admin-authorized identity provider.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

fn show_identity(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&status.hostname)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        if let Some(role) = &status.role {
            ui.add_space(Style::SP_S);
            ui.colored_label(Style::ACCENT, RichText::new(role).size(Style::SMALL));
        }
        if status.is_leader() {
            ui.add_space(Style::SP_S);
            ui.label(RichText::new(DOT).color(Style::OK).size(Style::SMALL));
            ui.colored_label(Style::OK, RichText::new("mesh leader").size(Style::SMALL));
        }
    });
    ui.add_space(Style::SP_XS);

    mde_egui::field(
        ui,
        "Overlay IP",
        status.overlay_ip.as_deref().unwrap_or("—"),
        if status.overlay_ip.is_some() {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        },
    );
    if let Some(cipher) = &status.cipher {
        mde_egui::field(ui, "Tunnel cipher", cipher, Style::TEXT);
    }

    // Presence + heartbeat freshness.
    match &status.presence {
        Some(p) => {
            let tone = presence_tone(p);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Presence")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                ui.label(RichText::new(DOT).color(tone).size(Style::SMALL));
                ui.add_space(Style::SP_XS);
                ui.colored_label(tone, RichText::new(p).size(Style::SMALL));
                if let Some(age) = status.heartbeat_label() {
                    ui.add_space(Style::SP_S);
                    mde_egui::muted_note(ui, format!("\u{00B7} heartbeat {age}"));
                }
            });
        }
        None => mde_egui::field(
            ui,
            "Presence",
            "not yet in the peer directory",
            Style::TEXT_DIM,
        ),
    }

    // Installed version + update hint.
    match &status.version {
        Some(ver) => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Version")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                ui.colored_label(Style::TEXT, RichText::new(ver).size(Style::SMALL));
                if status.update_available {
                    ui.add_space(Style::SP_S);
                    let hint = status.latest_version.as_deref().map_or_else(
                        || "update available".to_string(),
                        |latest| format!("update available \u{2192} {latest}"),
                    );
                    ui.colored_label(Style::WARN, RichText::new(hint).size(Style::SMALL));
                }
            });
        }
        None => mde_egui::field(ui, "Version", "unknown", Style::TEXT_DIM),
    }
}

/// The lifecycle page's bounded read-only provider projection. A mesh version
/// comparison is evidence of posture only; it is never treated as an update
/// transaction or as proof that a package manager is available.
fn show_update_posture(ui: &mut egui::Ui, status: &NodeStatus) {
    ui.label(RichText::new("Updates & lifecycle posture").strong());
    match (status.version.as_deref(), status.latest_version.as_deref()) {
        (Some(current), Some(latest)) => {
            mde_egui::field(ui, "Installed version", current, Style::TEXT);
            mde_egui::field(ui, "Newest mesh version", latest, Style::TEXT);
            if status.stale {
                ui.colored_label(
                    Style::WARN,
                    "Update posture is stale; refresh before acting.",
                );
            } else if status.update_available {
                ui.colored_label(Style::WARN, "An update is available from the mesh posture.");
            } else {
                ui.colored_label(
                    Style::OK,
                    "This node matches the newest reported mesh version.",
                );
            }
        }
        (Some(current), None) => {
            mde_egui::field(ui, "Installed version", current, Style::TEXT);
            ui.colored_label(Style::TEXT_DIM, "Newest mesh version is unavailable.");
        }
        (None, _) => {
            mde_egui::field(ui, "Installed version", "unknown", Style::TEXT_DIM);
            ui.colored_label(
                Style::TEXT_DIM,
                "Update posture is unavailable until the node publishes a version.",
            );
        }
    }
    ui.label(
        RichText::new(
            "No update, restart, rollback, or reset control is enabled from this read-only projection. A lifecycle provider must supply authorization, expected impact, confirmation, audit, and recovery before mutation is exposed.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

/// The node-services card: one health row per catalog daemon present in the
/// snapshot, or an honest "not yet reported" when this node hasn't published a
/// status record.
fn show_services(ui: &mut egui::Ui, status: &NodeStatus) {
    if status.services.is_empty() {
        let msg = if status.in_directory {
            "Service health not yet reported by this node."
        } else {
            "This node hasn't published a status record yet."
        };
        mde_egui::muted_note(ui, msg);
        return;
    }
    let mut rows = DenseList::new();
    for (label, up) in &status.services {
        rows.row(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot, word, tone) = if *up {
                    (Style::OK, "up", Style::TEXT_DIM)
                } else {
                    (Style::TEXT_DIM, "down", Style::WARN)
                };
                ui.label(RichText::new(DOT).color(dot).size(Style::SMALL));
                ui.add_space(Style::SP_XS);
                ui.label(RichText::new(*label).color(Style::TEXT).size(Style::SMALL));
                ui.add_space(Style::SP_XS);
                ui.colored_label(tone, RichText::new(word).size(Style::SMALL));
            });
        });
    }
}

/// The mesh-context card: the live peer count (online / total) and the elected
/// leader.
fn show_mesh(ui: &mut egui::Ui, status: &NodeStatus) {
    match status.peer_counts {
        Some((online, total)) => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Peers")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                let tone = if total == 0 {
                    Style::TEXT_DIM
                } else if online == total {
                    Style::OK
                } else {
                    Style::WARN
                };
                ui.colored_label(
                    tone,
                    RichText::new(format!("{online}/{total} live")).size(Style::SMALL),
                );
            });
        }
        None => mde_egui::field(ui, "Peers", "unavailable", Style::TEXT_DIM),
    }
    match &status.leader {
        Some(leader) => mde_egui::field(ui, "Leader", leader, Style::TEXT),
        None => mde_egui::field(ui, "Leader", "no leader elected", Style::TEXT_DIM),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};

    /// A faithful mesh-status snapshot: `self` + a `nodes` directory (this node plus
    /// two peers), the fleet counts, and the network overview — the exact shape
    /// `mesh-status-snapshot.sh` writes. `leader` names the mesh leader so both the
    /// is-leader and not-leader paths are reachable from one fixture.
    fn snapshot(self_host: &str, leader: &str) -> String {
        format!(
            r#"{{
              "generated_ms": 1000000,
              "self": "{self_host}",
              "latest_version": "11.2.0",
              "online": 2,
              "total": 3,
              "power_profile":{{"active":"balanced","available":["balanced","performance","power-saver"]}},
              "telemetry":{{"cpu_cores":8,"load_1m_milli":1275,
                "memory_total_bytes":17179869184,"memory_available_bytes":8589934592,
                "root_total_bytes":536870912000,"root_used_bytes":268435456000,
                "root_available_bytes":268435456000,"root_used_percent":50}},
              "power_source":{{"battery_count":1,"battery_percent":72,
                "battery_status":"Discharging","ac_online":false}},
              "display":{{"connectors":2,"connected":1,"modes":8,
                "backlights":1,"backlight_percent":64}},
              "input":{{"event_devices":6}},
              "users":{{"provider":true,"account_count":12,"login_count":3,"admin_groups":1}},
              "hardware":{{"storage_devices":2,"storage_total_bytes":1099511627776,
                "storage_removable":1,"thermal_zones":3,"thermal_max_milli_c":61250,
                "fan_devices":1}},
              "audio":{{"pulse_available":true,"pipewire_graph":true,
                "wireplumber_policy":true,"alsa_devices":2,"playback":true,
                "capture":true,"recovery":""}},
              "nodes": [
                {{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
                  "last_seen_ms":990000,"version":"11.1.0",
                  "services":{{"mackesd":true,"nebula":true,"sync":true,"bus":true,"dns":true,
                    "voice":false,"music":false,"kdc":true,"workbench":true}},
                  "role":"workstation","update":true}},
                {{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"online",
                  "last_seen_ms":995000,"version":"11.2.0","services":{{}},
                  "role":"lighthouse","update":false}},
                {{"hostname":"peer-2","overlay_ip":"10.42.0.9","presence":"offline",
                  "last_seen_ms":100,"version":"11.1.0","services":{{}},
                  "role":"server","update":true}}
              ],
              "network": {{"overlay_if":"nebula1","leader":"{leader}","overlay_ip":"10.42.0.7",
                "overlay_cidr":"10.42.0.0/16","routes":[],"default_gw":"",
                "gateway_endpoints":[],"lighthouse_ips":["10.42.0.1"],"cipher":"AES-256-GCM"}}
            }}"#
        )
    }

    fn connectivity_snapshot(network: &str) -> String {
        format!(
            r#"{{"generated_ms":1000000,"self":"this-node",
              "nodes":[{{"hostname":"this-node","presence":"online"}}],
              "network":{network}}}"#
        )
    }

    /// Drive one headless 960×640 frame of `show_status` and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives minus
    /// the GPU. Returns whether it produced any draw primitives.
    fn renders(status: &NodeStatus) -> bool {
        renders_at(status, 960.0, 1.0)
    }

    fn landing_texts(status: &NodeStatus) -> Vec<String> {
        fn collect(shape: &egui::Shape, texts: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, texts);
                    }
                }
                _ => {}
            }
        }

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_status(ui, status, LocalFreshnessSummary::default())
            });
        });
        let mut texts = Vec::new();
        for clipped in &out.shapes {
            collect(&clipped.shape, &mut texts);
        }
        texts
    }

    fn renders_at(status: &NodeStatus, width: f32, zoom: f32) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.set_zoom_factor(zoom);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_status(ui, status, LocalFreshnessSummary::default())
            });
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    fn renders_actions(status: &NodeStatus) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_actions_workflow(ui, status));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    fn renders_detail(status: &NodeStatus, page: PageEntry) -> bool {
        renders_detail_at(status, page, 960.0, 1.0)
    }

    fn renders_detail_at(status: &NodeStatus, page: PageEntry, width: f32, zoom: f32) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.set_zoom_factor(zoom);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            let mut system = crate::system::SystemState::default();
            egui::CentralPanel::default().show(ctx, |ui| {
                show_section_detail(
                    ui,
                    status,
                    page,
                    Some(&mut system),
                    &[],
                    &[],
                    &LocalServiceProvider::default(),
                    &LocalLocaleProvider::default(),
                    &LocalPrinterProvider::default(),
                    &LocalSecurityProvider::default(),
                    &LocalBackupProvider::new(PathBuf::from("/nonexistent/mde-state-backup.enc")),
                    &LocalApplicationProvider::new(
                        PathBuf::from("/nonexistent/apps-installed.json"),
                        PathBuf::from("/nonexistent/running-apps.json"),
                    ),
                )
            });
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    fn action_accesskit_nodes(status: &NodeStatus) -> Vec<egui::accesskit::Node> {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.enable_accesskit();
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show_actions_workflow(ui, status));
            },
        );
        out.platform_output
            .accesskit_update
            .expect("This Node accesskit update")
            .nodes
            .into_iter()
            .map(|(_, node)| node)
            .collect()
    }

    fn connectivity_text_bounds(
        status: &NodeStatus,
        width: f32,
        zoom: f32,
    ) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text.visual_bounding_rect()));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.set_zoom_factor(zoom);
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 640.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show_connectivity(ui, status));
            },
        );
        let mut bounds = Vec::new();
        for clipped in &out.shapes {
            walk(&clipped.shape, &mut bounds);
        }
        bounds
    }

    #[test]
    fn unseen_before_the_first_snapshot() {
        let s = NodeStatus::default();
        assert!(!s.seen, "the pre-read status is unseen (connecting)");
        // Even the connecting state is a full paint path, never a blank panel.
        assert!(
            renders(&s),
            "the connecting state produced no draw primitives"
        );
    }

    #[test]
    fn telemetry_history_normalizes_only_bounded_aggregate_facts() {
        let facts = TelemetryFacts {
            cpu_cores: Some(4),
            load_1m_milli: Some(2_000),
            memory_total_bytes: Some(1_000),
            memory_available_bytes: Some(250),
            root_total_bytes: Some(2_000),
            root_used_bytes: Some(1_500),
            root_used_percent: None,
            ..TelemetryFacts::default()
        };
        let sample = telemetry_sample(&facts);
        assert_eq!(sample.load_percent, Some(50.0));
        assert_eq!(sample.memory_percent, Some(75.0));
        assert_eq!(sample.root_percent, Some(75.0));
        assert!(format!("{sample:?}").contains("load_percent"));
    }

    #[test]
    fn printer_projection_is_bounded_and_keeps_job_data_out() {
        let (printers, default) = parse_local_printers(
            "printer office is idle. enabled since today\nprinter lab is disabled.\nsystem default destination: office\njob-42 secret-document.pdf",
        );
        assert_eq!(
            printers,
            vec![
                LocalPrinter {
                    name: "office".into(),
                    state: "idle.".into()
                },
                LocalPrinter {
                    name: "lab".into(),
                    state: "disabled.".into()
                },
            ]
        );
        assert_eq!(default.as_deref(), Some("office"));
        assert!(format!("{printers:?}").contains("office"));
        assert!(!format!("{printers:?}").contains("secret-document"));
    }

    #[test]
    fn locale_values_are_bounded_and_strip_only_quotes() {
        assert_eq!(
            bounded_locale_value(" \"en_US.UTF-8\" "),
            Some("en_US.UTF-8".to_owned())
        );
        assert_eq!(bounded_locale_value("America/New_York\nsecret"), None);
        assert_eq!(bounded_locale_value("en US"), None);
    }

    #[test]
    fn inventory_freshness_summary_keeps_fresh_stale_and_awaiting_distinct() {
        let mut summary = LocalFreshnessSummary::default();
        summary.add(&LocalFreshness::default());
        summary.add(&LocalFreshness {
            last_success: Some(Instant::now()),
        });
        summary.add(&LocalFreshness {
            last_success: Some(Instant::now() - LOCAL_PROVIDER_MAX_AGE - Duration::from_secs(1)),
        });
        assert_eq!(
            summary,
            LocalFreshnessSummary {
                fresh: 1,
                stale: 1,
                awaiting: 1
            }
        );
    }

    #[test]
    fn firewall_state_is_only_accepted_from_the_fixed_provider_values() {
        assert_eq!(parse_firewall_state("running"), Ok(FirewallState::Running));
        assert_eq!(
            parse_firewall_state("not running"),
            Ok(FirewallState::NotRunning)
        );
        assert!(parse_firewall_state("zone public rules=secret").is_err());
    }

    #[test]
    fn encryption_provider_counts_only_bounded_luks_mappings() {
        let dir = tempfile::tempdir().expect("encryption tempdir");
        std::fs::create_dir_all(dir.path().join("dm-0/dm")).expect("dm directory");
        std::fs::write(dir.path().join("dm-0/dm/uuid"), "CRYPT-LUKS2-aabb\n").expect("uuid");
        std::fs::create_dir_all(dir.path().join("dm-1/dm")).expect("second dm directory");
        std::fs::write(dir.path().join("dm-1/dm/uuid"), "LVM-foo\n").expect("second uuid");
        std::fs::create_dir_all(dir.path().join("sda")).expect("non-dm entry");
        assert_eq!(
            read_encryption_state(dir.path()),
            Ok(EncryptionState::Observed {
                mappings: 2,
                encrypted: 1
            })
        );
    }

    #[test]
    fn encryption_provider_keeps_empty_mapping_state_truthful() {
        let dir = tempfile::tempdir().expect("empty encryption tempdir");
        assert_eq!(
            read_encryption_state(dir.path()),
            Ok(EncryptionState::NoMappings)
        );
    }

    #[test]
    fn backup_metadata_provider_fails_closed_on_missing_artifacts() {
        assert_eq!(
            read_backup_metadata(Path::new("/nonexistent/mde-state-backup.enc")),
            Ok(BackupState::Missing)
        );
        assert!(MAX_BACKUP_BUNDLE_BYTES <= 2 * 1024 * 1024);
    }

    #[test]
    fn application_mirror_missing_files_remain_unknown_without_fabrication() {
        assert_eq!(
            read_application_facts(
                Path::new("/nonexistent/apps-installed.json"),
                Path::new("/nonexistent/running-apps.json"),
            ),
            Ok(ApplicationFacts {
                installed: None,
                running: None
            })
        );
    }

    #[test]
    fn vendor_pack_projection_is_bounded_versioned_and_fail_closed() {
        let value = serde_json::json!([
            {
                "name": "Surface Controls",
                "version": "2.1.0",
                "status": "installed",
                "capabilities": ["platform-profile", "fan-mode", "raw-path-should-not-matter"]
            },
            {
                "name": "Outdated Pack",
                "version": "1.0.0",
                "status": "outdated",
                "capabilities": ["firmware"]
            },
            {"name": "\u{0000}hostile", "version": "bad", "status": "installed"}
        ]);
        let packs = vendor_pack_facts(Some(&value));
        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0].status, "installed");
        assert_eq!(packs[1].status, "outdated");
        assert_eq!(vendor_pack_facts(None), Vec::new());
        assert!(packs
            .iter()
            .all(|pack| pack.capabilities.len() <= MAX_VENDOR_CAPABILITIES));
        assert!(packs
            .iter()
            .all(|pack| pack.name.len() <= MAX_VENDOR_TEXT_CHARS));
    }

    #[test]
    fn audit_export_payload_is_bounded_and_redacted() {
        let records = vec![crate::system::ActionAuditRecord {
            action: "Display brightness",
            outcome: "accepted",
            occurred_ms: 42,
        }];
        let payload = redacted_audit_payload(&records);
        assert_eq!(payload["schema"], "mde.this-node.audit.v1");
        assert_eq!(payload["operator"], "local trusted session");
        assert_eq!(payload["records"][0]["action"], "Display brightness");
        assert_eq!(payload["records"][0]["outcome"], "accepted");
        assert_eq!(payload["records"][0]["occurred_ms"], 42);
        assert!(!payload.to_string().contains("/dev/"));
        assert!(!payload.to_string().contains("password"));
    }

    #[test]
    fn garbage_or_fragment_snapshot_stays_unseen() {
        for bad in ["", "not json", "{}", "[]", r#"{"network":{}}"#] {
            let s = NodeStatus::project(bad, "this-node");
            assert!(!s.seen, "{bad:?} must not read as a live snapshot");
        }
    }

    #[test]
    fn project_folds_this_nodes_own_row_with_real_fields() {
        // The mesh leader is a peer (lh-01), so this node is NOT the leader.
        let s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        assert!(s.seen && s.in_directory, "this node's own row was found");

        // Identity — every field is the node's real directory reality (§7).
        assert_eq!(s.hostname, "this-node");
        assert_eq!(s.role.as_deref(), Some("workstation"));
        assert_eq!(s.overlay_ip.as_deref(), Some("10.42.0.7"));
        assert_eq!(s.cipher.as_deref(), Some("AES-256-GCM"));

        // Presence + heartbeat: generated 1_000_000, last_seen 990_000 → 10s ago.
        assert_eq!(s.presence.as_deref(), Some("online"));
        assert_eq!(s.heartbeat_label().as_deref(), Some("10s ago"));

        // Version + the fleet-wide update hint (this node runs 11.1.0 < 11.2.0).
        assert_eq!(s.version.as_deref(), Some("11.1.0"));
        assert!(s.update_available);
        assert_eq!(s.latest_version.as_deref(), Some("11.2.0"));

        // Node services parse in catalog order; the map's real up/down is kept.
        assert_eq!(
            s.services.len(),
            SERVICE_CATALOG.len(),
            "all 9 daemons present"
        );
        assert_eq!(s.services[0], ("Mesh daemon", true));
        assert!(s.services.iter().any(|(l, up)| *l == "Voice HUD" && !*up));

        // Mesh context — the live peer count + the elected leader.
        assert_eq!(s.peer_counts, Some((2, 3)));
        assert_eq!(s.leader.as_deref(), Some("lh-01"));
        assert!(!s.is_leader(), "the leader is a peer, not this node");
        assert_eq!(s.power_profile.active.as_deref(), Some("balanced"));
        assert_eq!(
            s.power_profile.available,
            vec!["balanced", "performance", "power-saver"]
        );
        assert_eq!(s.audio.pulse_available, Some(true));
        assert_eq!(s.audio.pipewire_graph, Some(true));
        assert_eq!(s.audio.alsa_devices, Some(2));
        assert_eq!(s.audio.playback, Some(true));
        assert_eq!(s.audio.capture, Some(true));
        assert_eq!(s.telemetry.cpu_cores, Some(8));
        assert_eq!(s.telemetry.root_used_percent, Some(50));
        assert_eq!(s.power_source.battery_percent, Some(72));
        assert_eq!(
            s.power_source.battery_status.as_deref(),
            Some("Discharging")
        );
        assert_eq!(s.display.connected, Some(1));
        assert_eq!(s.display.backlight_percent, Some(64));
        assert_eq!(s.input.event_devices, Some(6));
        assert!(s.users.provider);
        assert_eq!(s.users.account_count, Some(12));
        assert_eq!(s.users.login_count, Some(3));
        assert_eq!(s.hardware.storage_devices, Some(2));
        assert_eq!(s.hardware.thermal_max_milli_c, Some(61250));

        // And the whole live panel tessellates.
        assert!(
            renders(&s),
            "the live ThisNode panel produced no draw primitives"
        );
    }

    #[test]
    fn audio_projection_is_bounded_and_does_not_invent_provider_health() {
        let value = serde_json::json!({
            "pulse_available": false,
            "pipewire_graph": true,
            "wireplumber_policy": null,
            "alsa_devices": 999,
            "playback": false,
            "capture": true,
            "recovery": "  restart PipeWire and refresh the snapshot  ",
        });
        let facts = audio_facts(Some(&value));
        assert_eq!(facts.pulse_available, Some(false));
        assert_eq!(facts.pipewire_graph, Some(true));
        assert_eq!(facts.wireplumber_policy, None);
        assert_eq!(facts.alsa_devices, None);
        assert_eq!(facts.playback, Some(false));
        assert_eq!(facts.capture, Some(true));
        assert_eq!(
            facts.recovery.as_deref(),
            Some("restart PipeWire and refresh the snapshot")
        );
        assert_eq!(audio_facts(None), AudioFacts::default());

        let typed = serde_json::json!({
            "availability": "available",
            "pulse_audio_compatibility": {
                "availability": "available",
                "compatibility": "compatible"
            },
            "pipewire_graph": {"availability": "available"},
            "wireplumber_policy": {"availability": "unavailable"},
            "alsa_ucm_discovery": {"availability": "available", "observed_items": 3},
            "playback": {"availability": "available"},
            "capture": {"availability": "unavailable"},
            "recovery": {"availability": "unavailable"}
        });
        let typed_facts = audio_facts(Some(&typed));
        assert_eq!(typed_facts.pulse_available, Some(true));
        assert_eq!(typed_facts.pipewire_graph, Some(true));
        assert_eq!(typed_facts.wireplumber_policy, Some(false));
        assert_eq!(typed_facts.alsa_devices, Some(3));
        assert_eq!(typed_facts.playback, Some(true));
        assert_eq!(typed_facts.capture, Some(false));
        assert!(typed_facts.recovery.is_some());
    }

    #[test]
    fn privacy_projection_bounds_device_count_and_keeps_missing_camera_privacy_unknown() {
        let facts = privacy_facts(Some(&serde_json::json!({
            "microphone_muted": true,
            "camera_devices": 65,
            "camera_privacy": null,
        })));
        assert_eq!(facts.microphone_muted, Some(true));
        assert_eq!(facts.camera_devices, None);
        assert_eq!(facts.camera_privacy, None);
        assert_eq!(privacy_facts(None), PrivacyFacts::default());
    }

    #[test]
    fn bluetooth_projection_is_bounded_and_does_not_expose_device_identity() {
        let facts = bluetooth_facts(Some(&serde_json::json!({
            "adapters": 1,
            "powered": true,
            "devices": 65,
            "name": "secret-headset",
            "address": "AA:BB:CC:DD:EE:FF",
        })));
        assert_eq!(facts.adapters, Some(1));
        assert_eq!(facts.powered, Some(true));
        assert_eq!(facts.devices, None);
        assert_eq!(bluetooth_facts(None), BluetoothFacts::default());
        let debug = format!("{facts:?}");
        assert!(!debug.contains("secret-headset"));
        assert!(!debug.contains("AA:BB"));
    }

    #[test]
    fn telemetry_projection_is_bounded_and_keeps_invalid_load_unknown() {
        let facts = telemetry_facts(Some(&serde_json::json!({
            "cpu_cores": 8,
            "load_1m_milli": 1275,
            "memory_total_bytes": 16_u64 << 30,
            "memory_available_bytes": 8_u64 << 30,
            "root_total_bytes": 500_u64 << 30,
            "root_used_bytes": 250_u64 << 30,
            "root_available_bytes": 250_u64 << 30,
            "root_used_percent": 50,
            "processes": ["secret-command"],
            "mount": "/private/path",
        })));
        assert_eq!(facts.cpu_cores, Some(8));
        assert_eq!(facts.load_1m_milli, Some(1275));
        assert_eq!(facts.root_used_percent, Some(50));
        assert_eq!(
            telemetry_facts(Some(&serde_json::json!({
                "cpu_cores": 257,
                "load_1m_milli": 256001,
                "memory_total_bytes": (1_u64 << 44) + 1,
                "root_used_percent": 101,
            }))),
            TelemetryFacts::default()
        );
        let debug = format!("{facts:?}");
        assert!(!debug.contains("secret-command"));
        assert!(!debug.contains("private/path"));
    }

    #[test]
    fn power_source_projection_is_aggregate_and_bounded() {
        let facts = power_source_facts(Some(&serde_json::json!({
            "battery_count": 1,
            "battery_percent": 72,
            "battery_status": "Discharging",
            "ac_online": false,
            "name": "BAT0",
            "serial": "secret",
        })));
        assert_eq!(facts.battery_count, Some(1));
        assert_eq!(facts.battery_percent, Some(72));
        assert_eq!(facts.battery_status.as_deref(), Some("Discharging"));
        assert_eq!(facts.ac_online, Some(false));
        let invalid = power_source_facts(Some(&serde_json::json!({
            "battery_count": 17,
            "battery_percent": 101,
            "battery_status": "charging-now!",
        })));
        assert_eq!(invalid, PowerSourceFacts::default());
        let debug = format!("{facts:?}");
        assert!(!debug.contains("BAT0"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn display_and_input_projection_is_bounded_without_identity() {
        let display = display_facts(Some(&serde_json::json!({
            "connectors": 2,
            "connected": 1,
            "modes": 8,
            "backlights": 1,
            "backlight_percent": 64,
            "connector_name": "DP-1",
        })));
        assert_eq!(display.connected, Some(1));
        assert_eq!(display.backlight_percent, Some(64));
        assert_eq!(
            display_facts(Some(&serde_json::json!({
                "connectors": 65,
                "modes": 513,
                "backlight_percent": 101,
            }))),
            DisplayFacts::default()
        );

        let input = input_facts(Some(&serde_json::json!({
            "event_devices": 6,
            "device_name": "secret-keyboard",
        })));
        assert_eq!(input.event_devices, Some(6));
        assert_eq!(
            input_facts(Some(&serde_json::json!({"event_devices": 65}))),
            InputFacts::default()
        );
        let debug = format!("{display:?} {input:?}");
        assert!(!debug.contains("DP-1"));
        assert!(!debug.contains("secret-keyboard"));
    }

    #[test]
    fn hardware_projection_bounds_storage_and_thermal_observations() {
        let facts = hardware_facts(Some(&serde_json::json!({
            "storage_devices": 2,
            "storage_total_bytes": 1_u64 << 40,
            "storage_removable": 1,
            "thermal_zones": 3,
            "thermal_max_milli_c": 61250,
            "fan_devices": 1,
            "block_name": "nvme0n1",
            "hwmon_path": "/sys/class/hwmon/hwmon0",
        })));
        assert_eq!(facts.storage_devices, Some(2));
        assert_eq!(facts.storage_total_bytes, Some(1_u64 << 40));
        assert_eq!(facts.thermal_max_milli_c, Some(61250));
        assert_eq!(
            hardware_facts(Some(&serde_json::json!({
                "storage_devices": 129,
                "storage_total_bytes": 1_u64 << 44 | 1,
                "thermal_max_milli_c": 200001,
            }))),
            HardwareFacts::default()
        );
        let debug = format!("{facts:?}");
        assert!(!debug.contains("nvme0n1"));
        assert!(!debug.contains("hwmon0"));
    }

    #[test]
    fn users_projection_is_aggregate_and_fail_closed() {
        let facts = users_facts(Some(&serde_json::json!({
            "provider": true,
            "account_count": 12,
            "login_count": 3,
            "admin_groups": 1,
            "username": "operator",
            "home": "/home/operator",
        })));
        assert!(facts.provider);
        assert_eq!(facts.account_count, Some(12));
        assert_eq!(facts.login_count, Some(3));
        assert_eq!(facts.admin_groups, Some(1));
        let bounded = users_facts(Some(&serde_json::json!({
            "provider": true,
            "account_count": 4097,
            "login_count": 3,
            "admin_groups": 17,
        })));
        assert!(bounded.provider);
        assert_eq!(bounded.account_count, None);
        assert_eq!(bounded.admin_groups, None);
        let debug = format!("{facts:?}");
        assert!(!debug.contains("operator"));
        assert!(!debug.contains("/home/operator"));
    }

    #[test]
    fn inventory_hierarchy_tracks_the_fixed_tree() {
        let s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");

        let flattened: Vec<_> = SectionGroup::ALL
            .into_iter()
            .flat_map(SectionGroup::sections)
            .copied()
            .collect();
        assert_eq!(flattened, Section::ALL);
        assert!(renders(&s), "the inventory hierarchy must paint");
    }

    #[test]
    fn inventory_summary_keeps_missing_provider_facts_truthful() {
        let unseen = NodeStatus::default();
        assert_eq!(
            inventory_summary(Section::Connectivity, &unseen),
            "provider facts unavailable"
        );
        assert_eq!(
            inventory_summary(Section::Hardware, &unseen),
            "device, firmware, storage, and dock inventory unavailable"
        );

        let live = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        assert_eq!(
            inventory_summary(Section::Connectivity, &live),
            "interface nebula1"
        );
        assert!(
            renders(&live),
            "inventory-first landing must remain renderable"
        );
    }

    #[test]
    fn inventory_landing_uses_progressive_disclosure_instead_of_duplicate_details() {
        let live = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        let texts = landing_texts(&live);

        assert!(texts.iter().any(|text| text == "Node inventory"));
        for duplicated_heading in ["Node services", "Display & sound", "Resource telemetry"] {
            assert!(
                !texts.iter().any(|text| text == duplicated_heading),
                "landing view must not repeat the full {duplicated_heading} detail card"
            );
        }
        assert!(
            renders_at(&live, 520.0, 1.4),
            "compact landing must remain renderable at narrow large-text size"
        );
    }

    #[test]
    fn stale_provider_projection_remains_truthful() {
        let mut s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        s.mark_stale("provider stopped publishing");

        assert!(renders(&s), "stale inventory rows must remain renderable");
        assert!(s
            .provider_projection()
            .iter()
            .all(|projection| projection.recovery == ProviderRecovery::RefreshSnapshot));
    }

    #[test]
    fn connectivity_fixture_projects_and_renders_published_facts() {
        let s = NodeStatus::project(
            &connectivity_snapshot(
                r#"{"overlay_if":"nebula1","overlay_cidr":"10.42.0.7/16",
                   "default_gw":"192.168.1.1","lighthouse_ips":["10.42.0.1"],
                   "dns_servers":["10.42.0.1","1.1.1.1"]}"#,
            ),
            "fallback",
        );

        assert_eq!(s.connectivity.interface.as_deref(), Some("nebula1"));
        assert_eq!(s.connectivity.cidr.as_deref(), Some("10.42.0.7/16"));
        assert_eq!(s.connectivity.default_route.as_deref(), Some("192.168.1.1"));
        assert_eq!(s.connectivity.lighthouses, vec!["10.42.0.1"]);
        assert_eq!(s.connectivity.dns_servers, vec!["10.42.0.1", "1.1.1.1"]);
        assert!(matches!(
            s.connectivity_availability(),
            ConnectivityAvailability::Available(_)
        ));
        let providers = s.connectivity.provider_projection();
        assert!(matches!(
            providers
                .iter()
                .find(|projection| projection.provider == ConnectivityProvider::Mesh)
                .expect("mesh provider")
                .availability,
            ConnectivityAvailability::Available(_)
        ));
        assert!(matches!(
            providers
                .iter()
                .find(|projection| projection.provider == ConnectivityProvider::DnsLighthouse)
                .expect("DNS/lighthouse provider")
                .availability,
            ConnectivityAvailability::Available(_)
        ));
        assert!(matches!(
            providers
                .iter()
                .find(|projection| projection.provider == ConnectivityProvider::Wifi)
                .expect("Wi-Fi provider")
                .availability,
            ConnectivityAvailability::Unavailable(_)
        ));
        assert_eq!(
            providers
                .iter()
                .find(|projection| projection.provider == ConnectivityProvider::Wifi)
                .expect("Wi-Fi provider")
                .recovery,
            ProviderRecovery::AwaitProvider
        );
        assert_eq!(
            providers
                .iter()
                .find(|projection| projection.provider == ConnectivityProvider::Mesh)
                .expect("mesh provider")
                .recovery,
            ProviderRecovery::None
        );
        assert!(matches!(
            s.capability_projection()
                .iter()
                .find(|projection| {
                    projection.capability == NodeCapability::ConnectivityProviders
                })
                .expect("connectivity provider capability")
                .availability,
            CapabilityAvailability::Available(_)
        ));
        assert!(renders(&s), "published connectivity facts must render");
    }

    #[test]
    fn missing_or_impossible_peer_counts_stay_unavailable() {
        for snapshot in [
            r#"{"self":"this-node","nodes":[],"total":3}"#,
            r#"{"self":"this-node","nodes":[],"online":2,"total":1}"#,
            r#"{"self":"this-node","nodes":[],"online":"2","total":3}"#,
        ] {
            let status = NodeStatus::project(snapshot, "fallback");
            assert!(status.seen, "the snapshot itself is readable");
            assert_eq!(
                status.peer_counts, None,
                "invalid counts must not become 0/0"
            );

            let mesh = status
                .capability_projection()
                .into_iter()
                .find(|projection| projection.capability == NodeCapability::MeshContext)
                .expect("mesh context capability");
            assert!(matches!(
                mesh.availability,
                CapabilityAvailability::Unavailable(_)
            ));
            assert!(mesh.availability.detail().contains("peer counts"));
            assert!(
                renders(&status),
                "the unavailable peer state must still render"
            );
        }
    }

    #[test]
    fn explicit_underlay_provider_states_are_typed_and_credentials_are_not_projected() {
        let s = NodeStatus::project(
            &connectivity_snapshot(
                r#"{
                    "interfaces":[
                      {"kind":"wifi","name":"wlan0","state":"connected",
                       "cidr":"192.0.2.20/24","ssid":"private-network","psk":"do-not-store"},
                      {"type":"ethernet","ifname":"enp1s0","state":"down"},
                      {"provider":"cellular","interface":"wwan0","status":"connecting",
                       "apn":"private-apn","password":"do-not-store"}
                    ]
                }"#,
            ),
            "fallback",
        );
        let providers = s.connectivity.provider_projection();

        let wifi = providers
            .iter()
            .find(|projection| projection.provider == ConnectivityProvider::Wifi)
            .expect("Wi-Fi provider");
        assert!(matches!(
            wifi.availability,
            ConnectivityAvailability::Available(_)
        ));
        assert_eq!(wifi.interface.as_deref(), Some("wlan0"));
        assert_eq!(wifi.cidr.as_deref(), Some("192.0.2.20/24"));

        let ethernet = providers
            .iter()
            .find(|projection| projection.provider == ConnectivityProvider::Ethernet)
            .expect("Ethernet provider");
        assert!(matches!(
            ethernet.availability,
            ConnectivityAvailability::Unavailable(_)
        ));
        assert_eq!(ethernet.interface.as_deref(), Some("enp1s0"));
        assert_eq!(ethernet.recovery, ProviderRecovery::RefreshSnapshot);

        let cellular = providers
            .iter()
            .find(|projection| projection.provider == ConnectivityProvider::Cellular)
            .expect("cellular provider");
        assert!(matches!(
            cellular.availability,
            ConnectivityAvailability::Degraded(_)
        ));
        assert_eq!(cellular.interface.as_deref(), Some("wwan0"));
        assert_eq!(cellular.recovery, ProviderRecovery::RefreshSnapshot);

        assert!(matches!(
            s.connectivity_availability(),
            ConnectivityAvailability::Degraded(_)
        ));
        let change = s
            .action_projection()
            .into_iter()
            .find(|projection| projection.action == ThisNodeAction::ChangeConnectivity)
            .expect("connectivity action");
        assert!(change.availability.detail().contains("provider state"));

        let debug = format!("{s:?}");
        assert!(!debug.contains("private-network"));
        assert!(!debug.contains("do-not-store"));
        assert!(!debug.contains("private-apn"));
        assert!(renders(&s), "typed underlay provider rows must render");
    }

    #[test]
    fn connectivity_absence_and_partial_facts_render_honest_states() {
        let absent = NodeStatus::project(&connectivity_snapshot(r"{}"), "fallback");
        assert!(matches!(
            absent.connectivity_availability(),
            ConnectivityAvailability::Unavailable(_)
        ));
        assert!(renders(&absent), "absent connectivity state must render");

        let partial = NodeStatus::project(
            &connectivity_snapshot(r#"{"overlay_if":"nebula1","overlay_cidr":"10.42.0.7/16"}"#),
            "fallback",
        );
        assert!(matches!(
            partial.connectivity_availability(),
            ConnectivityAvailability::Degraded(_)
        ));
        assert!(renders(&partial), "partial connectivity state must render");
    }

    #[test]
    fn connectivity_fields_reflow_inside_a_narrow_large_text_card() {
        let s = NodeStatus::project(
            &connectivity_snapshot(
                r#"{"overlay_if":"nebula1","overlay_cidr":"10.42.0.7/16",
                   "default_gw":"192.168.1.1",
                   "lighthouse_ips":["10.42.0.1","10.42.0.2","10.42.0.3","10.42.0.4"],
                   "dns_servers":["10.42.0.1","1.1.1.1","9.9.9.9"]}"#,
            ),
            "fallback",
        );
        let bounds = connectivity_text_bounds(&s, 240.0, 1.5);
        assert!(!bounds.is_empty(), "the connectivity card must paint text");
        for (text, rect) in bounds {
            assert!(
                rect.left() >= -0.5 && rect.right() <= 240.0 * 1.5 + 0.5,
                "{text:?} escaped the narrow card: {rect:?}"
            );
        }
    }

    #[test]
    fn status_card_keeps_unavailable_provider_states_visible_at_small_sizes() {
        let unseen = NodeStatus::default();
        for (width, zoom) in [(240.0, 1.0), (320.0, 1.5)] {
            assert!(
                renders_at(&unseen, width, zoom),
                "the unavailable status must remain painted at {width}x{zoom}"
            );
        }
        assert!(
            matches!(
                unseen.connectivity_availability(),
                ConnectivityAvailability::Unavailable(_)
            ),
            "missing hardware/provider facts remain explicitly unavailable"
        );
    }

    #[test]
    fn capability_projection_is_fixed_and_snapshot_driven() {
        let unseen = NodeStatus::default();
        let unseen_caps = unseen.capability_projection();
        assert_eq!(unseen_caps.len(), CAPABILITY_CATALOG.len());
        assert!(matches!(
            unseen_caps[0].availability,
            CapabilityAvailability::Unavailable(_)
        ));
        assert!(matches!(
            unseen_caps[5].availability,
            CapabilityAvailability::Unavailable(_)
        ));

        let live = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        let caps = live.capability_projection();
        assert!(matches!(
            caps.iter()
                .find(|projection| projection.capability == NodeCapability::MeshSnapshot)
                .expect("mesh snapshot capability")
                .availability,
            CapabilityAvailability::Available(_)
        ));
        assert!(matches!(
            caps.iter()
                .find(|projection| projection.capability == NodeCapability::ServiceHealth)
                .expect("service health capability")
                .availability,
            CapabilityAvailability::Available(_)
        ));
        assert!(matches!(
            caps.iter()
                .find(|projection| projection.capability == NodeCapability::LocalTelemetry)
                .expect("local telemetry capability")
                .availability,
            CapabilityAvailability::Available(_)
        ));
        assert!(matches!(
            caps.iter()
                .find(|projection| projection.capability == NodeCapability::MutationProviders)
                .expect("mutation provider capability")
                .availability,
            CapabilityAvailability::Unavailable(_)
        ));
    }

    #[test]
    fn typed_mutation_actions_remain_fail_closed_with_live_facts() {
        let live = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        let actions = live.action_projection();
        assert_eq!(actions.len(), ACTION_CATALOG.len());
        assert!(actions.iter().all(|projection| matches!(
            projection.availability,
            CapabilityAvailability::Unavailable(_)
        )));

        let update = actions
            .iter()
            .find(|projection| projection.action == ThisNodeAction::ApplyUpdate)
            .expect("update action");
        assert!(update.availability.detail().contains("update target"));
        assert!(actions
            .iter()
            .any(|projection| projection.action == ThisNodeAction::ChangeConnectivity));
    }

    #[test]
    fn every_typed_action_carries_safety_authorization_audit_and_recovery_contract() {
        for action in ACTION_CATALOG {
            let contract = action.contract();
            for field in [
                contract.impact,
                contract.confirmation,
                contract.authorization,
                contract.audit,
                contract.recovery,
            ] {
                assert!(
                    !field.trim().is_empty(),
                    "{} has a blank safety field",
                    action.label()
                );
            }
        }
    }

    #[test]
    fn typed_actions_export_disabled_accessibility_reasons() {
        let live = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        let nodes = action_accesskit_nodes(&live);
        let restart = nodes
            .iter()
            .find(|node| node.label() == Some("Restart a service"))
            .expect("This Node action should be discoverable to assistive technology");

        assert_eq!(restart.role(), egui::accesskit::Role::Button);
        assert!(restart.is_disabled());
        assert!(!restart.supports_action(egui::accesskit::Action::Click));
        assert!(restart
            .value()
            .expect("disabled action reason")
            .contains("no typed service-control provider"));
    }

    #[test]
    fn leader_row_identifies_this_node_when_it_holds_the_lease() {
        let s = NodeStatus::project(&snapshot("this-node", "this-node"), "fallback");
        assert!(s.is_leader(), "this node holds the leader lease");
        assert!(renders(&s));
    }

    #[test]
    fn self_marker_absent_falls_back_to_local_hostname() {
        // A snapshot with a nodes directory but no `self` marker → the plane still
        // identifies this node by the locally-resolved hostname.
        let snap = r#"{"generated_ms":1,"online":1,"total":1,
            "nodes":[{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
              "last_seen_ms":1,"role":"workstation","services":{"mackesd":true}}],
            "network":{"leader":"","cipher":""}}"#;
        let s = NodeStatus::project(snap, "this-node");
        assert!(s.seen && s.in_directory);
        assert_eq!(s.hostname, "this-node");
        assert_eq!(s.role.as_deref(), Some("workstation"));
    }

    #[test]
    fn seen_but_not_in_directory_shows_identity_without_fabricating_a_row() {
        // The snapshot is readable, but this node's heartbeat record isn't in the
        // directory yet: identity + mesh context still render off `self`/`network`,
        // and the per-node fields honestly say so (never a fake value, §7).
        let s = NodeStatus::project(&snapshot("ghost-node", "lh-01"), "fallback");
        assert!(s.seen, "the snapshot was parsed");
        assert!(!s.in_directory, "no matching directory row for this node");
        assert_eq!(s.hostname, "ghost-node");
        // Network-sourced identity is still available.
        assert_eq!(s.overlay_ip.as_deref(), Some("10.42.0.7"));
        assert_eq!(s.leader.as_deref(), Some("lh-01"));
        assert_eq!(s.peer_counts, Some((2, 3)));
        // Per-node fields are honestly empty, not fabricated.
        assert!(s.role.is_none());
        assert!(s.presence.is_none());
        assert!(s.services.is_empty());
        assert!(s.heartbeat_label().is_none());
        // The honest-partial panel still fully paints.
        assert!(renders(&s));
    }

    #[test]
    fn provider_loss_retains_last_snapshot_as_explicitly_stale() {
        let mut s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        assert!(s.seen);
        assert!(!s.stale);
        let hostname = s.hostname.clone();
        let overlay_ip = s.overlay_ip.clone();

        s.mark_stale("provider unavailable");

        assert!(s.stale);
        assert_eq!(s.stale_reason.as_deref(), Some("provider unavailable"));
        assert_eq!(s.hostname, hostname);
        assert_eq!(s.overlay_ip, overlay_ip);
        assert!(matches!(
            s.connectivity_availability(),
            ConnectivityAvailability::Degraded(_)
        ));
        assert!(s.capability_projection().iter().all(|projection| matches!(
            projection.availability,
            CapabilityAvailability::Degraded(_)
        )));
        assert!(s.provider_projection().iter().all(|projection| {
            !matches!(
                projection.availability,
                ConnectivityAvailability::Available(_)
            )
        }));
        let mesh = s
            .provider_projection()
            .into_iter()
            .find(|projection| projection.provider == ConnectivityProvider::Mesh)
            .expect("mesh provider projection");
        assert!(matches!(
            mesh.availability,
            ConnectivityAvailability::Degraded(
                "The last provider observation is stale; refresh before relying on it."
            )
        ));
        assert_eq!(
            mesh.recovery,
            ProviderRecovery::RefreshSnapshot,
            "stale provider rows must expose refresh as the safe next step"
        );
        assert!(s.action_projection().iter().all(|projection| matches!(
            projection.availability,
            CapabilityAvailability::Degraded(_)
        )));
        assert!(renders(&s), "stale retained state must remain renderable");
    }

    #[test]
    fn snapshot_age_is_bounded_and_future_timestamps_do_not_fake_staleness() {
        assert_eq!(snapshot_age_ms(0, 10_000), None);
        assert_eq!(snapshot_age_ms(9_000, 100_000), Some(91_000));
        assert_eq!(snapshot_age_ms(101_000, 100_000), Some(0));
        assert!(snapshot_age_ms(9_000, 100_000).is_some_and(|age| age > MAX_SNAPSHOT_AGE_MS));
    }

    #[test]
    fn heartbeat_label_is_none_without_a_recorded_beat() {
        let mut s = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        s.last_seen_ms = 0;
        assert!(
            s.heartbeat_label().is_none(),
            "no heartbeat recorded → no freshness claimed"
        );
    }

    #[test]
    fn thisnode_state_defaults_to_the_snapshot_path_unseen() {
        let st = ThisNodeState::default();
        assert_eq!(st.snapshot_path, PathBuf::from(SNAPSHOT_PATH));
        assert!(!st.status.seen);
        assert!(st.last_poll.is_none());
        assert_eq!(st.view, ThisNodeView::Inventory);
    }

    #[test]
    fn dedicated_actions_workflow_renders_fail_closed_state() {
        let live = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        assert!(
            live.action_projection().iter().all(|projection| matches!(
                projection.availability,
                CapabilityAvailability::Unavailable(_)
            )),
            "read-only live facts must not enable mutations"
        );
        assert!(renders_actions(&live));

        let mut stale = live.clone();
        stale.mark_stale("provider stopped publishing");
        assert!(renders_actions(&stale));
        assert!(stale.action_projection().iter().all(|projection| matches!(
            projection.availability,
            CapabilityAvailability::Degraded(_)
        )));
    }

    #[test]
    fn governed_sections_have_renderable_full_page_details() {
        let live = NodeStatus::project(&snapshot("this-node", "lh-01"), "fallback");
        for page in page_index() {
            assert!(
                renders_detail(&live, *page),
                "detail view should render for {}",
                page.route
            );
            assert!(
                renders_detail_at(&live, *page, 520.0, 1.4),
                "narrow large-text detail view should render for {}",
                page.route
            );
        }
    }

    #[test]
    fn inventory_and_hierarchy_sections_resolve_to_durable_detail_routes() {
        for section in Section::ALL {
            let page =
                page_for_section(section).expect("every governed section has a detail route");
            assert_eq!(page.section, section);
            assert!(page.route.starts_with("this-node/"));
        }
    }

    #[test]
    fn bounded_snapshot_reader_rejects_hostile_files_before_projection() {
        let dir = tempfile::tempdir().expect("snapshot tempdir");
        let valid = dir.path().join("valid.json");
        std::fs::write(&valid, snapshot("this-node", "lh-01")).expect("write valid snapshot");
        assert!(read_bounded_snapshot(&valid).is_some());

        let invalid_utf8 = dir.path().join("invalid.json");
        std::fs::write(&invalid_utf8, [0xff, 0xfe]).expect("write invalid snapshot");
        assert!(read_bounded_snapshot(&invalid_utf8).is_none());

        let oversized = dir.path().join("oversized.json");
        std::fs::write(&oversized, vec![b'{'; MAX_SNAPSHOT_BYTES + 1])
            .expect("write oversized snapshot");
        assert!(read_bounded_snapshot(&oversized).is_none());

        let special = dir.path().join("special.json");
        #[cfg(unix)]
        {
            use std::os::unix::net::UnixListener;
            let _socket = UnixListener::bind(&special).expect("create socket");
            assert!(read_bounded_snapshot(&special).is_none());
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir(&special).expect("create special fixture");
            assert!(read_bounded_snapshot(&special).is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn bounded_snapshot_reader_rejects_final_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("snapshot tempdir");
        let target = dir.path().join("outside.json");
        let link = dir.path().join("mesh-status.json");
        std::fs::write(&target, snapshot("outside", "lh-01")).expect("write target snapshot");
        symlink(&target, &link).expect("create final symlink");
        assert!(read_bounded_snapshot(&link).is_none());
    }
}
