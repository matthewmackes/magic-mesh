//! MG90 airspace survey adapter.
//!
//! This worker owns the daemon-side seam for the Maps Airspace surface. The
//! repository proves MG90 LCI, application, status-broadcast, and GPS planes,
//! but it does not prove a Wi-Fi/cellular/Bluetooth survey endpoint or command.
//! Consequently the production constructor has no guessed transport. A future
//! proven adapter, or a test fixture, injects the typed Mg90SurveyProbe seam.
//! Until then the worker publishes an explicit no-source snapshot with zero
//! contacts.

#![cfg(feature = "async-services")]

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mackes_mesh_types::airspace::{
    airspace_state_topic, AirspaceSnapshot, AirspaceSurvey, MAX_SNAPSHOT_BYTES,
};

use super::{ShutdownToken, Worker};

/// Mirror cadence and heartbeat interval.
pub const POLL: Duration = Duration::from_secs(5);

/// Allow a small MG90/host clock skew, but do not publish a source timestamp
/// that is implausibly newer than the local publication. A future-dated
/// observation can make stale cached contacts appear to be the newest scan.
const MAX_SCAN_FUTURE_SKEW_MS: i64 = 5_000;

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
    bus_root: Option<PathBuf>,
    poll: Duration,
}

impl AirspaceWorker {
    /// Construct the honest production default.
    ///
    /// No endpoint is guessed because the repository does not establish an
    /// MG90 survey protocol. Use with_probe only with a proven adapter or a
    /// captured test seam.
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            probe: None,
            bus_root: crate::bus_publish::default_bus_root(),
            poll: POLL,
        }
    }

    /// Inject a typed MG90 survey source.
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn Mg90SurveyProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override or disable Bus access.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
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
                if snapshot.scanned_at_ms.is_some_and(|scanned_at_ms| {
                    scanned_at_ms > published_at_ms.saturating_add(MAX_SCAN_FUTURE_SKEW_MS)
                }) {
                    snapshot.scanned_at_ms = None;
                    if snapshot.gaps.len() < mackes_mesh_types::airspace::MAX_GAPS {
                        snapshot
                            .gaps
                            .push("MG90 scan timestamp exceeded the allowed future skew".into());
                    }
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
    fn publish(&self, snapshot: &AirspaceSnapshot) {
        let snapshot = match snapshot.encoded_len() {
            Ok(size) if size <= MAX_SNAPSHOT_BYTES => snapshot.clone(),
            Ok(size) => {
                tracing::warn!(
                    target: "mackesd::airspace",
                    host = %self.host,
                    size,
                    limit = MAX_SNAPSHOT_BYTES,
                    "airspace snapshot exceeded wire bound; publishing offline status"
                );
                AirspaceSnapshot::offline(
                    &self.host,
                    now_ms(),
                    "airspace snapshot exceeded the published byte bound",
                )
            }
            Err(error) => {
                tracing::warn!(
                    target: "mackesd::airspace",
                    host = %self.host,
                    %error,
                    "airspace snapshot could not be encoded; publishing offline status"
                );
                AirspaceSnapshot::offline(
                    &self.host,
                    now_ms(),
                    "airspace snapshot could not be encoded",
                )
            }
        };
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(
                &mut persist,
                &airspace_state_topic(&self.host),
                &snapshot,
            );
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
                bus_root: None,
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

#[async_trait::async_trait]
impl Worker for AirspaceWorker {
    fn name(&self) -> &'static str {
        "airspace"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(probe) = self.probe.clone() else {
            self.publish(&AirspaceSnapshot::no_source(&self.host, now_ms()));
            shutdown.wait().await;
            return Ok(());
        };

        loop {
            let Some(snapshot) = self.poll_once(probe.clone(), &mut shutdown).await else {
                return Ok(());
            };
            self.publish(&snapshot);
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.poll) => {}
            }
        }
    }
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
    use std::sync::Arc;

    use mackes_mesh_types::airspace::{
        AirspaceAvailability, AirspaceContact, AirspaceContactKind, AirspaceSnapshot,
        AirspaceSurvey,
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

    #[tokio::test]
    async fn worker_publishes_ready_snapshot_to_node_topic() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let mut worker = AirspaceWorker::new("rig-1".to_string())
            .with_probe(Arc::new(FakeProbe {
                result: Ok(AirspaceSurvey {
                    scanned_at_ms: Some(123),
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
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let mut worker = AirspaceWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
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

        let snapshot = decoded.expect("no-source snapshot");
        assert_eq!(snapshot.availability, AirspaceAvailability::NoSource);
        assert!(snapshot.contacts.is_empty());
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("not proven")));
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
    fn future_scan_timestamp_is_dropped_without_fabricating_or_retracting_contacts() {
        let worker = AirspaceWorker::new("rig-1".to_string()).with_bus_root(None);
        let snapshot = worker.build_snapshot(
            Ok(AirspaceSurvey {
                scanned_at_ms: Some(10_001),
                contacts: vec![contact("aa:bb:cc")],
                gaps: Vec::new(),
            }),
            5_000,
        );

        assert_eq!(snapshot.availability, AirspaceAvailability::Ready);
        assert_eq!(snapshot.contacts.len(), 1);
        assert!(snapshot.scanned_at_ms.is_none());
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("future skew")));
    }
}
