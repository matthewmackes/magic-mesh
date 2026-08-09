//! MG90 airspace survey adapter.
//!
//! This worker owns the daemon-side seam for the Maps Airspace surface. The
//! repository proves MG90 LCI, application, status-broadcast, GPS, and pinned
//! root-SSH planes. The production adapter therefore uses only the proven
//! root-SSH plane and read-only OS survey commands (`iw`) for Wi-Fi contacts.
//! Cellular survey and Bluetooth RSSI are still not invented from Status
//! Broadcast or metadata-only inquiry output.

#![cfg(feature = "async-services")]

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mackes_mesh_types::airspace::{
    airspace_state_topic, AirspaceAvailability, AirspaceContact, AirspaceContactKind,
    AirspaceSnapshot, AirspaceSurvey, MAX_GAPS, MAX_SNAPSHOT_BYTES,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{vehicle, ShutdownToken, Worker};

use vehicle::VehicleProbe;

/// Mirror cadence and heartbeat interval.
pub const POLL: Duration = Duration::from_secs(5);
const FAILURE_RETRY_MAX: Duration = Duration::from_secs(60);
const BUS_RETRY_MIN: Duration = Duration::from_millis(10);
const BUS_RETRY_MAX: Duration = Duration::from_secs(2);
const MAX_INITIAL_PHASE: Duration = Duration::from_millis(250);

/// Spread the first expensive MG90 survey across a small deterministic window.
/// Failed surveys already back off, but without this phase every configured
/// seat still launches its first root-SSH/`iw` probe together after a restart.
#[must_use]
fn initial_phase_for(host: &str, cap: Duration) -> Duration {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in host.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Duration::from_millis(
        (hash % (MAX_INITIAL_PHASE.as_millis() as u64 + 1)).min(cap.as_millis() as u64),
    )
}

/// Allow a small MG90/host clock skew, but do not publish a source timestamp
/// that is implausibly newer than the local publication. A future-dated
/// observation can make stale cached contacts appear to be the newest scan.
const MAX_SCAN_FUTURE_SKEW_MS: i64 = 5_000;
/// Do not republish contacts from a source observation older than the worker's
/// bounded freshness window. The publication timestamp is local and therefore
/// cannot by itself prove that the scanner data is current.
const MAX_SCAN_AGE_MS: i64 = 30_000;
/// Upper bound for the complete MG90 root-SSH survey transcript.
const MAX_ROOT_SSH_SURVEY_BYTES: usize = 96 * 1024;

/// Read-only MG90 root-SSH survey script.
///
/// This intentionally filters to managed/station Wi-Fi interfaces so an MG90
/// access-point interface is not asked to scan. It does not alter wireless,
/// Bluetooth, cellular, or GPS configuration.
const ROOT_SSH_SURVEY_SCRIPT: &str = r#"PATH=/usr/sbin:/usr/bin:/sbin:/bin
echo MDE_AIRSPACE_SURVEY_V1
if command -v iw >/dev/null 2>&1; then
    ifaces="$(iw dev 2>/dev/null | awk '
        /^[[:space:]]*Interface[[:space:]]+/ { iface=$2 }
        /^[[:space:]]*type[[:space:]]+/ && ($2 == "managed" || $2 == "station") && iface != "" { print iface; iface="" }
    ' | head -4)"
    if [ -z "$ifaces" ]; then
        echo "GAP wifi: no managed interfaces reported by iw dev"
    fi
    for iface in $ifaces; do
        echo "BEGIN_WIFI_IFACE $iface"
        tmp="/tmp/mde-airspace-wifi-$$"
        if timeout 8 iw dev "$iface" scan >"$tmp" 2>&1; then
            rc=0
        else
            rc=$?
        fi
        head -220 "$tmp" 2>/dev/null || true
        rm -f "$tmp"
        echo "END_WIFI_IFACE $iface RC=$rc"
    done
else
    echo "GAP wifi: iw command unavailable"
fi
if command -v hcitool >/dev/null 2>&1; then
    echo "BEGIN_BT_INQUIRY"
    tmp="/tmp/mde-airspace-bt-$$"
    if timeout 10 hcitool scan >"$tmp" 2>&1; then
        rc=0
    else
        rc=$?
    fi
    head -80 "$tmp" 2>/dev/null || true
    rm -f "$tmp"
    echo "END_BT_INQUIRY RC=$rc"
else
    echo "GAP bluetooth: hcitool command unavailable"
fi
echo "GAP cellular: no MG90 root-SSH cellular-neighbor survey command is proven"
"#;

/// Injectable typed MG90 survey seam.
///
/// The seam intentionally accepts a typed survey rather than inventing a
/// transport URL, shell command, or response parser. The production MG90
/// protocol must be proven before an implementation is attached here.
pub trait Mg90SurveyProbe: Send + Sync {
    /// Read one complete survey from the MG90.
    ///
    /// An error means the source was configured/attempted but unavailable; the
    /// worker publishes an offline snapshot and never retains old contacts.
    fn survey(&self) -> io::Result<AirspaceSurvey>;
}

/// Short name for consumers that treat the seam as the generic airspace probe.
pub use Mg90SurveyProbe as AirspaceProbe;

/// Workstation-side MG90 airspace mirror worker.
pub struct AirspaceWorker {
    host: String,
    probe: Option<Arc<dyn Mg90SurveyProbe>>,
    bus_root_override: Option<PathBuf>,
    poll: Duration,
}

impl AirspaceWorker {
    /// Construct the production worker.
    ///
    /// When the seat already has `MDE_VEHICLE_GATEWAY` configured, this wires a
    /// read-only MG90 root-SSH survey probe through the same pinned credential
    /// path as the vehicle worker. Otherwise it publishes explicit no-source.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe: Option<Arc<dyn Mg90SurveyProbe>> = std::env::var(vehicle::GATEWAY_ENV)
            .ok()
            .map(|gateway| gateway.trim().to_string())
            .filter(|gateway| !gateway.is_empty())
            .map(|gateway| {
                Arc::new(Mg90RootSshSurveyProbe::from_gateway(&gateway)) as Arc<dyn Mg90SurveyProbe>
            });
        Self {
            host,
            probe,
            bus_root_override: None,
            poll: POLL,
        }
    }

    /// Inject a typed MG90 survey source.
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn Mg90SurveyProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override the Bus root. Without an override, each publication resolves
    /// the current user root and falls back to the canonical system spool.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root_override = root;
        self
    }

    /// Override the poll cadence for tests.
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Fold one probe result into the typed bounded mirror.
    #[must_use]
    pub fn build_snapshot(
        &self,
        result: io::Result<AirspaceSurvey>,
        published_at_ms: i64,
    ) -> AirspaceSnapshot {
        match result {
            Ok(survey) => {
                let mut snapshot =
                    AirspaceSnapshot::from_survey(&self.host, published_at_ms, survey);
                let scanned_at_ms = snapshot.scanned_at_ms.unwrap_or_else(|| {
                    if snapshot.gaps.len() < MAX_GAPS {
                        snapshot.gaps.push(
                            "MG90 scan timestamp absent; using local survey completion time"
                                .to_string(),
                        );
                    }
                    published_at_ms
                });
                snapshot.scanned_at_ms = Some(scanned_at_ms);
                if scanned_at_ms > published_at_ms.saturating_add(MAX_SCAN_FUTURE_SKEW_MS) {
                    snapshot = AirspaceSnapshot::offline(
                        &self.host,
                        published_at_ms,
                        "MG90 scan timestamp exceeded the allowed future skew",
                    );
                } else if published_at_ms.saturating_sub(scanned_at_ms) > MAX_SCAN_AGE_MS {
                    // Keep the stale source from looking live merely because this
                    // worker just published a fresh envelope. Emptying the
                    // snapshot retracts stale contacts at the latest-wins seam.
                    snapshot = AirspaceSnapshot::offline(
                        &self.host,
                        published_at_ms,
                        "MG90 scan timestamp exceeded the allowed freshness window",
                    );
                }
                snapshot
            }
            Err(error) => AirspaceSnapshot::offline(
                &self.host,
                published_at_ms,
                format!("MG90 airspace survey unavailable: {error}"),
            ),
        }
    }

    /// Publish one bounded latest-wins mirror record.
    fn publish(&self, snapshot: &AirspaceSnapshot) -> io::Result<()> {
        let Some(body) = self.body_for_publish(snapshot) else {
            return Err(io::Error::other("airspace snapshot could not be encoded"));
        };
        let root = airspace_bus_root(self.bus_root_override.clone());
        let persist = Persist::open(root).map_err(io_other)?;
        persist
            .write(
                &airspace_state_topic(&self.host),
                Priority::Default,
                None,
                Some(&body),
            )
            .map_err(io_other)?;
        Ok(())
    }

    async fn publish_until_success(
        &self,
        snapshot: &AirspaceSnapshot,
        shutdown: &mut ShutdownToken,
    ) -> bool {
        let mut retry = BUS_RETRY_MIN;
        loop {
            match self.publish(snapshot) {
                Ok(()) => return true,
                Err(error) => tracing::warn!(
                    target: "mackesd::airspace",
                    host = %self.host,
                    %error,
                    "airspace publication deferred until Bus recovery"
                ),
            }
            tokio::select! {
                () = shutdown.wait() => return false,
                () = tokio::time::sleep(retry) => {}
            }
            retry = retry.saturating_mul(2).min(BUS_RETRY_MAX);
        }
    }

    /// Materialize exactly one bounded JSON body for the retained Bus row.
    ///
    /// `AirspaceSnapshot::from_survey` bounds each contact, but the legal
    /// contact budget can still exceed the wire budget when every display
    /// field is at its maximum. Trim only the tail in that case, preserving
    /// the valid prefix and recording the omission. A malformed snapshot or
    /// one whose non-contact fields alone exceed the budget becomes an
    /// explicit offline state rather than reaching the Bus.
    fn body_for_publish(&self, snapshot: &AirspaceSnapshot) -> Option<String> {
        if snapshot
            .contacts
            .iter()
            .any(|contact| contact.clone().bounded().is_err())
        {
            tracing::warn!(
                target: "mackesd::airspace",
                host = %self.host,
                "airspace snapshot contained a malformed contact; publishing offline status"
            );
            return self.offline_body("airspace snapshot contained a malformed contact");
        }
        match serde_json::to_string(snapshot) {
            Ok(body) if body.len() <= MAX_SNAPSHOT_BYTES => return Some(body),
            Ok(body) => {
                let size = body.len();
                tracing::warn!(
                    target: "mackesd::airspace",
                    host = %self.host,
                    size,
                    limit = MAX_SNAPSHOT_BYTES,
                    "airspace snapshot exceeded wire bound; trimming contacts"
                );
                drop(body);
            }
            Err(error) => {
                tracing::warn!(
                    target: "mackesd::airspace",
                    host = %self.host,
                    %error,
                    "airspace snapshot could not be encoded; publishing offline status"
                );
                return self.offline_body("airspace snapshot could not be encoded");
            }
        };

        let mut candidate = snapshot.clone();
        let mut trimmed = 0_u32;
        loop {
            match serde_json::to_string(&candidate) {
                Ok(body) if body.len() <= MAX_SNAPSHOT_BYTES => {
                    if trimmed > 0 {
                        tracing::warn!(
                            target: "mackesd::airspace",
                            host = %self.host,
                            trimmed,
                            retained = candidate.contacts.len(),
                            "airspace snapshot contacts trimmed to fit wire bound"
                        );
                    }
                    return Some(body);
                }
                Ok(_) => {
                    if candidate.contacts.pop().is_none() {
                        return self
                            .offline_body("airspace snapshot exceeded the published byte bound");
                    }
                    trimmed = trimmed.saturating_add(1);
                    candidate.omitted_contacts = candidate.omitted_contacts.saturating_add(1);
                    if trimmed == 1 && candidate.gaps.len() < MAX_GAPS {
                        candidate.gaps.push(
                            "airspace contacts trimmed to honor the published byte bound".into(),
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        target: "mackesd::airspace",
                        host = %self.host,
                        %error,
                        "airspace snapshot remained unencodable after trimming; publishing offline status"
                    );
                    return self.offline_body("airspace snapshot could not be encoded");
                }
            }
        }
    }

    fn offline_body(&self, reason: &str) -> Option<String> {
        let snapshot = AirspaceSnapshot::offline(&self.host, now_ms(), reason);
        match serde_json::to_string(&snapshot) {
            Ok(body) if body.len() <= MAX_SNAPSHOT_BYTES => Some(body),
            Ok(body) => {
                tracing::error!(
                    target: "mackesd::airspace",
                    host = %self.host,
                    size = body.len(),
                    limit = MAX_SNAPSHOT_BYTES,
                    "airspace offline fallback exceeded wire bound"
                );
                None
            }
            Err(error) => {
                tracing::error!(
                    target: "mackesd::airspace",
                    host = %self.host,
                    %error,
                    "airspace offline fallback could not be encoded"
                );
                None
            }
        }
    }

    async fn poll_once(
        &self,
        probe: Arc<dyn Mg90SurveyProbe>,
        shutdown: &mut ShutdownToken,
    ) -> Option<AirspaceSnapshot> {
        let host = self.host.clone();
        let task = tokio::task::spawn_blocking(move || {
            let result = probe.survey();
            let worker = AirspaceWorker {
                host,
                probe: None,
                bus_root_override: None,
                poll: POLL,
            };
            worker.build_snapshot(result, now_ms())
        });
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(snapshot) => snapshot,
                Err(error) => AirspaceSnapshot::offline(
                    &self.host,
                    now_ms(),
                    format!("MG90 airspace survey task failed: {error}"),
                ),
            }),
        }
    }
}

/// Production MG90 survey probe using the already-proven pinned root-SSH plane.
pub struct Mg90RootSshSurveyProbe {
    transport: vehicle::SshHttpProbe,
}

impl Mg90RootSshSurveyProbe {
    /// Build the probe from the same gateway syntax used by the vehicle worker.
    #[must_use]
    pub fn from_gateway(gateway: &str) -> Self {
        Self {
            transport: vehicle::SshHttpProbe::from_env(gateway),
        }
    }
}

impl Mg90SurveyProbe for Mg90RootSshSurveyProbe {
    fn survey(&self) -> io::Result<AirspaceSurvey> {
        let body = self.transport.run_ssh(ROOT_SSH_SURVEY_SCRIPT)?;
        parse_root_ssh_survey(&body, now_ms())
    }
}

#[derive(Debug, Default)]
struct WifiBuilder {
    id: String,
    iface: String,
    name: String,
    signal_dbm: Option<i32>,
    channel: Option<u16>,
    encryption: Option<String>,
}

impl WifiBuilder {
    fn finish(self, contacts: &mut Vec<AirspaceContact>, gaps: &mut Vec<String>) {
        let Some(signal_dbm) = self.signal_dbm else {
            push_gap(
                gaps,
                format!(
                    "Wi-Fi contact {} on {} omitted: signal strength missing",
                    self.id, self.iface
                ),
            );
            return;
        };
        contacts.push(AirspaceContact {
            id: self.id,
            kind: AirspaceContactKind::Wifi,
            name: self.name,
            signal_dbm,
            // MG90 `iw` survey output does not report directional bearing.
            // Preserve the contact and record the missing-bearing gap once at
            // survey level instead of dropping real BSSID/signal evidence.
            bearing_deg: 0.0,
            channel: self.channel,
            encryption: self.encryption,
            notable: false,
            watchlist: false,
            own: false,
        });
    }
}

fn parse_root_ssh_survey(output: &str, scanned_at_ms: i64) -> io::Result<AirspaceSurvey> {
    if output.len() > MAX_ROOT_SSH_SURVEY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "MG90 airspace survey exceeded {MAX_ROOT_SSH_SURVEY_BYTES} bytes before parsing"
            ),
        ));
    }

    let mut lines = output.lines();
    if lines.next() != Some("MDE_AIRSPACE_SURVEY_V1") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "MG90 airspace survey header missing",
        ));
    }

    let mut contacts = Vec::new();
    let mut gaps = Vec::new();
    let mut current_wifi: Option<WifiBuilder> = None;
    let mut wifi_attempts = 0_u32;
    let mut wifi_successes = 0_u32;
    let mut bluetooth_rows_without_rssi = 0_u32;
    let mut in_bt = false;

    for raw in lines {
        let line = raw.trim_end_matches('\r');
        if let Some(iface) = line.strip_prefix("BEGIN_WIFI_IFACE ") {
            finish_wifi(&mut current_wifi, &mut contacts, &mut gaps);
            wifi_attempts = wifi_attempts.saturating_add(1);
            push_gap(&mut gaps, format!("Wi-Fi survey attempted on {iface}"));
            continue;
        }
        if let Some(rest) = line.strip_prefix("END_WIFI_IFACE ") {
            finish_wifi(&mut current_wifi, &mut contacts, &mut gaps);
            if rest.ends_with(" RC=0") {
                wifi_successes = wifi_successes.saturating_add(1);
            } else {
                push_gap(&mut gaps, format!("Wi-Fi survey failed: {rest}"));
            }
            continue;
        }
        if line == "BEGIN_BT_INQUIRY" {
            in_bt = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("END_BT_INQUIRY") {
            in_bt = false;
            if !rest.trim().ends_with("RC=0") {
                push_gap(&mut gaps, format!("Bluetooth inquiry failed: {rest}"));
            }
            continue;
        }
        if let Some(gap) = line.strip_prefix("GAP ") {
            push_gap(&mut gaps, gap.to_string());
            continue;
        }

        if in_bt {
            if bluetooth_inquiry_row(line).is_some() {
                bluetooth_rows_without_rssi = bluetooth_rows_without_rssi.saturating_add(1);
            }
            continue;
        }

        let trimmed = line.trim_start();
        if let Some(id) = wifi_bss_id(trimmed) {
            finish_wifi(&mut current_wifi, &mut contacts, &mut gaps);
            current_wifi = Some(WifiBuilder {
                id,
                iface: wifi_bss_iface(trimmed).unwrap_or_default(),
                ..WifiBuilder::default()
            });
            continue;
        }

        let Some(wifi) = current_wifi.as_mut() else {
            continue;
        };
        if let Some(signal) = wifi_signal_dbm(trimmed) {
            wifi.signal_dbm = Some(signal);
        } else if let Some(ssid) = trimmed.strip_prefix("SSID:") {
            wifi.name = ssid.trim_start().to_string();
        } else if let Some(channel) = wifi_channel(trimmed) {
            wifi.channel = Some(channel);
        } else if trimmed.starts_with("RSN:") {
            wifi.encryption = Some("WPA2/RSN".to_string());
        } else if trimmed.starts_with("WPA:") {
            wifi.encryption = Some("WPA".to_string());
        } else if trimmed.starts_with("capability:")
            && trimmed.split_whitespace().any(|part| part == "Privacy")
            && wifi.encryption.is_none()
        {
            wifi.encryption = Some("secured".to_string());
        }
    }
    finish_wifi(&mut current_wifi, &mut contacts, &mut gaps);

    if wifi_attempts == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "MG90 Wi-Fi survey source unavailable",
        ));
    }
    if wifi_successes == 0 {
        push_gap(
            &mut gaps,
            "MG90 Wi-Fi survey source reachable but no Wi-Fi scan completed successfully"
                .to_string(),
        );
    }
    if contacts.is_empty() {
        push_gap(
            &mut gaps,
            "MG90 Wi-Fi survey completed but observed zero contacts".to_string(),
        );
    } else {
        push_gap(
            &mut gaps,
            "MG90 Wi-Fi survey output does not report bearing; contacts use neutral bearing 0"
                .to_string(),
        );
    }
    if bluetooth_rows_without_rssi > 0 {
        push_gap(
            &mut gaps,
            format!(
                "Bluetooth inquiry observed {bluetooth_rows_without_rssi} device(s) but no RSSI; contacts omitted"
            ),
        );
    } else {
        push_gap(
            &mut gaps,
            "Bluetooth inquiry exposes no RSSI-bearing contacts in this MG90 plane".to_string(),
        );
    }

    Ok(AirspaceSurvey {
        scanned_at_ms: Some(scanned_at_ms),
        contacts,
        gaps,
    })
}

fn finish_wifi(
    current: &mut Option<WifiBuilder>,
    contacts: &mut Vec<AirspaceContact>,
    gaps: &mut Vec<String>,
) {
    if let Some(wifi) = current.take() {
        wifi.finish(contacts, gaps);
    }
}

fn push_gap(gaps: &mut Vec<String>, gap: String) {
    if gaps.len() < MAX_GAPS && !gaps.iter().any(|existing| existing == &gap) {
        gaps.push(gap);
    }
}

fn wifi_bss_id(line: &str) -> Option<String> {
    let rest = line.strip_prefix("BSS ")?;
    let id = rest
        .split([' ', '('])
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    is_mac_like(&id).then_some(id)
}

fn wifi_bss_iface(line: &str) -> Option<String> {
    line.split_once("(on ")
        .and_then(|(_, rest)| rest.split_once(')').map(|(iface, _)| iface.to_string()))
}

fn wifi_signal_dbm(line: &str) -> Option<i32> {
    let raw = line.strip_prefix("signal:")?.trim();
    let number = raw.split_whitespace().next()?.parse::<f32>().ok()?;
    if !number.is_finite() {
        return None;
    }
    Some(number.round().clamp(-150.0, 0.0) as i32)
}

fn wifi_channel(line: &str) -> Option<u16> {
    if let Some(raw) = line.strip_prefix("DS Parameter set: channel") {
        return raw.trim().parse::<u16>().ok();
    }
    if let Some(raw) = line.strip_prefix("primary channel:") {
        return raw.trim().parse::<u16>().ok();
    }
    let freq = line
        .strip_prefix("freq:")?
        .trim()
        .split_whitespace()
        .next()?
        .parse::<u32>()
        .ok()?;
    wifi_freq_to_channel(freq)
}

fn wifi_freq_to_channel(freq_mhz: u32) -> Option<u16> {
    match freq_mhz {
        2_412..=2_472 if (freq_mhz - 2_407).is_multiple_of(5) => {
            u16::try_from((freq_mhz - 2_407) / 5).ok()
        }
        2_484 => Some(14),
        5_000..=5_895 if (freq_mhz - 5_000).is_multiple_of(5) => {
            u16::try_from((freq_mhz - 5_000) / 5).ok()
        }
        5_955..=7_115 if (freq_mhz - 5_950).is_multiple_of(5) => {
            u16::try_from((freq_mhz - 5_950) / 5).ok()
        }
        _ => None,
    }
}

fn bluetooth_inquiry_row(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    let (mac, name) = trimmed.split_once(char::is_whitespace)?;
    is_mac_like(mac).then_some((mac, name.trim()))
}

fn is_mac_like(value: &str) -> bool {
    let mut parts = value.split(':');
    (0..6).all(|_| {
        parts
            .next()
            .is_some_and(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_hexdigit()))
    }) && parts.next().is_none()
}

#[async_trait::async_trait]
impl Worker for AirspaceWorker {
    fn name(&self) -> &'static str {
        "airspace"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(probe) = self.probe.clone() else {
            let snapshot = AirspaceSnapshot::no_source(&self.host, now_ms());
            if !self.publish_until_success(&snapshot, &mut shutdown).await {
                return Ok(());
            }
            shutdown.wait().await;
            return Ok(());
        };

        let phase = initial_phase_for(&self.host, self.poll);
        tokio::select! {
            () = shutdown.wait() => return Ok(()),
            () = tokio::time::sleep(phase) => {}
        }
        let mut retry = self.poll;
        loop {
            let Some(snapshot) = self.poll_once(probe.clone(), &mut shutdown).await else {
                return Ok(());
            };
            if !self.publish_until_success(&snapshot, &mut shutdown).await {
                return Ok(());
            }
            let delay = match snapshot.availability {
                AirspaceAvailability::Ready => {
                    retry = self.poll;
                    self.poll
                }
                AirspaceAvailability::Offline => {
                    let delay = retry;
                    retry = retry.saturating_mul(2).min(FAILURE_RETRY_MAX);
                    delay
                }
                AirspaceAvailability::NoSource => self.poll,
            };
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(delay) => {}
            }
        }
    }
}

fn airspace_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    override_root
        .or_else(crate::bus_publish::default_bus_root)
        .unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use mackes_mesh_types::airspace::{
        AirspaceAvailability, AirspaceContact, AirspaceContactKind, AirspaceSnapshot,
        AirspaceSurvey, MAX_RETAINED_CONTACTS, MAX_STRING_BYTES,
    };
    use mde_bus::persist::Persist;

    use super::*;

    fn contact(id: &str) -> AirspaceContact {
        AirspaceContact {
            id: id.to_string(),
            kind: AirspaceContactKind::Wifi,
            name: "captured-network".to_string(),
            signal_dbm: -61,
            bearing_deg: 15.0,
            channel: Some(11),
            encryption: Some("WPA2".to_string()),
            notable: false,
            watchlist: false,
            own: false,
        }
    }

    struct FakeProbe {
        result: io::Result<AirspaceSurvey>,
    }

    impl Mg90SurveyProbe for FakeProbe {
        fn survey(&self) -> io::Result<AirspaceSurvey> {
            self.result
                .as_ref()
                .map(Clone::clone)
                .map_err(|error| io::Error::new(error.kind(), error.to_string()))
        }
    }

    struct CountingProbe {
        calls: Arc<AtomicUsize>,
    }

    impl Mg90SurveyProbe for CountingProbe {
        fn survey(&self) -> io::Result<AirspaceSurvey> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(AirspaceSurvey {
                scanned_at_ms: Some(now_ms()),
                contacts: vec![contact("aa:bb:cc")],
                gaps: Vec::new(),
            })
        }
    }

    #[test]
    fn root_ssh_survey_parses_iw_bss_contacts() {
        let survey = parse_root_ssh_survey(
            r#"MDE_AIRSPACE_SURVEY_V1
BEGIN_WIFI_IFACE wlan0
BSS aa:bb:cc:dd:ee:ff(on wlan0)
	freq: 2412
	signal: -45.00 dBm
	SSID: shop-floor
	RSN:	 * Version: 1
END_WIFI_IFACE wlan0 RC=0
BEGIN_BT_INQUIRY
Scanning ...
END_BT_INQUIRY RC=0
GAP cellular: no MG90 root-SSH cellular-neighbor survey command is proven
"#,
            10_000,
        )
        .expect("survey parses");

        assert_eq!(survey.scanned_at_ms, Some(10_000));
        assert_eq!(survey.contacts.len(), 1);
        let contact = &survey.contacts[0];
        assert_eq!(contact.id, "aa:bb:cc:dd:ee:ff");
        assert_eq!(contact.kind, AirspaceContactKind::Wifi);
        assert_eq!(contact.name, "shop-floor");
        assert_eq!(contact.signal_dbm, -45);
        assert_eq!(contact.channel, Some(1));
        assert_eq!(contact.encryption.as_deref(), Some("WPA2/RSN"));
        assert!(survey.gaps.iter().any(|gap| gap.contains("bearing")));
        assert!(survey.gaps.iter().any(|gap| gap.contains("cellular")));
    }

    #[test]
    fn root_ssh_survey_ready_with_empty_successful_wifi_scan() {
        let survey = parse_root_ssh_survey(
            r#"MDE_AIRSPACE_SURVEY_V1
BEGIN_WIFI_IFACE wlan0
END_WIFI_IFACE wlan0 RC=0
BEGIN_BT_INQUIRY
Scanning ...
END_BT_INQUIRY RC=0
"#,
            10_000,
        )
        .expect("empty scan is a successful survey");

        assert!(survey.contacts.is_empty());
        assert!(survey
            .gaps
            .iter()
            .any(|gap| gap.contains("observed zero contacts")));
    }

    #[test]
    fn root_ssh_survey_ready_when_reachable_wifi_scan_times_out() {
        let survey = parse_root_ssh_survey(
            r#"MDE_AIRSPACE_SURVEY_V1
BEGIN_WIFI_IFACE wlan0
END_WIFI_IFACE wlan0 RC=124
BEGIN_BT_INQUIRY
Scanning ...
END_BT_INQUIRY RC=0
GAP cellular: no MG90 root-SSH cellular-neighbor survey command is proven
"#,
            10_000,
        )
        .expect("reachable MG90 with scan timeout still proves a fresh source");

        assert_eq!(survey.scanned_at_ms, Some(10_000));
        assert!(survey.contacts.is_empty());
        assert!(survey
            .gaps
            .iter()
            .any(|gap| gap.contains("Wi-Fi survey failed: wlan0 RC=124")));
        assert!(survey
            .gaps
            .iter()
            .any(|gap| gap.contains("source reachable")));
        assert!(survey
            .gaps
            .iter()
            .any(|gap| gap.contains("observed zero contacts")));
    }

    #[test]
    fn root_ssh_survey_rejects_missing_header_and_unavailable_wifi() {
        let missing_header = parse_root_ssh_survey(
            "BEGIN_WIFI_IFACE wlan0\nEND_WIFI_IFACE wlan0 RC=0\n",
            10_000,
        )
        .expect_err("header required");
        assert_eq!(missing_header.kind(), io::ErrorKind::InvalidData);

        let unavailable = parse_root_ssh_survey(
            r#"MDE_AIRSPACE_SURVEY_V1
GAP wifi: no managed interfaces reported by iw dev
BEGIN_BT_INQUIRY
Scanning ...
END_BT_INQUIRY RC=0
"#,
            10_000,
        )
        .expect_err("wifi source required");
        assert_eq!(unavailable.kind(), io::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn worker_publishes_ready_snapshot_to_node_topic() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let mut worker = AirspaceWorker::new("rig-1".to_string())
            .with_probe(Arc::new(FakeProbe {
                result: Ok(AirspaceSurvey {
                    scanned_at_ms: Some(now_ms()),
                    contacts: vec![contact("aa:bb:cc")],
                    gaps: Vec::new(),
                }),
            }))
            .with_bus_root(Some(root.clone()))
            .with_poll(Duration::from_secs(60));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });
        let topic = airspace_state_topic("rig-1");
        let mut decoded = None;
        for _ in 0..40 {
            if let Some(body) = Persist::open(root.clone())
                .ok()
                .and_then(|persist| persist.read_latest(&topic).ok().flatten())
                .and_then(|message| message.body)
            {
                decoded = serde_json::from_str::<AirspaceSnapshot>(&body).ok();
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tx.send(true).expect("shutdown");
        task.await.expect("join").expect("worker");

        let snapshot = decoded.expect("published snapshot");
        assert_eq!(snapshot.availability, AirspaceAvailability::Ready);
        assert_eq!(snapshot.contacts.len(), 1);
        assert_eq!(snapshot.contacts[0].id, "aa:bb:cc");
        assert!(snapshot.encoded_len().expect("encode") <= MAX_SNAPSHOT_BYTES);
    }

    #[tokio::test]
    async fn worker_publishes_explicit_no_source_without_contacts() {
        assert_eq!(
            airspace_bus_root(Some(PathBuf::from("/tmp/airspace-explicit-bus"))),
            PathBuf::from("/tmp/airspace-explicit-bus")
        );
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("late-bus");
        std::fs::write(&root, b"temporarily block Bus directory").expect("block Bus root");
        let mut worker = AirspaceWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        worker.probe = None;
        let (tx, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !task.is_finished(),
            "missing Bus must not terminate the no-source worker"
        );
        std::fs::remove_file(&root).expect("unblock Bus root");
        let topic = airspace_state_topic("rig-1");
        let mut decoded = None;
        for _ in 0..40 {
            if let Some(body) = Persist::open(root.clone())
                .ok()
                .and_then(|persist| persist.read_latest(&topic).ok().flatten())
                .and_then(|message| message.body)
            {
                decoded = serde_json::from_str::<AirspaceSnapshot>(&body).ok();
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tx.send(true).expect("shutdown");
        task.await.expect("join").expect("worker");

        let snapshot = decoded.expect("no-source snapshot");
        assert_eq!(snapshot.availability, AirspaceAvailability::NoSource);
        assert!(snapshot.contacts.is_empty());
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("not proven")));
    }

    #[tokio::test]
    async fn failed_publication_retries_snapshot_without_reprobing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("late-bus");
        std::fs::write(&root, b"temporarily block Bus directory").expect("block Bus root");
        let calls = Arc::new(AtomicUsize::new(0));
        let mut worker = AirspaceWorker::new("rig-1".to_string())
            .with_probe(Arc::new(CountingProbe {
                calls: Arc::clone(&calls),
            }))
            .with_bus_root(Some(root.clone()))
            .with_poll(Duration::from_secs(60));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });

        tokio::time::timeout(Duration::from_secs(2), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("initial survey");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        std::fs::remove_file(&root).expect("unblock Bus root");

        let topic = airspace_state_topic("rig-1");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if Persist::open(root.clone())
                    .ok()
                    .and_then(|persist| persist.read_latest(&topic).ok().flatten())
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("recovered publication");
        tx.send(true).expect("shutdown");
        task.await.expect("join").expect("worker");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn probe_error_is_offline_and_never_reuses_contacts() {
        let worker = AirspaceWorker::new("rig-1".to_string()).with_bus_root(None);
        let snapshot =
            worker.build_snapshot(Err(io::Error::new(io::ErrorKind::TimedOut, "timeout")), 55);
        assert_eq!(snapshot.availability, AirspaceAvailability::Offline);
        assert!(snapshot.contacts.is_empty());
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("timeout")));
    }

    #[test]
    fn initial_survey_phase_is_stable_bounded_and_capped_for_tests() {
        let phase = initial_phase_for("seat-15", POLL);
        assert_eq!(phase, initial_phase_for("seat-15", POLL));
        assert!(phase <= MAX_INITIAL_PHASE);
        assert!(
            initial_phase_for("seat-15", Duration::from_millis(10)) <= Duration::from_millis(10)
        );
        assert_ne!(phase, initial_phase_for("dell-laptop", POLL));
    }

    #[test]
    fn future_scan_timestamp_is_offline_and_retracts_contacts() {
        let worker = AirspaceWorker::new("rig-1".to_string()).with_bus_root(None);
        let snapshot = worker.build_snapshot(
            Ok(AirspaceSurvey {
                scanned_at_ms: Some(10_001),
                contacts: vec![contact("aa:bb:cc")],
                gaps: Vec::new(),
            }),
            5_000,
        );

        assert_eq!(snapshot.availability, AirspaceAvailability::Offline);
        assert!(snapshot.contacts.is_empty());
        assert!(snapshot.scanned_at_ms.is_none());
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("future skew")));
    }

    #[test]
    fn missing_scan_timestamp_uses_poll_completion_time() {
        let worker = AirspaceWorker::new("rig-1".to_string()).with_bus_root(None);
        let snapshot = worker.build_snapshot(
            Ok(AirspaceSurvey {
                scanned_at_ms: None,
                contacts: vec![contact("aa:bb:cc")],
                gaps: Vec::new(),
            }),
            5_000,
        );

        assert_eq!(snapshot.availability, AirspaceAvailability::Ready);
        assert_eq!(snapshot.scanned_at_ms, Some(5_000));
        assert_eq!(snapshot.contacts.len(), 1);
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("local survey completion time")));
    }

    #[test]
    fn stale_scan_is_offline_and_retracts_old_contacts() {
        let worker = AirspaceWorker::new("rig-1".to_string()).with_bus_root(None);
        let snapshot = worker.build_snapshot(
            Ok(AirspaceSurvey {
                scanned_at_ms: Some(5_000),
                contacts: vec![contact("aa:bb:cc")],
                gaps: Vec::new(),
            }),
            5_000 + MAX_SCAN_AGE_MS + 1,
        );

        assert_eq!(snapshot.availability, AirspaceAvailability::Offline);
        assert!(snapshot.contacts.is_empty());
        assert!(snapshot.scanned_at_ms.is_none());
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("freshness window")));
    }

    #[test]
    fn oversized_contact_rows_are_trimmed_before_bus_publish() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let worker = AirspaceWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let contacts = (0..MAX_RETAINED_CONTACTS)
            .map(|index| AirspaceContact {
                id: format!("wifi-{index}-{}", "i".repeat(MAX_STRING_BYTES)),
                kind: AirspaceContactKind::Wifi,
                name: "n".repeat(MAX_STRING_BYTES),
                signal_dbm: -61,
                bearing_deg: 15.0,
                channel: Some(11),
                encryption: Some("e".repeat(MAX_STRING_BYTES)),
                notable: false,
                watchlist: false,
                own: false,
            })
            .collect();
        let snapshot = worker.build_snapshot(
            Ok(AirspaceSurvey {
                scanned_at_ms: Some(2),
                contacts,
                gaps: Vec::new(),
            }),
            2,
        );
        assert_eq!(snapshot.contacts.len(), MAX_RETAINED_CONTACTS);

        worker.publish(&snapshot).expect("publish bounded snapshot");

        let body = Persist::open(root)
            .expect("open bus")
            .read_latest(&airspace_state_topic("rig-1"))
            .expect("read bus")
            .and_then(|message| message.body)
            .expect("published body");
        assert!(body.len() <= MAX_SNAPSHOT_BYTES);
        let published: AirspaceSnapshot = serde_json::from_str(&body).expect("decode body");
        assert_eq!(published.availability, AirspaceAvailability::Ready);
        assert!(!published.contacts.is_empty());
        assert!(published.contacts.len() < MAX_RETAINED_CONTACTS);
        assert!(published.omitted_contacts > 0);
        assert!(published
            .gaps
            .iter()
            .any(|gap| gap.contains("published byte bound")));
    }

    #[test]
    fn hostile_display_field_is_retracted_before_bus_publish() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let worker = AirspaceWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let snapshot = AirspaceSnapshot {
            host: "hostile-host".repeat(MAX_SNAPSHOT_BYTES),
            published_at_ms: 2,
            scanned_at_ms: None,
            availability: AirspaceAvailability::Ready,
            contacts: Vec::new(),
            omitted_contacts: 0,
            gaps: Vec::new(),
        };

        worker.publish(&snapshot).expect("publish offline fallback");

        let body = Persist::open(root)
            .expect("open bus")
            .read_latest(&airspace_state_topic("rig-1"))
            .expect("read bus")
            .and_then(|message| message.body)
            .expect("published body");
        assert!(body.len() <= MAX_SNAPSHOT_BYTES);
        let published: AirspaceSnapshot = serde_json::from_str(&body).expect("decode body");
        assert_eq!(published.availability, AirspaceAvailability::Offline);
        assert!(published.contacts.is_empty());
        assert!(published
            .gaps
            .iter()
            .any(|gap| gap.contains("published byte bound")));
        assert_eq!(published.host, "rig-1");
    }

    #[test]
    fn malformed_contact_is_offline_before_bus_publish() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let worker = AirspaceWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let mut snapshot = AirspaceSnapshot::from_survey(
            "rig-1",
            2,
            AirspaceSurvey {
                scanned_at_ms: Some(2),
                contacts: vec![contact("aa:bb:cc")],
                gaps: Vec::new(),
            },
        );
        snapshot.contacts[0].bearing_deg = f32::NAN;

        worker
            .publish(&snapshot)
            .expect("publish malformed-contact fallback");

        let body = Persist::open(root)
            .expect("open bus")
            .read_latest(&airspace_state_topic("rig-1"))
            .expect("read bus")
            .and_then(|message| message.body)
            .expect("published body");
        let published: AirspaceSnapshot = serde_json::from_str(&body).expect("decode body");
        assert_eq!(published.availability, AirspaceAvailability::Offline);
        assert!(published.contacts.is_empty());
        assert!(published
            .gaps
            .iter()
            .any(|gap| gap.contains("malformed contact")));
    }
}
