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
//! - `MDE_VEHICLE_SOURCE_ID` — optional governed MG90 ESN. When omitted, the
//!   first successful local identity poll establishes the source; an endpoint is
//!   never accepted as identity.
//! - `MDE_VEHICLE_MANAGERS` — optional comma-separated approved manager node IDs.
//!   The local node is included by the configured gateway and the bounded roster
//!   remains the sole v2 selection/publication authority.
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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mackes_mesh_types::vehicle::{
    parse_gpgga, vehicle_state_topic, vehicle_state_v2_topic, ApprovalState, CellLink,
    DeviceProbeStatus, FreshnessState, GpsFix, ImuSample, ManagerSet, ManagerSetState, RadioId,
    RadioInventory, RadioOperation, SnapshotProvenance, SnapshotSource, VehicleReply, VehicleState,
    VehicleStateV2, VehicleTelem, WanStatus, VEHICLE_ACTION_PREFIX,
    VEHICLE_STATE_V2_SCHEMA_VERSION,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

/// Env: the gateway endpoint (an IP or `ip:sshport`). Unset ⇒ the worker is a no-op.
pub const GATEWAY_ENV: &str = "MDE_VEHICLE_GATEWAY";

/// Optional governed MG90 ESN. When absent, the local probe's first confirmed
/// ESN establishes the sole roster source; it is never inferred from an endpoint.
pub const SOURCE_ID_ENV: &str = "MDE_VEHICLE_SOURCE_ID";

/// Optional comma-separated approved manager node IDs for this MG90. The local
/// node is always included when a gateway is configured, preserving the existing
/// single-manager deployment through the same roster authority.
pub const MANAGERS_ENV: &str = "MDE_VEHICLE_MANAGERS";

/// Env: the gateway `root` SSH password (later mde-seal; env is fine for now).
pub const ROOT_PW_ENV: &str = "MDE_VEHICLE_ROOT_PW";

/// Preferred env: path to the root-owned MG90 SSH password file.
pub const ROOT_PW_FILE_ENV: &str = "MDE_VEHICLE_ROOT_PW_FILE";

/// Default root-owned MG90 SSH password file used by the packaged worker/helper.
pub const ROOT_PW_FILE_DEFAULT: &str = "/etc/mackesd/mg90-root-password";

/// Password files contain one short line. Refuse unexpectedly large files before
/// converting their contents into a `String`.
const ROOT_PASSWORD_MAX_BYTES: usize = 4 * 1024;

/// The privileged-action journal is deliberately much smaller than the Bus. It
/// only bridges the claim/result/reply crash boundaries for in-flight reboots.
const ACTION_JOURNAL_SCHEMA_VERSION: u16 = 1;
const ACTION_JOURNAL_MAX_BYTES: u64 = 256 * 1024;
const ACTION_JOURNAL_MAX_ENTRIES: usize = 32;
const ACTION_JOURNAL_MAX_REPLY_BYTES: usize = 64 * 1024;
const ACTION_JOURNAL_NOFOLLOW_FLAG: i32 = 0o400_000;
static ACTION_JOURNAL_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Linux's `O_NOFOLLOW`: the final password-file path component must not be a
/// symlink. This worker is a Linux system service; keep the flag local rather
/// than adding a libc dependency just for the open boundary.
#[cfg(target_os = "linux")]
const ROOT_PASSWORD_NOFOLLOW_FLAG: i32 = 0o400_000;

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
const BUS_RETRY_MIN: Duration = Duration::from_millis(100);
const MAX_INITIAL_PHASE: Duration = Duration::from_millis(250);

/// Spread the first gateway status batch across a small deterministic window.
/// Later failures use the existing retry ladder; this phase prevents every
/// configured seat from opening its expensive root-SSH/HTTP path together.
#[must_use]
fn initial_phase_for(host: &str, cap: Duration) -> Duration {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in host.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    Duration::from_millis(
        (hash % (MAX_INITIAL_PHASE.as_millis() as u64 + 1)).min(cap.as_millis() as u64),
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
        let mut command = Command::new("curl");
        command
            .args([
                "--connect-timeout",
                CURL_CONNECT_TIMEOUT_SECONDS,
                "--max-time",
                CURL_MAX_TIME_SECONDS,
            ])
            .args(args);
        let out = crate::workers::proc::output_with_timeout(
            command,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        )?;
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
    /// The snapshot is pending or unknown rather than explicitly approved.
    ApprovalNotApproved {
        /// Exact non-approved state carried by the snapshot.
        state: ApprovalState,
    },
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
    if snapshot.approval == ApprovalState::Revoked {
        return Some(VehicleManagerRouteRejection::ApprovalRevoked);
    }
    if snapshot.approval != ApprovalState::Approved {
        return Some(VehicleManagerRouteRejection::ApprovalNotApproved {
            state: snapshot.approval,
        });
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

#[derive(Clone)]
struct VehiclePublishedState {
    snapshot: VehicleRosterSnapshot,
    published_at: Instant,
}

/// A configured source/manager assignment in the opt-in roster.
#[derive(Clone)]
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

#[derive(Clone)]
struct VehicleRosterAssignment {
    source: VehicleRosterSource,
    next_status: Instant,
    next_enrichment: Instant,
    next_heartbeat: Instant,
    enrichment_in_flight: bool,
    latest: Option<VehicleRosterSnapshot>,
    latest_received_at: Option<Instant>,
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
#[derive(Clone)]
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
                latest_received_at: None,
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

    fn take_due_kind(
        &mut self,
        now: Instant,
        kind: VehicleScheduleKind,
    ) -> Vec<VehicleScheduledWork> {
        let mut due = Vec::new();
        for assignment in self.assignments.values_mut() {
            let deadline = match kind {
                VehicleScheduleKind::CurrentStatus => &mut assignment.next_status,
                VehicleScheduleKind::Heartbeat => &mut assignment.next_heartbeat,
                VehicleScheduleKind::Enrichment => {
                    if assignment.enrichment_in_flight {
                        continue;
                    }
                    assignment.enrichment_in_flight = true;
                    &mut assignment.next_enrichment
                }
            };
            if now < *deadline {
                if kind == VehicleScheduleKind::Enrichment {
                    assignment.enrichment_in_flight = false;
                }
                continue;
            }
            let interval = match kind {
                VehicleScheduleKind::CurrentStatus => assignment.source.plan.poll,
                VehicleScheduleKind::Heartbeat => assignment.source.plan.heartbeat,
                VehicleScheduleKind::Enrichment => assignment.source.plan.enrichment,
            };
            *deadline = next_deadline(now, interval);
            due.push(VehicleScheduledWork {
                source_id: assignment.source.source_id.clone(),
                manager_id: assignment.source.manager_id.clone(),
                kind,
            });
        }
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
        self.ingest_at(snapshot, Instant::now())
    }

    fn ingest_at(
        &mut self,
        snapshot: VehicleRosterSnapshot,
        received_at: Instant,
    ) -> Result<bool, VehicleRosterError> {
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
            assignment.latest_received_at = Some(received_at);
        }
        Ok(replace)
    }

    /// Revoke one manager row immediately after an authoritative poll failure.
    /// Retained telemetry must not continue to claim that a lost manager is live.
    pub fn mark_unavailable(
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
        assignment.latest = None;
        assignment.latest_received_at = None;
        // A manager loss is not a source loss while another manager still has
        // an accepted snapshot. Preserve the source publication clock so an
        // unhealthy non-selected manager cannot manufacture a false Changed
        // publication on the next fold.
        let has_live_manager = self
            .assignments
            .iter()
            .any(|((candidate, _), assignment)| {
                candidate == source_id && assignment.latest.is_some()
            });
        if !has_live_manager {
            self.published.remove(source_id);
        }
        Ok(())
    }

    /// Expire manager rows that have stopped delivering their declared bounded
    /// heartbeat. Three missed intervals matches the vehicle consumer contract.
    pub fn expire_unavailable(&mut self, now: Instant) {
        let mut expired_sources = BTreeSet::new();
        for assignment in self.assignments.values_mut() {
            let Some(received_at) = assignment.latest_received_at else {
                continue;
            };
            let expiry = assignment
                .latest
                .as_ref()
                .map(|snapshot| {
                    Duration::from_millis(snapshot.snapshot.expected_interval_ms.max(1))
                        .min(MAX_ROSTER_HEARTBEAT)
                        .saturating_mul(3)
                })
                .unwrap_or(assignment.source.plan.heartbeat.saturating_mul(3));
            if now.saturating_duration_since(received_at) > expiry {
                expired_sources.insert(assignment.source.source_id.clone());
                assignment.latest = None;
                assignment.latest_received_at = None;
            }
        }
        for source_id in expired_sources {
            // A source may have several independent managers. Expiring one
            // manager must not reset the source publication epoch while
            // another manager still has an accepted snapshot; doing so emits
            // a false Changed publication and makes a healthy MG90 look like
            // it churned during manager failover.
            let has_live_manager = self
                .assignments
                .iter()
                .any(|((candidate, _), assignment)| {
                    candidate == &source_id && assignment.latest.is_some()
                });
            if !has_live_manager {
                self.published.remove(&source_id);
            }
        }
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

    /// Select approved/enrolled change-driven publications plus an unchanged
    /// heartbeat no slower than each source's configured interval.
    ///
    /// Multiple managers for one MG90 collapse through [`Self::route_latest`];
    /// multiple MG90 identities retain independent clocks and are returned in
    /// stable source-id order. A source with no accepted snapshot emits nothing.
    pub fn take_publications(&mut self, now: Instant) -> Vec<VehicleRosterPublication> {
        let mut ready = Vec::new();
        for source_id in self.source_ids() {
            let VehicleManagerRouteSelection::Routed(route) = self.route_latest(&source_id) else {
                self.published.remove(&source_id);
                continue;
            };
            let selected = VehicleRosterSnapshot::from_v2(
                source_id.clone(),
                route.manager_id.clone(),
                route.snapshot,
            )
            .expect("roster route preserves its admitted identity");
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

/// Retained WAN metrics are useful diagnostics during a transient MG90 probe
/// outage, but they are not a live radio-health observation. The v1 mirror has
/// no per-domain stale representation, so make the additive v2 projection
/// explicitly stale and revoke every retained active-path claim.
fn mark_retained_radio_state_stale(state: &VehicleState, snapshot: &mut VehicleStateV2) {
    if !state
        .gaps
        .iter()
        .any(|gap| gap.contains("wan status unavailable"))
    {
        return;
    }

    let mut radios = snapshot.radios.as_slice().to_vec();
    let mut retained_radio = false;
    for radio in &mut radios {
        if !matches!(
            &radio.id,
            RadioId::CellularA | RadioId::CellularB | RadioId::WifiA | RadioId::WifiB
        ) {
            continue;
        }
        if radio.age_ms.is_some()
            || radio.operation != RadioOperation::Unknown
            || radio.active_path
        {
            radio.operation = RadioOperation::Stale;
            radio.active_path = false;
            // The legacy WAN plane has no source timestamp. Zero would falsely
            // claim this retained sample was observed by the failed refresh.
            radio.age_ms = None;
            retained_radio = true;
        }
    }
    if retained_radio {
        snapshot.radios = RadioInventory::new(radios)
            .expect("the admitted native radio inventory remains bounded and unique");
        snapshot.freshness.radios.state = FreshnessState::Stale;
        snapshot.freshness.radios.age_ms = None;
        snapshot.freshness.radios.reason = Some("wan-probe-unavailable-retained".to_string());
    }
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
    source_before: Option<String>,
    source_after: Option<String>,
    gps: Option<GpsFix>,
    imu: Option<ImuSample>,
    gps_gaps: Vec<String>,
    wan: Option<WanStatus>,
    wan_gaps: Vec<String>,
    obd_probe_status: DeviceProbeStatus,
    obd_gaps: Vec<String>,
}

fn probe_enrichment_source(probe: &dyn VehicleProbe) -> Option<String> {
    probe
        .read_lci_general()
        .ok()
        .map(|general| strip_html(&general))
        .and_then(|general| find_token_after(&general, "ESN"))
        .filter(|source| !source.trim().is_empty())
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
        let source_matches = self.esn.as_deref().is_some_and(|expected| {
            enrichment.source_before.as_deref() == Some(expected)
                && enrichment.source_after.as_deref() == Some(expected)
        });
        if !source_matches {
            // Slow GNSS/WAN/application reads are a separate task from the
            // current-status generation. Revalidate the authoritative LCI ESN
            // on both sides of that batch so an endpoint replacement cannot
            // fold another MG90's telemetry into the retained source.
            self.mark_enrichment_unavailable(
                "MG90 source identity changed or was unavailable during enrichment",
            );
            return;
        }
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
        let mut diagnostics = Vec::with_capacity(
            self.current_gaps.len()
                + self.gps_gaps.len()
                + self.wan_gaps.len()
                + self.obd_gaps.len(),
        );
        diagnostics.extend(self.current_gaps.iter().cloned());
        diagnostics.extend(self.gps_gaps.iter().cloned());
        diagnostics.extend(self.wan_gaps.iter().cloned());
        diagnostics.extend(self.obd_gaps.iter().cloned());
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
            gaps: diagnostics,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VehicleBusIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

#[derive(Debug, Clone)]
enum VehicleBusRoot {
    Dynamic,
    Explicit(PathBuf),
    Disabled,
}

#[derive(Debug, Clone)]
struct PendingVehicleReply {
    source_index: VehicleBusIdentity,
    request_topic: String,
    request_ulid: String,
    body: String,
    privileged_journal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VehicleActionTxnPhase {
    Claimed,
    Completed,
    Delivered,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VehicleActionTxn {
    request_ulid: String,
    request_topic: String,
    verb: String,
    phase: VehicleActionTxnPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reply: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VehicleActionJournal {
    schema_version: u16,
    host: String,
    entries: Vec<VehicleActionTxn>,
}

impl VehicleActionJournal {
    fn empty(host: &str) -> Self {
        Self {
            schema_version: ACTION_JOURNAL_SCHEMA_VERSION,
            host: host.to_string(),
            entries: Vec::new(),
        }
    }
}

fn vehicle_action_journal_path(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("vehicle-action-journal.json")
}

#[cfg(unix)]
fn validate_action_journal_parent(path: &Path) -> io::Result<u32> {
    use std::os::unix::fs::MetadataExt;

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("vehicle action journal has no parent"))?;
    let metadata = fs::symlink_metadata(parent)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.mode() & 0o022 != 0 {
        return Err(io::Error::other(
            "vehicle action journal parent is not a trusted directory",
        ));
    }
    Ok(metadata.uid())
}

#[cfg(not(unix))]
fn validate_action_journal_parent(path: &Path) -> io::Result<u32> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("vehicle action journal has no parent"))?;
    if !fs::symlink_metadata(parent)?.is_dir() {
        return Err(io::Error::other(
            "vehicle action journal parent is not a directory",
        ));
    }
    Ok(0)
}

fn validate_action_journal_file(file: &File, owner_uid: u32) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > ACTION_JOURNAL_MAX_BYTES {
        return Err(io::Error::other(
            "vehicle action journal is not a bounded regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != owner_uid || metadata.mode() & 0o777 != 0o600 {
            return Err(io::Error::other(
                "vehicle action journal ownership or mode is not trusted",
            ));
        }
    }
    Ok(())
}

fn open_action_journal(path: &Path, owner_uid: u32) -> io::Result<File> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::other("vehicle action journal is a symlink"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(ACTION_JOURNAL_NOFOLLOW_FLAG);
    }
    let file = options.open(path)?;
    validate_action_journal_file(&file, owner_uid)?;
    Ok(file)
}

fn validate_action_journal(journal: &VehicleActionJournal, host: &str) -> io::Result<()> {
    if journal.schema_version != ACTION_JOURNAL_SCHEMA_VERSION || journal.host != host {
        return Err(io::Error::other(
            "vehicle action journal authority does not match this worker",
        ));
    }
    if journal.entries.len() > ACTION_JOURNAL_MAX_ENTRIES {
        return Err(io::Error::other(
            "vehicle action journal entry bound exceeded",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for entry in &journal.entries {
        if entry.request_ulid.is_empty()
            || entry.request_ulid.len() > 128
            || entry.request_topic.len() > 256
            || entry.request_topic != "action/vehicle/reboot"
            || entry.verb != "reboot"
            || !seen.insert(entry.request_ulid.as_str())
        {
            return Err(io::Error::other("invalid vehicle action journal entry"));
        }
        match (entry.phase, entry.reply.as_deref()) {
            (VehicleActionTxnPhase::Claimed, None)
            | (VehicleActionTxnPhase::Completed | VehicleActionTxnPhase::Delivered, Some(_)) => {}
            _ => return Err(io::Error::other("invalid vehicle action journal phase")),
        }
        if let Some(reply) = entry.reply.as_deref() {
            if reply.len() > ACTION_JOURNAL_MAX_REPLY_BYTES
                || serde_json::from_str::<VehicleReply>(reply).is_err()
            {
                return Err(io::Error::other(
                    "vehicle action journal contains an invalid typed reply",
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
struct VehicleActionDrainState {
    active_index: Option<VehicleBusIdentity>,
    cursors: HashMap<String, String>,
    pending_reply: Option<PendingVehicleReply>,
}

#[derive(Debug, Clone, Copy)]
enum VehiclePendingCommitKind {
    Current { healthy: bool, was_online: bool },
    Enrichment,
}

#[derive(Clone)]
struct VehiclePendingCommit {
    runtime: VehicleRuntimeSnapshot,
    cached: VehicleState,
    roster: VehicleRuntimeRoster,
    kind: VehiclePendingCommitKind,
    publish_roster: bool,
    publish_unavailable: bool,
}

fn vehicle_bus_identity(root: &Path) -> io::Result<VehicleBusIdentity> {
    let metadata = std::fs::metadata(root.join("index.sqlite"))?;
    if !metadata.is_file() {
        return Err(io::Error::other("vehicle Bus index is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(VehicleBusIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(VehicleBusIdentity {})
    }
}

/// Sole runtime owner of manager registration, accepted snapshots, manager
/// selection, and publication clocks. `VehicleWorker::run` feeds every local and
/// remote observation through this object; there is no second v2 cache.
#[derive(Clone)]
struct VehicleRuntimeRoster {
    roster: VehicleRoster,
    local_manager: String,
    managers: Vec<String>,
    source_id: Option<VehicleSourceId>,
    remote_cursors: HashMap<String, String>,
    remote_index: Option<VehicleBusIdentity>,
    plan: VehiclePollPlan,
}

impl VehicleRuntimeRoster {
    fn from_env(
        local_manager: &str,
        started_at: Instant,
        plan: VehiclePollPlan,
    ) -> Result<Self, VehicleRosterError> {
        let source_id = std::env::var(SOURCE_ID_ENV)
            .ok()
            .map(|value| VehicleSourceId::new(value.trim().to_string()))
            .transpose()?;
        let configured = std::env::var(MANAGERS_ENV).unwrap_or_default();
        Self::new(local_manager, source_id, &configured, started_at, plan)
    }

    fn new(
        local_manager: &str,
        source_id: Option<VehicleSourceId>,
        configured_managers: &str,
        started_at: Instant,
        plan: VehiclePollPlan,
    ) -> Result<Self, VehicleRosterError> {
        plan.validate()?;
        let local_manager = validate_manager_id(local_manager)?;
        let mut managers = vec![local_manager.clone()];
        for raw in configured_managers.split(',') {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let manager = validate_manager_id(raw)?;
            if !managers.contains(&manager) {
                managers.push(manager);
            }
        }
        if managers.len() > MAX_VEHICLE_ROSTER_MANAGERS {
            return Err(VehicleRosterError::ManagerCapacity);
        }
        let mut runtime = Self {
            roster: VehicleRoster::new(started_at),
            local_manager,
            managers,
            source_id: None,
            remote_cursors: HashMap::new(),
            remote_index: None,
            plan,
        };
        if let Some(source_id) = source_id {
            runtime.register_source(source_id)?;
        }
        Ok(runtime)
    }

    fn register_source(&mut self, source_id: VehicleSourceId) -> Result<(), VehicleRosterError> {
        if let Some(current) = self.source_id.as_ref() {
            if current != &source_id {
                return Err(VehicleRosterError::IdentityMismatch {
                    expected: current.clone(),
                    reported: source_id.to_string(),
                    manager_id: self.local_manager.clone(),
                });
            }
            return Ok(());
        }
        for manager in &self.managers {
            self.roster.register(VehicleRosterSource::remote(
                source_id.clone(),
                manager.clone(),
                self.plan,
            )?)?;
        }
        self.source_id = Some(source_id);
        Ok(())
    }

    fn ingest_local(
        &mut self,
        worker: &VehicleWorker,
        state: &VehicleState,
        received_at: Instant,
    ) -> Result<bool, VehicleRosterError> {
        let reported = state.esn.trim();
        if reported.is_empty() || !state.online {
            self.mark_local_unavailable();
            return Ok(false);
        }
        let source_id = VehicleSourceId::new(reported.to_string())?;
        self.register_source(source_id.clone())?;
        let mut snapshot = worker.snapshot_v2_with_interval_and_sequence(
            state,
            ROSTER_HEARTBEAT,
            worker.sequence.load(Ordering::Relaxed).saturating_add(1),
        );
        snapshot.approval = ApprovalState::Approved;
        snapshot.managers = ManagerSet::approved(self.managers.clone())
            .map_err(|error| VehicleRosterError::InvalidManagerId(error.to_string()))?;
        let admitted =
            VehicleRosterSnapshot::from_v2(source_id, self.local_manager.clone(), snapshot)?;
        self.roster.ingest_at(admitted, received_at)
    }

    fn mark_local_unavailable(&mut self) {
        if let Some(source_id) = self.source_id.clone() {
            let _ = self
                .roster
                .mark_unavailable(&source_id, &self.local_manager);
        }
    }

    fn ingest_remote(&mut self, worker: &VehicleWorker, received_at: Instant) -> io::Result<()> {
        let Some(source_id) = self.source_id.clone() else {
            return Ok(());
        };
        let Some((root, index, persist)) = worker.open_bus_transaction()? else {
            return Ok(());
        };
        let mut staged = self.clone();
        let managers = staged
            .managers
            .iter()
            .filter(|manager| *manager != &self.local_manager)
            .cloned()
            .collect::<Vec<_>>();
        if staged.remote_index != Some(index) {
            staged.remote_cursors.clear();
            for manager in &managers {
                staged
                    .roster
                    .mark_unavailable(&source_id, manager)
                    .map_err(|error| io::Error::other(error.to_string()))?;
            }
            staged.remote_index = Some(index);
        }
        for manager in managers {
            let topic = vehicle_state_v2_topic(&manager, source_id.as_str());
            #[cfg(test)]
            if worker
                .remote_read_failure
                .lock()
                .expect("remote read failure lock")
                .as_deref()
                == Some(topic.as_str())
            {
                return Err(io::Error::other("injected remote roster read failure"));
            }
            let cursor = staged.remote_cursors.get(&topic).map(String::as_str);
            let messages = persist
                .list_since(&topic, cursor)
                .map_err(|error| io::Error::other(error.to_string()))?;
            for message in messages {
                staged
                    .remote_cursors
                    .insert(topic.clone(), message.ulid.clone());
                let Some(body) = message.body.as_deref() else {
                    continue;
                };
                let Ok(snapshot) = serde_json::from_str::<VehicleStateV2>(body) else {
                    tracing::warn!(
                        target: "mackesd::vehicle",
                        manager = %manager,
                        source = %source_id,
                        "rejected malformed remote vehicle snapshot"
                    );
                    continue;
                };
                match VehicleRosterSnapshot::from_v2(source_id.clone(), manager.clone(), snapshot) {
                    Ok(admitted) => {
                        staged
                            .roster
                            .ingest_at(admitted, received_at)
                            .map_err(|error| io::Error::other(error.to_string()))?;
                    }
                    Err(error) => tracing::warn!(
                        target: "mackesd::vehicle",
                        manager = %manager,
                        source = %source_id,
                        %error,
                        "rejected identity-mismatched remote vehicle snapshot"
                    ),
                }
            }
        }
        worker.verify_bus_identity(&root, index)?;
        *self = staged;
        Ok(())
    }

    fn take_publications(&mut self, now: Instant) -> Vec<VehicleRosterPublication> {
        self.roster.expire_unavailable(now);
        self.roster.take_publications(now)
    }

    fn local_due(&mut self, now: Instant, kind: VehicleScheduleKind) -> bool {
        if self.source_id.is_none() {
            return kind == VehicleScheduleKind::CurrentStatus;
        }
        self.roster
            .take_due_kind(now, kind)
            .into_iter()
            .any(|work| work.manager_id == self.local_manager)
    }

    fn finish_local_enrichment(&mut self) {
        if let Some(source_id) = self.source_id.clone() {
            let _ = self
                .roster
                .finish_enrichment(&source_id, &self.local_manager);
        }
    }
}

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
    /// Dynamic production resolver, an explicit test root, or an explicit
    /// test-only disable selected through `with_bus_root(None)`.
    bus_root: VehicleBusRoot,
    /// The hash-chain audit DB (a performed `reboot` audits here — mirrors the
    /// `cloud` worker's destructive-op audit).
    db_path: PathBuf,
    /// Host-local crash journal for privileged action claim/result delivery.
    action_journal_path: PathBuf,
    /// Poll + heartbeat cadence.
    poll: Duration,
    heartbeat: Duration,
    current_timeout: Duration,
    /// Per-management-node monotonic v2 snapshot sequence.
    sequence: AtomicU64,
    /// Shared, fail-closed authorization gate for destructive Bus mutations.
    authorizer: Arc<ActionAuthorizer>,
    /// Hostile-test reply fault budget; production always leaves this at zero.
    reply_failures: AtomicU64,
    #[cfg(test)]
    remote_read_failure: std::sync::Mutex<Option<String>>,
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
        let db_path = crate::default_db_path();
        Self {
            host,
            probe,
            bus_root: VehicleBusRoot::Dynamic,
            action_journal_path: vehicle_action_journal_path(&db_path),
            db_path,
            poll: POLL,
            heartbeat: ROSTER_HEARTBEAT,
            current_timeout: CURRENT_STATUS_TIMEOUT,
            sequence: AtomicU64::new(0),
            authorizer: Arc::new(ActionAuthorizer::production()),
            reply_failures: AtomicU64::new(0),
            #[cfg(test)]
            remote_read_failure: std::sync::Mutex::new(None),
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
        self.bus_root = root.map_or(VehicleBusRoot::Disabled, VehicleBusRoot::Explicit);
        self
    }

    /// Override the audit DB path (tests point it at a tempdir).
    #[must_use]
    pub fn with_db_path(mut self, p: PathBuf) -> Self {
        self.action_journal_path = vehicle_action_journal_path(&p);
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

    fn bus_roots(&self) -> Option<Vec<PathBuf>> {
        match &self.bus_root {
            VehicleBusRoot::Disabled => None,
            VehicleBusRoot::Explicit(root) => Some(vec![root.clone()]),
            VehicleBusRoot::Dynamic => {
                let system = PathBuf::from(mde_bus::SYSTEM_BUS_ROOT);
                let mut roots = Vec::with_capacity(2);
                if let Some(current) = mde_bus::default_data_dir() {
                    roots.push(current);
                }
                if !roots.iter().any(|root| root == &system) {
                    roots.push(system);
                }
                Some(roots)
            }
        }
    }

    fn open_bus_transaction(&self) -> io::Result<Option<(PathBuf, VehicleBusIdentity, Persist)>> {
        let Some(roots) = self.bus_roots() else {
            return Ok(None);
        };
        let mut last_error = None;
        for root in roots {
            let before_open = match vehicle_bus_identity(&root) {
                Ok(identity) => Some(identity),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            };
            let persist = match Persist::open(root.clone()) {
                Ok(persist) => persist,
                Err(error) => {
                    last_error = Some(io::Error::other(error.to_string()));
                    continue;
                }
            };
            let opened = vehicle_bus_identity(&root)?;
            if before_open.is_some_and(|before| before != opened) {
                last_error = Some(io::Error::other(
                    "vehicle Bus index changed while opening transaction",
                ));
                continue;
            }
            return Ok(Some((root, opened, persist)));
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("vehicle Bus root unresolved")))
    }

    fn verify_bus_identity(&self, root: &Path, expected: VehicleBusIdentity) -> io::Result<()> {
        if vehicle_bus_identity(root).is_ok_and(|identity| identity == expected) {
            Ok(())
        } else {
            Err(io::Error::other(
                "vehicle Bus index changed during transaction",
            ))
        }
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

    #[cfg(test)]
    fn with_reply_failures(self, failures: u64) -> Self {
        self.reply_failures.store(failures, Ordering::Relaxed);
        self
    }

    #[cfg(test)]
    fn set_remote_read_failure(&self, topic: Option<String>) {
        *self
            .remote_read_failure
            .lock()
            .expect("remote read failure lock") = topic;
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
        let source_before = probe_enrichment_source(probe);
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

        let source_after = probe_enrichment_source(probe);
        VehicleEnrichmentObservation {
            source_before,
            source_after,
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
        let (mut ssh_gps, imu) = match probe.read_gps_nmea() {
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
            ssh_gps = merge_beacon_gps(ssh_gps, beacon_gps);
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
            moving: ssh_gps.speed_mph > 0.5,
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
            gps: ssh_gps,
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
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.snapshot_v2_with_interval_and_sequence(state, expected_interval, sequence)
    }

    fn snapshot_v2_with_interval_and_sequence(
        &self,
        state: &VehicleState,
        expected_interval: Duration,
        sequence: u64,
    ) -> VehicleStateV2 {
        let published_at_ms = now_ms();
        let mut snapshot = VehicleStateV2::from_v1(
            state,
            self.host.clone(),
            sequence,
            expected_interval.as_millis().try_into().unwrap_or(u64::MAX),
            published_at_ms,
            SnapshotProvenance {
                source: SnapshotSource::DirectGateway,
                source_id: Some(self.host.clone()),
                relay: None,
            },
        );
        mark_retained_radio_state_stale(state, &mut snapshot);
        snapshot
    }

    fn snapshot_v2(&self, state: &VehicleState) -> VehicleStateV2 {
        self.snapshot_v2_with_interval(state, self.poll)
    }

    /// Publish the v1 compatibility mirror and, when the gateway ESN is
    /// confirmed, the identity-addressed v2 mirror. An unknown ESN is never
    /// replaced with a synthetic topic segment.
    #[cfg(test)]
    fn publish_pair(&self, legacy: &VehicleState, observed: &VehicleState, interval: Duration) {
        let mut rows = vec![(
            vehicle_state_topic(&self.host),
            serde_json::to_string(legacy).expect("serialize legacy vehicle state"),
        )];
        let v2 = self.snapshot_v2_with_interval(observed, interval);
        if !v2.mg90.id.is_empty() {
            rows.push((
                vehicle_state_v2_topic(&v2.management_node_id, &v2.mg90.id),
                serde_json::to_string(&v2).expect("serialize v2 vehicle state"),
            ));
        } else {
            tracing::debug!(
                target: "mackesd::vehicle",
                host = %self.host,
                "v2 vehicle snapshot withheld until MG90 ESN is confirmed"
            );
        }
        self.publish_rows(&rows).expect("publish vehicle pair");
    }

    #[cfg(test)]
    fn publish(&self, state: &VehicleState) {
        self.publish_pair(state, state, self.poll);
    }

    fn publish_roster_updates(
        &self,
        roster: &mut VehicleRuntimeRoster,
        local_state: &VehicleState,
        now: Instant,
    ) -> io::Result<()> {
        let mut staged = roster.clone();
        let publications = staged.take_publications(now);
        let mut rows = Vec::new();
        let mut committed_sequence = None;
        for publication in publications {
            // A remote manager's accepted row is already present on its exact Bus
            // lane. Re-emitting it here would create an amplification loop. The
            // roster still selects it as authoritative and suppresses this node's
            // competing claim until the remote row expires.
            if publication.manager_id != self.host {
                continue;
            }
            rows.push((
                vehicle_state_v2_topic(&publication.manager_id, publication.source_id.as_str()),
                serde_json::to_string(&publication.snapshot).map_err(io_other)?,
            ));
            committed_sequence = Some(
                committed_sequence
                    .unwrap_or(0)
                    .max(publication.snapshot.sequence),
            );
            if local_state.online && local_state.esn == publication.source_id.as_str() {
                let mut legacy = local_state.clone();
                legacy.published_at_ms = now_ms();
                rows.push((
                    vehicle_state_topic(&self.host),
                    serde_json::to_string(&legacy).map_err(io_other)?,
                ));
            }
        }
        self.publish_rows(&rows)?;
        if let Some(sequence) = committed_sequence {
            self.sequence.store(sequence, Ordering::Relaxed);
        }
        *roster = staged;
        Ok(())
    }

    /// Preserve the one-release legacy availability signal without creating a
    /// v2 source or manager claim. Callers may use this only for explicit
    /// pending/unavailable state.
    fn publish_legacy_unavailable(&self, state: &VehicleState) -> io::Result<()> {
        debug_assert!(!state.online);
        let mut state = state.clone();
        state.published_at_ms = now_ms();
        self.publish_rows(&[(
            vehicle_state_topic(&self.host),
            serde_json::to_string(&state).map_err(io_other)?,
        )])
    }

    fn publish_rows(&self, rows: &[(String, String)]) -> io::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let Some((root, index, persist)) = self.open_bus_transaction()? else {
            return Ok(());
        };
        for (topic, body) in rows {
            persist
                .write(topic, Priority::Default, None, Some(body))
                .map_err(io_other)?;
        }
        self.verify_bus_identity(&root, index)
    }

    fn publish_pending_commit(&self, pending: &mut VehiclePendingCommit) -> io::Result<()> {
        if pending.publish_roster {
            self.publish_roster_updates(&mut pending.roster, &pending.cached, Instant::now())?;
        }
        if pending.publish_unavailable {
            self.publish_legacy_unavailable(&pending.cached)?;
        }
        Ok(())
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
    /// otherwise nothing is performed and the reply is honestly gated. After a
    /// performed reboot, `audited` reflects only a committed events-plane row;
    /// alert-hook delivery remains best-effort.
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
                let audited = self.audit_reboot(&esn);
                VehicleReply {
                    ok: true,
                    verb: verb_name.to_string(),
                    applied: Some("reboot issued".to_string()),
                    audited,
                    error: (!audited)
                        .then(|| "reboot issued, but the audit event did not commit".to_string()),
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
    /// events plane. Returns `true` only when that row commits; hook delivery is not
    /// part of this outcome. A store fault is logged and never changes the already
    /// applied reboot result.
    fn audit_reboot(&self, esn: &str) -> bool {
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
        )
    }

    fn load_action_journal(&self) -> io::Result<VehicleActionJournal> {
        if matches!(
            fs::symlink_metadata(&self.action_journal_path),
            Err(error) if error.kind() == io::ErrorKind::NotFound
        ) {
            return Ok(VehicleActionJournal::empty(&self.host));
        }
        let owner_uid = validate_action_journal_parent(&self.action_journal_path)?;
        let file = match open_action_journal(&self.action_journal_path, owner_uid) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(VehicleActionJournal::empty(&self.host));
            }
            Err(error) => return Err(error),
        };
        let mut body = Vec::new();
        file.take(ACTION_JOURNAL_MAX_BYTES + 1)
            .read_to_end(&mut body)?;
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > ACTION_JOURNAL_MAX_BYTES {
            return Err(io::Error::other(
                "vehicle action journal size bound exceeded",
            ));
        }
        let journal: VehicleActionJournal = serde_json::from_slice(&body).map_err(io_other)?;
        validate_action_journal(&journal, &self.host)?;
        Ok(journal)
    }

    fn save_action_journal(&self, journal: &VehicleActionJournal) -> io::Result<()> {
        validate_action_journal(journal, &self.host)?;
        let owner_uid = validate_action_journal_parent(&self.action_journal_path)?;
        match open_action_journal(&self.action_journal_path, owner_uid) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let body = serde_json::to_vec(journal).map_err(io_other)?;
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > ACTION_JOURNAL_MAX_BYTES {
            return Err(io::Error::other(
                "vehicle action journal size bound exceeded",
            ));
        }
        let parent = self
            .action_journal_path
            .parent()
            .ok_or_else(|| io::Error::other("vehicle action journal has no parent"))?;
        let sequence = ACTION_JOURNAL_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".vehicle-action-journal-{}-{sequence}.tmp",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            #[cfg(target_os = "linux")]
            options.custom_flags(ACTION_JOURNAL_NOFOLLOW_FLAG);
        }
        let mut file = options.open(&temp)?;
        let result = (|| {
            file.write_all(&body)?;
            file.sync_all()?;
            validate_action_journal_file(&file, owner_uid)?;
            fs::rename(&temp, &self.action_journal_path)?;
            let persisted = open_action_journal(&self.action_journal_path, owner_uid)?;
            persisted.sync_all()?;
            File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    fn claim_privileged_action(
        &self,
        request_ulid: &str,
        request_topic: &str,
    ) -> io::Result<VehicleActionTxn> {
        let mut journal = self.load_action_journal()?;
        if let Some(existing) = journal
            .entries
            .iter()
            .find(|entry| entry.request_ulid == request_ulid)
        {
            if existing.request_topic != request_topic || existing.verb != "reboot" {
                return Err(io::Error::other(
                    "vehicle action journal request identity collision",
                ));
            }
            return Ok(existing.clone());
        }
        if journal.entries.len() >= ACTION_JOURNAL_MAX_ENTRIES {
            return Err(io::Error::other("vehicle action journal is full"));
        }
        let entry = VehicleActionTxn {
            request_ulid: request_ulid.to_string(),
            request_topic: request_topic.to_string(),
            verb: "reboot".to_string(),
            phase: VehicleActionTxnPhase::Claimed,
            reply: None,
        };
        journal.entries.push(entry.clone());
        self.save_action_journal(&journal)?;
        Ok(entry)
    }

    fn complete_privileged_action(&self, request_ulid: &str, body: &str) -> io::Result<()> {
        if body.len() > ACTION_JOURNAL_MAX_REPLY_BYTES {
            return Err(io::Error::other(
                "vehicle action reply exceeds journal bound",
            ));
        }
        serde_json::from_str::<VehicleReply>(body).map_err(io_other)?;
        let mut journal = self.load_action_journal()?;
        let entry = journal
            .entries
            .iter_mut()
            .find(|entry| entry.request_ulid == request_ulid)
            .ok_or_else(|| io::Error::other("vehicle action journal claim is missing"))?;
        match entry.phase {
            VehicleActionTxnPhase::Claimed => {
                entry.phase = VehicleActionTxnPhase::Completed;
                entry.reply = Some(body.to_string());
            }
            VehicleActionTxnPhase::Completed if entry.reply.as_deref() == Some(body) => {
                return Ok(())
            }
            VehicleActionTxnPhase::Delivered if entry.reply.as_deref() == Some(body) => {
                return Ok(())
            }
            _ => return Err(io::Error::other("vehicle action journal result mismatch")),
        }
        self.save_action_journal(&journal)
    }

    fn deliver_privileged_action(&self, request_ulid: &str, body: &str) -> io::Result<()> {
        let mut journal = self.load_action_journal()?;
        let entry = journal
            .entries
            .iter_mut()
            .find(|entry| entry.request_ulid == request_ulid)
            .ok_or_else(|| io::Error::other("vehicle action journal result is missing"))?;
        if entry.reply.as_deref() != Some(body) {
            return Err(io::Error::other("vehicle action journal delivery mismatch"));
        }
        entry.phase = VehicleActionTxnPhase::Delivered;
        self.save_action_journal(&journal)?;
        journal
            .entries
            .retain(|entry| entry.request_ulid != request_ulid);
        if journal.entries.is_empty() {
            let owner_uid = validate_action_journal_parent(&self.action_journal_path)?;
            open_action_journal(&self.action_journal_path, owner_uid)?;
            fs::remove_file(&self.action_journal_path)?;
            File::open(
                self.action_journal_path
                    .parent()
                    .ok_or_else(|| io::Error::other("vehicle action journal has no parent"))?,
            )?
            .sync_all()
        } else {
            self.save_action_journal(&journal)
        }
    }

    fn indeterminate_reboot_reply() -> io::Result<String> {
        serde_json::to_string(&VehicleReply {
            ok: false,
            verb: "reboot".to_string(),
            gated: Some(
                "privileged reboot outcome is indeterminate after process recovery; the effect was not repeated"
                    .to_string(),
            ),
            error: Some(
                "a durable claim existed without a completed result; inspect the gateway and audit before retrying"
                    .to_string(),
            ),
            ..Default::default()
        })
        .map_err(io_other)
    }

    fn reply_body_exists(
        &self,
        persist: &Persist,
        request_ulid: &str,
        body: &str,
    ) -> io::Result<bool> {
        Ok(persist
            .list_since(&reply_topic(request_ulid), None)
            .map_err(io_other)?
            .iter()
            .any(|message| message.body.as_deref() == Some(body)))
    }

    fn recover_privileged_actions(
        &self,
        root: &Path,
        index: VehicleBusIdentity,
        persist: &Persist,
    ) -> io::Result<bool> {
        let entries = self.load_action_journal()?.entries;
        let mut recovered = false;
        for entry in entries {
            if entry.phase == VehicleActionTxnPhase::Delivered {
                let body = entry.reply.as_deref().ok_or_else(|| {
                    io::Error::other("delivered vehicle action journal reply is missing")
                })?;
                self.deliver_privileged_action(&entry.request_ulid, body)?;
                continue;
            }
            let body = match entry.phase {
                VehicleActionTxnPhase::Claimed => {
                    let body = Self::indeterminate_reboot_reply()?;
                    self.complete_privileged_action(&entry.request_ulid, &body)?;
                    body
                }
                VehicleActionTxnPhase::Completed => entry.reply.ok_or_else(|| {
                    io::Error::other("completed vehicle action journal reply is missing")
                })?,
                VehicleActionTxnPhase::Delivered => unreachable!(),
            };
            if !self.reply_body_exists(persist, &entry.request_ulid, &body)? {
                self.write_reply(persist, &entry.request_ulid, &body)?;
            }
            self.verify_bus_identity(root, index)?;
            self.deliver_privileged_action(&entry.request_ulid, &body)?;
            recovered = true;
        }
        Ok(recovered)
    }

    /// Atomically tail-prime every action lane on a newly observed Bus index.
    /// No activation state changes unless all topic/tail reads and the final
    /// index-stability check succeed.
    fn activate_actions(&self, state: &mut VehicleActionDrainState) -> io::Result<()> {
        let Some((root, index, persist)) = self.open_bus_transaction()? else {
            return Ok(());
        };
        if state.active_index == Some(index) {
            return Ok(());
        }
        let topics = persist.list_topics().map_err(io_other)?;
        let mut staged_cursors = HashMap::new();
        for topic in topics {
            if !topic.starts_with(VEHICLE_ACTION_PREFIX) {
                continue;
            }
            if let Some(ulid) = persist.latest_ulid(&topic).map_err(io_other)? {
                staged_cursors.insert(topic, ulid);
            }
        }
        self.verify_bus_identity(&root, index)?;
        state.active_index = Some(index);
        state.cursors = staged_cursors;
        Ok(())
    }

    /// Drain new transient actions only after complete lane reads. Privileged
    /// reboot claims and exact results cross the process-crash boundary in the
    /// trusted local journal before their corresponding effect/reply boundaries.
    fn drain_actions(&self, state: &mut VehicleActionDrainState) -> io::Result<bool> {
        self.activate_actions(state)?;
        let Some((mut root, mut index, mut persist)) = self.open_bus_transaction()? else {
            return Ok(false);
        };
        if state.active_index != Some(index) {
            self.activate_actions(state)?;
            let Some(reopened) = self.open_bus_transaction()? else {
                return Ok(false);
            };
            (root, index, persist) = reopened;
            if state.active_index != Some(index) {
                return Err(io::Error::other(
                    "vehicle action Bus changed repeatedly during activation",
                ));
            }
        }

        if let Some(pending) = state.pending_reply.clone() {
            if pending.privileged_journal {
                self.complete_privileged_action(&pending.request_ulid, &pending.body)?;
            }
            if !self.reply_body_exists(&persist, &pending.request_ulid, &pending.body)? {
                self.write_reply(&persist, &pending.request_ulid, &pending.body)?;
            }
            self.verify_bus_identity(&root, index)?;
            if pending.privileged_journal {
                self.deliver_privileged_action(&pending.request_ulid, &pending.body)?;
            }
            if pending.source_index == index {
                state
                    .cursors
                    .insert(pending.request_topic, pending.request_ulid);
            }
            state.pending_reply = None;
        }

        let mut acted = self.recover_privileged_actions(&root, index, &persist)?;

        let topics = persist.list_topics().map_err(io_other)?;
        let mut batches = Vec::new();
        for topic in topics {
            let Some(verb_name) = topic.strip_prefix(VEHICLE_ACTION_PREFIX) else {
                continue;
            };
            let verb_name = verb_name.to_string();
            let cursor = state.cursors.get(&topic).map(String::as_str);
            let messages = persist.list_since(&topic, cursor).map_err(io_other)?;
            batches.push((topic, verb_name, messages));
        }
        self.verify_bus_identity(&root, index)?;

        for (topic, verb_name, messages) in batches {
            for msg in messages {
                let body = msg.body.as_deref().unwrap_or("{}");
                let privileged_journal = verb_name == "reboot";
                let reply_body = if privileged_journal {
                    let claimed = self.claim_privileged_action(&msg.ulid, &topic)?;
                    match claimed.phase {
                        VehicleActionTxnPhase::Claimed => {
                            let reply = self.handle(&verb_name, body);
                            serde_json::to_string(&reply).map_err(io_other)?
                        }
                        VehicleActionTxnPhase::Completed | VehicleActionTxnPhase::Delivered => {
                            claimed.reply.ok_or_else(|| {
                                io::Error::other("vehicle action journal reply is missing")
                            })?
                        }
                    }
                } else {
                    serde_json::to_string(&self.handle(&verb_name, body)).map_err(io_other)?
                };
                let reply: VehicleReply = serde_json::from_str(&reply_body).map_err(io_other)?;
                tracing::info!(
                    target: "mackesd::vehicle",
                    ulid = %msg.ulid, verb = %verb_name, ok = reply.ok,
                    audited = reply.audited, "vehicle action handled"
                );
                state.pending_reply = Some(PendingVehicleReply {
                    source_index: index,
                    request_topic: topic.clone(),
                    request_ulid: msg.ulid.clone(),
                    body: reply_body,
                    privileged_journal,
                });
                let pending = state.pending_reply.as_ref().expect("pending reply staged");
                if pending.privileged_journal {
                    self.complete_privileged_action(&pending.request_ulid, &pending.body)?;
                }
                if !self.reply_body_exists(&persist, &pending.request_ulid, &pending.body)? {
                    self.write_reply(&persist, &pending.request_ulid, &pending.body)?;
                }
                self.verify_bus_identity(&root, index)?;
                if pending.privileged_journal {
                    self.deliver_privileged_action(&pending.request_ulid, &pending.body)?;
                }
                state.cursors.insert(topic.clone(), msg.ulid);
                state.pending_reply = None;
                acted = true;
            }
        }
        Ok(acted)
    }

    fn write_reply(&self, persist: &Persist, req_ulid: &str, body: &str) -> io::Result<()> {
        if self
            .reply_failures
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(io::Error::other("injected vehicle reply write failure"));
        }
        persist
            .write(&reply_topic(req_ulid), Priority::Default, None, Some(body))
            .map(|_| ())
            .map_err(io_other)
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
        let roster_plan = VehiclePollPlan {
            poll: self.poll,
            enrichment: ENRICHMENT_POLL,
            heartbeat: self.heartbeat,
        };
        let mut roster = VehicleRuntimeRoster::from_env(&self.host, Instant::now(), roster_plan)
            .map_err(|error| anyhow::anyhow!("invalid vehicle roster configuration: {error}"))?;
        let mut action_state = VehicleActionDrainState::default();
        // Until a poll confirms the configured MG90 identity, the roster has no
        // accepted source and publishes no v2 manager claim.
        let mut runtime = VehicleRuntimeSnapshot::pending(&self.host);
        let mut cached = runtime.render();
        let mut startup_retry = BUS_RETRY_MIN;
        loop {
            let activated = self.activate_actions(&mut action_state);
            let published = activated
                .as_ref()
                .map_err(|error| io::Error::other(error.to_string()))
                .and_then(|()| self.publish_legacy_unavailable(&cached));
            match published {
                Ok(()) => break,
                Err(error) => tracing::warn!(
                    target: "mackesd::vehicle",
                    host = %self.host,
                    %error,
                    "vehicle Bus activation/publication deferred"
                ),
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(startup_retry) => {}
            }
            startup_retry = startup_retry.saturating_mul(2).min(FAILURE_RETRY_MAX);
        }
        if let Err(error) = self.drain_actions(&mut action_state) {
            tracing::warn!(target: "mackesd::vehicle", %error, "vehicle action drain deferred");
        }
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
        let mut pending_commit: Option<VehiclePendingCommit> = None;
        loop {
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                _ = current_tick.tick() => {
                    if let Err(error) = self.drain_actions(&mut action_state) {
                        tracing::warn!(target: "mackesd::vehicle", %error, "vehicle action drain deferred");
                    }
                    if let Some(mut pending) = pending_commit.take() {
                        match self.publish_pending_commit(&mut pending) {
                            Ok(()) => {
                                let kind = pending.kind;
                                runtime = pending.runtime;
                                cached = pending.cached;
                                roster = pending.roster;
                                match kind {
                                    VehiclePendingCommitKind::Current { healthy, was_online } => {
                                        if !was_online && runtime.online && enrichment_task.is_none() {
                                            enrichment_task = Some(self.spawn_enrichment(probe.clone()));
                                            enrichment_deadline = Some(Box::pin(tokio::time::sleep(ENRICHMENT_TIMEOUT)));
                                            enrichment_timed_out = false;
                                        }
                                        if healthy {
                                            current_retry = self.poll;
                                            current_not_before = None;
                                        } else {
                                            current_not_before = Some(tokio::time::Instant::now() + current_retry);
                                            current_retry = current_retry.saturating_mul(2).min(FAILURE_RETRY_MAX);
                                        }
                                    }
                                    VehiclePendingCommitKind::Enrichment => {}
                                }
                            }
                            Err(error) => {
                                tracing::warn!(target: "mackesd::vehicle", %error, "vehicle state publication retry deferred");
                                pending_commit = Some(pending);
                            }
                        }
                    }
                    if pending_commit.is_none() {
                        if let Err(error) = roster.ingest_remote(self, Instant::now()) {
                            tracing::warn!(target: "mackesd::vehicle", %error, "remote vehicle roster read deferred");
                        }
                    }
                    let retry_ready = current_not_before
                        .map_or(true, |not_before| tokio::time::Instant::now() >= not_before);
                    let poll_due = pending_commit.is_none()
                        && roster.local_due(Instant::now(), VehicleScheduleKind::CurrentStatus);
                    if pending_commit.is_none() && current_task.is_none() && retry_ready && poll_due {
                        current_not_before = None;
                        current_task = Some(self.spawn_current_status(probe.clone()));
                        current_deadline =
                            Some(Box::pin(tokio::time::sleep(self.current_timeout)));
                        current_timed_out = false;
                    }
                }
                _ = enrichment_tick.tick() => {
                    let enrichment_due = pending_commit.is_none()
                        && runtime.online
                        && roster.local_due(Instant::now(), VehicleScheduleKind::Enrichment);
                    if pending_commit.is_none() && runtime.online && enrichment_task.is_none() && enrichment_due {
                        enrichment_task = Some(self.spawn_enrichment(probe.clone()));
                        enrichment_deadline = Some(Box::pin(tokio::time::sleep(ENRICHMENT_TIMEOUT)));
                        enrichment_timed_out = false;
                    }
                }
                _ = heartbeat_tick.tick() => {
                    if pending_commit.is_none() {
                        if let Err(error) = roster.ingest_remote(self, Instant::now()) {
                            tracing::warn!(target: "mackesd::vehicle", %error, "remote vehicle roster read deferred");
                        }
                        let mut staged_roster = roster.clone();
                        let heartbeat_due = staged_roster
                            .local_due(Instant::now(), VehicleScheduleKind::Heartbeat);
                        let publication = if heartbeat_due {
                            self.publish_roster_updates(&mut staged_roster, &cached, Instant::now())
                        } else {
                            Ok(())
                        }
                        .and_then(|()| {
                            if !cached.online {
                                self.publish_legacy_unavailable(&cached)
                            } else {
                                Ok(())
                            }
                        });
                        match publication {
                            Ok(()) => roster = staged_roster,
                            Err(error) => tracing::warn!(target: "mackesd::vehicle", %error, "vehicle heartbeat publication deferred"),
                        }
                    }
                }
                result = async {
                    current_task.as_mut().expect("guarded current-status task").await
                }, if current_task.is_some() && pending_commit.is_none() => {
                    current_task = None;
                    current_deadline = None;
                    if current_timed_out {
                        current_timed_out = false;
                    } else {
                        let healthy = result.as_ref().is_ok_and(|current| current.online);
                        let was_online = runtime.online;
                        let mut staged_runtime = runtime.clone();
                        let mut staged_roster = roster.clone();
                        match result {
                            Ok(current) => staged_runtime.apply_current(current),
                            Err(error) => staged_runtime.mark_current_unavailable(
                                &format!("task failed: {error}")
                            ),
                        }
                        let next = staged_runtime.render();
                        let changed = !vehicle_state_content_eq(&cached, &next);
                        let roster_ready = if healthy {
                            if let Err(error) = staged_roster.ingest_local(self, &next, Instant::now()) {
                                tracing::warn!(
                                    target: "mackesd::vehicle",
                                    %error,
                                    "local vehicle snapshot rejected by runtime roster"
                                );
                                false
                            } else {
                                true
                            }
                        } else {
                            staged_roster.mark_local_unavailable();
                            true
                        };
                        if roster_ready {
                            let mut pending = VehiclePendingCommit {
                                runtime: staged_runtime,
                                cached: next,
                                roster: staged_roster,
                                kind: VehiclePendingCommitKind::Current { healthy, was_online },
                                publish_roster: changed || healthy,
                                publish_unavailable: (changed || healthy) && !healthy,
                            };
                            match self.publish_pending_commit(&mut pending) {
                                Ok(()) => {
                                    runtime = pending.runtime;
                                    cached = pending.cached;
                                    roster = pending.roster;
                                    if !was_online && runtime.online && enrichment_task.is_none() {
                                        enrichment_task = Some(self.spawn_enrichment(probe.clone()));
                                        enrichment_deadline = Some(Box::pin(tokio::time::sleep(ENRICHMENT_TIMEOUT)));
                                        enrichment_timed_out = false;
                                    }
                                    if healthy {
                                        current_retry = self.poll;
                                        current_not_before = None;
                                    } else {
                                        current_not_before = Some(tokio::time::Instant::now() + current_retry);
                                        current_retry = current_retry.saturating_mul(2).min(FAILURE_RETRY_MAX);
                                    }
                                }
                                Err(error) => {
                                    tracing::warn!(target: "mackesd::vehicle", %error, "vehicle current-state publication deferred");
                                    pending_commit = Some(pending);
                                }
                            }
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
                }, if current_deadline.is_some() && pending_commit.is_none() => {
                    current_deadline = None;
                    current_timed_out = true;
                    let was_online = runtime.online;
                    let mut staged_runtime = runtime.clone();
                    staged_runtime.mark_current_unavailable("current-status timeout");
                    let next = staged_runtime.render();
                    let mut staged_roster = roster.clone();
                    staged_roster.mark_local_unavailable();
                    let mut pending = VehiclePendingCommit {
                        runtime: staged_runtime,
                        cached: next,
                        roster: staged_roster,
                        kind: VehiclePendingCommitKind::Current {
                            healthy: false,
                            was_online,
                        },
                        publish_roster: true,
                        publish_unavailable: true,
                    };
                    match self.publish_pending_commit(&mut pending) {
                        Ok(()) => {
                            runtime = pending.runtime;
                            cached = pending.cached;
                            roster = pending.roster;
                            current_not_before = Some(tokio::time::Instant::now() + current_retry);
                            current_retry = current_retry.saturating_mul(2).min(FAILURE_RETRY_MAX);
                        }
                        Err(error) => {
                            tracing::warn!(target: "mackesd::vehicle", %error, "vehicle timeout publication deferred");
                            pending_commit = Some(pending);
                        }
                    }
                }
                result = async {
                    enrichment_task.as_mut().expect("guarded enrichment task").await
                }, if enrichment_task.is_some() && pending_commit.is_none() => {
                    enrichment_task = None;
                    enrichment_deadline = None;
                    if enrichment_timed_out {
                        enrichment_timed_out = false;
                    } else {
                        let mut staged_runtime = runtime.clone();
                        let mut staged_roster = roster.clone();
                        staged_roster.finish_local_enrichment();
                        match result {
                            Ok(enrichment) => staged_runtime.apply_enrichment(enrichment),
                            Err(error) => staged_runtime.mark_enrichment_unavailable(
                                &format!("task failed: {error}")
                            ),
                        }
                        let next = staged_runtime.render();
                        let changed = !vehicle_state_content_eq(&cached, &next);
                        let roster_ready = if staged_runtime.online {
                            if let Err(error) = staged_roster.ingest_local(self, &next, Instant::now()) {
                                tracing::warn!(
                                    target: "mackesd::vehicle",
                                    %error,
                                    "enriched vehicle snapshot rejected by runtime roster"
                                );
                                false
                            } else {
                                true
                            }
                        } else {
                            true
                        };
                        if roster_ready {
                            let mut pending = VehiclePendingCommit {
                                runtime: staged_runtime,
                                cached: next,
                                roster: staged_roster,
                                kind: VehiclePendingCommitKind::Enrichment,
                                publish_roster: changed,
                                publish_unavailable: false,
                            };
                            match self.publish_pending_commit(&mut pending) {
                                Ok(()) => {
                                    runtime = pending.runtime;
                                    cached = pending.cached;
                                    roster = pending.roster;
                                }
                                Err(error) => {
                                    tracing::warn!(target: "mackesd::vehicle", %error, "vehicle enrichment publication deferred");
                                    pending_commit = Some(pending);
                                }
                            }
                        }
                    }
                }
                () = async {
                    enrichment_deadline
                        .as_mut()
                        .expect("guarded enrichment deadline")
                        .as_mut()
                        .await
                }, if enrichment_deadline.is_some() && pending_commit.is_none() => {
                    enrichment_deadline = None;
                    enrichment_timed_out = true;
                    let mut staged_runtime = runtime.clone();
                    let mut staged_roster = roster.clone();
                    staged_roster.finish_local_enrichment();
                    staged_runtime.mark_enrichment_unavailable("enrichment timeout");
                    let next = staged_runtime.render();
                    let changed = !vehicle_state_content_eq(&cached, &next);
                    if changed {
                        let roster_ready = if staged_runtime.online {
                            if let Err(error) = staged_roster.ingest_local(self, &next, Instant::now()) {
                                tracing::warn!(
                                    target: "mackesd::vehicle",
                                    %error,
                                    "vehicle timeout snapshot rejected by runtime roster"
                                );
                                false
                            } else {
                                true
                            }
                        } else {
                            true
                        };
                        if roster_ready {
                            let mut pending = VehiclePendingCommit {
                                runtime: staged_runtime,
                                cached: next,
                                roster: staged_roster,
                                kind: VehiclePendingCommitKind::Enrichment,
                                publish_roster: true,
                                publish_unavailable: false,
                            };
                            match self.publish_pending_commit(&mut pending) {
                                Ok(()) => {
                                    runtime = pending.runtime;
                                    cached = pending.cached;
                                    roster = pending.roster;
                                }
                                Err(error) => {
                                    tracing::warn!(target: "mackesd::vehicle", %error, "vehicle enrichment-timeout publication deferred");
                                    pending_commit = Some(pending);
                                }
                            }
                        }
                    } else {
                        runtime = staged_runtime;
                        roster = staged_roster;
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
    let parsed_gps = nmea
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
    (parsed_gps, imu)
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

    #[derive(Clone)]
    struct SwitchableCurrentProbe {
        inner: FakeProbe,
        available: Arc<std::sync::atomic::AtomicBool>,
    }

    impl SwitchableCurrentProbe {
        fn new() -> Self {
            Self {
                inner: FakeProbe::real(),
                available: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            }
        }

        fn set_available(&self, available: bool) {
            self.available
                .store(available, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl VehicleProbe for SwitchableCurrentProbe {
        fn read_gps_nmea(&self) -> io::Result<String> {
            self.inner.read_gps_nmea()
        }

        fn read_lci_general(&self) -> io::Result<String> {
            if self.available.load(std::sync::atomic::Ordering::SeqCst) {
                self.inner.read_lci_general()
            } else {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "scripted manager loss",
                ))
            }
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
    fn replaced_mg90_source_cannot_merge_enrichment_into_retained_generation() {
        let current = VehicleWorker::probe_current_status("rig-1", &FakeProbe::real());
        let mut runtime = VehicleRuntimeSnapshot::from_current("rig-1", current);
        runtime.apply_enrichment(VehicleWorker::probe_enrichment(&FakeProbe::real()));
        let retained_gps = runtime.enrichment_gps.clone();
        let retained_wan = runtime.wan.clone();

        // The configured endpoint now resolves to a different physical MG90.
        // Its GNSS and WAN payloads are individually valid and would otherwise
        // be folded beneath the first gateway's retained ESN.
        let mut replacement = FakeProbe::real();
        replacement.general = replacement
            .general
            .map(|html| html.replace("ND84720078011035", "ND84720078011999"));
        replacement.nmea = replacement
            .nmea
            .map(|nmea| nmea.replace("3210.07993", "4010.07993"));
        replacement.wan = replacement
            .wan
            .map(|wan| wan.replace("CellularA", "WiFi"));

        runtime.apply_enrichment(VehicleWorker::probe_enrichment(&replacement));
        let refused = runtime.render();

        assert_eq!(
            replacement.general_calls(),
            2,
            "the slow batch is identity-bracketed"
        );
        assert_eq!(runtime.enrichment_gps, retained_gps);
        assert_eq!(runtime.wan, retained_wan);
        assert_eq!(refused.esn, "ND84720078011035");
        assert!(refused.gaps.iter().any(|gap| {
            gap.contains("MG90 source identity changed or was unavailable during enrichment")
        }));
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
    fn failed_radio_refresh_cannot_republish_retained_link_as_live() {
        let worker = worker();
        let current = VehicleWorker::probe_current_status("rig-1", &FakeProbe::real());
        let mut runtime = VehicleRuntimeSnapshot::from_current("rig-1", current);
        runtime.apply_enrichment(VehicleWorker::probe_enrichment(&FakeProbe::real()));

        let fresh = worker.snapshot_v2(&runtime.render());
        let fresh_cellular = fresh
            .radios
            .by_id(&RadioId::CellularA)
            .expect("native cellular row");
        assert_eq!(fresh_cellular.operation, RadioOperation::Active);
        assert!(fresh_cellular.active_path);
        assert_eq!(fresh.freshness.radios.state, FreshnessState::Fresh);

        // The gateway anchor remains reachable, but the authoritative WAN
        // refresh fails. Retained diagnostics must not be stamped as a new live
        // radio observation by the next v2 publication.
        let failed = FakeProbe {
            wan: Err("hostile replay after radio timeout".to_string()),
            ..FakeProbe::real()
        };
        runtime.apply_enrichment(VehicleWorker::probe_enrichment(&failed));
        let stale = worker.snapshot_v2(&runtime.render());
        let stale_cellular = stale
            .radios
            .by_id(&RadioId::CellularA)
            .expect("retained cellular row");

        assert_eq!(stale_cellular.operation, RadioOperation::Stale);
        assert!(!stale_cellular.active_path);
        assert_eq!(stale_cellular.age_ms, None);
        assert_eq!(stale.freshness.radios.state, FreshnessState::Stale);
        assert_eq!(
            stale.freshness.radios.reason.as_deref(),
            Some("wan-probe-unavailable-retained")
        );
        assert_eq!(
            stale_cellular.metrics, fresh_cellular.metrics,
            "retained diagnostics remain visible without claiming fresh health"
        );

        // The compatibility mapper projects the single legacy Wi-Fi WAN label
        // onto both native Wi-Fi slots, including an otherwise unknown B row.
        // A failed refresh must revoke that active-path bit too; operation/age
        // alone are insufficient evidence that a row carries retained truth.
        let wifi_probe = FakeProbe {
            wan: Ok(FakeProbe::real()
                .wan
                .expect("real WAN fixture")
                .replace("CellularA", "WiFi")
                .replace("Disabled", "Active")),
            ..FakeProbe::real()
        };
        let current = VehicleWorker::probe_current_status("rig-1", &wifi_probe);
        let mut wifi_runtime = VehicleRuntimeSnapshot::from_current("rig-1", current);
        wifi_runtime.apply_enrichment(VehicleWorker::probe_enrichment(&wifi_probe));
        let fresh_wifi = worker.snapshot_v2(&wifi_runtime.render());
        assert!(
            fresh_wifi
                .radios
                .by_id(&RadioId::WifiB)
                .expect("native Wi-Fi B row")
                .active_path,
            "hostile compatibility row must exercise active-path-only retention"
        );

        wifi_runtime.apply_enrichment(VehicleWorker::probe_enrichment(&failed));
        let stale_wifi = worker.snapshot_v2(&wifi_runtime.render());
        for id in [RadioId::WifiA, RadioId::WifiB] {
            let radio = stale_wifi.radios.by_id(&id).expect("native Wi-Fi row");
            assert_eq!(radio.operation, RadioOperation::Stale);
            assert!(
                !radio.active_path,
                "{} retained an active-path claim",
                id.as_str()
            );
            assert_eq!(radio.age_ms, None);
        }
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

    #[test]
    fn vehicle_curl_uses_bounded_subprocess_capture() {
        let production = include_str!("vehicle.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source");
        assert!(production.contains("output_with_timeout"));
        assert!(production.contains("DEFAULT_CMD_TIMEOUT"));
        assert!(!production.contains(".args(args)\n            .output()?"));
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
        assert!(
            initial_phase_for("seat-15", Duration::from_millis(10)) <= Duration::from_millis(10)
        );
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
    async fn worker_runtime_roster_stops_claims_on_loss_and_reconnects_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let probe = Arc::new(SwitchableCurrentProbe::new());
        let mut worker = worker()
            .with_bus_root(Some(tmp.path().to_path_buf()))
            .with_probe(probe.clone())
            .with_poll(Duration::from_millis(10))
            .with_heartbeat(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let handle =
            tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        let topic = vehicle_state_v2_topic("rig-1", "ND84720078011035");

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !persist.list_since(&topic, None).unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("reachable manager must publish through the runtime roster");
        let first: VehicleStateV2 = serde_json::from_str(
            persist.list_since(&topic, None).unwrap()[0]
                .body
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(first.approval, ApprovalState::Approved);
        assert_eq!(
            first.managers,
            ManagerSet::approved(vec!["rig-1".to_string()]).unwrap()
        );

        probe.set_available(false);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let legacy = persist
                    .list_since(&vehicle_state_topic("rig-1"), None)
                    .unwrap();
                let lost = legacy.last().is_some_and(|message| {
                    serde_json::from_str::<VehicleState>(message.body.as_deref().unwrap())
                        .is_ok_and(|state| !state.online)
                });
                if lost {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("manager loss must become explicit on the legacy availability lane");
        let count_after_loss = persist.list_since(&topic, None).unwrap().len();
        tokio::time::sleep(Duration::from_millis(35)).await;
        assert_eq!(
            persist.list_since(&topic, None).unwrap().len(),
            count_after_loss,
            "a lost manager must stop identity-bound v2 heartbeats"
        );

        probe.set_available(true);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if persist.list_since(&topic, None).unwrap().len() > count_after_loss {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("reconnected manager must resume with a new Changed epoch");

        tx.send(true).expect("signal shutdown");
        assert!(tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("worker shutdown")
            .expect("worker join")
            .is_ok());
    }

    #[test]
    fn worker_runtime_roster_never_republishes_or_impersonates_remote_manager() {
        let tmp = tempfile::tempdir().unwrap();
        let worker = worker().with_bus_root(Some(tmp.path().to_path_buf()));
        let source = roster_source_id();
        let t0 = Instant::now();
        let mut runtime = VehicleRuntimeRoster::new(
            "rig-1",
            Some(source.clone()),
            "manager-b",
            t0,
            VehiclePollPlan::default(),
        )
        .unwrap();
        let local = worker.build_state(&FakeProbe::real());
        runtime.ingest_local(&worker, &local, t0).unwrap();
        worker
            .publish_roster_updates(&mut runtime, &local, t0)
            .unwrap();

        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        let local_topic = vehicle_state_v2_topic("rig-1", source.as_str());
        assert_eq!(persist.list_since(&local_topic, None).unwrap().len(), 1);

        let mut remote = worker.build_state_v2(&FakeProbe::real());
        remote.management_node_id = "manager-b".to_string();
        remote.approval = ApprovalState::Approved;
        remote.managers =
            ManagerSet::approved(vec!["rig-1".to_string(), "manager-b".to_string()]).unwrap();
        remote.observed_at_ms = local.published_at_ms.saturating_add(1_000);
        remote.published_at_ms = remote.observed_at_ms;
        let remote_topic = vehicle_state_v2_topic("manager-b", source.as_str());
        persist
            .write(
                &remote_topic,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&remote).unwrap()),
            )
            .unwrap();
        runtime.ingest_remote(&worker, t0).unwrap();
        worker
            .publish_roster_updates(&mut runtime, &local, t0)
            .unwrap();
        assert_eq!(
            persist.list_since(&remote_topic, None).unwrap().len(),
            1,
            "selected remote state must not be republished under the remote identity"
        );
        assert_eq!(
            persist.list_since(&local_topic, None).unwrap().len(),
            1,
            "fresh approved remote selection suppresses the local competing claim"
        );

        runtime
            .ingest_local(&worker, &local, t0 + Duration::from_secs(5))
            .unwrap();
        worker
            .publish_roster_updates(&mut runtime, &local, t0 + Duration::from_secs(7))
            .unwrap();
        assert_eq!(
            persist.list_since(&local_topic, None).unwrap().len(),
            2,
            "remote loss selects the still-live local manager as a new Changed epoch"
        );
        assert_eq!(persist.list_since(&remote_topic, None).unwrap().len(), 1);
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
        snapshot.approval = ApprovalState::Approved;
        snapshot.managers = ManagerSet::approved(vec![manager_id.to_string()]).unwrap();
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
    fn expiring_one_manager_preserves_live_source_publication_epoch() {
        let source = roster_source_id();
        let plan = VehiclePollPlan::new(Duration::from_secs(5), ROSTER_HEARTBEAT).unwrap();
        let t0 = Instant::now();
        let mut roster = VehicleRoster::new(t0);
        for manager in ["manager-a", "manager-b"] {
            roster
                .register(VehicleRosterSource::remote(source.clone(), manager, plan).unwrap())
                .unwrap();
        }

        roster
            .ingest_at(
                roster_snapshot(&source, "manager-a", 100, 100, 1),
                t0,
            )
            .unwrap();
        roster
            .ingest_at(
                roster_snapshot(&source, "manager-b", 100, 100, 1),
                t0,
            )
            .unwrap();
        assert_eq!(roster.take_publications(t0).len(), 1);

        // Manager A refreshed recently; manager B stopped delivering. At the
        // source level this is a manager failover condition, not MG90 loss.
        roster
            .ingest_at(
                roster_snapshot(&source, "manager-a", 200, 200, 2),
                t0 + Duration::from_secs(14),
            )
            .unwrap();
        // Drain the legitimate content change before checking the expiry
        // path; otherwise the assertion would observe manager A's new
        // snapshot instead of the manager-B failover behavior.
        assert_eq!(
            roster.take_publications(t0 + Duration::from_secs(14)).len(),
            1
        );
        let publications = roster.take_publications(t0 + Duration::from_secs(20));
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].manager_id, "manager-a");
        assert_eq!(
            publications[0].reason,
            VehiclePublicationReason::Heartbeat,
            "manager expiry must not manufacture a source Changed epoch"
        );
    }

    #[test]
    fn marking_non_selected_manager_unavailable_preserves_source_publication_epoch() {
        let source = roster_source_id();
        let plan = VehiclePollPlan::new(Duration::from_secs(5), ROSTER_HEARTBEAT).unwrap();
        let t0 = Instant::now();
        let mut roster = VehicleRoster::new(t0);
        for manager in ["manager-a", "manager-b"] {
            roster
                .register(VehicleRosterSource::remote(source.clone(), manager, plan).unwrap())
                .unwrap();
        }

        roster
            .ingest_at(
                roster_snapshot(&source, "manager-a", 200, 200, 2),
                t0,
            )
            .unwrap();
        roster
            .ingest_at(
                roster_snapshot(&source, "manager-b", 100, 100, 1),
                t0,
            )
            .unwrap();
        let initial = roster.take_publications(t0);
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].manager_id, "manager-a");

        roster
            .mark_unavailable(&source, "manager-b")
            .expect("registered manager can be marked unavailable");
        assert!(
            roster.take_publications(t0 + Duration::from_secs(1)).is_empty(),
            "losing a non-selected manager must not manufacture a source change"
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
    fn roster_requires_explicit_approval_even_with_complete_manager_set() {
        for approval in [ApprovalState::Pending, ApprovalState::Unknown] {
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
            let mut snapshot = roster_snapshot(&source, "manager-a", 350, 350, 3);
            snapshot.snapshot.approval = approval;
            roster.ingest(snapshot).unwrap();

            assert_eq!(
                roster.route_latest(&source),
                VehicleManagerRouteSelection::Rejected {
                    source_id: source,
                    manager_id: "manager-a".to_string(),
                    reason: VehicleManagerRouteRejection::ApprovalNotApproved { state: approval },
                }
            );
        }
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

        let mut snapshot = roster_snapshot(&source, "manager-a", 400, 400, 4);
        snapshot.snapshot.managers = ManagerSet::default();
        roster.ingest(snapshot).unwrap();

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
    fn successful_reboot_reports_audited_only_after_event_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let auth_tmp = tempfile::tempdir().unwrap();
        let fake = FakeProbe::real();
        let db_path = tmp.path().join("events.sqlite");
        let w = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_db_path(db_path.clone())
            .with_authorizer(test_authorizer(auth_tmp.path()));
        let body = authorized_reboot_body("vehicle-correct-name", "ND84720078011035");
        // The FakeProbe general.html reports ESN ND84720078011035.
        let reply = w.handle("reboot", &body);
        assert!(reply.ok, "gated: {:?} err: {:?}", reply.gated, reply.error);
        assert!(reply.audited, "the committed audit row is reported");
        assert!(reply.error.is_none());
        assert_eq!(reply.applied.as_deref(), Some("reboot issued"));
        assert_eq!(fake.ssh_calls().as_slice(), &["reboot"]);

        let conn = crate::store::open(&db_path).expect("reopen committed audit store");
        let (kind, actor, payload): (String, String, String) = conn
            .query_row(
                "SELECT kind, actor, payload_json FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("committed vehicle reboot audit row");
        assert_eq!(kind, "admin_action");
        assert_eq!(actor, "peer:rig-1");
        let event: crate::events::Event = serde_json::from_str(&payload).expect("typed event");
        assert_eq!(event.kind, crate::events::EventKind::AdminAction);
        assert_eq!(event.detail["action"], "vehicle");
        assert_eq!(event.detail["verb"], "reboot");
        assert_eq!(event.detail["esn"], "ND84720078011035");
    }

    #[test]
    fn successful_reboot_with_forced_audit_store_failure_is_not_fabricated() {
        let tmp = tempfile::tempdir().unwrap();
        let auth_tmp = tempfile::tempdir().unwrap();
        let blocked_parent = tmp.path().join("not-a-directory");
        std::fs::write(&blocked_parent, b"hostile audit parent").unwrap();
        let db_path = blocked_parent.join("events.sqlite");
        let fake = FakeProbe::real();
        let w = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_db_path(db_path.clone())
            .with_authorizer(test_authorizer(auth_tmp.path()));
        let body = authorized_reboot_body("vehicle-audit-failure", "ND84720078011035");

        let reply = w.handle("reboot", &body);

        assert!(reply.ok, "the SSH reboot succeeded: {reply:?}");
        assert_eq!(reply.applied.as_deref(), Some("reboot issued"));
        assert!(!reply.audited, "a failed store open cannot fabricate audit");
        assert_eq!(
            reply.error.as_deref(),
            Some("reboot issued, but the audit event did not commit")
        );
        assert_eq!(fake.ssh_calls().as_slice(), &["reboot"]);
        assert!(!db_path.exists(), "the hostile store never materialized");
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
        let w = worker()
            .with_probe(Arc::new(FakeProbe::real()))
            .with_bus_root(Some(bus.clone()));
        let mut state = VehicleActionDrainState::default();
        w.activate_actions(&mut state).unwrap();
        let req = persist
            .write(
                "action/vehicle/get-config",
                Priority::Default,
                None,
                Some(r#"{"file":"wan.yaml"}"#),
            )
            .unwrap();
        assert!(
            w.drain_actions(&mut state).unwrap(),
            "the gateway node acted"
        );
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
        let mut state = VehicleActionDrainState::default();
        w.activate_actions(&mut state).unwrap();
        assert!(
            !w.drain_actions(&mut state).unwrap(),
            "the backlog is not replayed after prime"
        );
    }

    #[test]
    fn action_activation_recovers_late_and_replaced_bus_without_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().join("bus");
        std::fs::write(&bus, "temporarily unavailable").unwrap();
        let fake = FakeProbe::real();
        let w = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_bus_root(Some(bus.clone()));
        let mut state = VehicleActionDrainState::default();

        assert!(w.activate_actions(&mut state).is_err());
        assert!(state.active_index.is_none());
        assert!(state.cursors.is_empty());

        std::fs::remove_file(&bus).unwrap();
        let first = Persist::open(bus.clone()).unwrap();
        first
            .write(
                "action/vehicle/get-config",
                Priority::Default,
                None,
                Some(r#"{"file":"retained.yaml"}"#),
            )
            .unwrap();
        w.activate_actions(&mut state).unwrap();
        assert!(!w.drain_actions(&mut state).unwrap());
        assert!(fake.ssh_calls().is_empty(), "retained command was skipped");

        first
            .write(
                "action/vehicle/get-config",
                Priority::Default,
                None,
                Some(r#"{"file":"forward.yaml"}"#),
            )
            .unwrap();
        assert!(w.drain_actions(&mut state).unwrap());
        assert_eq!(
            fake.ssh_calls().as_slice(),
            &["omgconf latest forward.yaml"]
        );
        drop(first);

        let replacement = tmp.path().join("replacement");
        let replacement_bus = Persist::open(replacement.clone()).unwrap();
        replacement_bus
            .write(
                "action/vehicle/get-config",
                Priority::Default,
                None,
                Some(r#"{"file":"replacement-retained.yaml"}"#),
            )
            .unwrap();
        drop(replacement_bus);
        std::fs::rename(replacement.join("index.sqlite"), bus.join("index.sqlite")).unwrap();

        assert!(!w.drain_actions(&mut state).unwrap());
        assert_eq!(
            fake.ssh_calls().as_slice(),
            &["omgconf latest forward.yaml"],
            "replacement retained command was skipped"
        );
        let replacement_bus = Persist::open(bus.clone()).unwrap();
        replacement_bus
            .write(
                "action/vehicle/get-config",
                Priority::Default,
                None,
                Some(r#"{"file":"replacement-forward.yaml"}"#),
            )
            .unwrap();
        assert!(w.drain_actions(&mut state).unwrap());
        assert_eq!(
            fake.ssh_calls().as_slice(),
            &[
                "omgconf latest forward.yaml",
                "omgconf latest replacement-forward.yaml",
            ]
        );
    }

    #[test]
    fn reboot_reply_failure_retries_result_without_repeating_effect_or_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let auth_tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().join("bus");
        let db_path = tmp.path().join("events.sqlite");
        let persist = Persist::open(bus.clone()).unwrap();
        let fake = FakeProbe::real();
        let w = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_bus_root(Some(bus.clone()))
            .with_db_path(db_path.clone())
            .with_authorizer(test_authorizer(auth_tmp.path()))
            .with_reply_failures(1);
        let mut state = VehicleActionDrainState::default();
        w.activate_actions(&mut state).unwrap();
        let request = persist
            .write(
                "action/vehicle/reboot",
                Priority::Default,
                None,
                Some(&authorized_reboot_body(
                    "vehicle-reply-retry",
                    "ND84720078011035",
                )),
            )
            .unwrap();

        assert!(w.drain_actions(&mut state).is_err());
        assert!(state.pending_reply.is_some());
        assert_eq!(fake.ssh_calls().as_slice(), &["reboot"]);
        assert!(!w.drain_actions(&mut state).unwrap());
        assert!(state.pending_reply.is_none());
        assert_eq!(
            fake.ssh_calls().as_slice(),
            &["reboot"],
            "reply retry must not repeat the privileged effect"
        );
        let replies = persist
            .list_since(&reply_topic(&request.ulid), None)
            .unwrap();
        assert_eq!(replies.len(), 1);
        let reply: VehicleReply =
            serde_json::from_str(replies[0].body.as_deref().unwrap()).unwrap();
        assert!(reply.ok);
        assert!(reply.audited);
        let conn = crate::store::open(&db_path).unwrap();
        let audit_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind = 'admin_action'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(audit_count, 1, "audit truth is not duplicated on retry");
    }

    #[test]
    fn completed_reboot_journal_survives_worker_restart_without_repeating_effect_or_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let auth_tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().join("bus");
        let db_path = tmp.path().join("events.sqlite");
        let persist = Persist::open(bus.clone()).unwrap();
        let fake = FakeProbe::real();
        let first = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_bus_root(Some(bus.clone()))
            .with_db_path(db_path.clone())
            .with_authorizer(test_authorizer(auth_tmp.path()))
            .with_reply_failures(1);
        let mut first_state = VehicleActionDrainState::default();
        first.activate_actions(&mut first_state).unwrap();
        let request = persist
            .write(
                "action/vehicle/reboot",
                Priority::Default,
                None,
                Some(&authorized_reboot_body(
                    "vehicle-crash-result",
                    "ND84720078011035",
                )),
            )
            .unwrap();

        assert!(first.drain_actions(&mut first_state).is_err());
        assert_eq!(fake.ssh_calls().as_slice(), &["reboot"]);
        assert!(first.action_journal_path.is_file());
        drop(first);
        drop(first_state);
        drop(persist);

        let replacement = tmp.path().join("replacement");
        drop(Persist::open(replacement.clone()).unwrap());
        fs::rename(replacement.join("index.sqlite"), bus.join("index.sqlite")).unwrap();

        let restarted = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_bus_root(Some(bus.clone()))
            .with_db_path(db_path.clone())
            .with_authorizer(test_authorizer(auth_tmp.path()));
        let mut restarted_state = VehicleActionDrainState::default();
        restarted.activate_actions(&mut restarted_state).unwrap();
        assert!(restarted.drain_actions(&mut restarted_state).unwrap());
        assert_eq!(
            fake.ssh_calls().as_slice(),
            &["reboot"],
            "completed recovery republishes onto a replacement Bus without reboot"
        );
        let persist = Persist::open(bus.clone()).unwrap();
        let replies = persist
            .list_since(&reply_topic(&request.ulid), None)
            .unwrap();
        assert_eq!(replies.len(), 1);
        let reply: VehicleReply =
            serde_json::from_str(replies[0].body.as_deref().unwrap()).unwrap();
        assert!(reply.ok);
        assert!(reply.audited);
        assert!(!restarted.action_journal_path.exists());
        let conn = crate::store::open(&db_path).unwrap();
        let audit_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind = 'admin_action'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(audit_count, 1, "restart must not duplicate audit truth");
    }

    #[test]
    fn claimed_reboot_journal_recovers_indeterminate_without_effect_or_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let auth_tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().join("bus");
        let db_path = tmp.path().join("events.sqlite");
        let persist = Persist::open(bus.clone()).unwrap();
        let fake = FakeProbe::real();
        let request = persist
            .write(
                "action/vehicle/reboot",
                Priority::Default,
                None,
                Some(&authorized_reboot_body(
                    "vehicle-crash-claim",
                    "ND84720078011035",
                )),
            )
            .unwrap();
        let crashed = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_bus_root(Some(bus.clone()))
            .with_db_path(db_path.clone())
            .with_authorizer(test_authorizer(auth_tmp.path()));
        crashed
            .claim_privileged_action(&request.ulid, "action/vehicle/reboot")
            .unwrap();
        drop(crashed);

        let restarted = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_bus_root(Some(bus.clone()))
            .with_db_path(db_path.clone())
            .with_authorizer(test_authorizer(auth_tmp.path()));
        let mut state = VehicleActionDrainState::default();
        restarted.activate_actions(&mut state).unwrap();
        assert!(restarted.drain_actions(&mut state).unwrap());
        assert!(fake.ssh_calls().is_empty(), "an orphan claim never reboots");
        assert_eq!(
            fake.general_calls(),
            0,
            "an orphan claim is not re-evaluated"
        );
        let replies = persist
            .list_since(&reply_topic(&request.ulid), None)
            .unwrap();
        assert_eq!(replies.len(), 1);
        let reply: VehicleReply =
            serde_json::from_str(replies[0].body.as_deref().unwrap()).unwrap();
        assert!(!reply.ok);
        assert!(!reply.audited);
        assert!(reply.gated.as_deref().unwrap().contains("indeterminate"));
        assert!(!db_path.exists(), "recovery does not fabricate an audit DB");
        assert!(!restarted.action_journal_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn hostile_privileged_journal_is_rejected_before_reboot() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let tmp = tempfile::tempdir().unwrap();
        let auth_tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().join("bus");
        let db_path = tmp.path().join("events.sqlite");
        let persist = Persist::open(bus.clone()).unwrap();
        let fake = FakeProbe::real();
        let w = worker()
            .with_probe(Arc::new(fake.clone()))
            .with_bus_root(Some(bus.clone()))
            .with_db_path(db_path)
            .with_authorizer(test_authorizer(auth_tmp.path()));
        let mut state = VehicleActionDrainState::default();
        w.activate_actions(&mut state).unwrap();
        persist
            .write(
                "action/vehicle/reboot",
                Priority::Default,
                None,
                Some(&authorized_reboot_body(
                    "vehicle-hostile-journal",
                    "ND84720078011035",
                )),
            )
            .unwrap();

        let target = tmp.path().join("attacker-owned-target");
        fs::write(&target, b"do not follow").unwrap();
        symlink(&target, &w.action_journal_path).unwrap();
        assert!(w.drain_actions(&mut state).is_err());
        assert!(fake.ssh_calls().is_empty());
        fs::remove_file(&w.action_journal_path).unwrap();

        fs::write(
            &w.action_journal_path,
            serde_json::to_vec(&VehicleActionJournal::empty(&w.host)).unwrap(),
        )
        .unwrap();
        fs::set_permissions(&w.action_journal_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(w.drain_actions(&mut state).is_err());
        assert!(fake.ssh_calls().is_empty());
        fs::remove_file(&w.action_journal_path).unwrap();

        fs::write(
            &w.action_journal_path,
            vec![b'x'; usize::try_from(ACTION_JOURNAL_MAX_BYTES).unwrap() + 1],
        )
        .unwrap();
        fs::set_permissions(&w.action_journal_path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(w.drain_actions(&mut state).is_err());
        assert!(
            fake.ssh_calls().is_empty(),
            "hostile journal never admits reboot"
        );
        assert_eq!(fs::read(&target).unwrap(), b"do not follow");
    }

    #[test]
    fn final_manager_read_failure_commits_no_roster_or_cursor_then_retries() {
        let tmp = tempfile::tempdir().unwrap();
        let worker = worker().with_bus_root(Some(tmp.path().to_path_buf()));
        let source = roster_source_id();
        let now = Instant::now();
        let mut runtime = VehicleRuntimeRoster::new(
            "rig-1",
            Some(source.clone()),
            "manager-b,manager-c",
            now,
            VehiclePollPlan::default(),
        )
        .unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        for (manager, published_at_ms) in [("manager-b", 100), ("manager-c", 200)] {
            let mut remote = worker.build_state_v2(&FakeProbe::real());
            remote.management_node_id = manager.to_string();
            remote.approval = ApprovalState::Approved;
            remote.managers = ManagerSet::approved(vec![
                "rig-1".to_string(),
                "manager-b".to_string(),
                "manager-c".to_string(),
            ])
            .unwrap();
            remote.observed_at_ms = published_at_ms;
            remote.published_at_ms = published_at_ms;
            persist
                .write(
                    &vehicle_state_v2_topic(manager, source.as_str()),
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(&remote).unwrap()),
                )
                .unwrap();
        }
        let final_topic = vehicle_state_v2_topic("manager-c", source.as_str());
        worker.set_remote_read_failure(Some(final_topic));

        assert!(runtime.ingest_remote(&worker, now).is_err());
        assert!(runtime.remote_cursors.is_empty());
        assert!(matches!(
            runtime.roster.select_latest(&source),
            VehicleRosterSelection::NoSource { .. }
        ));

        worker.set_remote_read_failure(None);
        runtime.ingest_remote(&worker, now).unwrap();
        assert_eq!(runtime.remote_cursors.len(), 2);
        match runtime.roster.select_latest(&source) {
            VehicleRosterSelection::Selected(snapshot) => {
                assert_eq!(snapshot.manager_id(), "manager-c")
            }
            other => panic!("expected corrected-forward remote snapshot, got {other:?}"),
        }
    }

    #[test]
    fn publication_failure_preserves_sequence_and_clock_until_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().join("bus");
        std::fs::write(&bus, "blocked").unwrap();
        let worker = worker().with_bus_root(Some(bus.clone()));
        let source = roster_source_id();
        let now = Instant::now();
        let mut runtime = VehicleRuntimeRoster::new(
            "rig-1",
            Some(source.clone()),
            "",
            now,
            VehiclePollPlan::default(),
        )
        .unwrap();
        let local = worker.build_state(&FakeProbe::real());
        runtime.ingest_local(&worker, &local, now).unwrap();

        assert!(worker
            .publish_roster_updates(&mut runtime, &local, now)
            .is_err());
        assert_eq!(worker.sequence.load(Ordering::Relaxed), 0);
        assert!(runtime.roster.published.is_empty());

        std::fs::remove_file(&bus).unwrap();
        worker
            .publish_roster_updates(&mut runtime, &local, now)
            .unwrap();
        assert_eq!(worker.sequence.load(Ordering::Relaxed), 1);
        assert_eq!(runtime.roster.published.len(), 1);
        assert_eq!(
            Persist::open(bus)
                .unwrap()
                .list_since(&vehicle_state_v2_topic("rig-1", source.as_str()), None)
                .unwrap()
                .len(),
            1
        );
    }
}
