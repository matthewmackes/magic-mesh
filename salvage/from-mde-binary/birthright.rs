//! `mde birthright` — the commissioning dashboard (E7.3 + E7.4).
//!
//! After install, the operator needs one screen that *attests the node came up
//! whole* — not merely that the installer exited 0. Birthright is that screen: a
//! Carbon status dashboard of live, re-runnable checks. It is launched as the
//! final step of the OOBE (see `oobe::Msg::Finish`) and re-surfaced at each login
//! by the labwc autostart while `state.birthright_show_at_startup` is true; the
//! operator unchecks "Show this at startup" to dismiss it for good.
//!
//! Sections:
//!   * **Desktop** (E7.3) — labwc up, `mde panel` up, autostart services ran. The
//!     checks that would have caught the second-login black-desktop regression.
//!   * **Mesh** (E7.4) — per-component rows for `mde-bus`, `mackesd`, the Nebula
//!     overlay, and LizardFS mesh-storage, probed live over the Bus, each with a
//!     remediation action when red.
//!
//! Only sections that are genuinely live are rendered — no placeholder cards
//! (CLAUDE.md §3). Voice + Network land in E7.5.
//!
//! Workstation-role only: `main.rs` gates `birthright` through
//! [`crate::role_gate`] (`DESKTOP_ONLY`), so a headless Server/Lighthouse refuses
//! it before a window is ever created.

use std::path::Path;
use std::process::{Command, ExitCode};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use iced::widget::{button, checkbox, container, scrollable, text, Column, Row, Space};
use iced::{Element, Length, Subscription, Task};

use mde_ui::{metrics, palette};

/// How often the dashboard re-reads the live probe state while open. The Desktop
/// checks are cheap `/proc` scans run inline; the Mesh checks are served from a
/// background poller thread (the Bus RPCs would otherwise stall the UI).
const POLL: Duration = Duration::from_secs(2);

/// How often the background thread re-probes the mesh over the Bus.
const MESH_POLL: Duration = Duration::from_secs(5);

/// Per-RPC Bus timeout — short so a down `mackesd` doesn't wedge the poller
/// (mirrors `mesh_status`).
const MESH_RPC_TIMEOUT: Duration = Duration::from_millis(800);

/// A check's state — Carbon tri-state plus a transient "checking…".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// A probe is in flight (initial open + manual Re-check).
    Checking,
    /// Healthy.
    Pass,
    /// Up but partial (e.g. an optional service down) — not a hard failure.
    Degraded,
    /// Down / not detected.
    Fail,
}

impl Status {
    /// The palette role for the status dot/label (remapped to Carbon by
    /// `palette::color`). `GRAY_TEXT` reads as "in progress / unknown".
    fn color(self) -> palette::Rgb {
        match self {
            Status::Checking => palette::GRAY_TEXT,
            Status::Pass => palette::STATUS_OK,
            Status::Degraded => palette::STATUS_WARN,
            Status::Fail => palette::STATUS_RISK,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Status::Checking => "…",
            Status::Pass => "OK",
            Status::Degraded => "!",
            Status::Fail => "X",
        }
    }
}

/// Worst-of rollup for a section / the whole dashboard: any `Fail` dominates,
/// then `Degraded`, then a still-in-flight `Checking`, else `Pass`.
fn rollup(checks: &[Check]) -> Status {
    if checks.iter().any(|c| c.status == Status::Fail) {
        Status::Fail
    } else if checks.iter().any(|c| c.status == Status::Degraded) {
        Status::Degraded
    } else if checks.iter().any(|c| c.status == Status::Checking) {
        Status::Checking
    } else {
        Status::Pass
    }
}

/// A remediation action offered on a failed/degraded row. Each maps to a real,
/// reachable command — never a stub (CLAUDE.md §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fix {
    /// Start the `mackesd` user service.
    StartMackesd,
    /// Re-run the OOBE (its mesh-enrolment stage, E7.2) to re-enroll on Nebula.
    ReEnroll,
    /// Open the Workbench — the mesh / compute / storage control surface.
    OpenWorkbench,
    /// Open the voice/SIP account configuration.
    OpenVoice,
}

impl Fix {
    fn label(self) -> &'static str {
        match self {
            Fix::StartMackesd => "Start mackesd",
            Fix::ReEnroll => "Re-enroll",
            Fix::OpenWorkbench => "Open Workbench",
            Fix::OpenVoice => "Voice settings",
        }
    }
}

/// The argv a [`Fix`] runs — split out pure so it is unit-tested without
/// spawning. `mde …` subcommands resolve via the running binary's path so a
/// dev-tree `./target/debug/mde` re-execs itself, not a stale `/usr/bin/mde`.
fn fix_argv(fix: Fix) -> Vec<String> {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "mde".to_string());
    match fix {
        Fix::StartMackesd => vec![
            "systemctl".into(),
            "--user".into(),
            "start".into(),
            "mackesd".into(),
        ],
        Fix::ReEnroll => vec![exe, "oobe".into()],
        Fix::OpenWorkbench => vec!["mde-workbench".into()],
        Fix::OpenVoice => vec!["mde-voice-config".into()],
    }
}

/// Spawn a [`Fix`]'s command, detached and best-effort.
fn run_fix(fix: Fix) {
    let argv = fix_argv(fix);
    let (cmd, rest) = argv.split_first().expect("fix_argv is never empty");
    let _ = Command::new(cmd).args(rest).spawn();
}

/// One attestation row.
#[derive(Debug, Clone)]
struct Check {
    label: &'static str,
    status: Status,
    detail: String,
    /// Remediation offered when this row is not green.
    fix: Option<Fix>,
}

impl Check {
    fn new(label: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Check {
            label,
            status,
            detail: detail.into(),
            fix: None,
        }
    }

    fn with_fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }

    fn checking(label: &'static str) -> Self {
        Check::new(label, Status::Checking, "checking…")
    }
}

/// Reader-side staleness window for the voice agent's `state/voice/status`
/// heartbeat: a snapshot older than this means the agent stopped. A small
/// multiple of the agent's `STATUS_HEARTBEAT_SECS` (15s).
const VOICE_STALE_SECS: u64 = 45;

/// Shared handle the background poller writes and the UI tick reads (`None`
/// until the first probe completes). Carries both the Mesh + Voice sections,
/// which both read the Bus, so one poller thread serves both.
type LiveHandle = Arc<Mutex<Option<LiveProbe>>>;

/// A snapshot of the live sections: the four mesh rows, the voice row, and the
/// network readouts.
#[derive(Debug, Clone)]
struct LiveProbe {
    mesh: MeshProbe,
    voice: Check,
    network: Vec<Check>,
}

impl LiveProbe {
    /// Every Bus-backed row failed for the same reason (no Bus / no runtime).
    /// The network section still probes (it reads NetworkManager + local report
    /// files, not the Bus), so it is filled by the caller even on Bus failure.
    fn all_fail(reason: &str) -> Self {
        LiveProbe {
            mesh: MeshProbe::all_fail(reason),
            voice: Check::new("Softphone (SIP)", Status::Fail, reason.to_string())
                .with_fix(Fix::OpenVoice),
            network: probe_network_local(),
        }
    }
}

/// A snapshot of the four mesh-component rows.
#[derive(Debug, Clone)]
struct MeshProbe {
    bus: Check,
    mackesd: Check,
    nebula: Check,
    meshfs: Check,
}

impl MeshProbe {
    /// Rendered top-down: the Bus underpins everything, then the control plane,
    /// then the overlay it rides, then the storage layered on top.
    fn rows(&self) -> Vec<Check> {
        vec![
            self.bus.clone(),
            self.mackesd.clone(),
            self.nebula.clone(),
            self.meshfs.clone(),
        ]
    }

    /// Every row failed for the same reason (no Bus / no runtime).
    fn all_fail(reason: &str) -> Self {
        MeshProbe {
            bus: Check::new("mde-bus", Status::Fail, reason.to_string()),
            mackesd: Check::new("mackesd (control plane)", Status::Fail, reason.to_string())
                .with_fix(Fix::StartMackesd),
            nebula: Check::new("Nebula overlay", Status::Fail, reason.to_string()),
            meshfs: Check::new("LizardFS storage", Status::Fail, reason.to_string()),
        }
    }
}

struct Birthright {
    desktop: Vec<Check>,
    live: LiveHandle,
    mesh_rows: Vec<Check>,
    voice_row: Check,
    network_rows: Vec<Check>,
    show_at_startup: bool,
    /// Transient confirmation after Copy/Save (shown in the footer).
    last_action: Option<String>,
}

#[derive(Debug, Clone)]
enum Message {
    /// User pressed "Re-check all": flash Checking, then re-probe.
    Recheck,
    /// Run the inline (Desktop) probes immediately after open / Recheck.
    Probe,
    /// Periodic live refresh — re-reads Desktop inline + the mesh handle.
    Tick,
    /// "Show this at startup" toggled — persisted to menu.json.
    ToggleStartup(bool),
    /// A remediation button was pressed.
    Fix(Fix),
    /// Copy the commissioning report to the clipboard (`wl-copy`).
    CopyReport,
    /// Save the commissioning report to a timestamped file.
    SaveReport,
    /// Close the dashboard.
    Close,
}

/// The three Desktop rows in their initial (pre-probe) Checking state.
fn desktop_checking() -> Vec<Check> {
    vec![
        Check::checking("Compositor (labwc)"),
        Check::checking("Taskbar (mde panel)"),
        Check::checking("Session services"),
    ]
}

/// The four Mesh rows in their initial Checking state (shown until the poller's
/// first probe lands).
fn mesh_checking() -> Vec<Check> {
    vec![
        Check::checking("mde-bus"),
        Check::checking("mackesd (control plane)"),
        Check::checking("Nebula overlay"),
        Check::checking("LizardFS storage"),
    ]
}

/// The Voice row's initial Checking state.
fn voice_checking() -> Check {
    Check::checking("Softphone (SIP)")
}

/// The three Network rows in their initial Checking state.
fn network_checking() -> Vec<Check> {
    vec![
        Check::checking("Internet (NetworkManager)"),
        Check::checking("Mesh peers (roster)"),
        Check::checking("LAN scan (netassess)"),
    ]
}

pub fn run(args: &[String]) -> ExitCode {
    // `--autostart`: the labwc autostart launches us this way every login. Honour
    // the per-user "show at startup" flag and exit silently when it's off, so an
    // operator who dismissed the dashboard isn't nagged. (A manual `mde birthright`
    // always shows.) The Workstation-role gate already ran in main.rs dispatch.
    if args.iter().any(|a| a == "--autostart") && !crate::state::load().birthright_show_at_startup {
        return ExitCode::SUCCESS;
    }

    let r = iced::application(
        |_: &Birthright| "Birthright Commissioning".to_string(),
        update,
        view,
    )
    .theme(|_| palette::iced_theme())
    .window_size(iced::Size::new(560.0, 860.0))
    .subscription(subscription)
    .font(mde_ui::font::REGULAR_BYTES)
    .font(mde_ui::font::BOLD_BYTES)
    .font(mde_ui::font::PLEX_REGULAR_BYTES)
    .font(mde_ui::font::PLEX_BOLD_BYTES)
    .default_font(mde_ui::font::ui())
    .run_with(|| {
        (
            Birthright {
                desktop: desktop_checking(),
                live: start_live_poller(),
                mesh_rows: mesh_checking(),
                voice_row: voice_checking(),
                network_rows: network_checking(),
                show_at_startup: crate::state::load().birthright_show_at_startup,
                last_action: None,
            },
            Task::done(Message::Probe),
        )
    });
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mde birthright: {e}");
            ExitCode::FAILURE
        }
    }
}

fn subscription(_state: &Birthright) -> Subscription<Message> {
    iced::time::every(POLL).map(|_| Message::Tick)
}

fn update(state: &mut Birthright, message: Message) -> Task<Message> {
    match message {
        Message::Recheck => {
            state.desktop = desktop_checking();
            state.mesh_rows = mesh_checking();
            state.voice_row = voice_checking();
            state.network_rows = network_checking();
            refresh_live(state.live.clone());
            return Task::done(Message::Probe);
        }
        Message::Probe | Message::Tick => {
            state.desktop = probe_desktop();
            // Non-blocking read of the background poller's latest snapshot; keep
            // the Checking rows until the first probe lands.
            if let Some(probe) = state.live.lock().ok().and_then(|g| g.clone()) {
                state.mesh_rows = probe.mesh.rows();
                state.voice_row = probe.voice;
                state.network_rows = probe.network;
            }
        }
        Message::ToggleStartup(on) => {
            state.show_at_startup = on;
            let mut st = crate::state::load();
            st.birthright_show_at_startup = on;
            let _ = crate::state::save(&st);
        }
        Message::Fix(fix) => {
            run_fix(fix);
            // The change (service start, re-enrol) lands asynchronously; nudge the
            // poller so the row reflects it sooner than the next 5s tick.
            refresh_live(state.live.clone());
        }
        Message::CopyReport => {
            state.last_action = Some(copy_report(&build_report(state)));
        }
        Message::SaveReport => {
            state.last_action = Some(save_report(&build_report(state)));
        }
        Message::Close => std::process::exit(0),
    }
    Task::none()
}

// --- probes: Desktop section ------------------------------------------------

/// The basename of an argv[0] (strips any directory part).
fn basename(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// True if this argv is the labwc compositor.
fn argv_is_labwc(argv: &[String]) -> bool {
    argv.first().is_some_and(|a| basename(a) == "labwc")
}

/// True if this argv is the canonical shell taskbar (`mde panel`, or the legacy
/// `mde-panel` basename). Deliberately does NOT match `mde birthright` itself.
fn argv_is_mde_panel(argv: &[String]) -> bool {
    let Some(a0) = argv.first().map(|s| basename(s)) else {
        return false;
    };
    (a0 == "mde" && argv.iter().any(|t| t == "panel")) || a0 == "mde-panel"
}

/// Read every process's argv from `/proc` (NUL-separated `cmdline`). Best-effort:
/// unreadable entries are skipped, never fatal.
fn proc_cmdlines() -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let is_pid = name
            .to_str()
            .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()));
        if !is_pid {
            continue;
        }
        if let Ok(raw) = std::fs::read(entry.path().join("cmdline")) {
            let argv: Vec<String> = raw
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect();
            if !argv.is_empty() {
                out.push(argv);
            }
        }
    }
    out
}

/// Is the clipboard-history daemon (an autostart-launched background service)
/// alive? Reuses its PID lockfile — a live PID proves the autostart block ran,
/// which is the exact thing the black-desktop regression broke.
fn clipboard_daemon_alive() -> bool {
    crate::clipboard::dir()
        .map(|d| d.join("daemon.lock"))
        .and_then(|lock| std::fs::read_to_string(lock).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(|pid| Path::new(&format!("/proc/{pid}")).exists())
}

/// Probe the three Desktop rows from live system state.
fn probe_desktop() -> Vec<Check> {
    let procs = proc_cmdlines();
    let labwc = procs.iter().any(|a| argv_is_labwc(a));
    let panel = procs.iter().any(|a| argv_is_mde_panel(a));
    let clip = clipboard_daemon_alive();

    vec![
        Check::new(
            "Compositor (labwc)",
            if labwc { Status::Pass } else { Status::Fail },
            if labwc {
                "labwc is running"
            } else {
                "labwc not detected — the Wayland session is not up"
            },
        ),
        Check::new(
            "Taskbar (mde panel)",
            if panel { Status::Pass } else { Status::Fail },
            if panel {
                "mde panel is running"
            } else {
                "mde panel is not running — the desktop autostart did not launch it"
            },
        ),
        // Optional background services: down is Degraded, not a hard Fail.
        Check::new(
            "Session services",
            if clip { Status::Pass } else { Status::Degraded },
            if clip {
                "clipboard-history daemon running (autostart completed)"
            } else {
                "clipboard-history daemon not running — autostart may be incomplete"
            },
        ),
    ]
}

// --- probes: Mesh section ---------------------------------------------------

/// Start the background poller; returns the shared handle the UI reads. Mirrors
/// `mesh_status::start` — a dedicated thread keeps the 800 ms-timeout Bus RPCs
/// (and the voice-status read) off the UI thread.
#[must_use]
fn start_live_poller() -> LiveHandle {
    let handle: LiveHandle = Arc::new(Mutex::new(None));
    let writer = handle.clone();
    let _ = thread::Builder::new()
        .name("mde-birthright-live".into())
        .spawn(move || loop {
            let probe = probe_live();
            if let Ok(mut g) = writer.lock() {
                *g = Some(probe);
            }
            thread::sleep(MESH_POLL);
        });
    handle
}

/// Fire a one-shot re-probe into `handle` (manual Re-check / post-fix).
fn refresh_live(handle: LiveHandle) {
    let _ = thread::Builder::new()
        .name("mde-birthright-live-once".into())
        .spawn(move || {
            let probe = probe_live();
            if let Ok(mut g) = handle.lock() {
                *g = Some(probe);
            }
        });
}

/// One full probe of the Bus-backed sections: open the Bus once, read the
/// retained voice status (sync), then RPC each mesh component (async). Runs on
/// the poller thread with its own current-thread tokio runtime.
fn probe_live() -> LiveProbe {
    let Some(bus_dir) = mde_bus::default_data_dir() else {
        return LiveProbe::all_fail("no Bus data dir configured");
    };
    let persist = match mde_bus::persist::Persist::open(bus_dir) {
        Ok(p) => p,
        Err(e) => return LiveProbe::all_fail(&format!("Bus store unreachable: {e}")),
    };
    // Voice: a sync read of the agent's retained `state/voice/status`.
    let voice = parse_voice(read_latest(&persist, mde_voice_status_topic()), now_unix());
    // Mesh: async request/reply RPCs.
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return LiveProbe {
            mesh: MeshProbe::all_fail("could not start probe runtime"),
            voice,
            network: probe_network_local(),
        };
    };
    let (mesh, peers) = rt.block_on(async {
        let bus = Check::new("mde-bus", Status::Pass, "Bus store reachable");
        let mackesd = parse_mackesd(rpc_body(&persist, "action/shell/healthz").await);
        let nebula = parse_nebula(rpc_body(&persist, "action/nebula/status").await);
        let meshfs = parse_meshfs(rpc_body(&persist, "action/meshfs/status").await);
        let peers = parse_nebula_peers(rpc_body(&persist, "action/nebula/list-peers").await);
        (
            MeshProbe {
                bus,
                mackesd,
                nebula,
                meshfs,
            },
            peers,
        )
    });
    // Network section: NetworkManager + the live peer roster + the latest LAN scan.
    let network = vec![nm_row(), peers, lan_scan_row()];
    LiveProbe {
        mesh,
        voice,
        network,
    }
}

/// Network rows that don't need the Bus (NetworkManager + the latest stored LAN
/// scan) — used when the Bus is unreachable so the section still renders.
fn probe_network_local() -> Vec<Check> {
    vec![nm_row(), lan_scan_row()]
}

/// NetworkManager connectivity from `nmcli` active connections (reuses
/// `crate::nm`). Pass when a non-loopback connection is active.
fn nm_row() -> Check {
    let label = "Internet (NetworkManager)";
    let active: Vec<_> = crate::nm::active_connections()
        .into_iter()
        .filter(|c| c.kind != "loopback")
        .collect();
    if active.is_empty() {
        Check::new(label, Status::Degraded, "no active network connection")
    } else {
        let names: Vec<&str> = active.iter().map(|c| c.name.as_str()).take(3).collect();
        Check::new(
            label,
            Status::Pass,
            format!("{} active: {}", active.len(), names.join(", ")),
        )
    }
}

/// Parse the `action/nebula/list-peers` reply (a `Vec<PeerRow>`) into the peer
/// roster row: how many paired peers are reachable. A lone (peerless) node is
/// Degraded, not failed.
fn parse_nebula_peers(body: Option<String>) -> Check {
    let label = "Mesh peers (roster)";
    let Some(body) = body else {
        return Check::new(label, Status::Fail, "no roster (mackesd down?)");
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Check::new(label, Status::Fail, "unparseable peer roster");
    };
    if v.get("error").is_some() {
        return Check::new(label, Status::Fail, "roster error from mackesd");
    }
    let Some(arr) = v.as_array() else {
        return Check::new(label, Status::Degraded, "no paired peers yet");
    };
    if arr.is_empty() {
        return Check::new(label, Status::Degraded, "no paired peers yet");
    }
    let online = arr
        .iter()
        .filter(|p| p.get("reachable").and_then(|x| x.as_str()) == Some("online"))
        .count();
    let total = arr.len();
    let status = if online > 0 {
        Status::Pass
    } else {
        Status::Degraded
    };
    Check::new(
        label,
        status,
        format!("{total} paired peer(s), {online} online"),
    )
}

/// The latest stored LAN assessment (`~/.local/share/mde/netassess/<host>/<iso>-<hash>.json`,
/// written by the netassess worker). Read-only — the worker owns the scan
/// cadence; this surfaces its most recent result.
fn lan_scan_row() -> Check {
    let label = "LAN scan (netassess)";
    let Some(latest) = latest_netassess_report() else {
        return Check::new(
            label,
            Status::Degraded,
            "no LAN scan yet — the netassess worker has not run",
        );
    };
    let (stamp, body) = latest;
    // The stored AssessmentSnapshot lists ARP-discovered LAN hosts under `arp`.
    let hosts = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("arp").and_then(|a| a.as_array()).map(Vec::len));
    match hosts {
        Some(n) => Check::new(
            label,
            Status::Pass,
            format!("last scan {stamp}: {n} LAN host(s) seen"),
        ),
        None => Check::new(
            label,
            Status::Degraded,
            format!("last scan {stamp}: unreadable"),
        ),
    }
}

/// Find the most recent netassess report file across all per-host subdirs;
/// returns its (filename-stamp, JSON body). The ISO8601 filename prefix sorts
/// lexicographically by time, so the max filename is the latest.
fn latest_netassess_report() -> Option<(String, String)> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        })?
        .join("mde")
        .join("netassess");
    let mut best: Option<std::path::PathBuf> = None;
    for host_dir in std::fs::read_dir(&base).ok()?.flatten() {
        let Ok(rd) = std::fs::read_dir(host_dir.path()) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if best.as_ref().is_none_or(|b| {
                p.file_name().unwrap_or_default() > b.file_name().unwrap_or_default()
            }) {
                best = Some(p);
            }
        }
    }
    let path = best?;
    let stamp = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split('-').next())
        .unwrap_or("?")
        .to_string();
    let body = std::fs::read_to_string(&path).ok()?;
    Some((stamp, body))
}

/// The Bus topic the voice agent publishes its status to (matches
/// `mde_voice_hud::sip::VOICE_STATUS_TOPIC` — kept as a literal here since that
/// const lives in a sibling binary crate, not a shared lib).
fn mde_voice_status_topic() -> &'static str {
    "state/voice/status"
}

/// Current wall-clock seconds since the Unix epoch (0 before it).
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The latest retained message body on `topic` (`None` if the topic is empty or
/// unreadable).
fn read_latest(persist: &mde_bus::persist::Persist, topic: &str) -> Option<String> {
    persist
        .list_since(topic, None)
        .ok()?
        .into_iter()
        .last()?
        .body
}

/// One Bus RPC → the reply body (`None` on timeout / no reply / no body).
async fn rpc_body(persist: &mde_bus::persist::Persist, topic: &str) -> Option<String> {
    mde_bus::rpc::request(
        persist,
        topic,
        mde_bus::hooks::config::Priority::Default,
        None,
        None,
        MESH_RPC_TIMEOUT,
    )
    .await
    .ok()?
    .body
}

/// Parse the `action/shell/healthz` reply (a `HealthReport` JSON line) into the
/// mackesd row. A missing reply means the control plane isn't answering.
fn parse_mackesd(body: Option<String>) -> Check {
    let label = "mackesd (control plane)";
    let Some(body) = body else {
        return Check::new(
            label,
            Status::Fail,
            "not responding — the mackesd control plane is down",
        )
        .with_fix(Fix::StartMackesd);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Check::new(label, Status::Fail, "unparseable health reply")
            .with_fix(Fix::StartMackesd);
    };
    if v.get("error").is_some() {
        return Check::new(label, Status::Fail, "health error from mackesd")
            .with_fix(Fix::StartMackesd);
    }
    let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("?");
    let nodes = v
        .get("node_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let unreachable = v
        .get("unreachable_nodes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let audit_ok = v
        .get("audit_chain_intact")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    if !audit_ok {
        Check::new(
            label,
            Status::Degraded,
            format!("v{version}, {nodes} node(s) — audit chain reported a break"),
        )
    } else if unreachable > 0 {
        Check::new(
            label,
            Status::Degraded,
            format!("v{version}, {nodes} node(s), {unreachable} unreachable"),
        )
    } else {
        Check::new(
            label,
            Status::Pass,
            format!("v{version} up, {nodes} node(s) known"),
        )
    }
}

/// Parse the `action/nebula/status` reply (a `StatusSnapshot`) into the overlay
/// row. Offline (no active transport) is Degraded — a lone node is legitimately
/// peerless — while a missing reply is a Fail.
fn parse_nebula(body: Option<String>) -> Check {
    let label = "Nebula overlay";
    let Some(body) = body else {
        return Check::new(label, Status::Fail, "no overlay status (mackesd down?)")
            .with_fix(Fix::ReEnroll);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Check::new(label, Status::Fail, "unparseable overlay status")
            .with_fix(Fix::ReEnroll);
    };
    if v.get("error").is_some() {
        return Check::new(label, Status::Fail, "overlay error from mackesd")
            .with_fix(Fix::ReEnroll);
    }
    let peers = v
        .get("peer_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let transport = v
        .get("active_transport")
        .and_then(|x| x.as_str())
        .unwrap_or("offline");
    if transport == "offline" {
        Check::new(
            label,
            Status::Degraded,
            "overlay offline — no active transport",
        )
        .with_fix(Fix::ReEnroll)
    } else {
        Check::new(
            label,
            Status::Pass,
            format!("on overlay ({transport}), {peers} peer(s)"),
        )
    }
}

/// Parse the `action/meshfs/status` reply (a `MeshFsStatusReport`) into the
/// storage row. Master unreachable / offline peers are Degraded (storage may be
/// optional or mid-bootstrap); a missing reply is a Fail.
fn parse_meshfs(body: Option<String>) -> Check {
    let label = "LizardFS storage";
    let Some(body) = body else {
        return Check::new(label, Status::Fail, "no storage status (mackesd down?)")
            .with_fix(Fix::OpenWorkbench);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Check::new(label, Status::Fail, "unparseable storage status")
            .with_fix(Fix::OpenWorkbench);
    };
    if v.get("error").is_some() {
        return Check::new(label, Status::Fail, "storage error from mackesd")
            .with_fix(Fix::OpenWorkbench);
    }
    let reachable = v
        .get("master_reachable")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let offline = v
        .get("offline_peers")
        .and_then(|x| x.as_array())
        .map_or(0, Vec::len);
    let peers = v
        .get("peers")
        .and_then(|x| x.as_array())
        .map_or(0, Vec::len);
    if !reachable {
        Check::new(
            label,
            Status::Degraded,
            "LizardFS master not reachable — storage not yet bootstrapped",
        )
        .with_fix(Fix::OpenWorkbench)
    } else if offline > 0 {
        Check::new(
            label,
            Status::Degraded,
            format!("master up, {peers} chunkserver(s), {offline} enrolled peer(s) offline"),
        )
        .with_fix(Fix::OpenWorkbench)
    } else {
        Check::new(
            label,
            Status::Pass,
            format!("master up, {peers} chunkserver(s)"),
        )
    }
}

/// Parse the voice agent's retained `state/voice/status` (published by
/// `mde-voice-hud --agent`) into the Voice row, treating a stale heartbeat
/// (`now - ts > VOICE_STALE_SECS`) as "agent not running". `registered +
/// listening` is Pass; `listening` without registration is Degraded; otherwise
/// Fail. The live-registrar path is the SIP-server bench (E5.4 / VOIP-28).
fn parse_voice(body: Option<String>, now: u64) -> Check {
    let label = "Softphone (SIP)";
    let Some(body) = body else {
        return Check::new(
            label,
            Status::Fail,
            "no SIP status on the Bus — the voice agent is not running",
        )
        .with_fix(Fix::OpenVoice);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Check::new(label, Status::Fail, "unparseable voice status")
            .with_fix(Fix::OpenVoice);
    };
    let ts = v.get("ts").and_then(serde_json::Value::as_u64).unwrap_or(0);
    if now.saturating_sub(ts) > VOICE_STALE_SECS {
        return Check::new(
            label,
            Status::Fail,
            "voice status is stale — the agent stopped publishing (not running?)",
        )
        .with_fix(Fix::OpenVoice);
    }
    let registered = v
        .get("registered")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let listening = v
        .get("listening")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let detail = v.get("detail").and_then(|x| x.as_str()).unwrap_or("");
    let server = v.get("server").and_then(|x| x.as_str()).unwrap_or("");
    if registered && listening {
        Check::new(
            label,
            Status::Pass,
            format!("registered · {server} — listening for inbound calls"),
        )
    } else if listening {
        Check::new(
            label,
            Status::Degraded,
            format!("listening, but not registered ({detail})"),
        )
        .with_fix(Fix::OpenVoice)
    } else {
        Check::new(
            label,
            Status::Fail,
            format!("not listening for calls ({detail})"),
        )
        .with_fix(Fix::OpenVoice)
    }
}

/// One-shot per-section health roll-up for the panel's post-commissioning watch
/// (E7.6b). Reuses the section probes; the labels match the dashboard headers so
/// a toast names the same section the operator sees in `mde birthright`. Blocks
/// on the Bus RPCs, so the panel calls this from a background thread.
#[must_use]
pub fn health_summary() -> Vec<(&'static str, Status)> {
    let live = probe_live();
    vec![
        ("Desktop", rollup(&probe_desktop())),
        ("Mesh", rollup(&live.mesh.rows())),
        ("Voice", live.voice.status),
        ("Network", rollup(&live.network)),
    ]
}

// --- report export (E7.6) ---------------------------------------------------

/// Stable string for a status, for the exported report.
fn status_str(s: Status) -> &'static str {
    match s {
        Status::Checking => "checking",
        Status::Pass => "pass",
        Status::Degraded => "degraded",
        Status::Fail => "fail",
    }
}

/// JSON array of `{check,status,detail}` for one section's rows.
fn rows_json(rows: &[Check]) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|c| {
            serde_json::json!({
                "check": c.label,
                "status": status_str(c.status),
                "detail": c.detail,
            })
        })
        .collect()
}

/// This host's name (`/proc/sys/kernel/hostname`), for the report header.
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Build the commissioning report (pretty JSON) from the dashboard's current
/// rows — the attestation artifact a Copy/Save produces.
fn build_report(state: &Birthright) -> String {
    let report = serde_json::json!({
        "product": "Mackes Workstation",
        "report": "birthright-commissioning",
        "host": hostname(),
        "generated_ts": now_unix(),
        "sections": {
            "desktop": rows_json(&state.desktop),
            "mesh": rows_json(&state.mesh_rows),
            "voice": rows_json(std::slice::from_ref(&state.voice_row)),
            "network": rows_json(&state.network_rows),
        },
    });
    serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
}

/// Pipe the report into `wl-copy`. Returns a footer confirmation string.
fn copy_report(report: &str) -> String {
    use std::io::Write;
    use std::process::Stdio;
    match Command::new("wl-copy").stdin(Stdio::piped()).spawn() {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(report.as_bytes());
            }
            let _ = child.wait();
            "Copied diagnostics to clipboard".to_string()
        }
        Err(_) => "Copy failed — wl-copy not available".to_string(),
    }
}

/// `~/.local/share/mde/birthright/` — where saved reports land.
fn report_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        })?;
    Some(base.join("mde").join("birthright"))
}

/// Write the report to a timestamped file. Returns a footer confirmation.
fn save_report(report: &str) -> String {
    let Some(dir) = report_dir() else {
        return "Export failed — no data dir".to_string();
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return format!("Export failed: {e}");
    }
    let path = dir.join(format!("{}-commissioning.json", now_unix()));
    match std::fs::write(&path, report) {
        Ok(()) => format!("Exported report to {}", path.display()),
        Err(e) => format!("Export failed: {e}"),
    }
}

// --- view -------------------------------------------------------------------

fn label(s: impl text::IntoFragment<'static>) -> iced::widget::Text<'static> {
    text(s).size(metrics::UI_PX)
}

/// One status row: a fixed-width status chip, the label + detail, and (when the
/// row is not green and offers one) a remediation button.
fn check_row(c: &Check) -> Element<'static, Message> {
    let chip = container(
        text(c.status.glyph())
            .size(metrics::UI_PX)
            .font(mde_ui::font::ui_bold())
            .color(palette::color(c.status.color())),
    )
    .width(Length::Fixed(28.0));

    let mut row = Row::new()
        .spacing(metrics::SPACING_03)
        .align_y(iced::Alignment::Center)
        .push(chip)
        .push(
            Column::new()
                .spacing(metrics::SPACING_01)
                .push(label(c.label).font(mde_ui::font::ui_bold()))
                .push(label(c.detail.clone()).color(palette::color(palette::GRAY_TEXT)))
                .width(Length::Fill),
        );

    if c.status != Status::Pass && c.status != Status::Checking {
        if let Some(fix) = c.fix {
            row = row.push(
                button(label(fix.label()))
                    .on_press(Message::Fix(fix))
                    .height(Length::Fixed(metrics::BUTTON_MD)),
            );
        }
    }
    row.into()
}

/// A titled section card with a rolled-up status chip in its header.
fn section_card(title: &'static str, checks: &[Check]) -> Element<'static, Message> {
    let roll = rollup(checks);
    let header = Row::new()
        .spacing(metrics::SPACING_02)
        .align_y(iced::Alignment::Center)
        .push(
            text(roll.glyph())
                .size(metrics::UI_PX)
                .font(mde_ui::font::ui_bold())
                .color(palette::color(roll.color())),
        )
        .push(
            text(title)
                .size(metrics::INFO_TITLE_PX)
                .font(mde_ui::font::ui_bold()),
        );

    let mut body = Column::new().spacing(metrics::SPACING_03).push(header);
    for c in checks {
        body = body.push(check_row(c));
    }

    container(
        body.spacing(metrics::SPACING_03)
            .padding(metrics::SPACING_04),
    )
    .width(Length::Fill)
    .style(|_| container::Style {
        background: Some(iced::Background::Color(palette::color(palette::WINDOW))),
        ..container::Style::default()
    })
    .into()
}

fn view(state: &Birthright) -> Element<'_, Message> {
    let header = Column::new()
        .spacing(metrics::SPACING_01)
        .push(
            text("Birthright Commissioning")
                .size(metrics::INFO_TITLE_PX)
                .font(mde_ui::font::ui_bold()),
        )
        .push(
            label("Confirms this workstation came up whole.")
                .color(palette::color(palette::GRAY_TEXT)),
        );

    let voice_rows = std::slice::from_ref(&state.voice_row);
    let sections = scrollable(
        Column::new()
            .spacing(metrics::SPACING_04)
            .push(section_card("Desktop", &state.desktop))
            .push(section_card("Mesh", &state.mesh_rows))
            .push(section_card("Voice", voice_rows))
            .push(section_card("Network", &state.network_rows)),
    )
    .height(Length::Fill);

    let buttons = Row::new()
        .spacing(metrics::SPACING_04)
        .align_y(iced::Alignment::Center)
        .push(
            checkbox("Show this at startup", state.show_at_startup)
                .on_toggle(Message::ToggleStartup)
                .size(metrics::UI_PX)
                .text_size(metrics::UI_PX),
        )
        .push(Space::with_width(Length::Fill))
        .push(
            button(label("Copy diagnostics"))
                .on_press(Message::CopyReport)
                .height(Length::Fixed(metrics::BUTTON_MD)),
        )
        .push(
            button(label("Export report"))
                .on_press(Message::SaveReport)
                .height(Length::Fixed(metrics::BUTTON_MD)),
        )
        .push(
            button(label("Re-check all"))
                .on_press(Message::Recheck)
                .height(Length::Fixed(metrics::BUTTON_MD)),
        )
        .push(
            button(label("Close"))
                .on_press(Message::Close)
                .height(Length::Fixed(metrics::BUTTON_MD)),
        );

    // Footer = the action row, plus a transient confirmation after Copy/Save.
    let mut footer = Column::new().spacing(metrics::SPACING_02).push(buttons);
    if let Some(msg) = &state.last_action {
        footer = footer.push(label(msg.clone()).color(palette::color(palette::GRAY_TEXT)));
    }

    let body = Column::new()
        .spacing(metrics::SPACING_05)
        .padding(metrics::SPACING_05)
        .push(header)
        .push(sections)
        .push(footer);

    container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(palette::color(palette::MENU))),
            ..container::Style::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn basename_strips_dirs() {
        assert_eq!(basename("/usr/bin/labwc"), "labwc");
        assert_eq!(basename("labwc"), "labwc");
        assert_eq!(basename("/usr/bin/mde"), "mde");
    }

    #[test]
    fn labwc_argv_matches_only_labwc() {
        assert!(argv_is_labwc(&s(&["/usr/bin/labwc"])));
        assert!(argv_is_labwc(&s(&["labwc", "-C", "/etc/labwc"])));
        assert!(!argv_is_labwc(&s(&["/usr/bin/mde", "panel"])));
        assert!(!argv_is_labwc(&[]));
    }

    #[test]
    fn panel_argv_matches_mde_panel_not_birthright() {
        assert!(argv_is_mde_panel(&s(&["/usr/bin/mde", "panel"])));
        assert!(argv_is_mde_panel(&s(&["mde", "panel"])));
        assert!(argv_is_mde_panel(&s(&["/usr/bin/mde-panel"])));
        // Must NOT match the dashboard itself, nor other mde subcommands.
        assert!(!argv_is_mde_panel(&s(&["/usr/bin/mde", "birthright"])));
        assert!(!argv_is_mde_panel(&s(&["/usr/bin/mde", "files"])));
        assert!(!argv_is_mde_panel(&[]));
    }

    #[test]
    fn rollup_is_worst_of() {
        let mk = |st: Status| Check::new("x", st, "");
        assert_eq!(rollup(&[mk(Status::Pass), mk(Status::Pass)]), Status::Pass);
        assert_eq!(
            rollup(&[mk(Status::Pass), mk(Status::Degraded)]),
            Status::Degraded
        );
        assert_eq!(
            rollup(&[mk(Status::Fail), mk(Status::Degraded)]),
            Status::Fail
        );
        assert_eq!(
            rollup(&[mk(Status::Checking), mk(Status::Pass)]),
            Status::Checking
        );
        // Fail dominates Checking.
        assert_eq!(
            rollup(&[mk(Status::Checking), mk(Status::Fail)]),
            Status::Fail
        );
    }

    #[test]
    fn checking_seeds_expected_rows() {
        assert_eq!(desktop_checking().len(), 3);
        assert!(desktop_checking()
            .iter()
            .all(|c| c.status == Status::Checking));
        assert_eq!(mesh_checking().len(), 4);
        assert!(mesh_checking().iter().all(|c| c.status == Status::Checking));
    }

    #[test]
    fn fix_argv_is_real_commands() {
        assert_eq!(
            fix_argv(Fix::StartMackesd),
            vec!["systemctl", "--user", "start", "mackesd"]
        );
        // Re-enroll re-execs this binary's `oobe`.
        let reenroll = fix_argv(Fix::ReEnroll);
        assert_eq!(reenroll.last().unwrap(), "oobe");
        assert_eq!(reenroll.len(), 2);
        assert_eq!(fix_argv(Fix::OpenWorkbench), vec!["mde-workbench"]);
    }

    #[test]
    fn mackesd_parse_missing_reply_is_fail_with_start_fix() {
        let c = parse_mackesd(None);
        assert_eq!(c.status, Status::Fail);
        assert_eq!(c.fix, Some(Fix::StartMackesd));
    }

    #[test]
    fn mackesd_parse_healthy_report_is_pass() {
        let body = r#"{"schema":1,"is_leader":true,"applied_revision":null,"node_count":3,"healthy_nodes":3,"degraded_nodes":0,"unreachable_nodes":0,"audit_chain_intact":true,"version":"10.0.0"}"#;
        let c = parse_mackesd(Some(body.to_string()));
        assert_eq!(c.status, Status::Pass);
        assert!(c.fix.is_none());
        assert!(c.detail.contains("10.0.0"));
    }

    #[test]
    fn mackesd_parse_broken_audit_is_degraded() {
        let body = r#"{"node_count":2,"unreachable_nodes":0,"audit_chain_intact":false,"version":"10.0.0"}"#;
        assert_eq!(
            parse_mackesd(Some(body.to_string())).status,
            Status::Degraded
        );
    }

    #[test]
    fn nebula_parse_offline_is_degraded_online_is_pass() {
        let off = r#"{"peer_count":0,"active_transport":"offline"}"#;
        assert_eq!(parse_nebula(Some(off.to_string())).status, Status::Degraded);
        let on = r#"{"peer_count":4,"active_transport":"nebula_direct"}"#;
        let c = parse_nebula(Some(on.to_string()));
        assert_eq!(c.status, Status::Pass);
        assert!(c.detail.contains("4 peer"));
    }

    #[test]
    fn meshfs_parse_master_states() {
        let down = r#"{"master_reachable":false,"peers":[],"offline_peers":[]}"#;
        assert_eq!(
            parse_meshfs(Some(down.to_string())).status,
            Status::Degraded
        );
        let healthy = r#"{"master_reachable":true,"peers":[{"addr":"a"}],"offline_peers":[]}"#;
        assert_eq!(parse_meshfs(Some(healthy.to_string())).status, Status::Pass);
        let degraded = r#"{"master_reachable":true,"peers":[{"addr":"a"}],"offline_peers":["b"]}"#;
        assert_eq!(
            parse_meshfs(Some(degraded.to_string())).status,
            Status::Degraded
        );
        assert_eq!(parse_meshfs(None).status, Status::Fail);
    }

    #[test]
    fn all_fail_marks_every_row_failed() {
        let p = MeshProbe::all_fail("no bus");
        assert!(p.rows().iter().all(|c| c.status == Status::Fail));
        assert_eq!(p.rows().len(), 4);
    }

    #[test]
    fn live_all_fail_covers_mesh_and_voice() {
        let p = LiveProbe::all_fail("no bus");
        assert!(p.mesh.rows().iter().all(|c| c.status == Status::Fail));
        assert_eq!(p.voice.status, Status::Fail);
        assert_eq!(p.voice.fix, Some(Fix::OpenVoice));
    }

    #[test]
    fn voice_parse_states() {
        let now = 1_700_000_000;
        // No status published → agent not running → Fail.
        assert_eq!(parse_voice(None, now).status, Status::Fail);
        // Registered + listening → Pass.
        let reg = format!(
            r#"{{"registered":true,"listening":true,"server":"sip.x:5060","detail":"Registered","ts":{now}}}"#
        );
        let c = parse_voice(Some(reg), now);
        assert_eq!(c.status, Status::Pass);
        assert!(c.detail.contains("sip.x:5060"));
        // Listening but not registered → Degraded.
        let unreg = format!(
            r#"{{"registered":false,"listening":true,"server":"","detail":"Not registered","ts":{now}}}"#
        );
        assert_eq!(parse_voice(Some(unreg), now).status, Status::Degraded);
        // Not listening → Fail.
        let down =
            format!(r#"{{"registered":false,"listening":false,"detail":"no route","ts":{now}}}"#);
        assert_eq!(parse_voice(Some(down), now).status, Status::Fail);
        // Stale heartbeat (older than the window) → Fail even if it claims up.
        let stale = r#"{"registered":true,"listening":true,"server":"s","ts":1}"#;
        assert_eq!(
            parse_voice(Some(stale.to_string()), now).status,
            Status::Fail
        );
    }

    #[test]
    fn open_voice_fix_argv_is_voice_config() {
        assert_eq!(fix_argv(Fix::OpenVoice), vec!["mde-voice-config"]);
    }

    #[test]
    fn nebula_peers_parse_states() {
        // No reply → Fail.
        assert_eq!(parse_nebula_peers(None).status, Status::Fail);
        // Empty roster → Degraded (lone node).
        assert_eq!(
            parse_nebula_peers(Some("[]".to_string())).status,
            Status::Degraded
        );
        // Error envelope → Fail.
        assert_eq!(
            parse_nebula_peers(Some(r#"{"error":"x"}"#.to_string())).status,
            Status::Fail
        );
        // Peers with one online → Pass.
        let peers = r#"[{"name":"a","reachable":"online"},{"name":"b","reachable":"offline"}]"#;
        let c = parse_nebula_peers(Some(peers.to_string()));
        assert_eq!(c.status, Status::Pass);
        assert!(c.detail.contains("2 paired"));
        assert!(c.detail.contains("1 online"));
        // Peers but none online → Degraded.
        let idle = r#"[{"name":"a","reachable":"idle"}]"#;
        assert_eq!(
            parse_nebula_peers(Some(idle.to_string())).status,
            Status::Degraded
        );
    }

    #[test]
    fn network_checking_seeds_three_rows() {
        assert_eq!(network_checking().len(), 3);
        assert!(network_checking()
            .iter()
            .all(|c| c.status == Status::Checking));
    }

    #[test]
    fn status_str_is_stable() {
        assert_eq!(status_str(Status::Pass), "pass");
        assert_eq!(status_str(Status::Degraded), "degraded");
        assert_eq!(status_str(Status::Fail), "fail");
        assert_eq!(status_str(Status::Checking), "checking");
    }

    #[test]
    fn rows_json_carries_check_status_detail() {
        let rows = vec![
            Check::new("Compositor", Status::Pass, "labwc is running"),
            Check::new("Panel", Status::Fail, "not running"),
        ];
        let j = rows_json(&rows);
        assert_eq!(j.len(), 2);
        assert_eq!(j[0]["check"], "Compositor");
        assert_eq!(j[0]["status"], "pass");
        assert_eq!(j[0]["detail"], "labwc is running");
        assert_eq!(j[1]["status"], "fail");
    }
}
