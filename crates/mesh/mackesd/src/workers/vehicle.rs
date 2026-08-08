//! Rolling Node — the mackesd `vehicle` worker: the workstation-side adapter that
//! SSH/HTTP-polls a mobile **Sierra AirLink MG90** (oMG) gateway and publishes a
//! latest-wins `state/vehicle/<node>` Bus mirror.
//!
//! The worker is the mesh-side runner + status publisher for one on-owned-vehicle
//! gateway. It:
//!
//! 1. **Reads raw sources** through the injectable [`VehicleProbe`] seam
//!    (production [`SshHttpProbe`]; tests inject a fake):
//!    - the GNSS/IMU NMEA blob (`/var/run/omgtime.g.info`, over SSH),
//!    - the LCI **general** status page (over the authed Tomcat HTTP session),
//!    - the LCI **WAN** status page (same session).
//! 2. **Folds them into a neutral [`VehicleState`]** — GPS via the pure
//!    [`parse_gpgga`], IMU via [`parse_psiwmmpu`], and TOLERANT label→value
//!    extractors over the (tag-stripped) HTML. Anything it cannot extract goes into
//!    `gaps` (honest-partial, §7) rather than being fabricated.
//! 3. **Publishes `state/vehicle/<node>`** (latest-wins) immediately on change
//!    and at least every two seconds as a heartbeat. Current LCI/status-beacon
//!    work and slow GNSS/WAN/application enrichment have independent blocking
//!    tasks, in-flight gates, and cadences, so enrichment cannot delay a cached
//!    snapshot's heartbeat or erase fresher current fields.
//! 4. **Drains `action/vehicle/*` control verbs** off the Bus
//!    ([`VEHICLE_ACTION_PREFIX`]) and answers each on `reply/<ulid>` with a
//!    [`VehicleReply`] — `get-config` (a READ that pulls a committed oMG config
//!    file over SSH) and `reboot` (a destructive MUTATION, typed-armed on the
//!    gateway ESN + audited). Only a node WITH a gateway (`MDE_VEHICLE_GATEWAY`
//!    set) drains; every other node idles and ignores the queue.
//!
//! ## Config (env for now; mde-seal later)
//! - `MDE_VEHICLE_GATEWAY` — the gateway endpoint, an IP or `ip:sshport`. **When
//!   unset the worker is a no-op** (logs once, publishes nothing) — most nodes have
//!   no vehicle gateway attached.
//! - `MDE_VEHICLE_ROOT_PW_FILE` — preferred root-only file containing the gateway's
//!   `root` SSH password (default `/etc/mackesd/mg90-root-password`). The legacy
//!   `MDE_VEHICLE_ROOT_PW` value remains a compatibility fallback only. The SSH
//!   password is fed over stdin to `sshpass`; it is never placed in process argv.
//!   The oMG SSH host is a legacy-crypto box, hence the explicit `+ssh-rsa` /
//!   `diffie-hellman-group1` options on the real probe.
//! - `MDE_VEHICLE_STATUS_PORT` — optional local UDP port for the MG90 documented
//!   JSON Status Broadcast. When configured, the beacon is preferred for GNSS,
//!   ignition, battery, and temperature; LCI/NMEA remain fallbacks.
//! - `MDE_VEHICLE_OBD_STATUS_PATH` — optional, explicit MG90 application path for
//!   an OBD/HDOBD diagnostic read. Only the documented `/obdii_status/` and
//!   `/hdobd_status/` paths are accepted. The response is currently a transport
//!   diagnostic only; it is never promoted into typed OBD telemetry without a
//!   verified payload schema.
//! - HTTP auth is the fixed oMG `admin`/`admin` LCI login for this deployment; the
//!   standalone `mg90-access` helper exposes the LCI and application planes with
//!   a root-only HTTP password file.
//!
//! On any anchor-probe error (the LCI general read — the gateway's reachability
//! signal) the worker publishes an honest [`VehicleState::offline`] snapshot; the
//! GPS (SSH) and WAN (HTTP) reads degrade to a `gaps` note without blanking the
//! whole mirror.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mackes_mesh_types::vehicle::{
    parse_gpgga, vehicle_state_topic, vehicle_state_v2_topic, CellLink, DeviceProbeStatus, GpsFix,
    ImuSample, ManagerSetState, SnapshotProvenance, SnapshotSource, VehicleReply, VehicleState,
    VehicleStateV2, VehicleTelem, WanStatus, VEHICLE_ACTION_PREFIX,
    VEHICLE_STATE_V2_SCHEMA_VERSION,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde::Deserialize;

use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

/// Env: the gateway endpoint (an IP or `ip:sshport`). Unset ⇒ the worker is a no-op.
pub const GATEWAY_ENV: &str = "MDE_VEHICLE_GATEWAY";

/// Env: the gateway `root` SSH password (later mde-seal; env is fine for now).
pub const ROOT_PW_ENV: &str = "MDE_VEHICLE_ROOT_PW";

/// Preferred env: path to the root-owned MG90 SSH password file.
pub const ROOT_PW_FILE_ENV: &str = "MDE_VEHICLE_ROOT_PW_FILE";

/// Default root-owned MG90 SSH password file used by the packaged worker/helper.
pub const ROOT_PW_FILE_DEFAULT: &str = "/etc/mackesd/mg90-root-password";

/// Password files contain one short line. Refuse unexpectedly large files before
/// converting their contents into a `String`.
const ROOT_PASSWORD_MAX_BYTES: usize = 4 * 1024;

/// Linux's `O_NOFOLLOW`: the final password-file path component must not be a
/// symlink. This worker is a Linux system service; keep the flag local rather
/// than adding a libc dependency just for the open boundary.
#[cfg(target_os = "linux")]
const ROOT_PASSWORD_NOFOLLOW_FLAG: i32 = 0o400000;

/// Optional env: local UDP port receiving the MG90 JSON Status Broadcast.
pub const STATUS_PORT_ENV: &str = "MDE_VEHICLE_STATUS_PORT";

/// Optional env: one of the documented MG90 application pages that exposes the
/// OBD/HDOBD status surface. Keeping this opt-in prevents an unverified app page
/// from being mistaken for typed OBD telemetry.
pub const OBD_STATUS_PATH_ENV: &str = "MDE_VEHICLE_OBD_STATUS_PATH";

/// Readiness of the local receiver for the MG90's documented Status Broadcast.
///
/// This describes only the workstation-side UDP socket. It does not claim that
/// the MG90's Status > Broadcast page is enabled or configured to send to this
/// port; that remains an explicit LCI operation from the Rev. 6 guide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusBroadcastReadiness {
    /// `MDE_VEHICLE_STATUS_PORT` was not configured.
    Disabled,
    /// The local receiver is bound and ready for the configured port.
    Listening {
        /// Local UDP port bound by the receiver.
        port: u16,
    },
    /// The local receiver could not be configured. The detail is safe for the
    /// vehicle gap surface and contains no credentials or packet contents.
    ConfigurationError {
        /// Bounded setup detail suitable for an operator-facing gap.
        detail: String,
    },
}

/// The Status Broadcast is a small selected-field JSON object, not an arbitrary
/// UDP transport. Keep the receive allocation bounded and leave one byte for
/// detecting a datagram that would otherwise be truncated by `recv`.
const STATUS_BEACON_MAX_DATAGRAM_BYTES: usize = 16 * 1024;

/// Do not let an unrelated local/LAN sender starve the status receiver forever.
/// One build tick may discard a small burst of non-MG90 packets, then reports an
/// honest gap instead of treating them as vehicle telemetry.
const STATUS_BEACON_MAX_PENDING_DATAGRAMS: usize = 8;

/// `GpsFix::satellites` is an unsigned byte. Do not silently clamp a hostile or
/// corrupt MG90 value into that type; drop the field and retain the NMEA value.
const STATUS_BEACON_MAX_SATELLITES: u16 = u8::MAX as u16;

/// Broad physical sanity bounds for the MG90's input-voltage and internal-board
/// temperature reports. They reject corrupt values without rejecting cold-crank
/// voltage or a hot enclosure that can still be reported by a reachable unit.
const STATUS_BEACON_MIN_BATTERY_V: f32 = 0.0;
const STATUS_BEACON_MAX_BATTERY_V: f32 = 36.0;
const STATUS_BEACON_MIN_TEMPERATURE_C: f32 = -40.0;
const STATUS_BEACON_MAX_TEMPERATURE_C: f32 = 125.0;

/// Pinned host-key file used by the packaged worker/helper.
const KNOWN_HOSTS_FILE_DEFAULT: &str = "/etc/mackesd/mg90_known_hosts";
const KNOWN_HOSTS_FILE_PACKAGED: &str = "/usr/share/magic-mesh/mg90-known-hosts";

/// Shared-Bus capability context for the destructive gateway reboot verb.
const VEHICLE_REBOOT_AUTH_VERB: &str = "vehicle-reboot";
const VEHICLE_REBOOT_AUTH_TARGET: &str = "gateway";

/// Poll cadence for a fresh MG90 observation. Heartbeats are independent and
/// use [`ROSTER_HEARTBEAT`] so a slow gateway probe cannot make consumers stale.
pub const POLL: Duration = Duration::from_secs(5);

/// Slow GNSS/WAN/application enrichment cadence for the production adapter.
pub const ENRICHMENT_POLL: Duration = Duration::from_secs(10);
const FAILURE_RETRY_MAX: Duration = Duration::from_secs(60);
const MAX_INITIAL_PHASE: Duration = Duration::from_millis(250);

/// Spread the first gateway status batch across a small deterministic window.
/// Later failures use the existing retry ladder; this phase prevents every
/// configured seat from opening its expensive root-SSH/HTTP path together.
#[must_use]
fn initial_phase_for(host: &str, cap: Duration) -> Duration {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in host.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Duration::from_millis(
        (hash % (MAX_INITIAL_PHASE.as_millis() as u64 + 1))
            .min(cap.as_millis() as u64),
    )
}

/// Deadline after which an outstanding enrichment is marked unavailable while
/// its blocking task remains gated until it actually exits.
pub const ENRICHMENT_TIMEOUT: Duration = Duration::from_secs(8);

/// Deadline for one current-status batch. The blocking operation remains
/// in-flight after this deadline until its bounded transport process exits.
pub const CURRENT_STATUS_TIMEOUT: Duration = Duration::from_secs(8);

const CURL_CONNECT_TIMEOUT_SECONDS: &str = "2";
const CURL_MAX_TIME_SECONDS: &str = "6";

/// Root-owned private directory for short-lived MG90 HTTP session jars. `/run`
/// is the trust boundary: this leaf is created mode 0700 and rejected unless it
/// is a real directory owned by the daemon's effective uid.
const HTTP_COOKIE_RUNTIME_DIR: &str = "/run/mackesd-vehicle-http";

const HTTP_COOKIE_RANDOM_BYTES: usize = 16;
const HTTP_COOKIE_CREATE_ATTEMPTS: usize = 8;

/// The oMG GNSS/IMU NMEA blob the SSH read `cat`s.
const GPS_INFO_PATH: &str = "/var/run/omgtime.g.info";

/// The LCI general status page (relative to the gateway root).
const LCI_GENERAL_URL: &str = "MG-LCI/status/general.html";

/// The LCI extended WAN status page.
const LCI_WAN_URL: &str = "MG-LCI/wan/status/status.html?displayExtended=true";

// ─────────────────────────── the injectable probe seam ───────────────────────────

/// The raw-text read seam the worker folds into a [`VehicleState`]: five methods,
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

    /// The optional documented MG90 JSON Status Broadcast. A missing configured
    /// stream is `Ok(None)`; malformed packets are an `Err` so callers preserve an
    /// honest gap rather than silently accepting partial data.
    fn read_status_beacon(&self) -> io::Result<Option<String>> {
        Ok(None)
    }

    /// The optional authenticated MG90 OBD/HDOBD application page. The current
    /// repository has evidence for the page paths but not a stable response
    /// schema, so callers must keep the result diagnostic-only until that schema
    /// is verified against a real device.
    fn read_obd_status(&self) -> io::Result<Option<String>> {
        Ok(None)
    }

    /// Run an arbitrary command on the gateway over SSH, returning its stdout — the
    /// seam the `action/vehicle/*` control verbs (`get-config` / `reboot`) shell
    /// through. Real: the same legacy-crypto SSH as [`Self::read_gps_nmea`]; tests
    /// inject a canned response + record the invocation.
    ///
    /// # Errors
    /// The SSH transport's failure (host unreachable / `sshpass` absent / auth).
    fn run_ssh(&self, cmd: &str) -> io::Result<String>;
}

/// The production probe: shells `sshpass`/`ssh` for the NMEA blob and `curl` for the
/// Tomcat FORM-auth'd LCI pages (single cookie-jar session, follow the 303).
pub struct SshHttpProbe {
    /// The gateway IP (no port).
    ip: String,
    /// The SSH port (default 2222 — the oMG SSH port).
    ssh_port: u16,
    /// The `root` SSH password (read from a root-only file, or the legacy env).
    ssh_pw: String,
    /// The pinned host-key file for this gateway.
    known_hosts_file: PathBuf,
    /// Optional nonblocking socket for the documented MG90 JSON status beacon.
    status_socket: Option<UdpSocket>,
    /// Typed local receiver state, retained so invalid configuration is visible
    /// through the normal VehicleState gap path instead of silently disabling the
    /// documented beacon plane.
    status_broadcast: StatusBroadcastReadiness,
}

impl SshHttpProbe {
    /// Build from a raw `MDE_VEHICLE_GATEWAY` value (an IP or `ip:sshport`) plus a
    /// root-only password file. The legacy password env remains a compatibility
    /// fallback so an older deployment can roll forward without a synchronized
    /// systemd drop-in update.
    #[must_use]
    pub fn from_env(gateway: &str) -> Self {
        let (ip, ssh_port) = parse_endpoint(gateway);
        let ssh_pw = std::env::var(ROOT_PW_FILE_ENV)
            .ok()
            .and_then(|path| read_root_password_file(&path))
            .or_else(|| read_root_password_file(ROOT_PW_FILE_DEFAULT))
            .or_else(|| std::env::var(ROOT_PW_ENV).ok())
            .unwrap_or_default();
        let known_hosts_file = if std::path::Path::new(KNOWN_HOSTS_FILE_DEFAULT).is_file() {
            PathBuf::from(KNOWN_HOSTS_FILE_DEFAULT)
        } else {
            PathBuf::from(KNOWN_HOSTS_FILE_PACKAGED)
        };
        let (status_socket, status_broadcast) = match std::env::var(STATUS_PORT_ENV) {
            Ok(raw) => configure_status_broadcast(Some(&raw)),
            Err(std::env::VarError::NotPresent) => configure_status_broadcast(None),
            Err(std::env::VarError::NotUnicode(_)) => (
                None,
                StatusBroadcastReadiness::ConfigurationError {
                    detail: format!("{STATUS_PORT_ENV} is not valid Unicode"),
                },
            ),
        };
        Self {
            ip,
            ssh_port,
            ssh_pw,
            known_hosts_file,
            status_socket,
            status_broadcast,
        }
    }

    /// Report local readiness for the documented Status Broadcast receiver.
    /// This is diagnostic state only; it never configures the MG90.
    #[must_use]
    pub fn status_broadcast_readiness(&self) -> &StatusBroadcastReadiness {
        &self.status_broadcast
    }

    /// The UDP Status Broadcast is unauthenticated at transport level, so the
    /// receiver only accepts packets whose source IP matches the configured MG90
    /// gateway. The JSON `vehicleID` check later binds the datagram to the LCI ESN.
    fn expected_status_peer(&self) -> io::Result<IpAddr> {
        self.ip.parse::<IpAddr>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{STATUS_PORT_ENV} requires {GATEWAY_ENV} to be an IP address"),
            )
        })
    }

    /// The LCI base URL (`http://<ip>/`).
    fn base_url(&self) -> String {
        format!("http://{}/", self.ip)
    }

    /// Create one unguessable, exclusive cookie jar in the daemon's private
    /// runtime directory. Current-status and enrichment may safely run at once.
    fn cookie_jar(&self) -> io::Result<TemporaryCookieJar> {
        create_cookie_jar_in(Path::new(HTTP_COOKIE_RUNTIME_DIR))
    }

    /// Run `curl` with `args`, returning stdout. An empty `Ok("")` is a legitimate
    /// (empty-page) result; only a spawn failure / non-zero exit is an `Err`.
    fn curl(args: &[&str]) -> io::Result<String> {
        let out = Command::new("curl")
            .args([
                "--connect-timeout",
                CURL_CONNECT_TIMEOUT_SECONDS,
                "--max-time",
                CURL_MAX_TIME_SECONDS,
            ])
            .args(args)
            .output()?;
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
        self.http_authed_get_at(80, Some("MG-LCI"), page_url)
    }

    /// Fetch one explicitly allowlisted MG90 application page on port 11532.
    /// This shares the same session shape as `mg90-access.sh app-get`, but does
    /// not interpret the returned page.
    fn http_app_authed_get(&self, page_url: &str) -> io::Result<String> {
        self.http_authed_get_at(11532, None, page_url)
    }

    /// Perform the MG90 form-authenticated GET on either the LCI or application
    /// HTTP service. Tomcat LCI uses `j_security_check`; the CherryPy
    /// application plane uses `do_login` and requires a Referer header.
    fn http_authed_get_at(
        &self,
        port: u16,
        auth_prefix: Option<&str>,
        page_url: &str,
    ) -> io::Result<String> {
        let jar = self.cookie_jar()?;
        let jar_str = jar.path().display().to_string();
        let base = if port == 80 {
            self.base_url()
        } else {
            format!("http://{}:{port}/", self.ip)
        };
        let prefix = auth_prefix.unwrap_or("").trim_matches('/');
        let login_base = if prefix.is_empty() {
            base.clone()
        } else {
            format!("{base}{prefix}/")
        };
        let app_plane = port == 11532;
        let login = if app_plane {
            format!("{base}do_login")
        } else {
            format!("{login_base}j_security_check")
        };
        let page = format!("{base}{}", page_url.trim_start_matches('/'));
        (|| {
            // 1) prime the Tomcat session (sets JSESSIONID in the jar).
            Self::curl(&["-s", "-c", &jar_str, "-b", &jar_str, "-L", &login_base])?;
            // 2) FORM auth — follow the 303 back to the app.
            let mut login_args = vec!["-s", "-c", &jar_str, "-b", &jar_str, "-L"];
            if app_plane {
                login_args.extend([
                    "-e",
                    &login_base,
                    "--data-urlencode",
                    "username=admin",
                    "--data-urlencode",
                    "password=admin",
                    "--data-urlencode",
                    "from_page=http://172.20.0.25:11532/",
                ]);
            } else {
                login_args.extend([
                    "--data-urlencode",
                    "j_username=admin",
                    "--data-urlencode",
                    "j_password=admin",
                ]);
            }
            login_args.push(&login);
            Self::curl(&login_args)?;
            // 3) the authed page fetch.
            Self::curl(&["-s", "-b", &jar_str, &page])
        })()
    }

    /// Run `remote_cmd` on the gateway over SSH, returning stdout. The oMG SSH host
    /// runs legacy crypto — hence the explicit `+ssh-rsa` / `group1` / `aes128-cbc`
    /// allowances (a modern OpenSSH refuses it otherwise). Shared by
    /// [`VehicleProbe::read_gps_nmea`] and [`VehicleProbe::run_ssh`].
    fn ssh(&self, remote_cmd: &str) -> io::Result<String> {
        let port = self.ssh_port.to_string();
        let target = format!("root@{}", self.ip);
        let known_hosts = format!("UserKnownHostsFile={}", self.known_hosts_file.display());
        // `ssh-dss` must NOT appear in HostKeyAlgorithms: modern OpenSSH REMOVED it
        // (not merely deprecated), so listing it makes ssh reject the whole option
        // value ("command-line: Bad key types") and every SSH read (GPS/IMU/control)
        // fails before connecting — the MG90 offers an ssh-rsa (and ed25519) host
        // key, so +ssh-rsa alone connects. Verified live against the bench MG90.
        let mut child = Command::new("sshpass")
            .args([
                "-d",
                "0",
                "ssh",
                "-p",
                &port,
                "-o",
                "HostKeyAlgorithms=+ssh-rsa",
                "-o",
                "KexAlgorithms=+diffie-hellman-group1-sha1,diffie-hellman-group14-sha1",
                "-o",
                "PubkeyAcceptedAlgorithms=+ssh-rsa",
                "-o",
                "Ciphers=+aes128-cbc,3des-cbc",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "GlobalKnownHostsFile=/dev/null",
                "-o",
                &known_hosts,
                "-o",
                "PreferredAuthentications=password",
                "-o",
                "PubkeyAuthentication=no",
                "-o",
                "NumberOfPasswordPrompts=1",
                "-o",
                "ConnectTimeout=8",
                "-o",
                "ConnectionAttempts=1",
                &target,
                remote_cmd,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        {
            let mut stdin = child.stdin.take().ok_or_else(|| {
                io::Error::other("sshpass stdin pipe unavailable for MG90 password")
            })?;
            stdin.write_all(self.ssh_pw.as_bytes())?;
            stdin.write_all(b"\n")?;
        }
        let out = child.wait_with_output()?;
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
}

/// One exclusively created HTTP cookie jar. The containing directory is private,
/// and the path is removed whether authentication succeeds or returns early.
struct TemporaryCookieJar {
    path: PathBuf,
}

impl TemporaryCookieJar {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryCookieJar {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Open or atomically establish the private cookie directory. The immediate
/// parent must itself be a non-symlink, same-owner, non-writable trust anchor;
/// this rejects a redirected `/run` boundary before creating anything.
fn open_private_cookie_runtime_directory(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};

    let trusted_uid = rustix::process::geteuid().as_raw();
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "cookie runtime directory has no parent",
        )
    })?;
    let parent_metadata = std::fs::symlink_metadata(parent)?;
    if parent_metadata.file_type().is_symlink()
        || !parent_metadata.is_dir()
        || parent_metadata.uid() != trusted_uid
        || parent_metadata.permissions().mode() & 0o022 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cookie runtime parent is not a trusted private boundary",
        ));
    }

    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cookie runtime path is not a real directory",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(path) {
                Ok(()) => {}
                // The current and enrichment lanes can establish the shared
                // directory concurrently. Open + validate the winner below.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    }

    let directory: std::fs::File = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?
    .into();
    let metadata = directory.metadata()?;
    if !metadata.is_dir()
        || metadata.uid() != trusted_uid
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cookie runtime directory has untrusted owner or mode",
        ));
    }
    Ok(directory)
}

fn create_cookie_jar_file(
    directory: &std::fs::File,
    runtime_dir: &Path,
    file_name: &str,
) -> io::Result<TemporaryCookieJar> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let file: std::fs::File = rustix::fs::openat(
        directory,
        file_name,
        rustix::fs::OFlags::WRONLY
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?
    .into();
    let jar = TemporaryCookieJar {
        path: runtime_dir.join(file_name),
    };
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cookie jar has untrusted owner, type, or mode",
        ));
    }
    drop(file);
    Ok(jar)
}

fn create_cookie_jar_in(runtime_dir: &Path) -> io::Result<TemporaryCookieJar> {
    use rand::RngCore as _;

    let directory = open_private_cookie_runtime_directory(runtime_dir)?;
    for _ in 0..HTTP_COOKIE_CREATE_ATTEMPTS {
        let mut random = [0_u8; HTTP_COOKIE_RANDOM_BYTES];
        rand::rngs::OsRng.fill_bytes(&mut random);
        let file_name = format!(
            ".mg90-cookie-{}.jar",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        match create_cookie_jar_file(&directory, runtime_dir, &file_name) {
            Ok(jar) => return Ok(jar),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique MG90 cookie jar",
    ))
}

/// Open the password file without following its final path component.
fn open_root_password_file(path: &str) -> Option<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(ROOT_PASSWORD_NOFOLLOW_FLAG);
    }
    options.open(path).ok()
}

/// Read the first line of an already-open root-owned password file without ever
/// placing its contents in a child-process argument. A malformed/untrusted file
/// is treated as absent; the caller then emits the normal honest SSH gap.
fn read_root_password_contents(file: std::fs::File) -> Option<String> {
    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() > ROOT_PASSWORD_MAX_BYTES as u64 {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != 0 || metadata.mode() & 0o077 != 0 {
            return None;
        }
    }

    let bytes = read_root_password_bytes(file)?;
    let text = String::from_utf8(bytes).ok()?;
    let first = text.split('\n').next()?.trim_end_matches('\r');
    (!first.is_empty()).then(|| first.to_string())
}

/// Read at most one byte beyond the accepted limit so a file that grows after
/// the metadata check is still rejected without an unbounded allocation.
fn read_root_password_bytes(file: std::fs::File) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(ROOT_PASSWORD_MAX_BYTES + 1);
    file.take((ROOT_PASSWORD_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > ROOT_PASSWORD_MAX_BYTES {
        return None;
    }
    Some(bytes)
}

/// Read the first line of a root-owned, mode-0400/0600 password file without ever
/// placing its contents in a child-process argument. A malformed/untrusted file is
/// treated as absent; the caller then emits the normal honest SSH gap.
fn read_root_password_file(path: &str) -> Option<String> {
    read_root_password_contents(open_root_password_file(path)?)
}

impl VehicleProbe for SshHttpProbe {
    fn read_gps_nmea(&self) -> io::Result<String> {
        self.ssh(&format!("cat {GPS_INFO_PATH}"))
    }

    fn read_lci_general(&self) -> io::Result<String> {
        self.http_authed_get(LCI_GENERAL_URL)
    }

    fn read_lci_wan(&self) -> io::Result<String> {
        self.http_authed_get(LCI_WAN_URL)
    }

    fn read_obd_status(&self) -> io::Result<Option<String>> {
        let raw = match std::env::var(OBD_STATUS_PATH_ENV) {
            Ok(raw) => raw,
            Err(std::env::VarError::NotPresent) => return Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{OBD_STATUS_PATH_ENV} is not valid Unicode"),
                ));
            }
        };
        let path = parse_obd_status_path(&raw)
            .map_err(|detail| io::Error::new(io::ErrorKind::Unsupported, detail))?;
        self.http_app_authed_get(path).map(Some)
    }

    fn read_status_beacon(&self) -> io::Result<Option<String>> {
        if let StatusBroadcastReadiness::ConfigurationError { detail } = &self.status_broadcast {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, detail.clone()));
        }
        let Some(socket) = &self.status_socket else {
            return Ok(None);
        };
        let expected_peer = self.expected_status_peer()?;
        let mut buf = [0_u8; STATUS_BEACON_MAX_DATAGRAM_BYTES + 1];
        let mut unexpected_peer: Option<IpAddr> = None;
        for _ in 0..STATUS_BEACON_MAX_PENDING_DATAGRAMS {
            match socket.recv_from(&mut buf) {
                Ok((_, peer)) if peer.ip() != expected_peer => {
                    unexpected_peer = Some(peer.ip());
                    continue;
                }
                Ok((size, _)) if size > STATUS_BEACON_MAX_DATAGRAM_BYTES => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "status broadcast datagram exceeds {} bytes",
                            STATUS_BEACON_MAX_DATAGRAM_BYTES
                        ),
                    ));
                }
                Ok((size, _)) => {
                    let payload = std::str::from_utf8(&buf[..size]).map_err(|error| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("status broadcast is not UTF-8: {error}"),
                        )
                    })?;
                    return Ok(Some(payload.to_owned()));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(peer) = unexpected_peer {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("status broadcast from unexpected peer {peer}; expected {expected_peer}"),
            ))
        } else {
            Ok(None)
        }
    }

    fn run_ssh(&self, cmd: &str) -> io::Result<String> {
        self.ssh(cmd)
    }
}

/// Parse the local receiver port without silently treating malformed
/// configuration as an unconfigured Status Broadcast plane.
fn parse_status_port(raw: &str) -> Result<u16, String> {
    let port = raw
        .parse::<u16>()
        .map_err(|_| "must be an integer from 1 to 65535".to_string())?;
    if port == 0 {
        return Err("must be an integer from 1 to 65535".to_string());
    }
    Ok(port)
}

/// Accept only the two MG90 OBD application paths documented by the repository's
/// access contract. A free-form URL here would turn a diagnostic opt-in into an
/// arbitrary authenticated HTTP fetch.
fn parse_obd_status_path(raw: &str) -> Result<&'static str, String> {
    match raw {
        "/obdii_status/" => Ok("/obdii_status/"),
        "/hdobd_status/" => Ok("/hdobd_status/"),
        _ => Err(format!(
            "{OBD_STATUS_PATH_ENV} must be /obdii_status/ or /hdobd_status/"
        )),
    }
}

/// Configure the local receiver for one documented MG90 Status Broadcast port.
/// The MG90-side broadcast remains externally configured through Status >
/// Broadcast; this helper only binds the receiving socket.
fn configure_status_broadcast(raw: Option<&str>) -> (Option<UdpSocket>, StatusBroadcastReadiness) {
    let Some(raw) = raw else {
        return (None, StatusBroadcastReadiness::Disabled);
    };
    let port = match parse_status_port(raw) {
        Ok(port) => port,
        Err(detail) => {
            return (
                None,
                StatusBroadcastReadiness::ConfigurationError {
                    detail: format!("{STATUS_PORT_ENV}: {detail}"),
                },
            );
        }
    };
    let socket = match UdpSocket::bind(("0.0.0.0", port)) {
        Ok(socket) => socket,
        Err(error) => {
            return (
                None,
                StatusBroadcastReadiness::ConfigurationError {
                    detail: format!(
                        "{STATUS_PORT_ENV}={port} could not bind local UDP receiver: {error}"
                    ),
                },
            );
        }
    };
    if let Err(error) = socket.set_nonblocking(true) {
        return (
            None,
            StatusBroadcastReadiness::ConfigurationError {
                detail: format!(
                    "{STATUS_PORT_ENV}={port} could not enable nonblocking UDP receive: {error}"
                ),
            },
        );
    }
    (Some(socket), StatusBroadcastReadiness::Listening { port })
}

/// Split a `MDE_VEHICLE_GATEWAY` value into `(ip, ssh_port)`. `ip:port` yields the
/// parsed port; a bare `ip` (or an unparsable suffix) defaults to **2222**, the oMG
/// SSH port — the MG90's SSH daemon listens on 2222, not 22 (verified live: port 22
/// is refused, 2222 connects). IPv4-only (the MG90 is an IPv4 box) — no
/// bracketed-IPv6 handling. Override with an explicit `ip:port` for a different unit.
const OMG_SSH_PORT: u16 = 2222;
fn parse_endpoint(raw: &str) -> (String, u16) {
    match raw.trim().rsplit_once(':') {
        Some((ip, port)) => match port.trim().parse::<u16>() {
            Ok(p) => (ip.trim().to_string(), p),
            Err(_) => (raw.trim().to_string(), OMG_SSH_PORT),
        },
        None => (raw.trim().to_string(), OMG_SSH_PORT),
    }
}

// ─────────────────────── bounded multi-source roster seam ───────────────────────

/// Maximum number of distinct MG90 source identities in one roster.
///
/// This is a scheduler/read-model seam, not a general fleet-management store. A
/// bounded roster keeps a bad or untrusted discovery input from turning one worker
/// into an unbounded collection of probes or retained snapshots.
pub const MAX_VEHICLE_ROSTER_SOURCES: usize = 16;

/// Maximum number of management nodes represented by one roster.
pub const MAX_VEHICLE_ROSTER_MANAGERS: usize = 8;

/// Maximum number of `(MG90, manager)` assignments retained by one roster.
pub const MAX_VEHICLE_ROSTER_ASSIGNMENTS: usize = 32;

/// The default heartbeat for both the single-gateway worker and the opt-in
/// multi-source roster. A slow poll must never make a retained snapshot stale.
pub const ROSTER_HEARTBEAT: Duration = Duration::from_secs(2);

/// Longest permitted publication heartbeat for a configured gateway.
///
/// Consumers expire vehicle domains after three declared intervals, so the
/// scheduler rejects a slower cadence instead of accepting a plan that can
/// recreate the historical live/stale flicker.
pub const MAX_ROSTER_HEARTBEAT: Duration = Duration::from_secs(2);

const MIN_ROSTER_INTERVAL: Duration = Duration::from_millis(1);
const MAX_ROSTER_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_ROSTER_ID_BYTES: usize = 128;

/// Stable identity for one MG90 source.
///
/// This is deliberately separate from a gateway endpoint and from the manager
/// node. In production it is the confirmed MG90 ESN; an endpoint, host name, or
/// vector position is not a stable source identity and cannot be substituted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VehicleSourceId(String);

impl VehicleSourceId {
    /// Validate and construct a source identity. The restricted token shape is
    /// suitable for the identity-addressed Bus topic and prevents path-like ids.
    pub fn new(value: impl Into<String>) -> Result<Self, VehicleRosterError> {
        let value = value.into();
        validate_roster_id(&value, "source", VehicleRosterError::InvalidSourceId)?;
        Ok(Self(value))
    }

    /// Borrow the stable identity token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for VehicleSourceId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for VehicleSourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors from configuring or ingesting the bounded vehicle roster seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VehicleRosterError {
    /// A source identity was empty, oversized, or path-like.
    InvalidSourceId(String),
    /// A manager identity was empty, oversized, or path-like.
    InvalidManagerId(String),
    /// A poll or heartbeat interval was outside the bounded scheduler range.
    InvalidPollPlan(String),
    /// The source-count bound was reached.
    SourceCapacity,
    /// The manager-count bound was reached.
    ManagerCapacity,
    /// The assignment-count bound was reached.
    AssignmentCapacity,
    /// The same source is already assigned to this manager.
    DuplicateAssignment {
        /// Stable MG90 source identity.
        source_id: VehicleSourceId,
        /// Management node identity.
        manager_id: String,
    },
    /// An observation was received for an assignment not present in the roster.
    UnregisteredAssignment {
        /// Stable MG90 source identity.
        source_id: VehicleSourceId,
        /// Management node identity.
        manager_id: String,
    },
    /// The confirmed identity in a snapshot did not match the roster key.
    IdentityMismatch {
        /// Stable MG90 source identity expected by the roster.
        expected: VehicleSourceId,
        /// Identity reported by the snapshot.
        reported: String,
        /// Management node that supplied the snapshot.
        manager_id: String,
    },
    /// The snapshot uses a schema version this roster cannot safely interpret.
    UnsupportedSchemaVersion {
        /// Schema version understood by this binary.
        expected: u16,
        /// Schema version carried by the incoming snapshot.
        actual: u16,
    },
}

impl fmt::Display for VehicleRosterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSourceId(detail) => write!(f, "invalid vehicle source id: {detail}"),
            Self::InvalidManagerId(detail) => write!(f, "invalid vehicle manager id: {detail}"),
            Self::InvalidPollPlan(detail) => write!(f, "invalid vehicle poll plan: {detail}"),
            Self::SourceCapacity => write!(f, "vehicle source roster capacity reached"),
            Self::ManagerCapacity => write!(f, "vehicle manager roster capacity reached"),
            Self::AssignmentCapacity => write!(f, "vehicle roster assignment capacity reached"),
            Self::DuplicateAssignment {
                source_id,
                manager_id,
            } => write!(
                f,
                "vehicle source {source_id} already assigned to manager {manager_id}"
            ),
            Self::UnregisteredAssignment {
                source_id,
                manager_id,
            } => write!(
                f,
                "vehicle source {source_id} is not assigned to manager {manager_id}"
            ),
            Self::IdentityMismatch {
                expected,
                reported,
                manager_id,
            } => write!(
                f,
                "vehicle source {expected} reported identity {reported} from manager {manager_id}"
            ),
            Self::UnsupportedSchemaVersion { expected, actual } => write!(
                f,
                "vehicle snapshot schema version {actual} is unsupported (expected {expected})"
            ),
        }
    }
}

impl std::error::Error for VehicleRosterError {}

/// Per-assignment polling and heartbeat cadence.
///
/// The roster and production adapter expose current status, slow enrichment,
/// and heartbeat as independent lanes. The synchronous [`VehicleWorker::build_state`]
/// compatibility helper still returns one fully folded snapshot to direct callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VehiclePollPlan {
    /// Fast current-status poll interval.
    pub poll: Duration,
    /// Slow enrichment interval. Enrichment has an independent in-flight gate
    /// and cannot consume or postpone a status/heartbeat deadline.
    pub enrichment: Duration,
    /// Independent latest-snapshot heartbeat interval.
    pub heartbeat: Duration,
}

impl VehiclePollPlan {
    /// Build a plan and reject zero, sub-millisecond, or excessively long periods.
    pub fn new(poll: Duration, heartbeat: Duration) -> Result<Self, VehicleRosterError> {
        let plan = Self {
            poll,
            enrichment: poll,
            heartbeat,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Override the slow-enrichment cadence without coupling it to current
    /// status or heartbeat scheduling.
    pub fn with_enrichment(mut self, enrichment: Duration) -> Result<Self, VehicleRosterError> {
        self.enrichment = enrichment;
        self.validate()?;
        Ok(self)
    }

    /// The compatibility plan for one gateway with an independent heartbeat.
    #[must_use]
    pub const fn single_gateway(poll: Duration) -> Self {
        Self {
            poll,
            enrichment: poll,
            heartbeat: ROSTER_HEARTBEAT,
        }
    }

    /// A multi-source default with the worker's normal poll and a separate
    /// two-second heartbeat.
    #[must_use]
    pub const fn multi_source(poll: Duration) -> Self {
        Self {
            poll,
            enrichment: poll,
            heartbeat: ROSTER_HEARTBEAT,
        }
    }

    fn validate(self) -> Result<(), VehicleRosterError> {
        for (name, interval) in [
            ("poll", self.poll),
            ("enrichment", self.enrichment),
            ("heartbeat", self.heartbeat),
        ] {
            if interval < MIN_ROSTER_INTERVAL || interval > MAX_ROSTER_INTERVAL {
                return Err(VehicleRosterError::InvalidPollPlan(format!(
                    "{name} must be between {:?} and {:?}",
                    MIN_ROSTER_INTERVAL, MAX_ROSTER_INTERVAL
                )));
            }
        }
        if self.heartbeat > MAX_ROSTER_HEARTBEAT {
            return Err(VehicleRosterError::InvalidPollPlan(format!(
                "heartbeat must be at most {:?}",
                MAX_ROSTER_HEARTBEAT
            )));
        }
        Ok(())
    }
}

impl Default for VehiclePollPlan {
    fn default() -> Self {
        Self::multi_source(POLL)
    }
}

/// One scheduled action returned by [`VehicleRoster::take_due`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VehicleScheduleKind {
    /// Read the current status plane for the assignment.
    CurrentStatus,
    /// Emit the already accepted latest snapshot, if one exists.
    Heartbeat,
    /// Run slow radio/GNSS/application enrichment independently.
    Enrichment,
}

/// A source/manager assignment due for current status, heartbeat, or enrichment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VehicleScheduledWork {
    /// Stable MG90 identity.
    pub source_id: VehicleSourceId,
    /// Management node that owns this assignment.
    pub manager_id: String,
    /// Work to perform.
    pub kind: VehicleScheduleKind,
}

/// Why the roster has no publishable source snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VehicleNoSourceReason {
    /// No assignments have been registered.
    EmptyRoster,
    /// The requested source is not in this roster.
    SourceNotRegistered,
    /// The assignment exists, but has no local probe and has not received a
    /// remote manager snapshot.
    NoAcceptedSnapshot,
    /// A local assignment exists without a configured probe.
    ProbeUnavailable,
    /// A reachable poll did not report a stable MG90 identity.
    IdentityUnconfirmed,
    /// A poll reported an identity different from the stable roster identity.
    IdentityMismatch {
        /// Identity reported by the attempted source.
        reported: String,
    },
}

/// An explicit latest-wins result. `NoSource` is intentionally not converted to
/// [`VehicleState::offline`]: absence of a source is not evidence that a gateway
/// is reachable and offline.
#[derive(Debug, Clone, PartialEq)]
pub enum VehicleRosterSelection {
    /// The freshest valid snapshot across all registered managers for one MG90.
    Selected(VehicleRosterSnapshot),
    /// Nothing valid may be published for the requested source.
    NoSource {
        /// Requested source, when selection was source-specific.
        source_id: Option<VehicleSourceId>,
        /// Explicit reason for the absence.
        reason: VehicleNoSourceReason,
    },
}

/// Why a typed snapshot was not eligible for manager-routed publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VehicleManagerRouteRejection {
    /// The gateway revoked management approval after this snapshot was emitted.
    ApprovalRevoked,
    /// The snapshot carries an authoritative manager set that excludes the
    /// manager which supplied it.
    ManagerNotEnrolled,
}

/// A typed, identity-addressed publication route selected from the roster.
///
/// The route owns the exact v2 snapshot and Bus topic together so callers cannot
/// accidentally publish telemetry under a different manager/source pair. The
/// snapshot remains read-only here; action authorization is a separate seam.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleManagerRoute {
    source_id: VehicleSourceId,
    manager_id: String,
    topic: String,
    snapshot: VehicleStateV2,
}

impl VehicleManagerRoute {
    /// Stable MG90 identity carried by this route.
    #[must_use]
    pub fn source_id(&self) -> &VehicleSourceId {
        &self.source_id
    }

    /// Manager that supplied and is authorized for this route.
    #[must_use]
    pub fn manager_id(&self) -> &str {
        &self.manager_id
    }

    /// Identity-addressed Bus topic for this exact snapshot.
    #[must_use]
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Borrow the typed telemetry snapshot without projecting or fabricating
    /// any of its domains.
    #[must_use]
    pub fn snapshot(&self) -> &VehicleStateV2 {
        &self.snapshot
    }
}

/// Result of manager-routing one source's latest accepted telemetry.
#[derive(Debug, Clone, PartialEq)]
pub enum VehicleManagerRouteSelection {
    /// The freshest eligible snapshot and its bound publication topic.
    Routed(VehicleManagerRoute),
    /// No accepted source snapshot exists.
    NoSource {
        /// Requested source.
        source_id: VehicleSourceId,
        /// Honest reason no source can be routed.
        reason: VehicleNoSourceReason,
    },
    /// Accepted snapshots exist, but none is eligible for publication by its
    /// supplying manager.
    Rejected {
        /// Requested source.
        source_id: VehicleSourceId,
        /// Manager associated with the deterministically freshest rejected row.
        manager_id: String,
        /// Why that manager cannot route this snapshot.
        reason: VehicleManagerRouteRejection,
    },
}

fn manager_route_rejection(
    snapshot: &VehicleStateV2,
    manager_id: &str,
) -> Option<VehicleManagerRouteRejection> {
    if matches!(
        snapshot.approval,
        mackes_mesh_types::vehicle::ApprovalState::Revoked
    ) {
        return Some(VehicleManagerRouteRejection::ApprovalRevoked);
    }
    // An unknown manager set is not proof that this manager is enrolled. Keep
    // the route fail-closed during legacy/partial snapshots rather than
    // allowing an un-enrolled manager to regain publication rights.
    if snapshot.managers.state != ManagerSetState::Complete
        || !snapshot.managers.ids.iter().any(|id| id == manager_id)
    {
        return Some(VehicleManagerRouteRejection::ManagerNotEnrolled);
    }
    None
}

/// One identity-checked snapshot retained by the bounded roster.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleRosterSnapshot {
    source_id: VehicleSourceId,
    manager_id: String,
    snapshot: VehicleStateV2,
}

impl VehicleRosterSnapshot {
    /// Accept a v2 snapshot only when its confirmed MG90 and manager identities
    /// agree with the roster assignment. No telemetry is changed or synthesized.
    pub fn from_v2(
        source_id: VehicleSourceId,
        manager_id: impl Into<String>,
        snapshot: VehicleStateV2,
    ) -> Result<Self, VehicleRosterError> {
        let manager_id = validate_manager_id(&manager_id.into())?;
        if snapshot.schema_version != VEHICLE_STATE_V2_SCHEMA_VERSION {
            return Err(VehicleRosterError::UnsupportedSchemaVersion {
                expected: VEHICLE_STATE_V2_SCHEMA_VERSION,
                actual: snapshot.schema_version,
            });
        }
        if snapshot.mg90.id != source_id.as_str() || snapshot.mg90.esn != source_id.as_str() {
            return Err(VehicleRosterError::IdentityMismatch {
                expected: source_id,
                reported: snapshot.mg90.id,
                manager_id,
            });
        }
        if snapshot.management_node_id != manager_id {
            return Err(VehicleRosterError::IdentityMismatch {
                expected: source_id,
                reported: format!(
                    "manager {} reported snapshot from {}",
                    manager_id, snapshot.management_node_id
                ),
                manager_id,
            });
        }
        Ok(Self {
            source_id,
            manager_id,
            snapshot,
        })
    }

    /// Stable MG90 identity.
    #[must_use]
    pub fn source_id(&self) -> &VehicleSourceId {
        &self.source_id
    }

    /// Management node identity.
    #[must_use]
    pub fn manager_id(&self) -> &str {
        &self.manager_id
    }

    /// Borrow the accepted v2 snapshot for publication or read-only rendering.
    #[must_use]
    pub fn snapshot(&self) -> &VehicleStateV2 {
        &self.snapshot
    }

    fn freshness_cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.snapshot
            .observed_at_ms
            .cmp(&other.snapshot.observed_at_ms)
            .then_with(|| {
                self.snapshot
                    .published_at_ms
                    .cmp(&other.snapshot.published_at_ms)
            })
            .then_with(|| self.snapshot.sequence.cmp(&other.snapshot.sequence))
            // A tie between managers is still deterministic. The lexical manager
            // id is only a tie-breaker; it never beats a newer observation.
            .then_with(|| self.manager_id.cmp(&other.manager_id))
    }

    fn content_eq(&self, other: &Self) -> bool {
        let mut left = self.clone();
        let mut right = other.clone();
        for snapshot in [&mut left.snapshot, &mut right.snapshot] {
            snapshot.sequence = 0;
            snapshot.observed_at_ms = 0;
            snapshot.published_at_ms = 0;
        }
        left == right
    }
}

/// Why an accepted MG90 snapshot is ready for publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehiclePublicationReason {
    /// The selected accepted snapshot changed semantically.
    Changed,
    /// The selected snapshot is unchanged, but its bounded heartbeat is due.
    Heartbeat,
}

/// One identity-bound publication selected by [`VehicleRoster`].
///
/// The snapshot is cloned exactly from an accepted manager row. Scheduling does
/// not refresh timestamps, fill absent values, or otherwise manufacture fields.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleRosterPublication {
    /// Stable gateway identity.
    pub source_id: VehicleSourceId,
    /// Manager whose accepted row won deterministic freshness selection.
    pub manager_id: String,
    /// Immediate change or bounded heartbeat.
    pub reason: VehiclePublicationReason,
    /// Exact accepted snapshot to publish.
    pub snapshot: VehicleStateV2,
}

struct VehiclePublishedState {
    snapshot: VehicleRosterSnapshot,
    published_at: Instant,
}

/// A configured source/manager assignment in the opt-in roster.
pub struct VehicleRosterSource {
    source_id: VehicleSourceId,
    manager_id: String,
    worker: Option<Arc<VehicleWorker>>,
    plan: VehiclePollPlan,
}

impl VehicleRosterSource {
    /// Configure one assignment. `worker: None` is useful for a remote manager
    /// whose snapshots arrive through the already-typed Bus/mesh ingest seam.
    pub fn new(
        source_id: VehicleSourceId,
        manager_id: impl Into<String>,
        worker: Option<Arc<VehicleWorker>>,
        plan: VehiclePollPlan,
    ) -> Result<Self, VehicleRosterError> {
        let manager_id = validate_manager_id(&manager_id.into())?;
        plan.validate()?;
        Ok(Self {
            source_id,
            manager_id,
            worker,
            plan,
        })
    }

    /// Configure a source with a local real [`VehicleWorker`] probe.
    pub fn local(
        source_id: VehicleSourceId,
        manager_id: impl Into<String>,
        worker: Arc<VehicleWorker>,
        plan: VehiclePollPlan,
    ) -> Result<Self, VehicleRosterError> {
        Self::new(source_id, manager_id, Some(worker), plan)
    }

    /// Configure a source assignment that receives snapshots from another
    /// manager. It does not claim that manager or gateway is reachable.
    pub fn remote(
        source_id: VehicleSourceId,
        manager_id: impl Into<String>,
        plan: VehiclePollPlan,
    ) -> Result<Self, VehicleRosterError> {
        Self::new(source_id, manager_id, None, plan)
    }
}

struct VehicleRosterAssignment {
    source: VehicleRosterSource,
    next_status: Instant,
    next_enrichment: Instant,
    next_heartbeat: Instant,
    enrichment_in_flight: bool,
    latest: Option<VehicleRosterSnapshot>,
}

/// Bounded scheduler and latest-wins read model around one or more
/// [`VehicleWorker`] instances.
///
/// The existing worker remains the production single-gateway path. This seam is
/// opt-in: callers register one stable MG90 id per manager, schedule each
/// assignment independently, feed local polls through [`Self::poll_source`],
/// and ingest already-typed remote-manager snapshots through [`Self::ingest`].
/// It retains at most one accepted snapshot per assignment and selects the
/// freshest valid one for an MG90 without inventing an offline state.
pub struct VehicleRoster {
    assignments: BTreeMap<(VehicleSourceId, String), VehicleRosterAssignment>,
    published: BTreeMap<VehicleSourceId, VehiclePublishedState>,
    started_at: Instant,
}

impl VehicleRoster {
    /// Start an empty roster. All newly registered assignments are immediately
    /// due once, making the first poll deterministic for tests and callers.
    #[must_use]
    pub fn new(started_at: Instant) -> Self {
        Self {
            assignments: BTreeMap::new(),
            published: BTreeMap::new(),
            started_at,
        }
    }

    /// Register one `(MG90, manager)` assignment.
    pub fn register(&mut self, source: VehicleRosterSource) -> Result<(), VehicleRosterError> {
        let key = (source.source_id.clone(), source.manager_id.clone());
        if self.assignments.contains_key(&key) {
            return Err(VehicleRosterError::DuplicateAssignment {
                source_id: source.source_id,
                manager_id: source.manager_id,
            });
        }
        let source_already_registered = self
            .assignments
            .keys()
            .any(|(source_id, _)| source_id == &source.source_id);
        if !source_already_registered
            && self
                .assignments
                .keys()
                .map(|(source_id, _)| source_id)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                >= MAX_VEHICLE_ROSTER_SOURCES
        {
            return Err(VehicleRosterError::SourceCapacity);
        }
        if self
            .assignments
            .values()
            .map(|assignment| assignment.source.manager_id.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            >= MAX_VEHICLE_ROSTER_MANAGERS
            && !self
                .assignments
                .values()
                .any(|assignment| assignment.source.manager_id == source.manager_id)
        {
            return Err(VehicleRosterError::ManagerCapacity);
        }
        if self.assignments.len() >= MAX_VEHICLE_ROSTER_ASSIGNMENTS {
            return Err(VehicleRosterError::AssignmentCapacity);
        }
        self.assignments.insert(
            key,
            VehicleRosterAssignment {
                source,
                next_status: self.started_at,
                next_enrichment: self.started_at,
                next_heartbeat: self.started_at,
                enrichment_in_flight: false,
                latest: None,
            },
        );
        Ok(())
    }

    /// Number of configured source/manager assignments.
    #[must_use]
    pub fn assignment_count(&self) -> usize {
        self.assignments.len()
    }

    /// Return the registered stable source identities in deterministic order.
    #[must_use]
    pub fn source_ids(&self) -> Vec<VehicleSourceId> {
        let mut ids = self
            .assignments
            .keys()
            .map(|(source_id, _)| source_id.clone())
            .collect::<Vec<_>>();
        ids.dedup();
        ids
    }

    /// Return all currently due status, heartbeat, and enrichment work in stable
    /// source/manager order. Missed intervals coalesce to one next deadline; the
    /// roster never emits an unbounded catch-up burst. A dispatched enrichment
    /// remains in flight until [`Self::finish_enrichment`] is called, but it never
    /// suppresses current-status or heartbeat work.
    pub fn take_due(&mut self, now: Instant) -> Vec<VehicleScheduledWork> {
        let mut due = Vec::new();
        for assignment in self.assignments.values_mut() {
            if now >= assignment.next_status {
                due.push(VehicleScheduledWork {
                    source_id: assignment.source.source_id.clone(),
                    manager_id: assignment.source.manager_id.clone(),
                    kind: VehicleScheduleKind::CurrentStatus,
                });
                assignment.next_status = next_deadline(now, assignment.source.plan.poll);
            }
            if now >= assignment.next_heartbeat {
                due.push(VehicleScheduledWork {
                    source_id: assignment.source.source_id.clone(),
                    manager_id: assignment.source.manager_id.clone(),
                    kind: VehicleScheduleKind::Heartbeat,
                });
                assignment.next_heartbeat = next_deadline(now, assignment.source.plan.heartbeat);
            }
            if now >= assignment.next_enrichment && !assignment.enrichment_in_flight {
                due.push(VehicleScheduledWork {
                    source_id: assignment.source.source_id.clone(),
                    manager_id: assignment.source.manager_id.clone(),
                    kind: VehicleScheduleKind::Enrichment,
                });
                assignment.next_enrichment = next_deadline(now, assignment.source.plan.enrichment);
                assignment.enrichment_in_flight = true;
            }
        }
        due.sort_by(|a, b| {
            a.source_id
                .cmp(&b.source_id)
                .then_with(|| a.manager_id.cmp(&b.manager_id))
                .then_with(|| a.kind.cmp(&b.kind))
        });
        due
    }

    /// Release one assignment's enrichment lane after either success or failure.
    ///
    /// This method deliberately carries no telemetry. A failed enrichment only
    /// releases the lane; it cannot erase or synthesize fields in the retained
    /// current snapshot.
    pub fn finish_enrichment(
        &mut self,
        source_id: &VehicleSourceId,
        manager_id: &str,
    ) -> Result<(), VehicleRosterError> {
        let key = (source_id.clone(), manager_id.to_string());
        let Some(assignment) = self.assignments.get_mut(&key) else {
            return Err(VehicleRosterError::UnregisteredAssignment {
                source_id: source_id.clone(),
                manager_id: manager_id.to_string(),
            });
        };
        assignment.enrichment_in_flight = false;
        Ok(())
    }

    /// Run one configured local worker poll and retain it only if its confirmed
    /// MG90 identity matches the stable roster identity. A missing probe or
    /// unconfirmed identity is an explicit no-source result and does not erase a
    /// previously accepted snapshot.
    pub fn poll_source(
        &mut self,
        source_id: &VehicleSourceId,
        manager_id: &str,
    ) -> VehicleRosterPollResult {
        let key = (source_id.clone(), manager_id.to_string());
        let Some(assignment) = self.assignments.get(&key) else {
            return VehicleRosterPollResult::NoSource {
                source_id: Some(source_id.clone()),
                reason: VehicleNoSourceReason::SourceNotRegistered,
            };
        };
        let Some(worker) = assignment.source.worker.clone() else {
            return VehicleRosterPollResult::NoSource {
                source_id: Some(source_id.clone()),
                reason: VehicleNoSourceReason::ProbeUnavailable,
            };
        };
        let snapshot = match worker.build_roster_snapshot(source_id) {
            Ok(snapshot) => snapshot,
            Err(reason) => {
                return VehicleRosterPollResult::NoSource {
                    source_id: Some(source_id.clone()),
                    reason,
                }
            }
        };
        match self.ingest(snapshot) {
            Ok(_) => VehicleRosterPollResult::Updated(self.select_latest(source_id)),
            Err(error) => VehicleRosterPollResult::NoSource {
                source_id: Some(source_id.clone()),
                reason: no_source_reason_from_roster_error(error),
            },
        }
    }

    /// Ingest one identity-checked snapshot from a local or remote manager.
    /// Older observations never replace a newer one for the same assignment.
    /// Returns whether the retained assignment snapshot changed.
    pub fn ingest(&mut self, snapshot: VehicleRosterSnapshot) -> Result<bool, VehicleRosterError> {
        let key = (snapshot.source_id.clone(), snapshot.manager_id.clone());
        let Some(assignment) = self.assignments.get_mut(&key) else {
            return Err(VehicleRosterError::UnregisteredAssignment {
                source_id: snapshot.source_id,
                manager_id: snapshot.manager_id,
            });
        };
        let replace = assignment
            .latest
            .as_ref()
            .is_none_or(|current| snapshot.freshness_cmp(current).is_gt());
        if replace {
            assignment.latest = Some(snapshot);
        }
        Ok(replace)
    }

    /// Select the freshest valid snapshot across all registered managers for one
    /// stable MG90 source. The manager id is only a deterministic tie-breaker.
    #[must_use]
    pub fn select_latest(&self, source_id: &VehicleSourceId) -> VehicleRosterSelection {
        let mut found_assignment = false;
        let mut selected: Option<&VehicleRosterSnapshot> = None;
        for ((candidate_source, _), assignment) in &self.assignments {
            if candidate_source != source_id {
                continue;
            }
            found_assignment = true;
            if let Some(candidate) = assignment.latest.as_ref() {
                if selected.is_none_or(|current| candidate.freshness_cmp(current).is_gt()) {
                    selected = Some(candidate);
                }
            }
        }
        if let Some(snapshot) = selected {
            VehicleRosterSelection::Selected(snapshot.clone())
        } else {
            VehicleRosterSelection::NoSource {
                source_id: Some(source_id.clone()),
                reason: if found_assignment {
                    VehicleNoSourceReason::NoAcceptedSnapshot
                } else if self.assignments.is_empty() {
                    VehicleNoSourceReason::EmptyRoster
                } else {
                    VehicleNoSourceReason::SourceNotRegistered
                },
            }
        }
    }

    /// Select the freshest snapshot whose supplying manager is still eligible
    /// to route it. A newer revoked or un-enrolled row is skipped in favor of an
    /// older eligible manager snapshot, which keeps deduplication honest during
    /// approval changes and manager takeover.
    #[must_use]
    pub fn route_latest(&self, source_id: &VehicleSourceId) -> VehicleManagerRouteSelection {
        let mut found_assignment = false;
        let mut selected: Option<&VehicleRosterSnapshot> = None;
        let mut rejected: Option<(&VehicleRosterSnapshot, VehicleManagerRouteRejection)> = None;

        for ((candidate_source, _), assignment) in &self.assignments {
            if candidate_source != source_id {
                continue;
            }
            found_assignment = true;
            let Some(candidate) = assignment.latest.as_ref() else {
                continue;
            };
            if let Some(reason) =
                manager_route_rejection(&candidate.snapshot, &candidate.manager_id)
            {
                if rejected
                    .as_ref()
                    .is_none_or(|(current, _)| candidate.freshness_cmp(current).is_gt())
                {
                    rejected = Some((candidate, reason));
                }
                continue;
            }
            if selected.is_none_or(|current| candidate.freshness_cmp(current).is_gt()) {
                selected = Some(candidate);
            }
        }

        if let Some(snapshot) = selected {
            return VehicleManagerRouteSelection::Routed(VehicleManagerRoute {
                source_id: snapshot.source_id.clone(),
                manager_id: snapshot.manager_id.clone(),
                topic: vehicle_state_v2_topic(
                    &snapshot.snapshot.management_node_id,
                    &snapshot.snapshot.mg90.id,
                ),
                snapshot: snapshot.snapshot.clone(),
            });
        }
        if let Some((snapshot, reason)) = rejected {
            return VehicleManagerRouteSelection::Rejected {
                source_id: source_id.clone(),
                manager_id: snapshot.manager_id.clone(),
                reason,
            };
        }
        VehicleManagerRouteSelection::NoSource {
            source_id: source_id.clone(),
            reason: if found_assignment {
                VehicleNoSourceReason::NoAcceptedSnapshot
            } else if self.assignments.is_empty() {
                VehicleNoSourceReason::EmptyRoster
            } else {
                VehicleNoSourceReason::SourceNotRegistered
            },
        }
    }

    /// Route every registered source in stable source-id order. No-source and
    /// rejected results stay explicit so consumers cannot turn them into an
    /// offline or fabricated telemetry row.
    #[must_use]
    pub fn route_latest_all(&self) -> Vec<VehicleManagerRouteSelection> {
        if self.assignments.is_empty() {
            return Vec::new();
        }
        self.source_ids()
            .iter()
            .map(|source_id| self.route_latest(source_id))
            .collect()
    }

    /// Select the freshest accepted snapshot for every registered MG90 source
    /// in stable source-id order. Each source remains explicit: a registered
    /// source without an accepted snapshot returns `NoSource` rather than an
    /// invented offline state. An empty roster returns one roster-level
    /// `NoSource` result so callers do not mistake an empty result for a
    /// successful empty read model.
    #[must_use]
    pub fn select_latest_all(&self) -> Vec<VehicleRosterSelection> {
        if self.assignments.is_empty() {
            return vec![VehicleRosterSelection::NoSource {
                source_id: None,
                reason: VehicleNoSourceReason::EmptyRoster,
            }];
        }
        self.source_ids()
            .iter()
            .map(|source_id| self.select_latest(source_id))
            .collect()
    }

    /// Select change-driven publications plus an unchanged heartbeat no slower
    /// than each source's configured (and validated) interval.
    ///
    /// Multiple managers for one MG90 collapse through [`Self::select_latest`];
    /// multiple MG90 identities retain independent clocks and are returned in
    /// stable source-id order. A source with no accepted snapshot emits nothing.
    pub fn take_publications(&mut self, now: Instant) -> Vec<VehicleRosterPublication> {
        let mut ready = Vec::new();
        for source_id in self.source_ids() {
            let VehicleRosterSelection::Selected(selected) = self.select_latest(&source_id) else {
                continue;
            };
            let heartbeat = self
                .assignments
                .iter()
                .filter(|((candidate, _), _)| candidate == &source_id)
                .map(|(_, assignment)| assignment.source.plan.heartbeat)
                .min()
                .unwrap_or(ROSTER_HEARTBEAT);
            let reason = match self.published.get(&source_id) {
                None => Some(VehiclePublicationReason::Changed),
                Some(previous) if !previous.snapshot.content_eq(&selected) => {
                    Some(VehiclePublicationReason::Changed)
                }
                Some(previous)
                    if now.saturating_duration_since(previous.published_at) >= heartbeat =>
                {
                    Some(VehiclePublicationReason::Heartbeat)
                }
                Some(_) => None,
            };

            if let Some(reason) = reason {
                self.published.insert(
                    source_id.clone(),
                    VehiclePublishedState {
                        snapshot: selected.clone(),
                        published_at: now,
                    },
                );
                ready.push(VehicleRosterPublication {
                    source_id,
                    manager_id: selected.manager_id.clone(),
                    reason,
                    snapshot: selected.snapshot,
                });
            } else if let Some(previous) = self.published.get_mut(&source_id) {
                // Preserve the publication clock while retaining the newest
                // observation for the next heartbeat.
                previous.snapshot = selected;
            }
        }
        ready
    }

    /// Select the source snapshot that a heartbeat may repeat. No accepted
    /// snapshot means no publication, even when a heartbeat deadline is due.
    #[must_use]
    pub fn heartbeat(&self, source_id: &VehicleSourceId) -> VehicleRosterSelection {
        self.select_latest(source_id)
    }
}

/// Result of one local roster poll.
#[derive(Debug, Clone, PartialEq)]
pub enum VehicleRosterPollResult {
    /// The poll was accepted and the resulting selected snapshot is returned.
    Updated(VehicleRosterSelection),
    /// No real source snapshot was available; nothing was fabricated.
    NoSource {
        /// Source involved in the attempted poll.
        source_id: Option<VehicleSourceId>,
        /// Explicit no-source reason.
        reason: VehicleNoSourceReason,
    },
}

fn validate_manager_id(value: &str) -> Result<String, VehicleRosterError> {
    validate_roster_id(value, "manager", VehicleRosterError::InvalidManagerId)
}

fn validate_roster_id<F>(
    value: &str,
    kind: &str,
    make_error: F,
) -> Result<String, VehicleRosterError>
where
    F: FnOnce(String) -> VehicleRosterError,
{
    if value.is_empty() {
        return Err(make_error(format!("{kind} id is empty")));
    }
    if value.len() > MAX_ROSTER_ID_BYTES {
        return Err(make_error(format!(
            "{kind} id exceeds {MAX_ROSTER_ID_BYTES} bytes"
        )));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(make_error(format!(
            "{kind} id contains a path or unsafe character"
        )));
    }
    Ok(value.to_string())
}

fn next_deadline(now: Instant, interval: Duration) -> Instant {
    now.checked_add(interval).unwrap_or(now)
}

fn vehicle_state_content_eq(left: &VehicleState, right: &VehicleState) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.published_at_ms = 0;
    right.published_at_ms = 0;
    left == right
}

fn no_source_reason_from_roster_error(error: VehicleRosterError) -> VehicleNoSourceReason {
    match error {
        VehicleRosterError::IdentityMismatch { reported, .. } => {
            VehicleNoSourceReason::IdentityMismatch { reported }
        }
        _ => VehicleNoSourceReason::NoAcceptedSnapshot,
    }
}

#[derive(Debug, Clone)]
struct VehicleCurrentStatusObservation {
    online: bool,
    model: Option<String>,
    esn: Option<String>,
    mgos_version: Option<String>,
    battery_v: Option<f32>,
    internal_temp_c: Option<f32>,
    ignition_on: Option<bool>,
    beacon_gps: Option<GpsFix>,
    gaps: Vec<String>,
    observed_at_ms: i64,
}

#[derive(Debug)]
struct VehicleEnrichmentObservation {
    gps: Option<GpsFix>,
    imu: Option<ImuSample>,
    gps_gaps: Vec<String>,
    wan: Option<WanStatus>,
    wan_gaps: Vec<String>,
    obd_probe_status: DeviceProbeStatus,
    obd_gaps: Vec<String>,
}

#[derive(Debug, Clone)]
struct VehicleRuntimeSnapshot {
    host: String,
    online: bool,
    model: Option<String>,
    esn: Option<String>,
    mgos_version: Option<String>,
    battery_v: Option<f32>,
    internal_temp_c: Option<f32>,
    ignition_on: Option<bool>,
    beacon_gps: Option<GpsFix>,
    enrichment_gps: Option<GpsFix>,
    imu: Option<ImuSample>,
    wan: Option<WanStatus>,
    obd_probe_status: DeviceProbeStatus,
    current_gaps: Vec<String>,
    gps_gaps: Vec<String>,
    wan_gaps: Vec<String>,
    obd_gaps: Vec<String>,
    observed_at_ms: i64,
}

impl VehicleRuntimeSnapshot {
    fn pending(host: &str) -> Self {
        Self::from_current(
            host,
            VehicleCurrentStatusObservation {
                online: false,
                model: None,
                esn: None,
                mgos_version: None,
                battery_v: None,
                internal_temp_c: None,
                ignition_on: None,
                beacon_gps: None,
                gaps: vec!["current status pending".to_string()],
                observed_at_ms: now_ms(),
            },
        )
    }

    fn from_current(host: &str, current: VehicleCurrentStatusObservation) -> Self {
        let mut snapshot = Self {
            host: host.to_string(),
            online: false,
            model: None,
            esn: None,
            mgos_version: None,
            battery_v: None,
            internal_temp_c: None,
            ignition_on: None,
            beacon_gps: None,
            enrichment_gps: None,
            imu: None,
            wan: None,
            obd_probe_status: DeviceProbeStatus::Unknown,
            current_gaps: Vec::new(),
            gps_gaps: vec!["gps/imu unavailable (enrichment pending)".to_string()],
            wan_gaps: vec!["wan status unavailable (enrichment pending)".to_string()],
            obd_gaps: vec!["OBD enrichment pending".to_string()],
            observed_at_ms: current.observed_at_ms,
        };
        snapshot.apply_current(current);
        snapshot
    }

    fn apply_current(&mut self, current: VehicleCurrentStatusObservation) {
        self.online = current.online;
        if let Some(value) = current.model {
            self.model = Some(value);
        }
        if let Some(value) = current.esn {
            self.esn = Some(value);
        }
        if let Some(value) = current.mgos_version {
            self.mgos_version = Some(value);
        }
        if let Some(value) = current.battery_v {
            self.battery_v = Some(value);
        }
        if let Some(value) = current.internal_temp_c {
            self.internal_temp_c = Some(value);
        }
        if let Some(value) = current.ignition_on {
            self.ignition_on = Some(value);
        }
        // A status batch owns only its current beacon sample. An absent packet
        // falls back to retained NMEA enrichment rather than replaying a beacon
        // as if it had been observed again.
        self.beacon_gps = current.beacon_gps;
        self.current_gaps = current.gaps;
        self.observed_at_ms = current.observed_at_ms;
    }

    fn apply_enrichment(&mut self, enrichment: VehicleEnrichmentObservation) {
        if let Some(gps) = enrichment.gps {
            self.enrichment_gps = Some(gps);
        }
        if let Some(imu) = enrichment.imu {
            self.imu = Some(imu);
        }
        if let Some(wan) = enrichment.wan {
            match self.wan.as_mut() {
                Some(current) => merge_sourced_wan(current, wan),
                None => self.wan = Some(wan),
            }
        }
        self.obd_probe_status = enrichment.obd_probe_status;
        self.gps_gaps = enrichment.gps_gaps;
        self.wan_gaps = enrichment.wan_gaps;
        self.obd_gaps = enrichment.obd_gaps;
    }

    fn mark_enrichment_unavailable(&mut self, reason: &str) {
        self.gps_gaps = vec![format!("gps/imu unavailable ({reason})")];
        self.wan_gaps = vec![format!("wan status unavailable ({reason})")];
        self.obd_gaps = vec![format!("OBD application unavailable ({reason})")];
        if !matches!(self.obd_probe_status, DeviceProbeStatus::Supported) {
            self.obd_probe_status = DeviceProbeStatus::Failed {
                reason: reason.to_string(),
            };
        }
    }

    fn mark_current_unavailable(&mut self, reason: &str) {
        self.online = false;
        self.beacon_gps = None;
        self.current_gaps = vec![format!("current status unavailable ({reason})")];
        self.observed_at_ms = now_ms();
    }

    fn render(&self) -> VehicleState {
        let mut gps = self.enrichment_gps.clone().unwrap_or_default();
        if let Some(beacon) = self.beacon_gps.clone() {
            gps = merge_beacon_gps(gps, beacon);
        }
        let mut gaps = Vec::with_capacity(
            self.current_gaps.len()
                + self.gps_gaps.len()
                + self.wan_gaps.len()
                + self.obd_gaps.len(),
        );
        gaps.extend(self.current_gaps.iter().cloned());
        gaps.extend(self.gps_gaps.iter().cloned());
        gaps.extend(self.wan_gaps.iter().cloned());
        gaps.extend(self.obd_gaps.iter().cloned());
        VehicleState {
            host: self.host.clone(),
            model: self.model.clone().unwrap_or_default(),
            esn: self.esn.clone().unwrap_or_default(),
            mgos_version: self.mgos_version.clone().unwrap_or_default(),
            online: self.online,
            gps: gps.clone(),
            imu: self.imu.clone(),
            wan: self.wan.clone().unwrap_or_default(),
            telem: VehicleTelem {
                battery_v: self.battery_v.unwrap_or_default(),
                internal_temp_c: self.internal_temp_c.unwrap_or_default(),
                ignition_on: self.ignition_on.unwrap_or_default(),
                moving: gps.speed_mph > 0.5,
                obd_present: self.obd_probe_status.is_supported(),
                obd_probe_status: self.obd_probe_status.clone(),
                ..Default::default()
            },
            gaps,
            published_at_ms: self.observed_at_ms,
        }
    }
}

fn merge_sourced_wan(current: &mut WanStatus, observed: WanStatus) {
    if !observed.active_wan.is_empty() {
        current.active_wan = observed.active_wan;
    }
    if cell_link_has_observation(&observed.cellular_a) {
        current.cellular_a = observed.cellular_a;
    }
    if cell_link_has_observation(&observed.cellular_b) {
        current.cellular_b = observed.cellular_b;
    }
    if !observed.wifi_state.is_empty() {
        current.wifi_state = observed.wifi_state;
    }
    if !observed.ethernet_state.is_empty() {
        current.ethernet_state = observed.ethernet_state;
    }
    if !observed.vpn_state.is_empty() {
        current.vpn_state = observed.vpn_state;
    }
    if observed.failover_events != 0 {
        current.failover_events = observed.failover_events;
    }
    if observed.latency_ms != 0 {
        current.latency_ms = observed.latency_ms;
    }
    if observed.packet_loss_percent != 0.0 {
        current.packet_loss_percent = observed.packet_loss_percent;
    }
    if !observed.link_quality.is_empty() {
        current.link_quality = observed.link_quality;
    }
}

fn cell_link_has_observation(link: &CellLink) -> bool {
    !link.sim_state.is_empty()
        || !link.carrier.is_empty()
        || link.signal_dbm != 0
        || !link.technology.is_empty()
        || !link.wan_ip.is_empty()
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
    /// The Bus root the mirror publish targets + the `action/vehicle/*` drain reads
    /// (`None` ⇒ publish/drain is a swallowed no-op — a pre-RPM dev box / a test).
    bus_root: Option<PathBuf>,
    /// The hash-chain audit DB (a performed `reboot` audits here — mirrors the
    /// `cloud` worker's destructive-op audit).
    db_path: PathBuf,
    /// Poll + heartbeat cadence.
    poll: Duration,
    heartbeat: Duration,
    current_timeout: Duration,
    /// Per-management-node monotonic v2 snapshot sequence.
    sequence: AtomicU64,
    /// Shared, fail-closed authorization gate for destructive Bus mutations.
    authorizer: Arc<ActionAuthorizer>,
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
            db_path: crate::default_db_path(),
            poll: POLL,
            heartbeat: ROSTER_HEARTBEAT,
            current_timeout: CURRENT_STATUS_TIMEOUT,
            sequence: AtomicU64::new(0),
            authorizer: Arc::new(ActionAuthorizer::production()),
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

    /// Override the audit DB path (tests point it at a tempdir).
    #[must_use]
    pub fn with_db_path(mut self, p: PathBuf) -> Self {
        self.db_path = p;
        self
    }

    /// Override the poll cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    #[cfg(test)]
    fn with_heartbeat(mut self, heartbeat: Duration) -> Self {
        assert!(heartbeat <= MAX_ROSTER_HEARTBEAT);
        self.heartbeat = heartbeat;
        self
    }

    #[cfg(test)]
    fn with_current_timeout(mut self, timeout: Duration) -> Self {
        self.current_timeout = timeout;
        self
    }

    fn spawn_current_status(
        &self,
        probe: Arc<dyn VehicleProbe>,
    ) -> tokio::task::JoinHandle<VehicleCurrentStatusObservation> {
        let host = self.host.clone();
        tokio::task::spawn_blocking(move || Self::probe_current_status(&host, probe.as_ref()))
    }

    fn spawn_enrichment(
        &self,
        probe: Arc<dyn VehicleProbe>,
    ) -> tokio::task::JoinHandle<VehicleEnrichmentObservation> {
        tokio::task::spawn_blocking(move || Self::probe_enrichment(probe.as_ref()))
    }

    /// Inject an isolated verifier and replay ledger for hostile action tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Build the current `state/vehicle/<host>` mirror from the probe's three raw
    /// reads. The LCI general read is the reachability anchor: its failure ⇒ an
    /// honest [`VehicleState::offline`] snapshot. GPS (SSH) + WAN (HTTP) failures
    /// degrade to a `gaps` note rather than blanking the mirror.
    #[must_use]
    pub fn build_state(&self, probe: &dyn VehicleProbe) -> VehicleState {
        Self::build_state_for_host(&self.host, probe)
    }

    fn probe_current_status(
        host: &str,
        probe: &dyn VehicleProbe,
    ) -> VehicleCurrentStatusObservation {
        let observed_at_ms = now_ms();
        let general = match probe.read_lci_general() {
            Ok(general) => general,
            Err(error) => {
                tracing::warn!(
                    target: "mackesd::vehicle",
                    host = %host,
                    error = %error,
                    "vehicle gateway LCI unreachable — retaining sourced fields as offline"
                );
                return VehicleCurrentStatusObservation {
                    online: false,
                    model: None,
                    esn: None,
                    mgos_version: None,
                    battery_v: None,
                    internal_temp_c: None,
                    ignition_on: None,
                    beacon_gps: None,
                    gaps: vec!["gateway unreachable".to_string()],
                    observed_at_ms,
                };
            }
        };

        let mut gaps = Vec::new();
        let general_text = strip_html(&general);
        let mut status_beacon = match probe.read_status_beacon() {
            Ok(Some(raw)) => parse_status_beacon(&raw, &mut gaps),
            Ok(None) => None,
            Err(error) => {
                let reason = if error.kind() == io::ErrorKind::InvalidInput {
                    "configuration error"
                } else {
                    "unavailable"
                };
                gaps.push(format!("status broadcast {reason} (udp): {error}"));
                None
            }
        };
        let esn = find_token_after(&general_text, "ESN");
        if esn.is_none() {
            gaps.push("esn not reported by general.html".to_string());
        }
        validate_status_beacon_identity(
            &mut status_beacon,
            esn.as_deref().unwrap_or_default(),
            &mut gaps,
        );
        let battery_v = status_beacon
            .as_ref()
            .and_then(|beacon| beacon.general_information.as_ref())
            .and_then(|general| general.battery_v)
            .or_else(|| find_number_after(&general_text, "Main Battery Voltage"));
        if battery_v.is_none() {
            gaps.push(
                "telem.battery_v not reported by MG90 status/general.html or status broadcast"
                    .to_string(),
            );
        }
        let internal_temp_c = status_beacon
            .as_ref()
            .and_then(|beacon| beacon.general_information.as_ref())
            .and_then(|general| general.internal_temp_c)
            .or_else(|| find_number_after(&general_text, "Internal Temperature"));
        if internal_temp_c.is_none() {
            gaps.push(
                "telem.internal_temp_c not reported by MG90 status/general.html or status broadcast"
                    .to_string(),
            );
        }
        let mgos_version = find_token_after(&general_text, "Version");
        if mgos_version.is_none() {
            gaps.push("mgos_version not reported by general.html".to_string());
        }
        let model = find_token_after(&general_text, "Model");
        if model.is_none() {
            gaps.push("model not reported by general.html".to_string());
        }
        let ignition_on = status_beacon
            .as_ref()
            .and_then(|beacon| beacon.general_information.as_ref())
            .and_then(|general| general.ignition_on)
            .or_else(|| parse_ignition_observation(&general_text, &mut gaps));
        let beacon_gps = status_beacon
            .as_ref()
            .and_then(|beacon| status_beacon_gps(beacon, &mut gaps));

        VehicleCurrentStatusObservation {
            online: true,
            model,
            esn,
            mgos_version,
            battery_v,
            internal_temp_c,
            ignition_on,
            beacon_gps,
            gaps,
            observed_at_ms,
        }
    }

    fn probe_enrichment(probe: &dyn VehicleProbe) -> VehicleEnrichmentObservation {
        let mut gps_gaps = Vec::new();
        let (gps, imu) = match probe.read_gps_nmea() {
            Ok(nmea) => {
                let (gps, imu) = parse_gps_imu(&nmea, &mut gps_gaps);
                ((!gps.fix_type.is_empty()).then_some(gps), imu)
            }
            Err(error) => {
                gps_gaps.push(format!("gps/imu unavailable (ssh): {error}"));
                (None, None)
            }
        };

        let mut wan_gaps = Vec::new();
        let wan = match probe.read_lci_wan() {
            Ok(html) => Some(parse_wan(&html, &mut wan_gaps)),
            Err(error) => {
                wan_gaps.push(format!("wan status unavailable (http): {error}"));
                None
            }
        };

        let mut obd_gaps = Vec::new();
        let obd_probe_status = match probe.read_obd_status() {
            Ok(Some(raw)) if raw.trim().is_empty() => {
                obd_gaps.push(
                    "OBD application returned an empty response; typed OBD telemetry remains unavailable"
                        .to_string(),
                );
                DeviceProbeStatus::Unsupported {
                    reason: "OBD/HDOBD response schema is not verified".to_string(),
                }
            }
            Ok(Some(_)) => {
                obd_gaps.push(
                    "OBD application HTTP response received; payload schema is not verified, so typed OBD telemetry remains unavailable"
                        .to_string(),
                );
                DeviceProbeStatus::Unsupported {
                    reason: "OBD/HDOBD response schema is not verified".to_string(),
                }
            }
            Ok(None) => {
                obd_gaps.push(format!(
                    "OBD not wired; set {OBD_STATUS_PATH_ENV} to /obdii_status/ or /hdobd_status/ for a diagnostic read"
                ));
                DeviceProbeStatus::NotInstalled
            }
            Err(error) => {
                let reason = if error.kind() == io::ErrorKind::Unsupported {
                    "unsupported"
                } else if error.kind() == io::ErrorKind::InvalidInput {
                    "configuration error"
                } else {
                    "unavailable"
                };
                obd_gaps.push(format!("OBD application {reason} (HTTP): {error}"));
                if error.kind() == io::ErrorKind::Unsupported {
                    DeviceProbeStatus::Unsupported {
                        reason: error.to_string(),
                    }
                } else {
                    DeviceProbeStatus::Failed {
                        reason: error.to_string(),
                    }
                }
            }
        };

        VehicleEnrichmentObservation {
            gps,
            imu,
            gps_gaps,
            wan,
            wan_gaps,
            obd_probe_status,
            obd_gaps,
        }
    }

    fn build_state_for_host(host: &str, probe: &dyn VehicleProbe) -> VehicleState {
        let general = match probe.read_lci_general() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::vehicle",
                    host = %host, error = %e,
                    "vehicle gateway LCI unreachable — publishing offline mirror"
                );
                let mut s = VehicleState::offline(host);
                s.published_at_ms = now_ms();
                return s;
            }
        };

        let mut gaps: Vec<String> = Vec::new();
        let general_text = strip_html(&general);
        let mut status_beacon = match probe.read_status_beacon() {
            Ok(Some(raw)) => parse_status_beacon(&raw, &mut gaps),
            Ok(None) => None,
            Err(e) => {
                let reason = if e.kind() == io::ErrorKind::InvalidInput {
                    "configuration error"
                } else {
                    "unavailable"
                };
                gaps.push(format!("status broadcast {reason} (udp): {e}"));
                None
            }
        };

        // ── general.html: MCU power/board + identity ──
        let esn = find_token_after(&general_text, "ESN").unwrap_or_else(|| {
            gaps.push("esn not reported by general.html".to_string());
            String::new()
        });
        validate_status_beacon_identity(&mut status_beacon, &esn, &mut gaps);
        let battery_v = status_beacon
            .as_ref()
            .and_then(|beacon| beacon.general_information.as_ref())
            .and_then(|general| general.battery_v)
            .or_else(|| find_number_after(&general_text, "Main Battery Voltage"))
            .unwrap_or_else(|| {
                gaps.push(
                    "telem.battery_v not reported by MG90 status/general.html or status broadcast"
                        .to_string(),
                );
                0.0
            });
        let internal_temp_c = status_beacon
            .as_ref()
            .and_then(|beacon| beacon.general_information.as_ref())
            .and_then(|general| general.internal_temp_c)
            .or_else(|| find_number_after(&general_text, "Internal Temperature"))
            .unwrap_or_else(|| {
                gaps.push("telem.internal_temp_c not reported by MG90 status/general.html or status broadcast".to_string());
                0.0
            });
        let mgos_version = find_token_after(&general_text, "Version").unwrap_or_else(|| {
            gaps.push("mgos_version not reported by general.html".to_string());
            String::new()
        });
        let model = find_token_after(&general_text, "Model").unwrap_or_else(|| {
            gaps.push("model not reported by general.html".to_string());
            String::new()
        });
        let ignition_on = status_beacon
            .as_ref()
            .and_then(|beacon| beacon.general_information.as_ref())
            .and_then(|general| general.ignition_on)
            .unwrap_or_else(|| parse_ignition_state(&general_text, &mut gaps));

        // ── GNSS/IMU over SSH ──
        let (mut gps, imu) = match probe.read_gps_nmea() {
            Ok(nmea) => parse_gps_imu(&nmea, &mut gaps),
            Err(e) => {
                gaps.push(format!("gps/imu unavailable (ssh): {e}"));
                (GpsFix::default(), None)
            }
        };
        if let Some(beacon_gps) = status_beacon
            .as_ref()
            .and_then(|beacon| status_beacon_gps(beacon, &mut gaps))
        {
            gps = merge_beacon_gps(gps, beacon_gps);
        }

        // ── WAN status over HTTP ──
        let wan = match probe.read_lci_wan() {
            Ok(html) => parse_wan(&html, &mut gaps),
            Err(e) => {
                gaps.push(format!("wan status unavailable (http): {e}"));
                WanStatus::default()
            }
        };

        // ── vehicle power + OBD telemetry ──
        // The authenticated LCI general page carries the MCU ignition-sense line;
        // OBD-II is a separate application plane. The optional app read below is
        // deliberately diagnostic-only: the repository documents the page paths
        // but not a stable payload schema, so no OBD field is inferred here.
        let obd_probe_status = match probe.read_obd_status() {
            Ok(Some(raw)) if raw.trim().is_empty() => {
                gaps.push(
                    "OBD application returned an empty response; typed OBD telemetry remains unavailable"
                        .to_string(),
                );
                DeviceProbeStatus::Unsupported {
                    reason: "OBD/HDOBD response schema is not verified".to_string(),
                }
            }
            Ok(Some(_)) => {
                gaps.push(
                    "OBD application HTTP response received; payload schema is not verified, so typed OBD telemetry remains unavailable"
                        .to_string(),
                );
                DeviceProbeStatus::Unsupported {
                    reason: "OBD/HDOBD response schema is not verified".to_string(),
                }
            }
            Ok(None) => {
                gaps.push(format!(
                    "OBD not wired; set {OBD_STATUS_PATH_ENV} to /obdii_status/ or /hdobd_status/ for a diagnostic read"
                ));
                DeviceProbeStatus::NotInstalled
            }
            Err(e) => {
                let reason = if e.kind() == io::ErrorKind::Unsupported {
                    "unsupported"
                } else if e.kind() == io::ErrorKind::InvalidInput {
                    "configuration error"
                } else {
                    "unavailable"
                };
                gaps.push(format!("OBD application {reason} (HTTP): {e}"));
                if e.kind() == io::ErrorKind::Unsupported {
                    DeviceProbeStatus::Unsupported {
                        reason: e.to_string(),
                    }
                } else {
                    DeviceProbeStatus::Failed {
                        reason: e.to_string(),
                    }
                }
            }
        };
        let telem = VehicleTelem {
            battery_v,
            internal_temp_c,
            ignition_on,
            moving: gps.speed_mph > 0.5,
            obd_present: obd_probe_status.is_supported(),
            obd_probe_status,
            ..Default::default()
        };

        VehicleState {
            host: host.to_string(),
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

    /// Build the additive v2 snapshot from the same probe fold as the v1
    /// compatibility mirror. The sequence is allocated only here/publish, so
    /// every worker instance has a monotonic stream without fabricating device
    /// timestamps or telemetry.
    #[must_use]
    pub fn build_state_v2(&self, probe: &dyn VehicleProbe) -> VehicleStateV2 {
        let state = self.build_state(probe);
        self.snapshot_v2(&state)
    }

    /// Build one roster snapshot from this worker's real configured probe. A
    /// source without a probe, or a poll that cannot confirm the MG90 ESN, is
    /// returned as no-source and never converted into synthetic telemetry.
    fn build_roster_snapshot(
        &self,
        source_id: &VehicleSourceId,
    ) -> Result<VehicleRosterSnapshot, VehicleNoSourceReason> {
        let Some(probe) = self.probe.clone() else {
            return Err(VehicleNoSourceReason::ProbeUnavailable);
        };
        let snapshot = self.build_state_v2(probe.as_ref());
        if snapshot.mg90.id.trim().is_empty() || snapshot.mg90.esn.trim().is_empty() {
            return Err(VehicleNoSourceReason::IdentityUnconfirmed);
        }
        VehicleRosterSnapshot::from_v2(source_id.clone(), self.host.clone(), snapshot)
            .map_err(no_source_reason_from_roster_error)
    }

    fn snapshot_v2_with_interval(
        &self,
        state: &VehicleState,
        expected_interval: Duration,
    ) -> VehicleStateV2 {
        let published_at_ms = now_ms();
        VehicleStateV2::from_v1(
            state,
            self.host.clone(),
            self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            expected_interval.as_millis().try_into().unwrap_or(u64::MAX),
            published_at_ms,
            SnapshotProvenance {
                source: SnapshotSource::DirectGateway,
                source_id: Some(self.host.clone()),
                relay: None,
            },
        )
    }

    fn snapshot_v2(&self, state: &VehicleState) -> VehicleStateV2 {
        self.snapshot_v2_with_interval(state, self.poll)
    }

    /// Publish the v1 compatibility mirror and, when the gateway ESN is
    /// confirmed, the identity-addressed v2 mirror. An unknown ESN is never
    /// replaced with a synthetic topic segment.
    fn publish_pair(&self, legacy: &VehicleState, observed: &VehicleState, interval: Duration) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(
                &mut persist,
                &vehicle_state_topic(&self.host),
                legacy,
            );
            let v2 = self.snapshot_v2_with_interval(observed, interval);
            if !v2.mg90.id.is_empty() {
                crate::bus_publish::publish_json(
                    &mut persist,
                    &vehicle_state_v2_topic(&v2.management_node_id, &v2.mg90.id),
                    &v2,
                );
            } else {
                tracing::debug!(
                    target: "mackesd::vehicle",
                    host = %self.host,
                    "v2 vehicle snapshot withheld until MG90 ESN is confirmed"
                );
            }
        }
    }

    fn publish(&self, state: &VehicleState) {
        self.publish_pair(state, state, self.poll);
    }

    /// Republish a cached observation without pretending that the gateway was
    /// polled again. The v1 compatibility mirror receives a current transport
    /// stamp, while v2 retains the original observation timestamp for freshness.
    fn publish_heartbeat(&self, observed: &VehicleState) {
        let mut legacy = observed.clone();
        legacy.published_at_ms = now_ms();
        self.publish_pair(&legacy, observed, self.heartbeat);
    }

    // ─────────────────────── Phase 4 · action/vehicle/* control drain ───────────────────────

    /// Handle one `action/vehicle/<verb>` request end to end → a typed
    /// [`VehicleReply`]. A node with no gateway attached (`probe: None`) honestly
    /// gates every verb (`no gateway on this node`) rather than faking a result; the
    /// run loop only reaches this on a gateway node (a no-gateway worker idles).
    #[must_use]
    pub fn handle(&self, verb_name: &str, body: &str) -> VehicleReply {
        let Some(verb) = VehicleVerb::from_verb(verb_name) else {
            return VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!("unknown vehicle verb `{verb_name}`")),
                ..Default::default()
            };
        };
        let Some(probe) = self.probe.clone() else {
            return VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                gated: Some("no gateway on this node".to_string()),
                ..Default::default()
            };
        };
        if verb == VehicleVerb::Reboot {
            if let Err(error) = self.authorizer.authorize(
                body,
                MutationContext {
                    verb: VEHICLE_REBOOT_AUTH_VERB,
                    node: &self.host,
                    target: VEHICLE_REBOOT_AUTH_TARGET,
                },
            ) {
                tracing::warn!(
                    target: "mackesd::action_auth",
                    host = %self.host,
                    verb = verb_name,
                    %error,
                    "refused unauthorized vehicle reboot"
                );
                return VehicleReply {
                    ok: false,
                    verb: verb_name.to_string(),
                    gated: Some(format!("privileged action refused: {error}")),
                    ..Default::default()
                };
            }
        }
        let body = VehicleActionBody::parse(body);
        match verb {
            VehicleVerb::GetConfig => self.handle_get_config(probe.as_ref(), verb_name, &body),
            VehicleVerb::Reboot => self.handle_reboot(probe.as_ref(), verb_name, &body),
        }
    }

    /// `get-config` (READ) — pull a committed oMG config file over SSH
    /// (`omgconf latest <file>`). `file` MUST be a bare `*.yaml` name (no path
    /// components / traversal), else an honest rejection.
    fn handle_get_config(
        &self,
        probe: &dyn VehicleProbe,
        verb_name: &str,
        body: &VehicleActionBody,
    ) -> VehicleReply {
        let Some(file) = body
            .file
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some("`get-config` requires a `file` field in the request body".to_string()),
                ..Default::default()
            };
        };
        if !is_safe_yaml_name(file) {
            return VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!(
                    "`file` must be a bare `*.yaml` name with no path components: `{file}`"
                )),
                ..Default::default()
            };
        }
        match probe.run_ssh(&format!("omgconf latest {file}")) {
            Ok(yaml) => VehicleReply {
                ok: true,
                verb: verb_name.to_string(),
                applied: Some(yaml),
                ..Default::default()
            },
            Err(e) => VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                gated: Some(format!("gateway ssh unavailable: {e}")),
                ..Default::default()
            },
        }
    }

    /// `reboot` (MUTATION, destructive) — typed-armed on the gateway ESN. The body's
    /// `typed_name` MUST equal the live gateway ESN BEFORE the SSH `reboot` runs;
    /// otherwise nothing is performed and the reply is honestly gated. A performed
    /// reboot is audited on the events plane (so `audited: true` is truthful),
    /// mirroring the `cloud` worker's destructive-op gate + audit.
    fn handle_reboot(
        &self,
        probe: &dyn VehicleProbe,
        verb_name: &str,
        body: &VehicleActionBody,
    ) -> VehicleReply {
        // Typed-arming: `typed_name` must equal the live gateway ESN.
        let esn = self.gateway_esn(probe);
        let typed = body.typed_name.as_deref().map(str::trim).unwrap_or("");
        let armed = !typed.is_empty() && !esn.is_empty() && typed == esn;
        if !armed {
            return VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                gated: Some(
                    "typed-arm required: `typed_name` must equal the gateway ESN".to_string(),
                ),
                ..Default::default()
            };
        }
        match probe.run_ssh("reboot") {
            Ok(_) => {
                self.audit_reboot(&esn);
                VehicleReply {
                    ok: true,
                    verb: verb_name.to_string(),
                    applied: Some("reboot issued".to_string()),
                    audited: true,
                    ..Default::default()
                }
            }
            Err(e) => VehicleReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!("reboot ssh failed: {e}")),
                ..Default::default()
            },
        }
    }

    /// The live gateway ESN (the reboot typed-arming target) — parsed from the LCI
    /// general page. Empty when the gateway is unreachable / the page omits it (so a
    /// reboot can NEVER arm without a confirmed ESN).
    fn gateway_esn(&self, probe: &dyn VehicleProbe) -> String {
        probe
            .read_lci_general()
            .ok()
            .map(|h| strip_html(&h))
            .and_then(|t| find_token_after(&t, "ESN"))
            .unwrap_or_default()
    }

    /// Write one hash-chain audit row for a performed `reboot` through the EXISTING
    /// events plane (best-effort — a store fault is logged, never fatal). Makes the
    /// reply's `audited: true` truthful. Mirrors [`CloudWorker::audit`].
    fn audit_reboot(&self, esn: &str) {
        crate::events::append_and_alert(
            &self.db_path,
            &format!("peer:{}", self.host),
            crate::events::EventKind::AdminAction,
            serde_json::json!({
                "action": "vehicle",
                "verb": "reboot",
                "host": self.host,
                "esn": esn,
            }),
        );
    }

    /// Drain every new `action/vehicle/*` request, advance the per-topic cursors, and
    /// answer each on `reply/<ulid>` with a typed [`VehicleReply`]. Returns `true`
    /// when any request was handled. A no-bus worker is a swallowed no-op.
    fn drain_actions(&self, cursors: &mut HashMap<String, String>) -> bool {
        let Some(root) = self.bus_root.clone() else {
            return false;
        };
        let Ok(persist) = Persist::open(root) else {
            return false;
        };
        let Ok(topics) = persist.list_topics() else {
            return false;
        };
        let mut acted = false;
        for topic in topics {
            let Some(verb_name) = topic.strip_prefix(VEHICLE_ACTION_PREFIX) else {
                continue;
            };
            let verb_name = verb_name.to_string();
            let cursor = cursors.get(&topic).cloned();
            let Ok(msgs) = persist.list_since(&topic, cursor.as_deref()) else {
                continue;
            };
            for msg in msgs {
                cursors.insert(topic.clone(), msg.ulid.clone());
                let body = msg.body.as_deref().unwrap_or("{}");
                let reply = self.handle(&verb_name, body);
                tracing::info!(
                    target: "mackesd::vehicle",
                    ulid = %msg.ulid, verb = %verb_name, ok = reply.ok,
                    audited = reply.audited, "vehicle action handled"
                );
                self.write_reply(&persist, &msg.ulid, &reply);
                acted = true;
            }
        }
        acted
    }

    /// Seed each existing `action/vehicle/*` topic's cursor to its newest message so
    /// a (re)start doesn't replay a backlog of verbs.
    fn prime_cursors(&self, cursors: &mut HashMap<String, String>) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let Ok(topics) = persist.list_topics() else {
            return;
        };
        for topic in topics {
            if !topic.starts_with(VEHICLE_ACTION_PREFIX) {
                continue;
            }
            if let Ok(Some(ulid)) = persist.latest_ulid(&topic) {
                cursors.insert(topic, ulid);
            }
        }
    }

    /// Write a typed reply to `reply/<request-ulid>` (best-effort).
    fn write_reply(&self, persist: &Persist, req_ulid: &str, reply: &VehicleReply) {
        let body = serde_json::to_string(reply).unwrap_or_default();
        if let Err(e) = persist.write(&reply_topic(req_ulid), Priority::Default, None, Some(&body))
        {
            tracing::warn!(target: "mackesd::vehicle", ulid = %req_ulid, error = %e, "vehicle reply write failed");
        }
    }
}

/// A drained `action/vehicle/<verb>` classified for dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VehicleVerb {
    /// `get-config` — pull a committed oMG config file over SSH (READ).
    GetConfig,
    /// `reboot` — reboot the gateway (MUTATION, destructive; typed-armed on the ESN).
    Reboot,
}

impl VehicleVerb {
    /// Classify a verb token, or `None` for an unrecognized verb (never guessed).
    fn from_verb(verb: &str) -> Option<Self> {
        Some(match verb {
            "get-config" => Self::GetConfig,
            "reboot" => Self::Reboot,
            _ => return None,
        })
    }
}

/// The parsed `action/vehicle/*` request body — the fields the verbs read off the
/// wire JSON. Every field is optional so a legacy `{}` request still parses; each
/// handler enforces what it actually requires.
#[derive(Debug, Clone, Default, Deserialize)]
struct VehicleActionBody {
    /// `get-config`'s target config file (a bare `*.yaml` name).
    #[serde(default)]
    file: Option<String>,
    /// `reboot`'s typed-arming confirmation (must equal the gateway ESN).
    #[serde(default)]
    typed_name: Option<String>,
}

impl VehicleActionBody {
    /// Parse a request body, degrading a malformed body to an all-empty request
    /// (the per-verb handlers then honestly reject what they require).
    fn parse(body: &str) -> Self {
        serde_json::from_str(body.trim()).unwrap_or_default()
    }
}

/// Whether `name` is a safe bare `*.yaml` config-file name — no path components, no
/// `..` traversal, only sane filename chars. Guards the `get-config` SSH arg.
fn is_safe_yaml_name(name: &str) -> bool {
    name.len() > ".yaml".len()
        && name.ends_with(".yaml")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
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
        // Seed the action cursors so a (re)start doesn't replay a backlog of verbs.
        let mut cursors: HashMap<String, String> = HashMap::new();
        self.prime_cursors(&mut cursors);
        // Publish an honest pending snapshot and start the heartbeat before any
        // potentially blocking gateway operation. A missing ESN withholds only
        // the v2 identity topic; the legacy current-status lane still heartbeats.
        self.drain_actions(&mut cursors);
        let mut runtime = VehicleRuntimeSnapshot::pending(&self.host);
        let mut cached = runtime.render();
        self.publish(&cached);
        let now = tokio::time::Instant::now();
        let phase = initial_phase_for(&self.host, self.poll);
        let mut current_tick = tokio::time::interval_at(now + phase, self.poll);
        let mut enrichment_tick = tokio::time::interval_at(now + ENRICHMENT_POLL, ENRICHMENT_POLL);
        let mut heartbeat_tick = tokio::time::interval_at(now + self.heartbeat, self.heartbeat);
        current_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        enrichment_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut current_task: Option<tokio::task::JoinHandle<VehicleCurrentStatusObservation>> =
            None;
        let mut current_deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
        let mut current_timed_out = false;
        let mut current_retry = self.poll;
        let mut current_not_before: Option<tokio::time::Instant> = None;
        let mut enrichment_task: Option<tokio::task::JoinHandle<VehicleEnrichmentObservation>> =
            None;
        let mut enrichment_deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
        let mut enrichment_timed_out = false;
        loop {
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                _ = current_tick.tick() => {
                    self.drain_actions(&mut cursors);
                    let retry_ready = current_not_before
                        .map_or(true, |not_before| tokio::time::Instant::now() >= not_before);
                    if current_task.is_none() && retry_ready {
                        current_not_before = None;
                        current_task = Some(self.spawn_current_status(probe.clone()));
                        current_deadline =
                            Some(Box::pin(tokio::time::sleep(self.current_timeout)));
                        current_timed_out = false;
                    }
                }
                _ = enrichment_tick.tick() => {
                    if runtime.online && enrichment_task.is_none() {
                        enrichment_task = Some(self.spawn_enrichment(probe.clone()));
                        enrichment_deadline = Some(Box::pin(tokio::time::sleep(ENRICHMENT_TIMEOUT)));
                        enrichment_timed_out = false;
                    }
                }
                _ = heartbeat_tick.tick() => {
                    self.publish_heartbeat(&cached);
                }
                result = async {
                    current_task.as_mut().expect("guarded current-status task").await
                }, if current_task.is_some() => {
                    current_task = None;
                    current_deadline = None;
                    if current_timed_out {
                        current_timed_out = false;
                    } else {
                        let healthy = result.as_ref().is_ok_and(|current| current.online);
                        let was_online = runtime.online;
                        match result {
                            Ok(current) => runtime.apply_current(current),
                            Err(error) => runtime.mark_current_unavailable(
                                &format!("task failed: {error}")
                            ),
                        }
                        let next = runtime.render();
                        let changed = !vehicle_state_content_eq(&cached, &next);
                        cached = next;
                        if changed {
                            self.publish(&cached);
                        }
                        if !was_online && runtime.online && enrichment_task.is_none() {
                            enrichment_task = Some(self.spawn_enrichment(probe.clone()));
                            enrichment_deadline =
                                Some(Box::pin(tokio::time::sleep(ENRICHMENT_TIMEOUT)));
                            enrichment_timed_out = false;
                        }
                        if healthy {
                            current_retry = self.poll;
                            current_not_before = None;
                        } else {
                            current_not_before =
                                Some(tokio::time::Instant::now() + current_retry);
                            current_retry = current_retry.saturating_mul(2).min(FAILURE_RETRY_MAX);
                        }
                    }
                }
                () = async {
                    current_deadline
                        .as_mut()
                        .expect("guarded current-status deadline")
                        .as_mut()
                        .await
                }, if current_deadline.is_some() => {
                    current_deadline = None;
                    current_timed_out = true;
                    current_not_before = Some(tokio::time::Instant::now() + current_retry);
                    current_retry = current_retry.saturating_mul(2).min(FAILURE_RETRY_MAX);
                    runtime.mark_current_unavailable("current-status timeout");
                    let next = runtime.render();
                    let changed = !vehicle_state_content_eq(&cached, &next);
                    cached = next;
                    if changed {
                        self.publish(&cached);
                    }
                }
                result = async {
                    enrichment_task.as_mut().expect("guarded enrichment task").await
                }, if enrichment_task.is_some() => {
                    enrichment_task = None;
                    enrichment_deadline = None;
                    if enrichment_timed_out {
                        enrichment_timed_out = false;
                    } else {
                        match result {
                            Ok(enrichment) => runtime.apply_enrichment(enrichment),
                            Err(error) => runtime.mark_enrichment_unavailable(
                                &format!("task failed: {error}")
                            ),
                        }
                        let next = runtime.render();
                        let changed = !vehicle_state_content_eq(&cached, &next);
                        cached = next;
                        if changed {
                            self.publish(&cached);
                        }
                    }
                }
                () = async {
                    enrichment_deadline
                        .as_mut()
                        .expect("guarded enrichment deadline")
                        .as_mut()
                        .await
                }, if enrichment_deadline.is_some() => {
                    enrichment_deadline = None;
                    enrichment_timed_out = true;
                    runtime.mark_enrichment_unavailable("enrichment timeout");
                    let next = runtime.render();
                    let changed = !vehicle_state_content_eq(&cached, &next);
                    cached = next;
                    if changed {
                        self.publish(&cached);
                    }
                }
            }
        }
    }
}

// ─────────────────────────── raw-text folds ───────────────────────────

/// The documented MG90 JSON Status Broadcast. Unknown fields are intentionally
/// ignored: the gateway emits WAN/GPIO/VPN fields that the current vehicle mirror
/// does not yet model, while these fields are the primary power/GNSS facts we can
/// fold without inventing values.
#[derive(Debug, Deserialize)]
struct Mg90StatusBeacon {
    #[serde(default, rename = "vehicleID")]
    vehicle_id: Option<String>,
    #[serde(default)]
    location: Option<Mg90BeaconLocation>,
    #[serde(default, rename = "gnssStatus")]
    gnss_status: Option<Mg90BeaconGnss>,
    #[serde(default, rename = "generalInformation")]
    general_information: Option<Mg90BeaconGeneral>,
}

#[derive(Debug, Deserialize)]
struct Mg90BeaconLocation {
    latitude: f64,
    longitude: f64,
}

#[derive(Debug, Deserialize)]
struct Mg90BeaconGnss {
    #[serde(default)]
    fix: Option<bool>,
    #[serde(default, rename = "numberSatellites")]
    number_satellites: Option<u16>,
    #[serde(default, rename = "antennaConnected")]
    antenna_connected: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct Mg90BeaconGeneral {
    #[serde(default, rename = "ignitionOn")]
    ignition_on: Option<bool>,
    #[serde(default, rename = "mainBatteryVoltage")]
    battery_v: Option<f32>,
    #[serde(default, rename = "internalTemperature")]
    internal_temp_c: Option<f32>,
}

/// Decode and lightly validate one MG90 Status Broadcast datagram. A valid JSON
/// object with only some documented fields is useful; absent fields fall back to
/// the LCI/NMEA planes. Invalid scalar values are dropped individually so one bad
/// field cannot turn a reachable gateway into an offline mirror.
fn parse_status_beacon(raw: &str, gaps: &mut Vec<String>) -> Option<Mg90StatusBeacon> {
    let mut beacon = match serde_json::from_str::<Mg90StatusBeacon>(raw.trim()) {
        Ok(beacon) => beacon,
        Err(error) => {
            gaps.push(format!("status broadcast invalid JSON: {error}"));
            return None;
        }
    };

    if beacon.location.is_none()
        && beacon.gnss_status.is_none()
        && beacon.general_information.is_none()
    {
        gaps.push("status broadcast has no documented telemetry fields".to_string());
        return None;
    }

    if let Some(location) = beacon.location.as_ref() {
        if !location.latitude.is_finite()
            || !location.longitude.is_finite()
            || !(-90.0..=90.0).contains(&location.latitude)
            || !(-180.0..=180.0).contains(&location.longitude)
        {
            gaps.push("status broadcast location invalid".to_string());
            beacon.location = None;
        }
    }
    if let Some(general) = beacon.general_information.as_mut() {
        if let Some(value) = general.battery_v {
            if !value.is_finite()
                || !(STATUS_BEACON_MIN_BATTERY_V..=STATUS_BEACON_MAX_BATTERY_V).contains(&value)
            {
                gaps.push("status broadcast battery voltage out of range".to_string());
                general.battery_v = None;
            }
        }
        if let Some(value) = general.internal_temp_c {
            if !value.is_finite()
                || !(STATUS_BEACON_MIN_TEMPERATURE_C..=STATUS_BEACON_MAX_TEMPERATURE_C)
                    .contains(&value)
            {
                gaps.push("status broadcast internal temperature out of range".to_string());
                general.internal_temp_c = None;
            }
        }
    }
    if let Some(gnss) = beacon.gnss_status.as_mut() {
        if gnss
            .number_satellites
            .is_some_and(|value| value > STATUS_BEACON_MAX_SATELLITES)
        {
            gaps.push("status broadcast satellite count out of range".to_string());
            gnss.number_satellites = None;
        }
    }
    if let Some(gnss) = beacon.gnss_status.as_ref() {
        if gnss.fix.is_none() {
            gaps.push("status broadcast GNSS fix flag missing".to_string());
        }
    }
    if let Some(gnss) = beacon.gnss_status.as_ref() {
        if gnss.antenna_connected.is_none() {
            gaps.push("status broadcast GNSS antenna state missing".to_string());
        }
    }
    Some(beacon)
}

/// Bind the unauthenticated UDP beacon to the gateway identity authenticated by
/// the LCI page. A beacon without the documented `vehicleID`, or one naming a
/// different vehicle, must not override the trusted LCI/NMEA planes.
fn validate_status_beacon_identity(
    beacon: &mut Option<Mg90StatusBeacon>,
    expected_esn: &str,
    gaps: &mut Vec<String>,
) {
    let Some(candidate) = beacon
        .as_ref()
        .and_then(|beacon| beacon.vehicle_id.as_deref())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
    else {
        if beacon.is_some() {
            gaps.push("status broadcast vehicleID missing; ignored".to_string());
            *beacon = None;
        }
        return;
    };
    if expected_esn.is_empty() {
        gaps.push("status broadcast identity cannot be verified; ignored".to_string());
        *beacon = None;
    } else if candidate != expected_esn {
        gaps.push(format!(
            "status broadcast vehicleID does not match gateway ESN; ignored: {candidate}"
        ));
        *beacon = None;
    }
}

/// Fold the beacon's GNSS fields into the neutral GPS shape. The antenna state is
/// retained only as an evidence check: a packet may report a fix even when the
/// antenna flag is absent, but a disconnected antenna is surfaced as a gap.
fn status_beacon_gps(beacon: &Mg90StatusBeacon, gaps: &mut Vec<String>) -> Option<GpsFix> {
    let gnss = beacon.gnss_status.as_ref()?;
    let fix = gnss.fix?;
    if gnss.antenna_connected == Some(false) {
        gaps.push("status broadcast reports GNSS antenna disconnected".to_string());
    }
    let satellites = match gnss.number_satellites {
        Some(value) if value <= STATUS_BEACON_MAX_SATELLITES => value as u8,
        Some(_) => {
            // Defensive duplicate of parse_status_beacon's validation: this
            // keeps this fold honest if its caller ever changes.
            gaps.push("status broadcast satellite count out of range".to_string());
            return None;
        }
        None => {
            gaps.push("status broadcast GNSS satellite count unavailable".to_string());
            return None;
        }
    };
    let has_position = fix && beacon.location.is_some();
    if fix && !has_position {
        gaps.push("status broadcast GNSS fix has no valid location".to_string());
        return None;
    }
    let (latitude, longitude) = beacon
        .location
        .as_ref()
        .filter(|_| has_position)
        .map(|location| (location.latitude, location.longitude))
        .unwrap_or((0.0, 0.0));
    Some(GpsFix {
        fix_type: if has_position { "gps" } else { "no-fix" }.to_string(),
        latitude,
        longitude,
        satellites,
        ..Default::default()
    })
}

/// Preserve richer NMEA fields (altitude, speed, heading, dilution, age, update
/// rate) while letting the documented beacon own the current fix/coordinates/sats.
fn merge_beacon_gps(mut nmea: GpsFix, beacon: GpsFix) -> GpsFix {
    nmea.fix_type = beacon.fix_type;
    nmea.satellites = beacon.satellites;
    nmea.latitude = beacon.latitude;
    nmea.longitude = beacon.longitude;
    nmea
}

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

/// TOLERANT WAN-status fold. Strips the HTML, then parses the extended status table's
/// per-interface rows: each cellular A/B section yields a full [`CellLink`]
/// (signal/technology/SIM/carrier/WAN-IP), and the section carrying an `IP Address` is
/// the active uplink. Degrades to a `gaps` note for anything genuinely absent (never a
/// fabricated value, §7). The simplified/general format (an explicit `Active WAN` label
/// + a single `dBm` reading, no per-modem rows) still folds through the fallbacks.
fn parse_wan(html: &str, gaps: &mut Vec<String>) -> WanStatus {
    let text = strip_html(html);
    let mut wan = WanStatus::default();

    // The extended table's per-interface sections (empty on the simplified format).
    let sections = wan_sections(&text);
    let section = |label: &str| sections.iter().find(|(l, _)| *l == label).map(|(_, s)| *s);

    // ── Cellular A / B — a full per-modem link when the extended rows are present ──
    if let Some(s) = section("Cellular A") {
        wan.cellular_a = parse_cell_link(s);
    } else {
        // Simplified format: fold the single dBm reading into cellular A best-effort.
        match find_signal_dbm(&text) {
            Some(dbm) => {
                wan.cellular_a.signal_dbm = dbm;
                wan.cellular_a.healthy = dbm > -110;
            }
            None => gaps.push("wan.cellular_a signal_dbm not reported".to_string()),
        }
    }
    if let Some(s) = section("Cellular B") {
        wan.cellular_b = parse_cell_link(s);
    }

    // ── active WAN — the explicit label (simplified) or the IP-bearing section ──
    if let Some(v) =
        find_token_after(&text, "Active WAN").or_else(|| find_token_after(&text, "Active Link"))
    {
        wan.active_wan = v;
    } else if let Some((label, _)) = sections.iter().find(|(_, s)| s.contains("IP Address")) {
        wan.active_wan = (*label).to_string();
    } else {
        gaps.push("wan.active_wan not reported".to_string());
    }

    // ── Ethernet / Wi-Fi state — derived from their extended section (active vs a
    // present-but-backup standby), else the simplified label, else a gap ──
    match section("Ethernet") {
        Some(_) => {
            wan.ethernet_state = if wan.active_wan == "Ethernet" {
                "active".to_string()
            } else {
                "standby".to_string()
            };
        }
        None => match find_token_after(&text, "Ethernet") {
            Some(v) => wan.ethernet_state = v,
            None => gaps.push("wan.ethernet_state not reported".to_string()),
        },
    }
    match section("WiFi") {
        Some(_) => {
            wan.wifi_state = if wan.active_wan == "WiFi" {
                "active".to_string()
            } else {
                "standby".to_string()
            };
        }
        None => {
            match find_token_after(&text, "Wi-Fi").or_else(|| find_token_after(&text, "Wifi")) {
                Some(v) => wan.wifi_state = v,
                None => gaps.push("wan.wifi_state not reported".to_string()),
            }
        }
    }
    match find_token_after(&text, "VPN") {
        Some(v) => wan.vpn_state = v,
        None => gaps.push("wan.vpn_state not reported".to_string()),
    }
    wan
}

/// The extended WAN table's per-interface section markers: `(section-label, needle)`.
/// Each WAN row starts with a device descriptor carrying one of these needles (e.g.
/// `... (Cellular A)`, `Panel Ethernet 5`, `... PCIe WiFi A`).
const WAN_SECTION_MARKERS: &[(&str, &str)] = &[
    ("Cellular A", "Cellular A"),
    ("Cellular B", "Cellular B"),
    ("Ethernet", "Panel Ethernet"),
    ("WiFi", "WiFi A"),
];

/// Slice the stripped WAN text into per-interface sections (document order). Each
/// section runs from its marker to the start of the next present marker, so a label
/// scan within a section stays scoped to that one interface's row.
fn wan_sections(text: &str) -> Vec<(&'static str, &str)> {
    let mut found: Vec<(&'static str, usize)> = WAN_SECTION_MARKERS
        .iter()
        .filter_map(|(label, needle)| text.find(needle).map(|i| (*label, i)))
        .collect();
    found.sort_by_key(|&(_, i)| i);
    let mut out = Vec::with_capacity(found.len());
    for k in 0..found.len() {
        let (label, start) = found[k];
        let end = found.get(k + 1).map_or(text.len(), |&(_, i)| i);
        out.push((label, &text[start..end]));
    }
    out
}

/// Fold one cellular section (scoped to a single modem's extended row) into a
/// [`CellLink`]: the primary RSSI dBm, the RAT, the SIM presence, the carrier, and
/// the WAN IP (present ⇒ this modem is the active uplink). Honest defaults for
/// anything the section omits.
fn parse_cell_link(section: &str) -> CellLink {
    let signal_dbm = rssi_dbm_in(section).unwrap_or(0);
    let sim_present = find_token_after(section, "SIM ID").is_some();
    let sim_state = if sim_present { "ready" } else { "absent" }.to_string();
    let carrier = find_token_after(section, "Carrier PRI ID").unwrap_or_default();
    let technology = if section.contains("5G") {
        "5G"
    } else if section.contains("LTE") {
        "LTE"
    } else {
        ""
    }
    .to_string();
    let wan_ip =
        find_token_after(section, "IP Address").unwrap_or_else(|| "not active".to_string());
    let healthy = signal_dbm > -110 && sim_present;
    CellLink {
        sim_state,
        carrier,
        signal_dbm,
        technology,
        wan_ip,
        healthy,
    }
}

/// The primary RSSI reading (dBm) in a cellular section: the FIRST `dBm` value after
/// the `RSSI` label, e.g. `RSSI  -98.0dBm / -102.0dBm` ⇒ `-98`. Parses the leading
/// signed (possibly-decimal) number before `dBm`, truncated to a whole dBm. `None`
/// when the section has no `RSSI` reading.
fn rssi_dbm_in(section: &str) -> Option<i32> {
    let rssi = section.find("RSSI")?;
    let after = &section[rssi..];
    let dbm = after.find("dBm")?;
    let prefix = &after[..dbm];
    // Walk back over the trailing float run (digits, one '.', a leading '-').
    let tail: String = prefix
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    let num: String = tail.chars().rev().collect();
    num.trim().parse::<f32>().ok().map(|f| f as i32)
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

/// Fold the authenticated LCI MCU ignition-sense row. Unknown or missing values
/// remain off and become an explicit gap; the worker never treats reachability or
/// battery voltage as an ignition signal.
fn parse_ignition_observation(text: &str, gaps: &mut Vec<String>) -> Option<bool> {
    let Some(value) = find_token_after(text, "Ignition State") else {
        gaps.push("telem.ignition_on not reported by general.html".to_string());
        return None;
    };
    match value.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "active" => Some(true),
        "off" | "false" | "no" | "inactive" => Some(false),
        other => {
            gaps.push(format!(
                "unrecognized ignition state in general.html: {other}"
            ));
            None
        }
    }
}

fn parse_ignition_state(text: &str, gaps: &mut Vec<String>) -> bool {
    parse_ignition_observation(text, gaps).unwrap_or_default()
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

    /// The canned `omgconf latest <file>` YAML the fake SSH seam returns for
    /// `get-config`.
    const FAKE_YAML: &str = "gateway:\n  mode: failover\nwan:\n  primary: cellular-a\n";

    /// The extended MG-LCI `wan/status` structure (tags already stripped, per the
    /// live layout): a per-modem A/B cellular table + a panel-ethernet + a Wi-Fi row.
    /// Cellular A carries the `IP Address` (the active uplink); B is SIM-ready but
    /// idle. Fed straight through `strip_html` (a no-op on tag-free text).
    const WAN_EXTENDED: &str = "\
Sierra Wireless EM75XX @ MiniCard USB3 CA (Cellular A)   Cellular   IP Address 100.65.12.34   \
Cellular Info   SIM ID 8901410123456789012   LTE   Band Number 4   Bandwidth 20MHz   \
RSSI  -98.0dBm / -102.0dBm   RSRP  -123.0dBm / -131.0dBm   Carrier PRI ID 9990198   LTE   \
Panel Ethernet 5   Ethernet   Standby   \
Sierra Wireless EM75XX @ MiniCard USB3 CB (Cellular B)   Cellular   Cellular Info   \
SIM ID 8901410987654321098   RSSI  -105.0dBm / -110.0dBm   Carrier PRI ID 9990199   LTE   \
WLE900VX 802.11AC @ MiniCard PCIe WiFi A   WiFi   Disabled";

    /// A scripted fake probe: each read yields a canned `Ok(text)` or `Err(msg)`, and
    /// `run_ssh` returns [`Self::ssh_out`] while recording the command in
    /// [`Self::ssh_calls`] (shared through the `Arc` across clones, so a test asserts
    /// what ran). (`Result<String, String>` is `Clone`, unlike `io::Result`, so the
    /// fixtures are reusable across the per-read calls `build_state` makes.)
    #[derive(Clone)]
    struct FakeProbe {
        nmea: Result<String, String>,
        general: Result<String, String>,
        wan: Result<String, String>,
        status: Option<String>,
        status_error: Option<String>,
        obd_status: Result<Option<String>, String>,
        ssh_out: Result<String, String>,
        ssh_calls: Arc<std::sync::Mutex<Vec<String>>>,
        general_calls: Arc<std::sync::Mutex<u32>>,
        nmea_calls: Arc<std::sync::Mutex<u32>>,
        wan_calls: Arc<std::sync::Mutex<u32>>,
        obd_calls: Arc<std::sync::Mutex<u32>>,
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
                <tr><td>Ignition State </td><td> on</td></tr>\
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
                status: None,
                status_error: None,
                obd_status: Ok(None),
                ssh_out: Ok(FAKE_YAML.to_string()),
                ssh_calls: Arc::new(std::sync::Mutex::new(Vec::new())),
                general_calls: Arc::new(std::sync::Mutex::new(0)),
                nmea_calls: Arc::new(std::sync::Mutex::new(0)),
                wan_calls: Arc::new(std::sync::Mutex::new(0)),
                obd_calls: Arc::new(std::sync::Mutex::new(0)),
            }
        }

        /// The commands `run_ssh` has been asked to run (a shared, clone-stable log).
        fn ssh_calls(&self) -> Vec<String> {
            self.ssh_calls.lock().unwrap().clone()
        }

        fn general_calls(&self) -> u32 {
            *self.general_calls.lock().unwrap()
        }

        fn enrichment_calls(&self) -> (u32, u32, u32) {
            (
                *self.nmea_calls.lock().unwrap(),
                *self.wan_calls.lock().unwrap(),
                *self.obd_calls.lock().unwrap(),
            )
        }
    }

    fn to_io(r: &Result<String, String>) -> io::Result<String> {
        r.clone()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    impl VehicleProbe for FakeProbe {
        fn read_gps_nmea(&self) -> io::Result<String> {
            *self.nmea_calls.lock().unwrap() += 1;
            to_io(&self.nmea)
        }
        fn read_lci_general(&self) -> io::Result<String> {
            *self.general_calls.lock().unwrap() += 1;
            to_io(&self.general)
        }
        fn read_lci_wan(&self) -> io::Result<String> {
            *self.wan_calls.lock().unwrap() += 1;
            to_io(&self.wan)
        }
        fn read_status_beacon(&self) -> io::Result<Option<String>> {
            if let Some(error) = &self.status_error {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, error.clone()));
            }
            Ok(self.status.clone())
        }
        fn read_obd_status(&self) -> io::Result<Option<String>> {
            *self.obd_calls.lock().unwrap() += 1;
            self.obd_status
                .clone()
                .map_err(|error| io::Error::new(io::ErrorKind::Other, error))
        }
        fn run_ssh(&self, cmd: &str) -> io::Result<String> {
            self.ssh_calls.lock().unwrap().push(cmd.to_string());
            to_io(&self.ssh_out)
        }
    }

    #[derive(Clone)]
    struct BlockingCurrentProbe {
        inner: FakeProbe,
        gate: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    }

    impl BlockingCurrentProbe {
        fn new() -> Self {
            Self {
                inner: FakeProbe::real(),
                gate: Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())),
            }
        }

        fn release(&self) {
            let (lock, wake) = &*self.gate;
            *lock.lock().unwrap() = true;
            wake.notify_all();
        }
    }

    impl VehicleProbe for BlockingCurrentProbe {
        fn read_gps_nmea(&self) -> io::Result<String> {
            self.inner.read_gps_nmea()
        }

        fn read_lci_general(&self) -> io::Result<String> {
            let (lock, wake) = &*self.gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
            self.inner.read_lci_general()
        }

        fn read_lci_wan(&self) -> io::Result<String> {
            self.inner.read_lci_wan()
        }

        fn read_status_beacon(&self) -> io::Result<Option<String>> {
            self.inner.read_status_beacon()
        }

        fn read_obd_status(&self) -> io::Result<Option<String>> {
            self.inner.read_obd_status()
        }

        fn run_ssh(&self, cmd: &str) -> io::Result<String> {
            self.inner.run_ssh(cmd)
        }
    }

    fn worker() -> VehicleWorker {
        VehicleWorker::new("rig-1".to_string()).with_bus_root(None)
    }

    const ACTION_KEY: &[u8] = b"vehicle-action-auth-test-key";
    const ACTION_NOW: i64 = 1_700_000_000_000;

    const STATUS_BEACON: &str = r#"{
        "vehicleID": "ND84720078011035",
        "location": {"latitude": 35.1234, "longitude": -78.4567},
        "gnssStatus": {"fix": true, "numberSatellites": 7, "antennaConnected": true},
        "generalInformation": {
            "ignitionOn": true,
            "mainBatteryVoltage": 13.6,
            "internalTemperature": 35.5
        }
    }"#;

    fn reboot_context() -> MutationContext<'static> {
        MutationContext {
            verb: VEHICLE_REBOOT_AUTH_VERB,
            node: "rig-1",
            target: VEHICLE_REBOOT_AUTH_TARGET,
        }
    }

    fn authorized_reboot_body(nonce: &str, typed_name: &str) -> String {
        crate::ipc::action_auth::authorize_test_body(
            ACTION_KEY,
            &format!(r#"{{"schema_version":1,"typed_name":"{typed_name}"}}"#),
            reboot_context(),
            nonce,
            ACTION_NOW + 30_000,
        )
    }

    fn test_authorizer(root: &std::path::Path) -> Arc<ActionAuthorizer> {
        Arc::new(ActionAuthorizer::for_test(
            ACTION_KEY,
            root.to_path_buf(),
            ACTION_NOW,
        ))
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

        // The LCI reports ignition directly; OBD remains honestly gapped.
        assert!(
            state.gaps.iter().any(|g| g.contains("OBD not wired")),
            "OBD is honestly gapped: {:?}",
            state.gaps
        );
        assert!(!state.telem.obd_present);
        assert_eq!(
            state.telem.obd_probe_status,
            DeviceProbeStatus::NotInstalled,
            "no configured probe is not a fabricated OBD reading"
        );
        assert!(state.telem.ignition_on);
    }

    #[test]
    fn builds_versioned_v2_snapshot_with_radio_health_and_monotonic_sequence() {
        let w = worker();
        let first = w.build_state_v2(&FakeProbe::real());
        let second = w.build_state_v2(&FakeProbe::real());

        assert_eq!(first.schema_version, 2);
        assert_eq!(first.sequence, 1);
        assert_eq!(second.sequence, 2);
        assert_eq!(first.management_node_id, "rig-1");
        assert_eq!(first.mg90.id, "ND84720078011035");
        assert_eq!(first.expected_interval_ms, POLL.as_millis() as u64);
        assert_eq!(first.radios.len(), 6);

        let cellular_a = &first.radios.as_slice()[0];
        assert_eq!(cellular_a.id.as_str(), "cellular-a");
        assert_eq!(
            cellular_a.operation,
            mackes_mesh_types::vehicle::RadioOperation::Active
        );
        assert_eq!(
            cellular_a.presence,
            mackes_mesh_types::vehicle::RadioPresence::Installed
        );
        match &cellular_a.metrics {
            mackes_mesh_types::vehicle::RadioMetrics::Cellular(metrics) => {
                assert_eq!(metrics.rssi_dbm, Some(-72));
                assert_eq!(metrics.rsrp_dbm, None, "unreported RSRP stays absent");
            }
            other => panic!("unexpected cellular metrics: {other:?}"),
        }
        assert_eq!(
            first.radios.as_slice()[4].operation,
            mackes_mesh_types::vehicle::RadioOperation::Unknown
        );
        assert_eq!(
            first.radios.as_slice()[5].operation,
            mackes_mesh_types::vehicle::RadioOperation::Acquiring
        );
        assert_eq!(
            first.freshness.radios.state,
            mackes_mesh_types::vehicle::FreshnessState::Fresh
        );
        assert_eq!(
            first.freshness.vehicle.state,
            mackes_mesh_types::vehicle::FreshnessState::Unknown,
            "OBD remains an explicit unavailable domain"
        );
    }

    #[test]
    fn publishes_legacy_and_identity_addressed_v2_topics() {
        let tmp = tempfile::tempdir().unwrap();
        let w = worker().with_bus_root(Some(tmp.path().to_path_buf()));
        let state = w.build_state(&FakeProbe::real());
        w.publish(&state);

        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        let legacy = persist
            .list_since(&vehicle_state_topic("rig-1"), None)
            .unwrap();
        assert_eq!(legacy.len(), 1);
        let v2_topic =
            mackes_mesh_types::vehicle::vehicle_state_v2_topic("rig-1", "ND84720078011035");
        let v2 = persist.list_since(&v2_topic, None).unwrap();
        assert_eq!(v2.len(), 1);
        let snapshot: mackes_mesh_types::vehicle::VehicleStateV2 =
            serde_json::from_str(v2[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(snapshot.schema_version, 2);
        assert_eq!(snapshot.management_node_id, "rig-1");
        assert_eq!(snapshot.mg90.esn, "ND84720078011035");
    }

    #[tokio::test]
    async fn completed_probe_tasks_use_the_parent_publication_sequence() {
        let tmp = tempfile::tempdir().unwrap();
        let probe: Arc<dyn VehicleProbe> = Arc::new(FakeProbe::real());
        let worker = worker().with_bus_root(Some(tmp.path().to_path_buf()));

        let current = worker.spawn_current_status(probe.clone()).await.unwrap();
        let mut runtime = VehicleRuntimeSnapshot::from_current("rig-1", current);
        let first = runtime.render();
        worker.publish(&first);
        let enrichment = worker.spawn_enrichment(probe).await.unwrap();
        runtime.apply_enrichment(enrichment);
        let second = runtime.render();
        worker.publish(&second);

        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        let topic = vehicle_state_v2_topic("rig-1", "ND84720078011035");
        let snapshots = persist
            .list_since(&topic, None)
            .unwrap()
            .into_iter()
            .map(|message| {
                serde_json::from_str::<VehicleStateV2>(message.body.as_deref().unwrap()).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            snapshots
                .iter()
                .map(|snapshot| snapshot.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn delayed_enrichment_cannot_block_or_erase_current_status() {
        let probe = FakeProbe::real();
        let current = VehicleWorker::probe_current_status("rig-1", &probe);
        assert_eq!(
            probe.enrichment_calls(),
            (0, 0, 0),
            "the current-status batch must not enter a slow enrichment method"
        );
        let mut runtime = VehicleRuntimeSnapshot::from_current("rig-1", current);
        let before = runtime.render();
        assert_eq!(before.esn, "ND84720078011035");
        assert!((before.telem.battery_v - 12.60).abs() < 0.01);

        // Advancing current status while enrichment remains in flight updates
        // only sourced current fields.
        let mut newer_probe = FakeProbe::real();
        newer_probe.general = newer_probe
            .general
            .map(|html| html.replace("12.60v", "13.25v"));
        runtime.apply_current(VehicleWorker::probe_current_status("rig-1", &newer_probe));
        runtime.mark_enrichment_unavailable("enrichment timeout");
        let after = runtime.render();

        assert_eq!(after.esn, before.esn);
        assert_eq!(after.model, before.model);
        assert!((after.telem.battery_v - 13.25).abs() < 0.01);
        assert!(after
            .gaps
            .iter()
            .any(|gap| gap == "gps/imu unavailable (enrichment timeout)"));
        assert!(after
            .gaps
            .iter()
            .any(|gap| gap == "wan status unavailable (enrichment timeout)"));
        assert_eq!(newer_probe.enrichment_calls(), (0, 0, 0));
    }

    #[test]
    fn failed_enrichment_retains_last_sourced_domains_with_explicit_gaps() {
        let current_probe = FakeProbe::real();
        let current = VehicleWorker::probe_current_status("rig-1", &current_probe);
        let mut runtime = VehicleRuntimeSnapshot::from_current("rig-1", current);
        runtime.apply_enrichment(VehicleWorker::probe_enrichment(&FakeProbe::real()));
        let sourced = runtime.render();

        let failed = FakeProbe {
            nmea: Err("ssh timeout".to_string()),
            wan: Err("http timeout".to_string()),
            obd_status: Err("application timeout".to_string()),
            ..FakeProbe::real()
        };
        runtime.apply_enrichment(VehicleWorker::probe_enrichment(&failed));
        let retained = runtime.render();

        assert_eq!(failed.enrichment_calls(), (1, 1, 1));
        assert_eq!(retained.gps, sourced.gps);
        assert_eq!(retained.imu, sourced.imu);
        assert_eq!(retained.wan, sourced.wan);
        assert_eq!(retained.esn, sourced.esn);
        assert_eq!(retained.telem.battery_v, sourced.telem.battery_v);
        assert!(matches!(
            retained.telem.obd_probe_status,
            DeviceProbeStatus::Failed { ref reason } if reason == "application timeout"
        ));
        assert!(retained
            .gaps
            .iter()
            .any(|gap| gap.contains("gps/imu unavailable") && gap.contains("ssh timeout")));
        assert!(retained
            .gaps
            .iter()
            .any(|gap| gap.contains("wan status unavailable") && gap.contains("http timeout")));
        assert!(retained
            .gaps
            .iter()
            .any(|gap| gap.contains("OBD application unavailable")
                && gap.contains("application timeout")));
    }

    #[test]
    fn configured_obd_app_response_is_diagnostic_only_until_schema_is_verified() {
        let probe = FakeProbe {
            // `currentStatus` is one of the app-side calls named by the MG90
            // access contract; the payload is intentionally not interpreted.
            obd_status: Ok(Some(r#"{"currentStatus":{"rpm":1800}}"#.to_string())),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);

        assert!(
            !state.telem.obd_present,
            "unknown app fields must not become telemetry"
        );
        assert!(matches!(
            state.telem.obd_probe_status,
            DeviceProbeStatus::Unsupported { ref reason }
                if reason.contains("schema is not verified")
        ));
        assert!(state.gaps.iter().any(|gap| {
            gap.contains("OBD application HTTP response received")
                && gap.contains("payload schema is not verified")
        }));
    }

    #[test]
    fn obd_app_failure_is_reported_without_collapsing_a_reachable_gateway() {
        let probe = FakeProbe {
            obd_status: Err("connection refused".to_string()),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);

        assert!(
            state.online,
            "the LCI anchor still makes the gateway reachable"
        );
        assert!(!state.telem.obd_present);
        assert!(matches!(
            state.telem.obd_probe_status,
            DeviceProbeStatus::Failed { ref reason }
                if reason == "connection refused"
        ));
        assert!(state
            .gaps
            .iter()
            .any(|gap| gap.contains("OBD application unavailable")
                && gap.contains("connection refused")));
    }

    #[test]
    fn obd_probe_verdicts_survive_v2_wire_without_fabricating_telemetry() {
        let cases = vec![
            (FakeProbe::real(), DeviceProbeStatus::NotInstalled),
            (
                FakeProbe {
                    obd_status: Ok(Some(r#"{"currentStatus":{"rpm":1800}}"#.to_string())),
                    ..FakeProbe::real()
                },
                DeviceProbeStatus::Unsupported {
                    reason: "OBD/HDOBD response schema is not verified".to_string(),
                },
            ),
            (
                FakeProbe {
                    obd_status: Err("connection refused".to_string()),
                    ..FakeProbe::real()
                },
                DeviceProbeStatus::Failed {
                    reason: "connection refused".to_string(),
                },
            ),
        ];

        for (probe, expected) in cases {
            let snapshot = worker().build_state_v2(&probe);
            assert_eq!(snapshot.telem.obd_probe_status, expected);
            assert!(!snapshot.telem.obd_present);
            assert_eq!(snapshot.telem.rpm, 0);
            assert_eq!(snapshot.telem.speed_mph, 0.0);

            let json = serde_json::to_string(&snapshot).expect("v2 snapshot serializes");
            let round_trip: VehicleStateV2 =
                serde_json::from_str(&json).expect("v2 snapshot deserializes");
            assert_eq!(round_trip.telem.obd_probe_status, expected);
            assert!(!round_trip.telem.obd_present);
        }
    }

    #[test]
    fn ignition_parser_rejects_unknown_values_without_fabricating_on() {
        let mut gaps = Vec::new();
        assert!(!parse_ignition_state(
            "Ignition State definitely-maybe",
            &mut gaps
        ));
        assert!(gaps.iter().any(|gap| gap.contains("unrecognized ignition")));
    }

    #[test]
    fn status_beacon_overrides_lci_and_nmea_for_primary_telemetry() {
        let probe = FakeProbe {
            status: Some(STATUS_BEACON.to_string()),
            nmea: Err("ssh unavailable".to_string()),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);

        assert!(state.online);
        assert!((state.telem.battery_v - 13.6).abs() < 0.01);
        assert!((state.telem.internal_temp_c - 35.5).abs() < 0.01);
        assert!(state.telem.ignition_on);
        assert!(state.gps.has_fix());
        assert_eq!(state.gps.satellites, 7);
        assert!((state.gps.latitude - 35.1234).abs() < 0.0001);
        assert!((state.gps.longitude + 78.4567).abs() < 0.0001);
        assert!(state
            .gaps
            .iter()
            .any(|gap| gap.contains("gps/imu unavailable")));
    }

    #[test]
    fn status_beacon_from_different_vehicle_cannot_override_trusted_telemetry() {
        let baseline = worker().build_state(&FakeProbe::real());
        let probe = FakeProbe {
            status: Some(STATUS_BEACON.replace("ND84720078011035", "different-mg90")),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);

        assert_eq!(state.gps, baseline.gps);
        assert_eq!(state.telem.battery_v, baseline.telem.battery_v);
        assert_eq!(state.telem.internal_temp_c, baseline.telem.internal_temp_c);
        assert!(state
            .gaps
            .iter()
            .any(|gap| gap.contains("vehicleID does not match gateway ESN")));
    }

    #[test]
    fn status_beacon_reader_rejects_oversized_and_non_utf8_datagrams() {
        let cases = [
            (vec![b'x'; STATUS_BEACON_MAX_DATAGRAM_BYTES + 1], "exceeds"),
            (vec![b'{', 0xff], "not UTF-8"),
        ];

        for (payload, expected_error) in cases {
            let receiver = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
            let sender = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
            let port = receiver.local_addr().unwrap().port();
            sender
                .send_to(&payload, receiver.local_addr().unwrap())
                .unwrap();
            let probe = SshHttpProbe {
                ip: "127.0.0.1".to_string(),
                ssh_port: 2222,
                ssh_pw: String::new(),
                known_hosts_file: PathBuf::new(),
                status_socket: Some(receiver),
                status_broadcast: StatusBroadcastReadiness::Listening { port },
            };

            let error = probe.read_status_beacon().unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(
                error.to_string().contains(expected_error),
                "unexpected status datagram error: {error}"
            );
        }
    }

    #[test]
    fn status_beacon_reader_rejects_packets_from_unconfigured_peer() {
        let receiver = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        receiver.set_nonblocking(true).unwrap();
        let sender = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let port = receiver.local_addr().unwrap().port();
        sender
            .send_to(STATUS_BEACON.as_bytes(), receiver.local_addr().unwrap())
            .unwrap();
        let probe = SshHttpProbe {
            ip: "127.0.0.2".to_string(),
            ssh_port: 2222,
            ssh_pw: String::new(),
            known_hosts_file: PathBuf::new(),
            status_socket: Some(receiver),
            status_broadcast: StatusBroadcastReadiness::Listening { port },
        };

        let error = probe.read_status_beacon().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("unexpected peer")
                && error.to_string().contains("127.0.0.2"),
            "unexpected status peer error: {error}"
        );
    }

    #[test]
    fn status_broadcast_port_parser_rejects_invalid_values_without_defaulting() {
        assert_eq!(parse_status_port("5067"), Ok(5067));
        assert_eq!(parse_status_port("65535"), Ok(65535));
        for raw in ["", "0", "65536", "udp", " 5067"] {
            assert!(
                parse_status_port(raw).is_err(),
                "invalid status port was accepted: {raw:?}"
            );
        }
    }

    #[test]
    fn obd_status_path_parser_accepts_only_documented_app_surfaces() {
        assert_eq!(
            parse_obd_status_path("/obdii_status/"),
            Ok("/obdii_status/")
        );
        assert_eq!(
            parse_obd_status_path("/hdobd_status/"),
            Ok("/hdobd_status/")
        );
        for raw in ["/", "/obdii_status", "/hdobd_status/extra", "https://host/"] {
            assert!(
                parse_obd_status_path(raw).is_err(),
                "arbitrary app path was accepted: {raw:?}"
            );
        }
    }

    #[test]
    fn invalid_status_broadcast_configuration_reaches_the_typed_reader_error() {
        let (socket, readiness) = configure_status_broadcast(Some("not-a-port"));
        assert!(socket.is_none());
        assert!(matches!(
            &readiness,
            StatusBroadcastReadiness::ConfigurationError { detail }
                if detail.contains(STATUS_PORT_ENV)
        ));
        let probe = SshHttpProbe {
            ip: "127.0.0.1".to_string(),
            ssh_port: 2222,
            ssh_pw: String::new(),
            known_hosts_file: PathBuf::new(),
            status_socket: None,
            status_broadcast: readiness,
        };
        let error = probe
            .read_status_beacon()
            .expect_err("invalid config must be visible");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains(STATUS_PORT_ENV));
    }

    #[test]
    fn occupied_status_broadcast_port_is_not_silently_disabled() {
        let receiver = UdpSocket::bind(("0.0.0.0", 0)).expect("reserve UDP port");
        let port = receiver.local_addr().expect("port").port().to_string();
        let (socket, readiness) = configure_status_broadcast(Some(&port));
        assert!(socket.is_none());
        assert!(matches!(
            readiness,
            StatusBroadcastReadiness::ConfigurationError { detail }
                if detail.contains("could not bind local UDP receiver")
        ));
    }

    #[test]
    fn out_of_range_status_values_fall_back_without_silent_clamping() {
        let baseline = worker().build_state(&FakeProbe::real());
        let probe = FakeProbe {
            status: Some(
                r#"{
                    "vehicleID": "ND84720078011035",
                    "location": {"latitude": 35.1234, "longitude": -78.4567},
                    "gnssStatus": {"fix": true, "numberSatellites": 256,
                                   "antennaConnected": true},
                    "generalInformation": {
                        "ignitionOn": true,
                        "mainBatteryVoltage": -1.0,
                        "internalTemperature": 500.0
                    }
                }"#
                .to_string(),
            ),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);

        // Invalid beacon fields do not overwrite the existing LCI/NMEA planes;
        // in particular, 256 must not become the representable value 255.
        assert_eq!(state.gps, baseline.gps);
        assert_eq!(state.telem.battery_v, baseline.telem.battery_v);
        assert_eq!(state.telem.internal_temp_c, baseline.telem.internal_temp_c);
        assert!(state
            .gaps
            .iter()
            .any(|gap| gap.contains("satellite count out of range")));
        assert!(state
            .gaps
            .iter()
            .any(|gap| gap.contains("battery voltage out of range")));
        assert!(state
            .gaps
            .iter()
            .any(|gap| gap.contains("internal temperature out of range")));
    }

    #[test]
    fn malformed_status_beacon_preserves_lci_values_and_records_gap() {
        let probe = FakeProbe {
            status: Some("{not-json".to_string()),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);

        assert!((state.telem.battery_v - 12.60).abs() < 0.01);
        assert!((state.telem.internal_temp_c - 33.89).abs() < 0.01);
        assert!(state.telem.ignition_on);
        assert!(state
            .gaps
            .iter()
            .any(|gap| gap.contains("status broadcast invalid JSON")));
    }

    #[test]
    fn status_broadcast_configuration_error_is_visible_in_vehicle_gap() {
        let probe = FakeProbe {
            status_error: Some(format!(
                "{STATUS_PORT_ENV}: must be an integer from 1 to 65535"
            )),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);
        assert!(state.gaps.iter().any(|gap| {
            gap.contains("status broadcast configuration error") && gap.contains(STATUS_PORT_ENV)
        }));
    }

    #[test]
    fn empty_status_beacon_preserves_lci_values_and_records_schema_gap() {
        let probe = FakeProbe {
            status: Some("{}".to_string()),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);

        assert!((state.telem.battery_v - 12.60).abs() < 0.01);
        assert!(state.telem.ignition_on);
        assert!(state
            .gaps
            .iter()
            .any(|gap| gap.contains("no documented telemetry fields")));
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
        // A bare IP defaults to the oMG SSH port 2222 (the MG90 listens there).
        assert_eq!(
            parse_endpoint("192.168.13.31"),
            ("192.168.13.31".to_string(), 2222)
        );
        assert_eq!(
            parse_endpoint("192.168.13.31:2222"),
            ("192.168.13.31".to_string(), 2222)
        );
        // An explicit port still wins over the default.
        assert_eq!(
            parse_endpoint("192.168.13.31:22"),
            ("192.168.13.31".to_string(), 22)
        );
        // An unparsable suffix falls back to the whole string + default port.
        assert_eq!(
            parse_endpoint("host.local:ssh"),
            ("host.local:ssh".to_string(), 2222)
        );
    }

    #[test]
    fn root_password_reader_rejects_oversized_file_before_string_materialization() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("oversized-password");
        std::fs::write(&path, vec![b'x'; ROOT_PASSWORD_MAX_BYTES + 1]).unwrap();

        let file = std::fs::File::open(path).unwrap();
        assert!(
            read_root_password_bytes(file).is_none(),
            "password bytes beyond the bounded read must be rejected"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn root_password_reader_rejects_symlinked_file() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("password");
        let link = tmp.path().join("password-link");
        std::fs::write(&target, b"mg90-secret\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &link).unwrap();

        assert!(
            open_root_password_file(link.to_str().unwrap()).is_none(),
            "the password path must reject a final symlink before reading"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cookie_jar_is_private_exclusive_and_removed_on_drop() {
        use std::os::unix::fs::{symlink, MetadataExt as _, PermissionsExt as _};

        let tmp = tempfile::tempdir().unwrap();
        let runtime = tmp.path().join("vehicle-http");
        let jar_path = {
            let jar = create_cookie_jar_in(&runtime).expect("create private cookie jar");
            let path = jar.path().to_path_buf();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            assert!(metadata.file_type().is_file());
            assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
            path
        };
        assert!(
            !jar_path.exists(),
            "cookie jar must be removed by the all-path RAII cleanup"
        );

        let target = tmp.path().join("attacker-target");
        std::fs::write(&target, b"do not clobber").unwrap();
        let hostile_name = ".mg90-cookie-hostile.jar";
        symlink(&target, runtime.join(hostile_name)).unwrap();
        let directory = open_private_cookie_runtime_directory(&runtime).unwrap();
        let error = match create_cookie_jar_file(&directory, &runtime, hostile_name) {
            Ok(_) => panic!("exclusive cookie creation followed a hostile symlink"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&target).unwrap(), b"do not clobber");
    }

    #[cfg(unix)]
    #[test]
    fn cookie_runtime_directory_rejects_symlink() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("real-runtime");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = tmp.path().join("redirected-runtime");
        symlink(&target, &link).unwrap();

        let error = open_private_cookie_runtime_directory(&link)
            .err()
            .expect("symlinked cookie runtime must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
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

    #[test]
    fn initial_gateway_phase_is_stable_bounded_and_capped_for_tests() {
        let phase = initial_phase_for("seat-15", POLL);
        assert_eq!(phase, initial_phase_for("seat-15", POLL));
        assert!(phase <= MAX_INITIAL_PHASE);
        assert!(initial_phase_for("seat-15", Duration::from_millis(10)) <= Duration::from_millis(10));
        assert_ne!(phase, initial_phase_for("dell-laptop", POLL));
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
    async fn heartbeat_continues_while_initial_current_probe_is_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let probe = Arc::new(BlockingCurrentProbe::new());
        let mut worker = worker()
            .with_bus_root(Some(tmp.path().to_path_buf()))
            .with_probe(probe.clone())
            .with_poll(Duration::from_millis(10))
            .with_heartbeat(Duration::from_millis(10))
            .with_current_timeout(Duration::from_millis(15));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { worker.run(token).await });

        tokio::time::sleep(Duration::from_millis(45)).await;
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        let messages = persist
            .list_since(&vehicle_state_topic("rig-1"), None)
            .unwrap();
        assert!(
            messages.len() >= 4,
            "initial pending publication plus independent heartbeats expected, got {}",
            messages.len()
        );
        let states = messages
            .iter()
            .map(|message| {
                serde_json::from_str::<VehicleState>(message.body.as_deref().unwrap()).unwrap()
            })
            .collect::<Vec<_>>();
        assert!(states.iter().all(|state| !state.online));
        assert!(states
            .iter()
            .any(|state| state.gaps.iter().any(|gap| gap == "current status pending")));
        assert!(states.iter().any(|state| state
            .gaps
            .iter()
            .any(|gap| { gap == "current status unavailable (current-status timeout)" })));

        probe.release();
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(
            joined.is_ok(),
            "blocked-probe worker must exit after release"
        );
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

    fn roster_source_id() -> VehicleSourceId {
        VehicleSourceId::new("ND84720078011035").unwrap()
    }

    fn roster_snapshot(
        source_id: &VehicleSourceId,
        manager_id: &str,
        observed_at_ms: i64,
        published_at_ms: i64,
        sequence: u64,
    ) -> VehicleRosterSnapshot {
        let mut snapshot = VehicleWorker::new(manager_id.to_string())
            .with_bus_root(None)
            .with_probe(Arc::new(FakeProbe::real()))
            .build_state_v2(&FakeProbe::real());
        snapshot.mg90.id = source_id.as_str().to_string();
        snapshot.mg90.esn = source_id.as_str().to_string();
        snapshot.observed_at_ms = observed_at_ms;
        snapshot.published_at_ms = published_at_ms;
        snapshot.sequence = sequence;
        VehicleRosterSnapshot::from_v2(source_id.clone(), manager_id, snapshot).unwrap()
    }

    #[test]
    fn roster_keeps_source_identity_separate_from_endpoint_and_manager() {
        let source = roster_source_id();
        assert_eq!(source.as_str(), "ND84720078011035");
        assert!(VehicleSourceId::new("192.168.13.31:2222").is_err());

        let t0 = Instant::now();
        let mut roster = VehicleRoster::new(t0);
        roster
            .register(
                VehicleRosterSource::remote(
                    source.clone(),
                    "manager-a",
                    VehiclePollPlan::new(Duration::from_secs(5), ROSTER_HEARTBEAT).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        roster
            .register(
                VehicleRosterSource::remote(
                    source.clone(),
                    "manager-b",
                    VehiclePollPlan::new(Duration::from_secs(7), ROSTER_HEARTBEAT).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(roster.assignment_count(), 2);
        assert_eq!(roster.source_ids(), vec![source]);
    }

    #[test]
    fn roster_rejects_unsupported_snapshot_schema_before_acceptance() {
        let source = roster_source_id();
        let mut snapshot = VehicleWorker::new("manager-a".to_string())
            .with_bus_root(None)
            .with_probe(Arc::new(FakeProbe::real()))
            .build_state_v2(&FakeProbe::real());
        snapshot.mg90.id = source.as_str().to_string();
        snapshot.mg90.esn = source.as_str().to_string();
        snapshot.schema_version = VEHICLE_STATE_V2_SCHEMA_VERSION + 1;

        assert_eq!(
            VehicleRosterSnapshot::from_v2(source, "manager-a", snapshot),
            Err(VehicleRosterError::UnsupportedSchemaVersion {
                expected: VEHICLE_STATE_V2_SCHEMA_VERSION,
                actual: VEHICLE_STATE_V2_SCHEMA_VERSION + 1,
            })
        );
    }

    #[test]
    fn roster_schedules_each_manager_assignment_with_independent_cadences() {
        let source_a = VehicleSourceId::new("mg90-a").unwrap();
        let source_b = VehicleSourceId::new("mg90-b").unwrap();
        let t0 = Instant::now();
        let mut roster = VehicleRoster::new(t0);
        roster
            .register(
                VehicleRosterSource::remote(
                    source_a.clone(),
                    "manager-a",
                    VehiclePollPlan::new(Duration::from_secs(5), Duration::from_secs(2)).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        roster
            .register(
                VehicleRosterSource::remote(
                    source_b.clone(),
                    "manager-b",
                    VehiclePollPlan::new(Duration::from_secs(7), ROSTER_HEARTBEAT).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();

        assert_eq!(
            roster.take_due(t0),
            vec![
                VehicleScheduledWork {
                    source_id: source_a.clone(),
                    manager_id: "manager-a".to_string(),
                    kind: VehicleScheduleKind::CurrentStatus,
                },
                VehicleScheduledWork {
                    source_id: source_a.clone(),
                    manager_id: "manager-a".to_string(),
                    kind: VehicleScheduleKind::Heartbeat,
                },
                VehicleScheduledWork {
                    source_id: source_a.clone(),
                    manager_id: "manager-a".to_string(),
                    kind: VehicleScheduleKind::Enrichment,
                },
                VehicleScheduledWork {
                    source_id: source_b.clone(),
                    manager_id: "manager-b".to_string(),
                    kind: VehicleScheduleKind::CurrentStatus,
                },
                VehicleScheduledWork {
                    source_id: source_b.clone(),
                    manager_id: "manager-b".to_string(),
                    kind: VehicleScheduleKind::Heartbeat,
                },
                VehicleScheduledWork {
                    source_id: source_b.clone(),
                    manager_id: "manager-b".to_string(),
                    kind: VehicleScheduleKind::Enrichment,
                },
            ]
        );
        assert_eq!(
            roster.take_due(t0 + Duration::from_secs(2)),
            vec![
                VehicleScheduledWork {
                    source_id: source_a.clone(),
                    manager_id: "manager-a".to_string(),
                    kind: VehicleScheduleKind::Heartbeat,
                },
                VehicleScheduledWork {
                    source_id: source_b.clone(),
                    manager_id: "manager-b".to_string(),
                    kind: VehicleScheduleKind::Heartbeat,
                },
            ]
        );
        assert!(roster.take_due(t0 + Duration::from_secs(3)).is_empty());
        assert_eq!(
            roster.take_due(t0 + Duration::from_secs(4)),
            vec![
                VehicleScheduledWork {
                    source_id: source_a.clone(),
                    manager_id: "manager-a".to_string(),
                    kind: VehicleScheduleKind::Heartbeat,
                },
                VehicleScheduledWork {
                    source_id: source_b,
                    manager_id: "manager-b".to_string(),
                    kind: VehicleScheduleKind::Heartbeat,
                },
            ]
        );
        assert_eq!(
            roster.take_due(t0 + Duration::from_secs(5)),
            vec![VehicleScheduledWork {
                source_id: source_a,
                manager_id: "manager-a".to_string(),
                kind: VehicleScheduleKind::CurrentStatus,
            }]
        );
    }

    #[test]
    fn roster_rejects_a_heartbeat_slower_than_two_seconds() {
        assert!(matches!(
            VehiclePollPlan::new(Duration::from_secs(5), Duration::from_millis(2_001)),
            Err(VehicleRosterError::InvalidPollPlan(detail))
                if detail.contains("heartbeat must be at most")
        ));
    }

    #[test]
    fn delayed_or_failed_enrichment_does_not_delay_gateway_heartbeats() {
        let source_a = VehicleSourceId::new("mg90-a").unwrap();
        let source_b = VehicleSourceId::new("mg90-b").unwrap();
        let t0 = Instant::now();
        let plan = VehiclePollPlan::new(Duration::from_secs(5), ROSTER_HEARTBEAT)
            .unwrap()
            .with_enrichment(Duration::from_secs(30))
            .unwrap();
        let mut roster = VehicleRoster::new(t0);
        for (source, manager) in [(&source_b, "manager-b"), (&source_a, "manager-a")] {
            roster
                .register(VehicleRosterSource::remote(source.clone(), manager, plan).unwrap())
                .unwrap();
            roster
                .ingest(roster_snapshot(source, manager, 100, 100, 1))
                .unwrap();
        }

        let initial = roster.take_due(t0);
        assert_eq!(
            initial
                .iter()
                .filter(|work| work.kind == VehicleScheduleKind::Enrichment)
                .map(|work| work.source_id.as_str())
                .collect::<Vec<_>>(),
            vec!["mg90-a", "mg90-b"]
        );
        assert!(roster
            .take_publications(t0)
            .iter()
            .all(|publication| publication.reason == VehiclePublicationReason::Changed));

        // A remains delayed. B fails and only releases its enrichment lane;
        // neither outcome is allowed to mutate accepted telemetry.
        roster.finish_enrichment(&source_b, "manager-b").unwrap();
        let heartbeat_work = roster.take_due(t0 + ROSTER_HEARTBEAT);
        assert_eq!(
            heartbeat_work,
            vec![
                VehicleScheduledWork {
                    source_id: source_a.clone(),
                    manager_id: "manager-a".to_string(),
                    kind: VehicleScheduleKind::Heartbeat,
                },
                VehicleScheduledWork {
                    source_id: source_b.clone(),
                    manager_id: "manager-b".to_string(),
                    kind: VehicleScheduleKind::Heartbeat,
                },
            ]
        );

        let publications = roster.take_publications(t0 + ROSTER_HEARTBEAT);
        assert_eq!(
            publications
                .iter()
                .map(|publication| publication.source_id.as_str())
                .collect::<Vec<_>>(),
            vec!["mg90-a", "mg90-b"]
        );
        assert!(publications
            .iter()
            .all(|publication| publication.reason == VehiclePublicationReason::Heartbeat));
        assert_eq!(
            publications[0].snapshot.telem,
            roster_snapshot(&source_a, "manager-a", 100, 100, 1)
                .snapshot
                .telem
        );
        assert_eq!(
            publications[1].snapshot.telem,
            roster_snapshot(&source_b, "manager-b", 100, 100, 1)
                .snapshot
                .telem
        );
    }

    #[test]
    fn gateway_change_and_heartbeat_publication_clocks_are_isolated() {
        let source_a = VehicleSourceId::new("mg90-a").unwrap();
        let source_b = VehicleSourceId::new("mg90-b").unwrap();
        let t0 = Instant::now();
        let plan = VehiclePollPlan::new(Duration::from_secs(5), ROSTER_HEARTBEAT).unwrap();
        let mut roster = VehicleRoster::new(t0);
        for (source, manager) in [(&source_b, "manager-b"), (&source_a, "manager-a")] {
            roster
                .register(VehicleRosterSource::remote(source.clone(), manager, plan).unwrap())
                .unwrap();
            roster
                .ingest(roster_snapshot(source, manager, 100, 100, 1))
                .unwrap();
        }

        let first = roster.take_publications(t0);
        assert_eq!(
            first
                .iter()
                .map(|publication| publication.source_id.as_str())
                .collect::<Vec<_>>(),
            vec!["mg90-a", "mg90-b"]
        );

        // A has a newer observation with identical reported fields. It refreshes
        // the retained exact snapshot but does not trigger a false change.
        let metadata_only_a = roster_snapshot(&source_a, "manager-a", 200, 200, 2);
        assert!(roster.ingest(metadata_only_a.clone()).unwrap());

        // B changes one real reported field and must publish immediately without
        // disturbing A's independent heartbeat clock.
        let mut changed_b = roster_snapshot(&source_b, "manager-b", 200, 200, 2);
        changed_b.snapshot.telem.ignition_on = !changed_b.snapshot.telem.ignition_on;
        roster.ingest(changed_b.clone()).unwrap();
        let changed = roster.take_publications(t0 + Duration::from_secs(1));
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].source_id, source_b);
        assert_eq!(changed[0].reason, VehiclePublicationReason::Changed);
        assert_eq!(changed[0].snapshot, changed_b.snapshot);

        let heartbeat = roster.take_publications(t0 + ROSTER_HEARTBEAT);
        assert_eq!(heartbeat.len(), 1);
        assert_eq!(heartbeat[0].source_id, source_a);
        assert_eq!(heartbeat[0].reason, VehiclePublicationReason::Heartbeat);
        assert_eq!(heartbeat[0].snapshot, metadata_only_a.snapshot);
    }

    #[test]
    fn roster_selects_freshest_manager_snapshot_deterministically() {
        let source = roster_source_id();
        let plan = VehiclePollPlan::new(Duration::from_secs(5), ROSTER_HEARTBEAT).unwrap();
        let t0 = Instant::now();
        let mut roster = VehicleRoster::new(t0);
        roster
            .register(VehicleRosterSource::remote(source.clone(), "manager-a", plan).unwrap())
            .unwrap();
        roster
            .register(VehicleRosterSource::remote(source.clone(), "manager-b", plan).unwrap())
            .unwrap();

        assert!(roster
            .ingest(roster_snapshot(&source, "manager-a", 100, 100, 1))
            .unwrap());
        assert!(roster
            .ingest(roster_snapshot(&source, "manager-b", 200, 200, 1))
            .unwrap());
        match roster.select_latest(&source) {
            VehicleRosterSelection::Selected(snapshot) => {
                assert_eq!(snapshot.manager_id(), "manager-b");
                assert_eq!(snapshot.snapshot().observed_at_ms, 200);
            }
            other => panic!("expected selected source, got {other:?}"),
        }

        assert!(!roster
            .ingest(roster_snapshot(&source, "manager-b", 150, 150, 99))
            .unwrap());
        assert!(roster
            .ingest(roster_snapshot(&source, "manager-a", 300, 300, 9))
            .unwrap());
        assert!(roster
            .ingest(roster_snapshot(&source, "manager-b", 300, 300, 9))
            .unwrap());
        match roster.select_latest(&source) {
            VehicleRosterSelection::Selected(snapshot) => {
                assert_eq!(snapshot.manager_id(), "manager-b");
                assert_eq!(snapshot.snapshot().observed_at_ms, 300);
            }
            other => panic!("expected selected source, got {other:?}"),
        }
    }

    #[test]
    fn roster_routes_freshest_eligible_manager_and_binds_topic_to_snapshot() {
        let source = roster_source_id();
        let plan = VehiclePollPlan::default();
        let mut roster = VehicleRoster::new(Instant::now());
        roster
            .register(VehicleRosterSource::remote(source.clone(), "manager-a", plan).unwrap())
            .unwrap();
        roster
            .register(VehicleRosterSource::remote(source.clone(), "manager-b", plan).unwrap())
            .unwrap();

        let mut eligible = roster_snapshot(&source, "manager-a", 100, 100, 1);
        eligible.snapshot.managers =
            mackes_mesh_types::vehicle::ManagerSet::approved(vec!["manager-a".to_string()])
                .unwrap();
        let mut newer_but_unenrolled = roster_snapshot(&source, "manager-b", 200, 200, 2);
        newer_but_unenrolled.snapshot.managers =
            mackes_mesh_types::vehicle::ManagerSet::approved(vec!["manager-c".to_string()])
                .unwrap();
        roster.ingest(eligible).unwrap();
        roster.ingest(newer_but_unenrolled).unwrap();

        match roster.route_latest(&source) {
            VehicleManagerRouteSelection::Routed(route) => {
                assert_eq!(route.manager_id(), "manager-a");
                assert_eq!(route.source_id(), &source);
                assert_eq!(route.topic(), "state/vehicle/manager-a/ND84720078011035");
                assert_eq!(route.snapshot().observed_at_ms, 100);
                assert_eq!(route.snapshot().mg90.esn, source.as_str());
            }
            other => panic!("expected eligible manager route, got {other:?}"),
        }
    }

    #[test]
    fn roster_rejects_revoked_or_unenrolled_manager_without_fabricating_telemetry() {
        let source = roster_source_id();
        let plan = VehiclePollPlan::default();
        let mut roster = VehicleRoster::new(Instant::now());
        roster
            .register(VehicleRosterSource::remote(source.clone(), "manager-a", plan).unwrap())
            .unwrap();

        let mut snapshot = roster_snapshot(&source, "manager-a", 300, 300, 3);
        snapshot.snapshot.managers =
            mackes_mesh_types::vehicle::ManagerSet::approved(vec!["manager-a".to_string()])
                .unwrap();
        snapshot.snapshot.approval = mackes_mesh_types::vehicle::ApprovalState::Revoked;
        roster.ingest(snapshot).unwrap();
        assert_eq!(
            roster.route_latest(&source),
            VehicleManagerRouteSelection::Rejected {
                source_id: source,
                manager_id: "manager-a".to_string(),
                reason: VehicleManagerRouteRejection::ApprovalRevoked,
            }
        );
    }

    #[test]
    fn roster_rejects_manager_when_enrollment_is_not_authoritative() {
        let source = roster_source_id();
        let mut roster = VehicleRoster::new(Instant::now());
        roster
            .register(
                VehicleRosterSource::remote(
                    source.clone(),
                    "manager-a",
                    VehiclePollPlan::default(),
                )
                .unwrap(),
            )
            .unwrap();

        // The default v2 snapshot has an unknown manager set. That must not
        // be treated as implicit enrollment for manager-routed publication.
        roster
            .ingest(roster_snapshot(&source, "manager-a", 400, 400, 4))
            .unwrap();

        assert_eq!(
            roster.route_latest(&source),
            VehicleManagerRouteSelection::Rejected {
                source_id: source,
                manager_id: "manager-a".to_string(),
                reason: VehicleManagerRouteRejection::ManagerNotEnrolled,
            }
        );
    }

    #[test]
    fn roster_select_latest_all_returns_each_source_in_stable_order() {
        let source_a = VehicleSourceId::new("mg90-a").unwrap();
        let source_b = VehicleSourceId::new("mg90-b").unwrap();
        let plan = VehiclePollPlan::new(Duration::from_secs(5), ROSTER_HEARTBEAT).unwrap();
        let mut roster = VehicleRoster::new(Instant::now());
        // Register in reverse source order to prove the read model is not
        // dependent on discovery or manager insertion order.
        for (source, manager) in [
            (&source_b, "manager-c"),
            (&source_b, "manager-a"),
            (&source_a, "manager-b"),
            (&source_a, "manager-a"),
        ] {
            roster
                .register(VehicleRosterSource::remote(source.clone(), manager, plan).unwrap())
                .unwrap();
        }

        assert!(roster
            .ingest(roster_snapshot(&source_a, "manager-a", 200, 200, 4))
            .unwrap());
        assert!(roster
            .ingest(roster_snapshot(&source_a, "manager-b", 200, 200, 4))
            .unwrap());
        assert!(roster
            .ingest(roster_snapshot(&source_b, "manager-a", 100, 100, 1))
            .unwrap());
        assert!(roster
            .ingest(roster_snapshot(&source_b, "manager-c", 300, 300, 1))
            .unwrap());

        let selections = roster.select_latest_all();
        assert_eq!(selections.len(), 2);
        match &selections[0] {
            VehicleRosterSelection::Selected(snapshot) => {
                assert_eq!(snapshot.source_id(), &source_a);
                assert_eq!(snapshot.manager_id(), "manager-b");
                assert_eq!(snapshot.snapshot().observed_at_ms, 200);
            }
            other => panic!("expected source-a selection, got {other:?}"),
        }
        match &selections[1] {
            VehicleRosterSelection::Selected(snapshot) => {
                assert_eq!(snapshot.source_id(), &source_b);
                assert_eq!(snapshot.manager_id(), "manager-c");
                assert_eq!(snapshot.snapshot().observed_at_ms, 300);
            }
            other => panic!("expected source-b selection, got {other:?}"),
        }
    }

    #[test]
    fn roster_select_latest_all_reports_empty_and_unaccepted_sources() {
        let source_a = VehicleSourceId::new("mg90-a").unwrap();
        let source_b = VehicleSourceId::new("mg90-b").unwrap();
        let empty = VehicleRoster::new(Instant::now());
        assert_eq!(
            empty.select_latest_all(),
            vec![VehicleRosterSelection::NoSource {
                source_id: None,
                reason: VehicleNoSourceReason::EmptyRoster,
            }]
        );

        let plan = VehiclePollPlan::default();
        let mut roster = VehicleRoster::new(Instant::now());
        roster
            .register(VehicleRosterSource::remote(source_b, "manager-b", plan).unwrap())
            .unwrap();
        roster
            .register(VehicleRosterSource::remote(source_a.clone(), "manager-a", plan).unwrap())
            .unwrap();

        assert_eq!(
            roster.select_latest_all(),
            vec![
                VehicleRosterSelection::NoSource {
                    source_id: Some(source_a),
                    reason: VehicleNoSourceReason::NoAcceptedSnapshot,
                },
                VehicleRosterSelection::NoSource {
                    source_id: Some(VehicleSourceId::new("mg90-b").unwrap()),
                    reason: VehicleNoSourceReason::NoAcceptedSnapshot,
                },
            ]
        );
    }

    #[test]
    fn roster_no_source_is_explicit_and_does_not_publish_offline_telemetry() {
        let source = roster_source_id();
        let t0 = Instant::now();
        let empty = VehicleRoster::new(t0);
        assert!(matches!(
            empty.select_latest(&source),
            VehicleRosterSelection::NoSource {
                reason: VehicleNoSourceReason::EmptyRoster,
                ..
            }
        ));

        let mut remote_only = VehicleRoster::new(t0);
        remote_only
            .register(
                VehicleRosterSource::remote(
                    source.clone(),
                    "manager-a",
                    VehiclePollPlan::default(),
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            remote_only.heartbeat(&source),
            VehicleRosterSelection::NoSource {
                reason: VehicleNoSourceReason::NoAcceptedSnapshot,
                ..
            }
        ));

        let mut no_probe_worker = worker();
        no_probe_worker.probe = None;
        let mut local_no_source = VehicleRoster::new(t0);
        local_no_source
            .register(
                VehicleRosterSource::local(
                    source.clone(),
                    "rig-1",
                    Arc::new(no_probe_worker),
                    VehiclePollPlan::default(),
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            local_no_source.poll_source(&source, "rig-1"),
            VehicleRosterPollResult::NoSource {
                reason: VehicleNoSourceReason::ProbeUnavailable,
                ..
            }
        ));

        let wrong_source = VehicleSourceId::new("wrong-source").unwrap();
        let mut mismatch = VehicleRoster::new(t0);
        mismatch
            .register(
                VehicleRosterSource::local(
                    wrong_source.clone(),
                    "rig-1",
                    Arc::new(worker().with_probe(Arc::new(FakeProbe::real()))),
                    VehiclePollPlan::default(),
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            mismatch.poll_source(&wrong_source, "rig-1"),
            VehicleRosterPollResult::NoSource {
                reason: VehicleNoSourceReason::IdentityMismatch { .. },
                ..
            }
        ));
        assert!(matches!(
            mismatch.select_latest(&wrong_source),
            VehicleRosterSelection::NoSource {
                reason: VehicleNoSourceReason::NoAcceptedSnapshot,
                ..
            }
        ));
    }

    // ─────────────────────── Change 1 · per-modem cellular A/B parser ───────────────────────

    #[test]
    fn parses_per_modem_cellular_a_b_from_extended_wan_status() {
        let probe = FakeProbe {
            wan: Ok(WAN_EXTENDED.to_string()),
            ..FakeProbe::real()
        };
        let state = worker().build_state(&probe);

        // Cellular A — the active modem (carries the WAN IP).
        assert_eq!(state.wan.cellular_a.signal_dbm, -98, "primary RSSI dBm");
        assert_eq!(state.wan.cellular_a.technology, "LTE");
        assert_eq!(state.wan.cellular_a.sim_state, "ready");
        assert_eq!(state.wan.cellular_a.carrier, "9990198");
        assert_eq!(state.wan.cellular_a.wan_ip, "100.65.12.34");
        assert!(state.wan.cellular_a.healthy, "-98 dBm + SIM ⇒ healthy");

        // Cellular B — SIM-ready but idle (no IP Address ⇒ not active).
        assert_eq!(state.wan.cellular_b.signal_dbm, -105);
        assert_eq!(state.wan.cellular_b.sim_state, "ready");
        assert_eq!(state.wan.cellular_b.wan_ip, "not active");

        // The active WAN is derived from the IP-bearing section.
        assert_eq!(state.wan.active_wan, "Cellular A");
        assert_eq!(state.wan.active_cellular().map(|c| c.signal_dbm), Some(-98));

        // The now-satisfied "per-modem … not yet parsed" gap is gone.
        assert!(
            !state.gaps.iter().any(|g| g.contains("not yet parsed")),
            "stale per-modem gap removed: {:?}",
            state.gaps
        );
    }

    #[test]
    fn old_simplified_wan_fixture_still_folds_through_the_fallback() {
        // The simplified format (explicit `Active WAN` + a single `Signal … dBm`)
        // has no per-modem rows — it must still fold through the fallbacks.
        let state = worker().build_state(&FakeProbe::real());
        assert_eq!(state.wan.active_wan, "CellularA");
        assert_eq!(state.wan.cellular_a.signal_dbm, -72);
        assert_eq!(state.wan.vpn_state, "Connected");
        assert_eq!(state.wan.ethernet_state, "Down");
        assert_eq!(state.wan.wifi_state, "Disabled");
    }

    // ─────────────────────── Change 2 · action/vehicle/* control drain ───────────────────────

    #[test]
    fn get_config_returns_the_committed_yaml_over_ssh() {
        let fake = FakeProbe::real();
        let w = worker().with_probe(Arc::new(fake.clone()));
        let reply = w.handle("get-config", r#"{"file":"wan.yaml"}"#);
        assert!(reply.ok, "gated: {:?} err: {:?}", reply.gated, reply.error);
        assert_eq!(reply.applied.as_deref(), Some(FAKE_YAML));
        assert_eq!(fake.ssh_calls().as_slice(), &["omgconf latest wan.yaml"]);
    }

    #[test]
    fn get_config_rejects_a_non_bare_yaml_or_path_traversal() {
        let fake = FakeProbe::real();
        let w = worker().with_probe(Arc::new(fake.clone()));
        for bad in [
            "../etc/passwd.yaml",
            "/etc/wan.yaml",
            "sub/wan.yaml",
            "wan.txt",
            "wan",
            "..yaml",
        ] {
            let reply = w.handle("get-config", &format!(r#"{{"file":"{bad}"}}"#));
            assert!(!reply.ok, "`{bad}` must be rejected");
            assert!(reply.error.is_some(), "`{bad}` is an honest error");
        }
        // A missing `file` is an honest error, too.
        assert!(!w.handle("get-config", "{}").ok);
        // Nothing was ever shelled for the rejected inputs.
        assert!(fake.ssh_calls().is_empty(), "no rejected input reached ssh");
    }

    #[test]
    fn reboot_without_typed_name_is_gated_and_runs_no_ssh() {
        let fake = FakeProbe::real();
        let w = worker().with_probe(Arc::new(fake.clone()));
        let reply = w.handle("reboot", "{}");
        assert!(!reply.ok);
        assert!(
            reply.gated.unwrap().contains("privileged action refused"),
            "an unsigned reboot is authorization gated"
        );
        assert!(!reply.audited);
        assert!(fake.ssh_calls().is_empty(), "a gated reboot never runs ssh");
        assert_eq!(
            fake.general_calls(),
            0,
            "authorization runs before the ESN probe"
        );
    }

    #[test]
    fn reboot_with_the_wrong_typed_name_is_gated_after_authorization() {
        let auth_tmp = tempfile::tempdir().unwrap();
        let fake = FakeProbe::real();
        let body = authorized_reboot_body("vehicle-wrong-name", "reboot");
        let w = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_authorizer(test_authorizer(auth_tmp.path()));
        let reply = w.handle("reboot", &body);
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("typed-arm"));
        assert!(fake.ssh_calls().is_empty());
        assert_eq!(
            fake.general_calls(),
            1,
            "typed arming follows authorization"
        );
    }

    #[test]
    fn reboot_with_the_correct_esn_performs_and_audits() {
        let tmp = tempfile::tempdir().unwrap();
        let auth_tmp = tempfile::tempdir().unwrap();
        let fake = FakeProbe::real();
        let w = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_db_path(tmp.path().join("events.sqlite"))
            .with_authorizer(test_authorizer(auth_tmp.path()));
        let body = authorized_reboot_body("vehicle-correct-name", "ND84720078011035");
        // The FakeProbe general.html reports ESN ND84720078011035.
        let reply = w.handle("reboot", &body);
        assert!(reply.ok, "gated: {:?} err: {:?}", reply.gated, reply.error);
        assert!(reply.audited, "a performed reboot is audited");
        assert_eq!(reply.applied.as_deref(), Some("reboot issued"));
        assert_eq!(fake.ssh_calls().as_slice(), &["reboot"]);
    }

    #[test]
    fn reboot_rejects_tamper_and_replay_before_second_ssh() {
        let auth_tmp = tempfile::tempdir().unwrap();
        let fake = FakeProbe::real();
        let w = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_authorizer(test_authorizer(auth_tmp.path()));
        let body = authorized_reboot_body("vehicle-replay", "ND84720078011035");
        let tampered = body.replace("ND84720078011035", "ND84720078011036");

        let refusal = w.handle("reboot", &tampered);
        assert!(!refusal.ok);
        assert!(refusal.gated.unwrap().contains("refused"));
        assert_eq!(
            fake.general_calls(),
            0,
            "tamper is refused before the ESN probe"
        );
        assert!(fake.ssh_calls().is_empty());

        let first = w.handle("reboot", &body);
        assert!(first.ok);
        assert_eq!(fake.general_calls(), 1);
        assert_eq!(fake.ssh_calls().as_slice(), &["reboot"]);

        let replay = w.handle("reboot", &body);
        assert!(!replay.ok);
        assert!(replay.gated.unwrap().contains("already used"));
        assert_eq!(
            fake.general_calls(),
            1,
            "replay is refused before the ESN probe"
        );
        assert_eq!(fake.ssh_calls().as_slice(), &["reboot"]);
    }

    #[test]
    fn verbs_gate_when_this_node_has_no_gateway() {
        let mut w = worker();
        w.probe = None; // the MDE_VEHICLE_GATEWAY-unset node.
        let reply = w.handle("get-config", r#"{"file":"wan.yaml"}"#);
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("no gateway on this node"));
    }

    #[test]
    fn an_unknown_verb_is_an_honest_error() {
        let w = worker().with_probe(Arc::new(FakeProbe::real()));
        let reply = w.handle("frobnicate", "{}");
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("unknown vehicle verb"));
    }

    #[tokio::test]
    async fn drain_answers_get_config_on_the_reply_topic() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        let req = persist
            .write(
                "action/vehicle/get-config",
                Priority::Default,
                None,
                Some(r#"{"file":"wan.yaml"}"#),
            )
            .unwrap();
        let w = worker()
            .with_probe(Arc::new(FakeProbe::real()))
            .with_bus_root(Some(bus.clone()));
        let mut cursors = HashMap::new();
        assert!(w.drain_actions(&mut cursors), "the gateway node acted");
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        assert_eq!(replies.len(), 1, "exactly one reply");
        let reply: VehicleReply =
            serde_json::from_str(replies[0].body.as_deref().unwrap()).unwrap();
        assert!(reply.ok);
        assert_eq!(reply.applied.as_deref(), Some(FAKE_YAML));
    }

    #[tokio::test]
    async fn prime_cursors_skips_the_backlog_so_a_restart_does_not_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        persist
            .write(
                "action/vehicle/get-config",
                Priority::Default,
                None,
                Some(r#"{"file":"wan.yaml"}"#),
            )
            .unwrap();
        let w = worker()
            .with_probe(Arc::new(FakeProbe::real()))
            .with_bus_root(Some(bus.clone()));
        let mut cursors = HashMap::new();
        w.prime_cursors(&mut cursors);
        assert!(
            !w.drain_actions(&mut cursors),
            "the backlog is not replayed after prime"
        );
    }
}
