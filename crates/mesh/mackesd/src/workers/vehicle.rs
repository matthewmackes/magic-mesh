//! Rolling Node — the mackesd `vehicle` worker: the workstation-side adapter that
//! SSH/HTTP-polls a mobile **Sierra AirLink MG90** (oMG) gateway and publishes a
//! latest-wins `state/vehicle/<node>` Bus mirror.
//!
//! The worker is the mesh-side runner + status publisher for one on-owned-vehicle
//! gateway. It:
//!
//! 1. **Reads three raw sources** through the injectable [`VehicleProbe`] seam
//!    (production [`SshHttpProbe`]; tests inject a fake):
//!    - the GNSS/IMU NMEA blob (`/var/run/omgtime.g.info`, over SSH),
//!    - the LCI **general** status page (over the authed Tomcat HTTP session),
//!    - the LCI **WAN** status page (same session).
//! 2. **Folds them into a neutral [`VehicleState`]** — GPS via the pure
//!    [`parse_gpgga`], IMU via [`parse_psiwmmpu`], and TOLERANT label→value
//!    extractors over the (tag-stripped) HTML. Anything it cannot extract goes into
//!    `gaps` (honest-partial, §7) rather than being fabricated.
//! 3. **Publishes `state/vehicle/<node>`** (latest-wins) on a ~5 s poll that
//!    doubles as the heartbeat, via [`crate::bus_publish::publish_json`] — exactly
//!    like the `cloud` worker's mirror publish.
//!
//! ## Config (env for now; mde-seal later)
//! - `MDE_VEHICLE_GATEWAY` — the gateway endpoint, an IP or `ip:sshport`. **When
//!   unset the worker is a no-op** (logs once, publishes nothing) — most nodes have
//!   no vehicle gateway attached.
//! - `MDE_VEHICLE_ROOT_PW` — the gateway's `root` SSH password (the oMG SSH host is
//!   a legacy-crypto box, hence the explicit `+ssh-rsa` / `diffie-hellman-group1`
//!   options on the real probe). HTTP auth is the fixed oMG `admin`/`admin` LCI
//!   login.
//!
//! On any anchor-probe error (the LCI general read — the gateway's reachability
//! signal) the worker publishes an honest [`VehicleState::offline`] snapshot; the
//! GPS (SSH) and WAN (HTTP) reads degrade to a `gaps` note without blanking the
//! whole mirror.

#![cfg(feature = "async-services")]

use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use mackes_mesh_types::vehicle::{
    parse_gpgga, vehicle_state_topic, GpsFix, ImuSample, VehicleState, VehicleTelem, WanStatus,
};

use super::{ShutdownToken, Worker};

/// Env: the gateway endpoint (an IP or `ip:sshport`). Unset ⇒ the worker is a no-op.
pub const GATEWAY_ENV: &str = "MDE_VEHICLE_GATEWAY";

/// Env: the gateway `root` SSH password (later mde-seal; env is fine for now).
pub const ROOT_PW_ENV: &str = "MDE_VEHICLE_ROOT_PW";

/// Poll cadence — build + publish a fresh mirror every ~5 s. Doubles as the
/// latest-wins heartbeat (a stale consumer always sees a recent stamp).
pub const POLL: Duration = Duration::from_secs(5);

/// The oMG GNSS/IMU NMEA blob the SSH read `cat`s.
const GPS_INFO_PATH: &str = "/var/run/omgtime.g.info";

/// The LCI general status page (relative to the gateway root).
const LCI_GENERAL_URL: &str = "MG-LCI/status/general.html";

/// The LCI extended WAN status page.
const LCI_WAN_URL: &str = "MG-LCI/wan/status/status.html?displayExtended=true";

// ─────────────────────────── the injectable probe seam ───────────────────────────

/// The raw-text read seam the worker folds into a [`VehicleState`]: three methods,
/// each returning the RAW text the adapter reads, so tests inject a fake without a
/// live gateway (the same applier-injection idiom as the `cloud` worker's
/// `CloudRunner` seam).
pub trait VehicleProbe: Send + Sync {
    /// The GNSS/IMU NMEA blob (real: SSH `cat /var/run/omgtime.g.info`).
    ///
    /// # Errors
    /// The SSH transport's failure (host unreachable / `sshpass` absent / auth).
    fn read_gps_nmea(&self) -> io::Result<String>;

    /// The LCI **general** status HTML (real: authed HTTP GET of
    /// `.../MG-LCI/status/general.html`). This is the worker's reachability anchor.
    ///
    /// # Errors
    /// The HTTP transport's failure (host unreachable / `curl` absent / auth).
    fn read_lci_general(&self) -> io::Result<String>;

    /// The LCI extended **WAN** status HTML (real: authed HTTP GET of
    /// `.../MG-LCI/wan/status/status.html?displayExtended=true`).
    ///
    /// # Errors
    /// The HTTP transport's failure.
    fn read_lci_wan(&self) -> io::Result<String>;
}

/// The production probe: shells `sshpass`/`ssh` for the NMEA blob and `curl` for the
/// Tomcat FORM-auth'd LCI pages (single cookie-jar session, follow the 303).
pub struct SshHttpProbe {
    /// The gateway IP (no port).
    ip: String,
    /// The SSH port (default 22).
    ssh_port: u16,
    /// The `root` SSH password (from [`ROOT_PW_ENV`]).
    ssh_pw: String,
}

impl SshHttpProbe {
    /// Build from a raw `MDE_VEHICLE_GATEWAY` value (an IP or `ip:sshport`) + the
    /// `root` password from [`ROOT_PW_ENV`] (empty when unset).
    #[must_use]
    pub fn from_env(gateway: &str) -> Self {
        let (ip, ssh_port) = parse_endpoint(gateway);
        Self {
            ip,
            ssh_port,
            ssh_pw: std::env::var(ROOT_PW_ENV).unwrap_or_default(),
        }
    }

    /// The LCI base URL (`http://<ip>/`).
    fn base_url(&self) -> String {
        format!("http://{}/", self.ip)
    }

    /// The per-process cookie jar the single authed session shares across the LCI
    /// login + page fetches (mirrors the `-c jar -b jar` pattern).
    fn jar_path(&self) -> PathBuf {
        std::env::temp_dir().join(format!("mde-vehicle-cookies-{}.jar", std::process::id()))
    }

    /// Run `curl` with `args`, returning stdout. An empty `Ok("")` is a legitimate
    /// (empty-page) result; only a spawn failure / non-zero exit is an `Err`.
    fn curl(args: &[&str]) -> io::Result<String> {
        let out = Command::new("curl").args(args).output()?;
        if !out.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "curl exited {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Fetch `page_url` off the authed LCI session: prime the session (GET `/MG-LCI/`),
    /// POST the fixed `admin`/`admin` FORM login (`j_security_check`, follow the 303),
    /// then GET the target page carrying the session cookie.
    fn http_authed_get(&self, page_url: &str) -> io::Result<String> {
        let jar = self.jar_path();
        let jar_str = jar.display().to_string();
        let base = self.base_url();
        let lci = format!("{base}MG-LCI/");
        let login = format!("{base}MG-LCI/j_security_check");
        let page = format!("{base}{page_url}");
        // 1) prime the Tomcat session (sets JSESSIONID in the jar).
        Self::curl(&["-s", "-c", &jar_str, "-b", &jar_str, "-L", &lci])?;
        // 2) FORM auth — follow the 303 back to the app.
        Self::curl(&[
            "-s",
            "-c",
            &jar_str,
            "-b",
            &jar_str,
            "-L",
            "--data-urlencode",
            "j_username=admin",
            "--data-urlencode",
            "j_password=admin",
            &login,
        ])?;
        // 3) the authed page fetch.
        Self::curl(&["-s", "-b", &jar_str, &page])
    }
}

impl VehicleProbe for SshHttpProbe {
    fn read_gps_nmea(&self) -> io::Result<String> {
        let port = self.ssh_port.to_string();
        let target = format!("root@{}", self.ip);
        let remote_cmd = format!("cat {GPS_INFO_PATH}");
        // The oMG SSH host runs legacy crypto — hence the explicit +ssh-rsa /
        // group1 / aes128-cbc allowances (a modern OpenSSH refuses it otherwise).
        // `ssh-dss` must NOT appear here: modern OpenSSH REMOVED it (not merely
        // deprecated), so listing it makes ssh reject the whole option value
        // ("command-line: Bad key types") and every SSH read (GPS/IMU/control)
        // fails before connecting — the MG90 offers an ssh-rsa (and ed25519) host
        // key, so +ssh-rsa alone connects. Verified live against the bench MG90.
        let out = Command::new("sshpass")
            .args([
                "-p",
                &self.ssh_pw,
                "ssh",
                "-p",
                &port,
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "HostKeyAlgorithms=+ssh-rsa",
                "-o",
                "KexAlgorithms=+diffie-hellman-group1-sha1,diffie-hellman-group14-sha1",
                "-o",
                "PubkeyAcceptedAlgorithms=+ssh-rsa",
                "-o",
                "Ciphers=+aes128-cbc,3des-cbc",
                &target,
                &remote_cmd,
            ])
            .output()?;
        if !out.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "ssh exited {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn read_lci_general(&self) -> io::Result<String> {
        self.http_authed_get(LCI_GENERAL_URL)
    }

    fn read_lci_wan(&self) -> io::Result<String> {
        self.http_authed_get(LCI_WAN_URL)
    }
}

/// Split a `MDE_VEHICLE_GATEWAY` value into `(ip, ssh_port)`. `ip:port` yields the
/// parsed port; a bare `ip` (or an unparsable suffix) defaults to port 22. IPv4-only
/// (the MG90 is an IPv4 box) — no bracketed-IPv6 handling.
fn parse_endpoint(raw: &str) -> (String, u16) {
    match raw.trim().rsplit_once(':') {
        Some((ip, port)) => match port.trim().parse::<u16>() {
            Ok(p) => (ip.trim().to_string(), p),
            Err(_) => (raw.trim().to_string(), 22),
        },
        None => (raw.trim().to_string(), 22),
    }
}

// ─────────────────────────── the worker ───────────────────────────

/// The `vehicle` worker (per-node, rank-0 universal — but a genuine no-op on the
/// overwhelming majority of nodes that have no gateway). Mirrors the `cloud`
/// worker's lifecycle: an injectable transport seam, a `bus_root: Option<PathBuf>`
/// (`None` ⇒ publish is a no-op), and a poll-and-publish run loop.
pub struct VehicleWorker {
    /// This node's id — the `state/vehicle/<host>` namespace + the mirror `host` stamp.
    host: String,
    /// The transport seam (production [`SshHttpProbe`]). `None` ⇒ no
    /// `MDE_VEHICLE_GATEWAY` configured ⇒ the worker idles (publishes nothing).
    probe: Option<Arc<dyn VehicleProbe>>,
    /// The Bus root the mirror publish targets (`None` ⇒ publish is a swallowed
    /// no-op — a pre-RPM dev box / a test).
    bus_root: Option<PathBuf>,
    /// Poll + heartbeat cadence.
    poll: Duration,
}

impl VehicleWorker {
    /// Construct with production wiring: the [`SshHttpProbe`] from
    /// [`GATEWAY_ENV`]/[`ROOT_PW_ENV`] (absent gateway ⇒ `None` ⇒ idle) and the
    /// persisted Bus tree. `host` is this node's id (the `peer:`-stripped node id).
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe: Option<Arc<dyn VehicleProbe>> = match std::env::var(GATEWAY_ENV) {
            Ok(g) if !g.trim().is_empty() => Some(Arc::new(SshHttpProbe::from_env(g.trim()))),
            _ => None,
        };
        Self {
            host,
            probe,
            bus_root: crate::bus_publish::default_bus_root(),
            poll: POLL,
        }
    }

    /// Inject a probe (tests supply a fake; also the seam a future mde-seal wiring
    /// swaps the real transport through).
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn VehicleProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override the Bus root (tests point it at a tempdir; `None` disables publish).
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    /// Override the poll cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Build the current `state/vehicle/<host>` mirror from the probe's three raw
    /// reads. The LCI general read is the reachability anchor: its failure ⇒ an
    /// honest [`VehicleState::offline`] snapshot. GPS (SSH) + WAN (HTTP) failures
    /// degrade to a `gaps` note rather than blanking the mirror.
    #[must_use]
    pub fn build_state(&self, probe: &dyn VehicleProbe) -> VehicleState {
        let general = match probe.read_lci_general() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::vehicle",
                    host = %self.host, error = %e,
                    "vehicle gateway LCI unreachable — publishing offline mirror"
                );
                let mut s = VehicleState::offline(&self.host);
                s.published_at_ms = now_ms();
                return s;
            }
        };

        let mut gaps: Vec<String> = Vec::new();
        let general_text = strip_html(&general);

        // ── general.html: MCU power/board + identity ──
        let battery_v =
            find_number_after(&general_text, "Main Battery Voltage").unwrap_or_else(|| {
                gaps.push("telem.battery_v not reported by general.html".to_string());
                0.0
            });
        let internal_temp_c = find_number_after(&general_text, "Internal Temperature")
            .unwrap_or_else(|| {
                gaps.push("telem.internal_temp_c not reported by general.html".to_string());
                0.0
            });
        let esn = find_token_after(&general_text, "ESN").unwrap_or_else(|| {
            gaps.push("esn not reported by general.html".to_string());
            String::new()
        });
        let mgos_version = find_token_after(&general_text, "Version").unwrap_or_else(|| {
            gaps.push("mgos_version not reported by general.html".to_string());
            String::new()
        });
        let model = find_token_after(&general_text, "Model").unwrap_or_else(|| {
            gaps.push("model not reported by general.html".to_string());
            String::new()
        });

        // ── GNSS/IMU over SSH ──
        let (gps, imu) = match probe.read_gps_nmea() {
            Ok(nmea) => parse_gps_imu(&nmea, &mut gaps),
            Err(e) => {
                gaps.push(format!("gps/imu unavailable (ssh): {e}"));
                (GpsFix::default(), None)
            }
        };

        // ── WAN status over HTTP ──
        let wan = match probe.read_lci_wan() {
            Ok(html) => parse_wan(&html, &mut gaps),
            Err(e) => {
                gaps.push(format!("wan status unavailable (http): {e}"));
                WanStatus::default()
            }
        };

        // ── vehicle power + OBD telemetry ──
        // ignition/OBD source (the MCU ignition-sense line + an OBD-II dongle) is a
        // follow-up; leave the flags false with an honest gap rather than guessing.
        gaps.push(
            "ignition/OBD not wired (MCU ignition-sense + OBD-II source is a follow-up)"
                .to_string(),
        );
        let telem = VehicleTelem {
            battery_v,
            internal_temp_c,
            ignition_on: false,
            moving: gps.speed_mph > 0.5,
            obd_present: false,
            ..Default::default()
        };

        VehicleState {
            host: self.host.clone(),
            model,
            esn,
            mgos_version,
            online: true,
            gps,
            imu,
            wan,
            telem,
            gaps,
            published_at_ms: now_ms(),
        }
    }

    /// Publish the current mirror to `state/vehicle/<host>` (best-effort, exactly
    /// like the `cloud` worker's `publish_state`).
    fn publish(&self, state: &VehicleState) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(&mut persist, &vehicle_state_topic(&self.host), state);
        }
    }
}

#[async_trait::async_trait]
impl Worker for VehicleWorker {
    fn name(&self) -> &'static str {
        "vehicle"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // No gateway on this node ⇒ log once + idle (publish nothing). Most nodes
        // never have a vehicle gateway attached.
        let Some(probe) = self.probe.clone() else {
            tracing::info!(
                target: "mackesd::vehicle",
                host = %self.host,
                env = GATEWAY_ENV,
                "no vehicle gateway configured — vehicle worker idle"
            );
            shutdown.wait().await;
            return Ok(());
        };
        loop {
            let state = self.build_state(probe.as_ref());
            self.publish(&state);
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.poll) => {}
            }
        }
    }
}

// ─────────────────────────── raw-text folds ───────────────────────────

/// Parse the GNSS `$GPGGA` + IMU `$PSIWMMPU` lines out of an oMG NMEA blob. GPS via
/// the pure [`parse_gpgga`]; IMU best-effort (a missing line ⇒ `None` + a gap).
fn parse_gps_imu(nmea: &str, gaps: &mut Vec<String>) -> (GpsFix, Option<ImuSample>) {
    let gps = nmea
        .lines()
        .find(|l| l.contains("GGA,"))
        .and_then(parse_gpgga)
        .unwrap_or_else(|| {
            gaps.push("no $GPGGA line in the gateway NMEA".to_string());
            GpsFix::default()
        });
    let imu = nmea
        .lines()
        .find(|l| l.contains("PSIWMMPU"))
        .and_then(parse_psiwmmpu);
    if imu.is_none() {
        gaps.push("no $PSIWMMPU IMU line in the gateway NMEA".to_string());
    }
    (gps, imu)
}

/// Parse an oMG `$PSIWMMPU,<t>,<ax>,<ay>,<az>,<gx>,<gy>,<gz>` line into an
/// [`ImuSample`] (accel g, gyro deg/s). `None` when the line is malformed.
fn parse_psiwmmpu(line: &str) -> Option<ImuSample> {
    let tag = "PSIWMMPU,";
    let start = line.find(tag)?;
    let body = &line[start + tag.len()..];
    // Drop the checksum suffix if present.
    let body = body.split('*').next().unwrap_or(body);
    let f: Vec<&str> = body.split(',').collect();
    // f: 0=timestamp, 1..4 = accel x/y/z, 4..7 = gyro x/y/z.
    if f.len() < 7 {
        return None;
    }
    let ax: f32 = f.get(1)?.trim().parse().ok()?;
    let ay: f32 = f.get(2)?.trim().parse().ok()?;
    let az: f32 = f.get(3)?.trim().parse().ok()?;
    let gx: f32 = f.get(4)?.trim().parse().ok()?;
    let gy: f32 = f.get(5)?.trim().parse().ok()?;
    let gz: f32 = f.get(6)?.trim().parse().ok()?;
    Some(ImuSample {
        accel_g: [ax, ay, az],
        gyro_dps: [gx, gy, gz],
    })
}

/// TOLERANT WAN-status fold: strip the HTML, then pull the labels we recognize into
/// [`WanStatus`], recording an honest `gaps` note for every field the page didn't
/// yield (never a fabricated value, §7). The full per-modem A/B breakdown of the
/// extended status table is gated on a live captured fixture; the first `dBm`
/// reading is folded into cellular A as a best-effort signal.
fn parse_wan(html: &str, gaps: &mut Vec<String>) -> WanStatus {
    let text = strip_html(html);
    let mut wan = WanStatus::default();

    match find_token_after(&text, "Active WAN").or_else(|| find_token_after(&text, "Active Link")) {
        Some(v) => wan.active_wan = v,
        None => gaps.push("wan.active_wan not reported".to_string()),
    }
    match find_token_after(&text, "Wi-Fi").or_else(|| find_token_after(&text, "Wifi")) {
        Some(v) => wan.wifi_state = v,
        None => gaps.push("wan.wifi_state not reported".to_string()),
    }
    match find_token_after(&text, "Ethernet") {
        Some(v) => wan.ethernet_state = v,
        None => gaps.push("wan.ethernet_state not reported".to_string()),
    }
    match find_token_after(&text, "VPN") {
        Some(v) => wan.vpn_state = v,
        None => gaps.push("wan.vpn_state not reported".to_string()),
    }
    match find_signal_dbm(&text) {
        Some(dbm) => {
            wan.cellular_a.signal_dbm = dbm;
            wan.cellular_a.healthy = dbm > -100;
        }
        None => gaps.push("wan.cellular signal_dbm not reported".to_string()),
    }
    // The structured per-modem carrier/technology/sim-state + the B-modem split need
    // the extended status table's row structure — honestly gapped until a live
    // wan-status fixture pins the parse (§7 honest-partial, never fabricated).
    gaps.push(
        "wan: per-modem carrier/technology/sim-state + B-modem not yet parsed (needs live \
         wan-status fixture)"
            .to_string(),
    );
    wan
}

// ─────────────────────────── tolerant HTML extractors ───────────────────────────

/// Replace every `<...>` tag with a space so a label→value scan works over the
/// text content (e.g. `Foo </td><td> 12.3` ⇒ `Foo    12.3`).
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if in_tag => {}
            _ => out.push(c),
        }
    }
    out
}

/// The first number appearing AFTER `label` (optional sign, digits, one dot),
/// ignoring any non-numeric run (tags-turned-spaces, a leading unit) between them.
/// A trailing unit (e.g. the `v` in `12.60v`) is not consumed. `None` when the label
/// is absent or no number follows it.
fn find_number_after(text: &str, label: &str) -> Option<f32> {
    let idx = text.find(label)?;
    let rest = &text[idx + label.len()..];
    let bytes = rest.as_bytes();
    let mut i = 0;
    // Find the start of a numeric run (a digit, or a '-' immediately before one).
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_digit() || (c == b'-' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            break;
        }
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let start = i;
    if bytes[i] == b'-' {
        i += 1;
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    rest[start..i].parse::<f32>().ok()
}

/// The first whitespace-delimited token appearing after `label` (the value cell in a
/// stripped `Label </td><td> VALUE` row). `None` when the label is absent or nothing
/// non-whitespace follows.
fn find_token_after(text: &str, label: &str) -> Option<String> {
    let idx = text.find(label)?;
    let rest = text[idx + label.len()..].trim_start();
    let tok: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    if tok.is_empty() {
        None
    } else {
        Some(tok)
    }
}

/// The signed integer immediately preceding the first `dBm` token (e.g. `-72 dBm` ⇒
/// `-72`). `None` when there is no `dBm` reading.
fn find_signal_dbm(text: &str) -> Option<i32> {
    let idx = text.find("dBm")?;
    let prefix = text[..idx].trim_end();
    // Walk back over the trailing digit/sign run, then parse it forwards.
    let tail: String = prefix
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    let num: String = tail.chars().rev().collect();
    num.parse::<i32>().ok()
}

/// Wall-clock milliseconds since the Unix epoch (the mirror stamp).
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted fake probe: each read yields a canned `Ok(text)` or `Err(msg)`.
    /// (`Result<String, String>` is `Clone`, unlike `io::Result`, so the fixtures
    /// are reusable across the per-read calls `build_state` makes.)
    #[derive(Clone)]
    struct FakeProbe {
        nmea: Result<String, String>,
        general: Result<String, String>,
        wan: Result<String, String>,
    }

    impl FakeProbe {
        /// The captured bench-MG90 fixtures — a no-lock GGA + a real IMU line, and
        /// the general.html rows carrying battery/temp/esn/version.
        fn real() -> Self {
            let nmea = "$GPGGA,111504.000,3210.07993,N,09550.95445,W,0,00,99.0,081.94,M,-24.2,M,,*66\n\
                        $PSIWMMPU,49.050,0.25218,0.12537,-10.02395,-3.39966,-0.99182,-0.90637,*3C\n"
                .to_string();
            let general = "<table>\
                <tr><td>Model </td><td> MG90</td></tr>\
                <tr><td>ESN </td><td> ND84720078011035</td></tr>\
                <tr><td>Version </td><td> 4.3.0.1</td></tr>\
                <tr><td>Main Battery Voltage </td><td> 12.60v</td></tr>\
                <tr><td>Internal Temperature </td><td> 33.89</td></tr>\
                </table>"
                .to_string();
            let wan = "<table>\
                <tr><td>Active WAN </td><td> CellularA</td></tr>\
                <tr><td>Wi-Fi </td><td> Disabled</td></tr>\
                <tr><td>Ethernet </td><td> Down</td></tr>\
                <tr><td>VPN </td><td> Connected</td></tr>\
                <tr><td>Signal </td><td> -72 dBm</td></tr>\
                </table>"
                .to_string();
            Self {
                nmea: Ok(nmea),
                general: Ok(general),
                wan: Ok(wan),
            }
        }
    }

    fn to_io(r: &Result<String, String>) -> io::Result<String> {
        r.clone()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    impl VehicleProbe for FakeProbe {
        fn read_gps_nmea(&self) -> io::Result<String> {
            to_io(&self.nmea)
        }
        fn read_lci_general(&self) -> io::Result<String> {
            to_io(&self.general)
        }
        fn read_lci_wan(&self) -> io::Result<String> {
            to_io(&self.wan)
        }
    }

    fn worker() -> VehicleWorker {
        VehicleWorker::new("rig-1".to_string()).with_bus_root(None)
    }

    #[test]
    fn builds_state_from_real_fixtures() {
        let probe = FakeProbe::real();
        let state = worker().build_state(&probe);

        assert!(state.online, "a reachable LCI is online");
        assert_eq!(state.host, "rig-1");

        // GPS — the captured no-lock GGA (quality 0 / 0 sats).
        assert_eq!(state.gps.satellites, 0);
        assert!(!state.gps.has_fix(), "quality 0 / 0 sats ⇒ no lock");
        assert!((state.gps.altitude_m - 81.94).abs() < 0.01);

        // IMU — the $PSIWMMPU accel/gyro parsed (non-zero, honest values).
        let imu = state.imu.expect("IMU sample parsed");
        assert!(
            (imu.accel_g[0] - 0.25218).abs() < 1e-4,
            "ax {}",
            imu.accel_g[0]
        );
        assert!(
            (imu.accel_g[2] + 10.02395).abs() < 1e-4,
            "az {}",
            imu.accel_g[2]
        );
        assert!(
            (imu.gyro_dps[0] + 3.39966).abs() < 1e-4,
            "gx {}",
            imu.gyro_dps[0]
        );

        // general.html — battery / temp / esn / version / model.
        assert!(
            (state.telem.battery_v - 12.60).abs() < 0.01,
            "battery {}",
            state.telem.battery_v
        );
        assert!(
            (state.telem.internal_temp_c - 33.89).abs() < 0.01,
            "temp {}",
            state.telem.internal_temp_c
        );
        assert_eq!(state.esn, "ND84720078011035");
        assert_eq!(state.mgos_version, "4.3.0.1");
        assert_eq!(state.model, "MG90");

        // WAN — the tolerant fold picked up the states + the signal.
        assert_eq!(state.wan.active_wan, "CellularA");
        assert_eq!(state.wan.vpn_state, "Connected");
        assert_eq!(state.wan.cellular_a.signal_dbm, -72);

        // Honest-partial: OBD/ignition is explicitly gapped, never fabricated.
        assert!(
            state.gaps.iter().any(|g| g.contains("ignition/OBD")),
            "ignition/OBD is honestly gapped: {:?}",
            state.gaps
        );
        assert!(!state.telem.obd_present);
        assert!(!state.telem.ignition_on);
    }

    #[test]
    fn probe_error_yields_offline_snapshot() {
        // The LCI general read is the reachability anchor — its failure ⇒ offline.
        let probe = FakeProbe {
            general: Err("connection refused".to_string()),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);
        assert!(!state.online, "an unreachable gateway is offline");
        assert!(!state.gps.has_fix());
        assert!(
            state.gaps.iter().any(|g| g.contains("unreachable")),
            "offline snapshot carries the honest gap: {:?}",
            state.gaps
        );
        assert!(state.published_at_ms > 0, "offline mirror is still stamped");
    }

    #[test]
    fn partial_reads_degrade_to_gaps_not_offline() {
        // GPS (SSH) + WAN (HTTP) fail, but the anchor LCI general succeeds — the
        // mirror stays online with honest gaps rather than blanking.
        let probe = FakeProbe {
            nmea: Err("ssh: connect timeout".to_string()),
            wan: Err("curl: 28".to_string()),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);
        assert!(state.online, "the anchor read succeeded ⇒ online");
        assert!(state.imu.is_none());
        assert!(state.gaps.iter().any(|g| g.contains("gps/imu unavailable")));
        assert!(state
            .gaps
            .iter()
            .any(|g| g.contains("wan status unavailable")));
        // The general.html data still landed.
        assert_eq!(state.esn, "ND84720078011035");
    }

    #[test]
    fn parse_endpoint_splits_ip_and_ssh_port() {
        assert_eq!(
            parse_endpoint("192.168.13.31"),
            ("192.168.13.31".to_string(), 22)
        );
        assert_eq!(
            parse_endpoint("192.168.13.31:2222"),
            ("192.168.13.31".to_string(), 2222)
        );
        // An unparsable suffix falls back to the whole string + default port.
        assert_eq!(
            parse_endpoint("host.local:ssh"),
            ("host.local:ssh".to_string(), 22)
        );
    }

    #[test]
    fn tolerant_extractors_ignore_tags_and_units() {
        let t = strip_html("<td>Main Battery Voltage </td><td> 12.60v</td>");
        assert!((find_number_after(&t, "Main Battery Voltage").unwrap() - 12.60).abs() < 0.01);
        assert_eq!(
            find_token_after(&t, "Main Battery Voltage"),
            Some("12.60v".to_string())
        );
        assert_eq!(find_signal_dbm("Signal   -72 dBm"), Some(-72));
        assert!(find_number_after("no number here", "Label").is_none());
    }

    #[tokio::test]
    async fn run_loop_publishes_then_exits_promptly_on_shutdown() {
        let mut w = worker()
            .with_probe(Arc::new(FakeProbe::real()))
            .with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }

    #[tokio::test]
    async fn no_gateway_worker_idles_without_publishing() {
        // No probe (the MDE_VEHICLE_GATEWAY-unset path) ⇒ idle until shutdown.
        let mut w = worker();
        w.probe = None;
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "the idle worker still exits promptly");
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
