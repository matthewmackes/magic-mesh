//! Live Browser VM RDP observer for `serve-browser-vm-performance.py`.
//!
//! The observer deliberately uses the same public `mde-vdi-rdp` connection,
//! decode, damage, and input APIs as Construct's shell.  It emits only measured
//! internal telemetry on stdout.  It does not implement the public v4 endpoint,
//! invent guest counters, or retain a credential.

use std::fs;
use std::io::{self, BufRead, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use mde_vdi_core::FrameDamage;
use mde_vdi_rdp::egui::{pos2, ColorImage, Event, Modifiers, PointerButton};
use mde_vdi_rdp::{PumpOutcome, RdpConfig, RdpConnection, RdpSession};
use serde_json::{json, Value};

const WIDTH: u16 = 1920;
const HEIGHT: u16 = 1080;
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);
const PUMP_TIMEOUT: Duration = Duration::from_millis(40);
const FIRST_INPUT_AT_MS: u64 = 305_000;
const LAST_INPUT_AT_MS: u64 = 545_000;
const INPUT_INTERVAL_MS: u64 = 15_000;
const INPUT_SETTLE: Duration = Duration::from_millis(250);
const INPUT_TIMEOUT: Duration = Duration::from_millis(1_000);
const RECONNECT_AT_MS: u64 = 560_000;
const RECONNECT_HOLD: Duration = Duration::from_secs(7);
const BEACON_POINT: (u16, u16) = (104, 183);
const BEACON_PATCH: (usize, usize, usize, usize) = (32, 142, 152, 72);

#[derive(Debug)]
struct Args {
    host: String,
    port: u16,
    username: String,
    credential_file: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Begin,
    Hidden,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    Connected,
    Reconnecting,
    Failed,
}

impl ConnectionState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Default)]
struct CpuSampler {
    previous_process_ticks: Option<u64>,
    previous_total_ticks: Option<u64>,
}

impl CpuSampler {
    fn sample(&mut self) -> Option<u32> {
        let process = fs::read_to_string("/proc/self/stat")
            .ok()
            .and_then(|raw| parse_process_ticks(&raw));
        let total = fs::read_to_string("/proc/stat")
            .ok()
            .and_then(|raw| {
                raw.lines()
                    .find(|line| line.starts_with("cpu "))
                    .map(str::to_owned)
            })
            .and_then(|line| parse_total_ticks(&line));
        let value = match (
            process,
            total,
            self.previous_process_ticks,
            self.previous_total_ticks,
        ) {
            (Some(process), Some(total), Some(previous_process), Some(previous_total)) => {
                let process_delta = process.saturating_sub(previous_process);
                let total_delta = total.saturating_sub(previous_total);
                (total_delta > 0).then(|| {
                    u32::try_from(
                        process_delta
                            .saturating_mul(100_000)
                            .checked_div(total_delta)
                            .unwrap_or(0)
                            .min(100_000),
                    )
                    .unwrap_or(100_000)
                })
            }
            _ => None,
        };
        self.previous_process_ticks = process;
        self.previous_total_ticks = total;
        value
    }
}

#[derive(Debug)]
struct PendingInput {
    baseline: u64,
    sent_at: Instant,
}

#[derive(Debug)]
struct Measurements {
    begun_at: Option<Instant>,
    frames_received: u64,
    full_uploads: u64,
    partial_uploads: u64,
    partial_rects: u64,
    surface_repaints: u64,
    browser_visible: bool,
    reconnects: u64,
    pointer_updates: u64,
    pointer_x: u16,
    pointer_y: u16,
    max_frame_gap_ms: u64,
    previous_connected_frame: Option<Instant>,
    connection_state: ConnectionState,
    last_frame: Option<ColorImage>,
    next_input_at_ms: u64,
    input_baseline_due: Option<Instant>,
    pending_input: Option<PendingInput>,
    new_session_latencies_ms: Vec<u64>,
    reconnect_started: bool,
    reconnect_due: Option<Instant>,
    cpu: CpuSampler,
}

impl Default for Measurements {
    fn default() -> Self {
        Self {
            begun_at: None,
            frames_received: 0,
            full_uploads: 0,
            partial_uploads: 0,
            partial_rects: 0,
            surface_repaints: 0,
            browser_visible: false,
            reconnects: 0,
            pointer_updates: 0,
            pointer_x: 0,
            pointer_y: 0,
            max_frame_gap_ms: 0,
            previous_connected_frame: None,
            connection_state: ConnectionState::Connected,
            last_frame: None,
            next_input_at_ms: FIRST_INPUT_AT_MS,
            input_baseline_due: None,
            pending_input: None,
            new_session_latencies_ms: Vec::new(),
            reconnect_started: false,
            reconnect_due: None,
            cpu: CpuSampler::default(),
        }
    }
}

impl Measurements {
    fn begin(&mut self) {
        let last_frame = self.last_frame.take();
        *self = Self::default();
        self.last_frame = last_frame;
        self.begun_at = Some(Instant::now());
        self.browser_visible = true;
    }

    fn elapsed_ms(&self) -> u64 {
        self.begun_at
            .map(|started| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    fn browser_visible(&self) -> bool {
        self.browser_visible
    }

    fn note_frame(&mut self, frame: ColorImage, damage: FrameDamage) {
        let now = Instant::now();
        if self.connection_state == ConnectionState::Connected && self.browser_visible() {
            if let Some(previous) = self.previous_connected_frame {
                let gap =
                    u64::try_from(now.duration_since(previous).as_millis()).unwrap_or(u64::MAX);
                self.max_frame_gap_ms = self.max_frame_gap_ms.max(gap);
            }
            self.previous_connected_frame = Some(now);
        }
        self.frames_received = self.frames_received.saturating_add(1);
        match damage {
            FrameDamage::Full => self.full_uploads = self.full_uploads.saturating_add(1),
            FrameDamage::Rects(rects) => {
                self.partial_uploads = self.partial_uploads.saturating_add(1);
                self.partial_rects = self
                    .partial_rects
                    .saturating_add(u64::try_from(rects.len()).unwrap_or(u64::MAX));
            }
        }
        // This is the real RDP presentation damage count.  It continues across
        // the hidden phase so quiescence can only pass when transport damage
        // actually stops after Chromium has been minimized by the controller.
        self.surface_repaints = self.surface_repaints.saturating_add(1);
        if let Some(pending) = self.pending_input.as_ref() {
            let observed = patch_hash(&frame, BEACON_PATCH);
            if observed != pending.baseline {
                let latency =
                    u64::try_from(pending.sent_at.elapsed().as_millis()).unwrap_or(u64::MAX);
                if latency > 0 {
                    self.new_session_latencies_ms.push(latency);
                }
                self.pending_input = None;
            } else if pending.sent_at.elapsed() > INPUT_TIMEOUT {
                self.pending_input = None;
            }
        }
        self.last_frame = Some(frame);
    }

    fn snapshot(&mut self, source_instance_id: &str) -> Value {
        let latencies = std::mem::take(&mut self.new_session_latencies_ms);
        json!({
            "type": "rdp_snapshot",
            "source_instance_id": source_instance_id,
            "elapsed_ms": self.elapsed_ms(),
            "frames_received": self.frames_received,
            "max_frame_gap_ms": self.max_frame_gap_ms,
            "pointer_updates": self.pointer_updates,
            "pointer_x": self.pointer_x,
            "pointer_y": self.pointer_y,
            "full_uploads": self.full_uploads,
            "partial_uploads": self.partial_uploads,
            "partial_rects": self.partial_rects,
            "surface_repaints": self.surface_repaints,
            "reconnects": self.reconnects,
            "connection_state": self.connection_state.as_str(),
            "browser_visible": self.browser_visible(),
            "session_latencies_ms": latencies,
            "host_process_cpu_permille": self.cpu.sample(),
            "host_process_rss_kib": process_rss_kib(),
        })
    }
}

fn parse_process_ticks(raw: &str) -> Option<u64> {
    let fields = raw
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    fields
        .get(11)?
        .parse::<u64>()
        .ok()?
        .checked_add(fields.get(12)?.parse::<u64>().ok()?)
}

fn parse_total_ticks(raw: &str) -> Option<u64> {
    let mut fields = raw.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    fields.try_fold(0_u64, |total, value| {
        total.checked_add(value.parse::<u64>().ok()?)
    })
}

fn process_rss_kib() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
}

fn patch_hash(image: &ColorImage, patch: (usize, usize, usize, usize)) -> u64 {
    let (x, y, width, height) = patch;
    let right = x.saturating_add(width).min(image.size[0]);
    let bottom = y.saturating_add(height).min(image.size[1]);
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for row in y.min(image.size[1])..bottom {
        for column in x.min(image.size[0])..right {
            for byte in image.pixels[row * image.size[0] + column].to_array() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    hash
}

fn emit(stdout: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

fn control_reader(sender: mpsc::Sender<Control>) {
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let control = match line.trim() {
            "begin" => Some(Control::Begin),
            "hidden" => Some(Control::Hidden),
            "stop" => Some(Control::Stop),
            _ => None,
        };
        if let Some(control) = control {
            if sender.send(control).is_err() {
                break;
            }
        }
    }
}

fn parse_args() -> Result<Args, String> {
    let mut host = None;
    let mut port = 3389_u16;
    let mut username = None;
    let mut credential_file = None;
    let mut raw = std::env::args().skip(1);
    while let Some(argument) = raw.next() {
        let value = match argument.as_str() {
            "--host" | "--port" | "--username" | "--credential-file" => raw
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))?,
            _ => return Err(format!("unexpected argument: {argument}")),
        };
        match argument.as_str() {
            "--host" => host = Some(value),
            "--port" => {
                port = value
                    .parse::<u16>()
                    .map_err(|_| "--port must be an integer from 1 through 65535".to_string())?;
                if port == 0 {
                    return Err("--port must be nonzero".to_string());
                }
            }
            "--username" => username = Some(value),
            "--credential-file" => credential_file = Some(PathBuf::from(value)),
            _ => unreachable!(),
        }
    }
    let host = host.ok_or_else(|| "--host is required".to_string())?;
    let username = username.ok_or_else(|| "--username is required".to_string())?;
    if host.trim().is_empty() || host.chars().any(char::is_whitespace) {
        return Err("--host is malformed".to_string());
    }
    if username.trim().is_empty() || username.chars().any(char::is_whitespace) {
        return Err("--username is malformed".to_string());
    }
    Ok(Args {
        host,
        port,
        username,
        credential_file: credential_file
            .ok_or_else(|| "--credential-file is required".to_string())?,
    })
}

fn read_credential(path: &Path) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("credential file is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("credential file must be a regular non-symlink file".to_string());
    }
    if metadata.mode() & 0o077 != 0 {
        return Err("credential file must not grant group/other permissions".to_string());
    }
    let credential = fs::read_to_string(path)
        .map_err(|error| format!("credential file is unreadable: {error}"))?;
    let credential = credential.trim_end_matches(['\r', '\n']).to_owned();
    if credential.is_empty() || credential.len() > 4_096 || credential.contains('\0') {
        return Err("credential file contains no bounded secret".to_string());
    }
    Ok(credential)
}

fn source_instance_id() -> Result<String, String> {
    let value = fs::read_to_string("/proc/sys/kernel/random/uuid")
        .map_err(|error| format!("cannot allocate source instance identity: {error}"))?;
    let value = value.trim().to_ascii_lowercase();
    if value.len() != 36 {
        return Err("kernel returned a malformed source instance identity".to_string());
    }
    Ok(value)
}

fn connect(session: &mut RdpSession) -> Result<RdpConnection, String> {
    RdpConnection::connect(session).map_err(|error| format!("RDP connect failed: {error}"))
}

fn send_input_probe(
    connection: &mut RdpConnection,
    session: &mut RdpSession,
    measurements: &mut Measurements,
) -> Result<(), String> {
    let Some(frame) = measurements.last_frame.as_ref() else {
        return Ok(());
    };
    let baseline = patch_hash(frame, BEACON_PATCH);
    let position = pos2(f32::from(BEACON_POINT.0), f32::from(BEACON_POINT.1));
    session.send_input(&Event::PointerButton {
        pos: position,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::default(),
    });
    connection
        .flush_input(session)
        .map_err(|error| format!("RDP pointer-down flush failed: {error}"))?;
    thread::sleep(Duration::from_millis(75));
    session.send_input(&Event::PointerButton {
        pos: position,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::default(),
    });
    connection
        .flush_input(session)
        .map_err(|error| format!("RDP pointer-up flush failed: {error}"))?;
    measurements.pending_input = Some(PendingInput {
        baseline,
        sent_at: Instant::now(),
    });
    Ok(())
}

fn arm_pointer(
    connection: &mut RdpConnection,
    session: &mut RdpSession,
    measurements: &mut Measurements,
) -> Result<(), String> {
    session.send_input(&Event::PointerMoved(pos2(
        f32::from(BEACON_POINT.0),
        f32::from(BEACON_POINT.1),
    )));
    connection
        .flush_input(session)
        .map_err(|error| format!("RDP pointer move failed: {error}"))?;
    measurements.pointer_x = BEACON_POINT.0;
    measurements.pointer_y = BEACON_POINT.1;
    measurements.pointer_updates = measurements.pointer_updates.saturating_add(1);
    measurements.input_baseline_due = Some(Instant::now() + INPUT_SETTLE);
    Ok(())
}

fn run(args: Args) -> Result<(), String> {
    let secret = read_credential(&args.credential_file)?;
    let source_instance_id = source_instance_id()?;
    let config = RdpConfig::new(args.host, args.username, secret)
        .with_port(args.port)
        .with_resolution(WIDTH, HEIGHT);
    let mut session =
        RdpSession::new(config).map_err(|error| format!("invalid RDP config: {error}"))?;
    let _initial_black_frame = session.frame();
    let mut connection = Some(connect(&mut session)?);
    let negotiated = connection
        .as_ref()
        .expect("connection just installed")
        .negotiated()
        .desktop_size;
    if negotiated != (WIDTH, HEIGHT) {
        return Err(format!(
            "RDP server negotiated {}x{} instead of {WIDTH}x{HEIGHT}",
            negotiated.0, negotiated.1
        ));
    }

    let mut stdout = io::BufWriter::new(io::stdout().lock());
    emit(
        &mut stdout,
        &json!({
            "type": "rdp_ready",
            "source_instance_id": source_instance_id,
            "width": negotiated.0,
            "height": negotiated.1,
            "pid": std::process::id(),
        }),
    )
    .map_err(|error| format!("stdout failed: {error}"))?;

    let (control_tx, control_rx) = mpsc::channel();
    thread::Builder::new()
        .name("performance-rdp-control".to_string())
        .spawn(move || control_reader(control_tx))
        .map_err(|error| format!("cannot spawn control reader: {error}"))?;

    let mut measurements = Measurements::default();
    let mut next_snapshot = Instant::now() + SNAPSHOT_INTERVAL;
    loop {
        while let Ok(control) = control_rx.try_recv() {
            match control {
                Control::Begin => {
                    measurements.begin();
                    next_snapshot = Instant::now();
                }
                Control::Hidden => {
                    measurements.browser_visible = false;
                    measurements.previous_connected_frame = None;
                    emit(&mut stdout, &measurements.snapshot(&source_instance_id))
                        .map_err(|error| format!("stdout failed: {error}"))?;
                }
                Control::Stop => {
                    if let Some(connected) = connection.take() {
                        let _ = connected.shutdown(&mut session);
                    }
                    return Ok(());
                }
            }
        }

        if measurements.begun_at.is_some() {
            let elapsed = measurements.elapsed_ms();
            if (FIRST_INPUT_AT_MS..=LAST_INPUT_AT_MS).contains(&elapsed)
                && elapsed >= measurements.next_input_at_ms
                && measurements.input_baseline_due.is_none()
                && measurements.pending_input.is_none()
                && measurements.connection_state == ConnectionState::Connected
            {
                if measurements.pointer_updates == 0 {
                    if let Some(connected) = connection.as_mut() {
                        arm_pointer(connected, &mut session, &mut measurements)?;
                    }
                } else if let Some(connected) = connection.as_mut() {
                    send_input_probe(connected, &mut session, &mut measurements)?;
                    measurements.next_input_at_ms = measurements
                        .next_input_at_ms
                        .saturating_add(INPUT_INTERVAL_MS);
                }
            }
            if measurements
                .input_baseline_due
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                measurements.input_baseline_due = None;
                if let Some(connected) = connection.as_mut() {
                    send_input_probe(connected, &mut session, &mut measurements)?;
                    measurements.next_input_at_ms = measurements
                        .next_input_at_ms
                        .saturating_add(INPUT_INTERVAL_MS);
                }
            }

            if elapsed >= RECONNECT_AT_MS && !measurements.reconnect_started {
                measurements.reconnect_started = true;
                measurements.connection_state = ConnectionState::Reconnecting;
                measurements.reconnects = measurements.reconnects.saturating_add(1);
                measurements.previous_connected_frame = None;
                if let Some(connected) = connection.take() {
                    connected
                        .shutdown(&mut session)
                        .map_err(|error| format!("RDP disconnect failed: {error}"))?;
                }
                measurements.reconnect_due = Some(Instant::now() + RECONNECT_HOLD);
                emit(&mut stdout, &measurements.snapshot(&source_instance_id))
                    .map_err(|error| format!("stdout failed: {error}"))?;
            }
            if measurements
                .reconnect_due
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                measurements.reconnect_due = None;
                match connect(&mut session) {
                    Ok(connected) => {
                        connection = Some(connected);
                        measurements.connection_state = ConnectionState::Connected;
                        measurements.previous_connected_frame = None;
                    }
                    Err(error) => {
                        measurements.connection_state = ConnectionState::Failed;
                        emit(&mut stdout, &json!({"type": "rdp_error", "reason": error}))
                            .map_err(|write_error| format!("stdout failed: {write_error}"))?;
                    }
                }
            }
        }

        if let Some(connected) = connection.as_mut() {
            match connected.pump_once(&mut session, PUMP_TIMEOUT) {
                Ok(PumpOutcome::Processed { painted_rects }) if painted_rects > 0 => {
                    if let Some((frame, damage)) = session.frame_with_damage() {
                        measurements.note_frame(frame, damage);
                    }
                }
                Ok(PumpOutcome::Processed { .. } | PumpOutcome::TimedOut) => {}
                Ok(PumpOutcome::Terminated { reason }) => {
                    measurements.connection_state = ConnectionState::Failed;
                    emit(&mut stdout, &json!({"type": "rdp_error", "reason": reason}))
                        .map_err(|error| format!("stdout failed: {error}"))?;
                    connection = None;
                }
                Err(error) => {
                    measurements.connection_state = ConnectionState::Failed;
                    emit(
                        &mut stdout,
                        &json!({"type": "rdp_error", "reason": error.to_string()}),
                    )
                    .map_err(|write_error| format!("stdout failed: {write_error}"))?;
                    connection = None;
                }
            }
        } else {
            thread::sleep(Duration::from_millis(20));
        }

        if measurements.begun_at.is_some() && Instant::now() >= next_snapshot {
            emit(&mut stdout, &measurements.snapshot(&source_instance_id))
                .map_err(|error| format!("stdout failed: {error}"))?;
            next_snapshot += SNAPSHOT_INTERVAL;
            if next_snapshot < Instant::now() {
                next_snapshot = Instant::now() + SNAPSHOT_INTERVAL;
            }
        }
    }
}

fn self_test() {
    assert_eq!(
        parse_process_ticks("1 (a process) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14"),
        Some(23)
    );
    assert_eq!(parse_total_ticks("cpu  1 2 3 4 5\n"), Some(15));
    assert_eq!(ConnectionState::Connected.as_str(), "connected");
    let black = ColorImage::new([192, 256], mde_vdi_rdp::egui::Color32::BLACK);
    let mut white = black.clone();
    white.pixels[150 * 192 + 40] = mde_vdi_rdp::egui::Color32::WHITE;
    assert_ne!(
        patch_hash(&black, (32, 142, 152, 72)),
        patch_hash(&white, (32, 142, 152, 72))
    );
    println!("serve-browser-vm-performance-rdp: self-test passed");
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--self-test") {
        self_test();
        return;
    }
    let result = parse_args().and_then(run);
    if let Err(error) = result {
        let mut stdout = io::BufWriter::new(io::stdout().lock());
        let _ = emit(&mut stdout, &json!({"type": "rdp_error", "reason": error}));
        std::process::exit(1);
    }
}
