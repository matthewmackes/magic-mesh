//! DATACENTER-12 (scheduled-snapshot executor) — the missing consumer of the
//! Storage tab's "Save schedule".
//!
//! The Workbench Datacenter panel's Storage tab persists a scheduled-snapshot
//! config by publishing an `event/dc/snap-schedule/<sr>` record to the Bus
//! (the retired Workbench's `snap_schedule_save` originated the shape): `{ kind:
//! "snap-schedule", id, sr, retention, backup_target, dom0 }`. Until this worker,
//! NOTHING consumed that topic — the config was honest persistence with no
//! executor, so no snapshot was ever taken on a schedule and retention was never
//! enforced. This leader-gated periodic worker closes that gap: it reads the
//! latest schedule record per SR off the Bus, decides per-tick whether each SR is
//! **due** for a snapshot per its cadence, and when due reuses the EXISTING
//! storage snapshot path — `xe vdi-snapshot` over the mesh-key SSH through the
//! same injection-guarded, dom0-allow-listed contract `ipc::storage_ops` uses
//! (the `xen_ssh_key` / `xen_dom0s` resolvers + the same `ssh … xe …` shape) —
//! never re-implementing SSH or `xe`.
//!
//! Design (mirrors `dr_scheduler` + `dc_health`): the *brain* is a set of pure,
//! unit-tested helpers — [`due`] (cadence elapsed vs not), [`prune_targets`]
//! (which scheduler-made snapshots to destroy to keep N), and the record
//! (de)serialization ([`Schedule::parse`] / [`RunRecord::body`]) — and the worker
//! is thin best-effort I/O around them. It is **leader-gated** (the shared
//! `.mackesd-leader.lock`) so a multi-node mesh runs exactly one snapshot per SR
//! per interval, and it degrades cleanly (no panic) when there is no Bus, no
//! schedule config, or no dom0 — per §2.
//!
//! Retention safety: every scheduler-made snapshot is name-labelled with the
//! [`SNAP_LABEL_PREFIX`] prefix, and pruning only ever lists + destroys snapshots
//! carrying that prefix — an operator's hand-made snapshot is never touched.
//!
//! Run results land on `event/dc/snap-schedule-run/<sr>`
//! (`{ status: "ok"|"fail", sr, ts, snapshot?, detail? }`); the worker also reads
//! that lane back to recover the last-run timestamp across restarts, so a daemon
//! bounce doesn't re-snapshot every SR on the first tick. A failure additionally
//! drops an alert onto the `alert_relay` lane.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// Loop cadence — wake every ~5 min and ask [`due`] which SRs are ready.
/// Decoupling the wake cadence from the (much longer) per-SR snapshot interval
/// keeps the worker responsive to shutdown while the cadence clock is coarse.
pub const TICK_INTERVAL: Duration = Duration::from_secs(300);

/// Initial Bus recovery backoff. Small enough to recover promptly from startup
/// ordering without spinning on a missing spool.
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Maximum Bus recovery backoff. The supervised worker remains alive and keeps
/// retrying rather than delegating recovery to a service restart.
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Bound in-memory results whose xe effect completed but whose durable
/// run-history write has not. This is a same-worker duplicate-effect barrier,
/// not crash-durable state. Once full, new effects defer until publication
/// makes room.
const MAX_PENDING_RESULTS: usize = 128;

/// Default snapshot cadence when a schedule record carries no explicit
/// `interval_secs`/`cadence` — daily. The panel save (today) records retention +
/// target but not a cadence, so the executor must pick a sane honest default
/// rather than snapshot on every tick.
pub const DEFAULT_INTERVAL_SECS: u64 = 86_400;

/// Bus topic PREFIX the schedule config records live under (one per SR):
/// `event/dc/snap-schedule/<sr>`.
pub const SCHEDULE_PREFIX: &str = "event/dc/snap-schedule/";

/// Durable history prefix corresponding one-for-one with schedule SRs.
pub const RUN_PREFIX: &str = "event/dc/snap-schedule-run/";

/// Name-label prefix every scheduler-made snapshot carries. Retention pruning
/// only ever lists + destroys snapshots with this prefix, so an operator's
/// hand-made snapshot (any other label) is NEVER pruned by the scheduler.
pub const SNAP_LABEL_PREFIX: &str = "mcnf-sched-snap";

/// Max characters of a failure `detail` carried into a run record / alert. Keeps
/// the run lane compact.
pub const DETAIL_LEN: usize = 200;

/// Generous-but-finite overall bound for one SSH `xe …` invocation. The SSH args
/// already cap *connection* setup at 8 s (`ConnectTimeout`); this bounds the
/// WHOLE command so a connection that establishes then stalls mid-`xe` (a slow
/// `vdi-snapshot` on a large SR, a wedged dom0) is killed rather than blocking.
/// A snapshot of a large SR can legitimately take a couple of minutes. On expiry
/// the child is killed and the op degrades to a `fail` run record / a skipped
/// prune, exactly like a non-zero `xe` exit (mackesd-02: `run_pass` also runs off
/// the async runtime thread — see `run()` — so it can't starve the watchdog).
pub const SSH_XE_TIMEOUT: Duration = Duration::from_secs(300);

/// Bus topic the schedule config for `sr` is published to (the panel's write).
#[must_use]
pub fn schedule_topic(sr: &str) -> String {
    format!("{SCHEDULE_PREFIX}{sr}")
}

/// Bus topic a run result for `sr` is published to:
/// `event/dc/snap-schedule-run/<sr>`.
#[must_use]
pub fn run_topic(sr: &str) -> String {
    format!("{RUN_PREFIX}{sr}")
}

/// First [`DETAIL_LEN`] characters of a string (char-boundary safe).
#[must_use]
fn detail_summary(detail: &str) -> String {
    detail.chars().take(DETAIL_LEN).collect()
}

// ---- pure record (de)serialization ----

/// One scheduled-snapshot config, parsed from an `event/dc/snap-schedule/<sr>`
/// record body. Mirrors the JSON the Workbench Storage tab's "Save schedule"
/// writes; an absent/zero `interval_secs` falls back to [`DEFAULT_INTERVAL_SECS`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Schedule {
    /// The SR uuid this schedule targets.
    pub sr: String,
    /// The dom0 (Xen host) the SR lives on — allow-list-checked before any SSH.
    pub dom0: String,
    /// How many scheduler-made snapshots to keep for this SR (≥1). Older ones are
    /// destroyed after each successful snapshot.
    pub retention: u64,
    /// Snapshot cadence in seconds (≥1). Defaults to [`DEFAULT_INTERVAL_SECS`].
    pub interval_secs: u64,
    /// Optional backup target (e.g. a remote SR uuid). Carried through to the run
    /// record for the operator; the snapshot itself is local.
    pub backup_target: String,
}

impl Schedule {
    /// Parse a schedule record body. Returns `None` for non-JSON, a record whose
    /// `kind` isn't `"snap-schedule"`, an empty `sr`, or a `retention` of 0 — all
    /// of which the worker skips rather than acting on garbage (§2 degrade).
    ///
    /// `interval_secs` is read from either an explicit `interval_secs` integer or
    /// a `cadence` integer; absent/zero falls back to [`DEFAULT_INTERVAL_SECS`].
    #[must_use]
    pub fn parse(body: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(body).ok()?;
        if v.get("kind").and_then(serde_json::Value::as_str) != Some("snap-schedule") {
            return None;
        }
        let sr = v.get("sr").and_then(serde_json::Value::as_str)?.trim();
        if sr.is_empty() {
            return None;
        }
        let retention = v.get("retention").and_then(serde_json::Value::as_u64)?;
        if retention == 0 {
            return None;
        }
        let interval_secs = v
            .get("interval_secs")
            .and_then(serde_json::Value::as_u64)
            .or_else(|| v.get("cadence").and_then(serde_json::Value::as_u64))
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_INTERVAL_SECS);
        let dom0 = v
            .get("dom0")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let backup_target = v
            .get("backup_target")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        Some(Self {
            sr: sr.to_string(),
            dom0,
            retention,
            interval_secs,
            backup_target,
        })
    }
}

/// One run result the executor decided to record for an SR.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunRecord {
    /// The SR uuid this run was for.
    pub sr: String,
    /// `true` on a successful snapshot, `false` on failure.
    pub ok: bool,
    /// Unix seconds the run completed at.
    pub ts: u64,
    /// The new snapshot uuid on success (empty on failure).
    pub snapshot: String,
    /// A short failure detail on failure (empty on success).
    pub detail: String,
}

impl RunRecord {
    /// JSON body for the `event/dc/snap-schedule-run/<sr>` write.
    #[must_use]
    pub fn body(&self) -> String {
        serde_json::json!({
            "status": if self.ok { "ok" } else { "fail" },
            "sr": self.sr,
            "ts": self.ts,
            "snapshot": self.snapshot,
            "detail": self.detail,
        })
        .to_string()
    }

    /// Recover the last-run unix-seconds timestamp from a previously-written run
    /// record body (`ts` field). `None` for a non-JSON / fieldless body.
    #[must_use]
    pub fn last_ts_from_body(body: &str) -> Option<u64> {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()?
            .get("ts")
            .and_then(serde_json::Value::as_u64)
    }
}

// ---- pure cadence + retention logic (unit-tested without xe) ----

/// Pure cadence decision: is a snapshot due now for an SR?
///
/// Returns `true` when the SR has never been snapshotted (`last_run_secs ==
/// None`) or when at least `interval` seconds have elapsed since the last run
/// (`now - last >= interval`). A `now` earlier than `last` (clock skew) is treated
/// as not-yet-due. Mirrors `dr_scheduler::due`.
#[must_use]
pub fn due(last_run_secs: Option<u64>, now_secs: u64, interval: u64) -> bool {
    match last_run_secs {
        None => true,
        Some(last) => now_secs.saturating_sub(last) >= interval,
    }
}

/// Pure retention selection: given the scheduler-made snapshots for an SR as
/// `(uuid, snapshot_time)` pairs (oldest-or-newest order irrelevant) and a
/// retention count `keep`, return the uuids of the snapshots to DESTROY — the
/// oldest beyond the newest `keep`. Stable: ties on time keep input order.
///
/// `keep == 0` is treated as `keep == 1` (the schedule guarantees ≥1, but the
/// helper is defensive so it never destroys a freshly-made snapshot). The caller
/// only ever feeds this its OWN (prefix-tagged) snapshots, so the result can
/// never name an operator's hand-made snapshot.
#[must_use]
pub fn prune_targets(snapshots: &[(String, i64)], keep: u64) -> Vec<String> {
    let keep = usize::try_from(keep.max(1)).unwrap_or(usize::MAX);
    if snapshots.len() <= keep {
        return Vec::new();
    }
    // Sort newest-first (descending time); stable so equal-time ties hold input
    // order. The first `keep` survive; the rest are destroyed.
    let mut idx: Vec<usize> = (0..snapshots.len()).collect();
    idx.sort_by(|&a, &b| snapshots[b].1.cmp(&snapshots[a].1));
    idx.into_iter()
        .skip(keep)
        .map(|i| snapshots[i].0.clone())
        .collect()
}

/// Parse the `xe snapshot-list … params=uuid,snapshot-time --minimal` output the
/// worker lists scheduler snapshots with. XAPI `--minimal` prints one record per
/// line as comma-joined param values in the requested order (`uuid,snapshot-time`);
/// the snapshot-time is an ISO-8601-ish `20260625T12:00:00Z`. Returns
/// `(uuid, epoch_secs)` pairs, skipping malformed lines (best-effort). Pure.
#[must_use]
pub fn parse_snapshot_list(stdout: &str) -> Vec<(String, i64)> {
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (uuid, ts) = line.split_once(',')?;
            let uuid = uuid.trim();
            if uuid.is_empty() {
                return None;
            }
            Some((uuid.to_string(), parse_xapi_time(ts.trim())))
        })
        .collect()
}

/// Parse XAPI's `snapshot-time` (`20260625T12:00:00Z` or `2026-06-25T12:00:00Z`)
/// to epoch seconds. Falls back to `0` for an unparseable value so such a snapshot
/// sorts oldest (it gets pruned first, never kept over a parseable one). Pure.
#[must_use]
fn parse_xapi_time(ts: &str) -> i64 {
    // XAPI's basic-format `YYYYMMDDTHH:MM:SSZ`.
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y%m%dT%H:%M:%SZ") {
        return dt.and_utc().timestamp();
    }
    // Extended-format fallback `YYYY-MM-DDTHH:MM:SSZ`.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        return dt.timestamp();
    }
    0
}

/// Build the `xe vdi-snapshot` argument string for an SR's snapshot, validated +
/// labelled. PURE. Snapshots EVERY VDI on the SR (XAPI has no SR-level snapshot —
/// same loop `storage_ops::sr_snapshot_all_command` uses), tagging each new
/// snapshot with the [`SNAP_LABEL_PREFIX`]-prefixed name-label so retention can
/// recognise its own. Echoes the LAST new-snapshot uuid on stdout.
///
/// # Errors
/// Returns `Err` for an empty/invalid `sr_uuid` (the same injection guard the
/// storage RPCs use — `[0-9a-fA-F-]` only).
pub fn snapshot_command(sr_uuid: &str, label: &str) -> Result<String, String> {
    check_uuid("sr_uuid", sr_uuid)?;
    check_label(label)?;
    // For each VDI on the SR: snapshot it, then label the new snapshot so
    // retention recognises it. `vdi-snapshot` prints the new uuid; capture it,
    // set its name-label, and echo the last one for the run record.
    Ok(format!(
        "last=; for v in $(xe vdi-list sr-uuid={sr_uuid} params=uuid --minimal | tr , ' '); do \
         s=$(xe vdi-snapshot uuid=$v 2>/dev/null) && \
         xe vdi-param-set uuid=$s name-label={label} >/dev/null 2>&1 && last=$s; done; echo \"$last\""
    ))
}

/// Build the `xe vdi-list` argument string that lists THIS SR's scheduler-made
/// snapshot VDIs (those whose name-label carries [`SNAP_LABEL_PREFIX`]), printing
/// `uuid,snapshot-time` per line. PURE. Validated `sr_uuid`.
///
/// # Errors
/// Returns `Err` for an empty/invalid `sr_uuid`.
pub fn list_snapshots_command(sr_uuid: &str, label: &str) -> Result<String, String> {
    check_uuid("sr_uuid", sr_uuid)?;
    check_label(label)?;
    Ok(format!(
        "vdi-list sr-uuid={sr_uuid} name-label={label} params=uuid,snapshot-time --minimal"
    ))
}

/// Build the `xe vdi-destroy` argument string for one scheduler snapshot. PURE.
///
/// # Errors
/// Returns `Err` for an empty/invalid `uuid`.
pub fn destroy_command(uuid: &str) -> Result<String, String> {
    check_uuid("uuid", uuid)?;
    Ok(format!("vdi-destroy uuid={uuid}"))
}

/// A xen object uuid is a hex+dash string — the SAME command-injection guard
/// `ipc::storage_ops::check_uuid` applies before interpolating into a remote
/// `xe …` string. Returns `Err` for an empty value or any char outside
/// `[0-9a-fA-F-]`. Pure.
fn check_uuid(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("empty {field}"));
    }
    if !value.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(format!("{field} contains invalid characters"));
    }
    Ok(())
}

/// The scheduler's own name-label must be `[A-Za-z0-9._-]` only — the SAME class
/// the storage module sanitizes name-labels to, since it is interpolated into the
/// remote `xe … name-label=<label>` string. Pure. (The label is worker-built, not
/// caller-supplied, but the guard keeps the command-builder injection-proof by
/// construction.)
fn check_label(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("empty label".into());
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("label contains invalid characters".into());
    }
    Ok(())
}

// ---- thin I/O: read schedules, run snapshots over SSH, write run records ----

/// Current unix-seconds wall clock (0 on a pre-epoch skew, never panics).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Current unix-minute bucket — the alert-id dedupe granularity (mirrors
/// `etcd_watch::now_minute`).
fn now_minute() -> u64 {
    now_secs() / 60
}

/// The SSH-`xe` runner — mirrors `ipc::storage_ops::ssh_xe_status` EXACTLY (same
/// flags: identity, no host-key prompt, batch mode, 8 s connect timeout), reusing
/// the orchestrator's mesh-key resolver. The remote string is a full `xe …`
/// command (or a `for`-loop over `xe …`), already injection-guarded by the pure
/// command-builders above.
fn ssh_xe(key: &str, dom0: &str, remote: &str) -> std::io::Result<std::process::Output> {
    let mut cmd = std::process::Command::new("ssh");
    cmd.args([
        "-i",
        key,
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=8",
        &format!("root@{dom0}"),
        remote,
    ]);
    // Bound the whole command (not just connect setup) so a stalled `xe` is
    // killed at SSH_XE_TIMEOUT instead of blocking indefinitely (mackesd-02).
    super::proc::output_with_timeout(cmd, SSH_XE_TIMEOUT)
}

/// Drop one failure alert into the `alert_relay` watch dir (best-effort — a dir or
/// write failure is logged via the relay's own absence, never fatal). Mirrors
/// `etcd_watch::emit`.
fn emit_alert(alerts_dir: &std::path::Path, sr: &str, detail: &str) {
    if std::fs::create_dir_all(alerts_dir).is_err() {
        return;
    }
    let minute = now_minute();
    let id = format!("dc-snap-sched-fail-{sr}-{minute}");
    let alert = serde_json::json!({
        "id": id,
        "severity": "warn",
        "alert": "dc.snap_schedule.failed",
        "host": sr,
        "summary": format!("Scheduled snapshot of SR {sr} failed: {}", detail_summary(detail)),
    });
    let path = alerts_dir.join(format!("{id}.json"));
    let _ = std::fs::write(path, alert.to_string());
}

/// One complete, effect-free read of durable scheduler authority.
#[derive(Default)]
struct StagedPass {
    schedules: BTreeMap<String, Schedule>,
    last_runs: BTreeMap<String, u64>,
}

/// Stage scheduler authority without advancing a cursor or performing an xe
/// effect. Implemented as a seam so hostile read failures are deterministic.
trait PassReader: Send + Sync {
    fn stage(&self, persist: &Persist) -> Result<StagedPass, String>;
}

struct DurablePassReader;

impl PassReader for DurablePassReader {
    fn stage(&self, persist: &Persist) -> Result<StagedPass, String> {
        stage_pass(persist)
    }
}

/// Stage every schedule plus its corresponding run-history lane as one
/// transaction. All values remain local until every enumeration/read succeeds;
/// one failed topic therefore defers the whole sweep without a partial fold.
fn stage_pass(persist: &Persist) -> Result<StagedPass, String> {
    let topics = persist
        .list_topics()
        .map_err(|error| format!("enumerate scheduler topics: {error}"))?;
    let mut schedules = BTreeMap::new();
    for topic in topics
        .iter()
        .filter(|topic| topic.starts_with(SCHEDULE_PREFIX))
    {
        let messages = persist
            .list_since(topic, None)
            .map_err(|error| format!("read durable schedule {topic}: {error}"))?;
        // Preserve the supported durable-config fold: newest parseable save wins.
        if let Some(schedule) = messages
            .iter()
            .rev()
            .find_map(|message| message.body.as_deref().and_then(Schedule::parse))
        {
            schedules.insert(schedule.sr.clone(), schedule);
        }
    }

    let mut last_runs = BTreeMap::new();
    for sr in schedules.keys() {
        let topic = run_topic(sr);
        let messages = persist
            .list_since(&topic, None)
            .map_err(|error| format!("read durable run history {topic}: {error}"))?;
        if let Some(ts) = messages.iter().rev().find_map(|message| {
            message
                .body
                .as_deref()
                .and_then(RunRecord::last_ts_from_body)
        }) {
            last_runs.insert(sr.clone(), ts);
        }
    }
    Ok(StagedPass {
        schedules,
        last_runs,
    })
}

/// Run-history publication is a required durability boundary, not a best-effort
/// log. Errors retain the completed result in the bounded in-memory pending
/// ledger for the lifetime of this worker process.
trait RunPublisher: Send + Sync {
    fn publish(&self, persist: &Persist, record: &RunRecord) -> Result<(), String>;
}

struct DurableRunPublisher;

impl RunPublisher for DurableRunPublisher {
    fn publish(&self, persist: &Persist, record: &RunRecord) -> Result<(), String> {
        persist
            .write(
                &run_topic(&record.sr),
                mde_bus::hooks::config::Priority::Default,
                Some("snap-schedule-run"),
                Some(&record.body()),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Take a snapshot of one SR over SSH and return the run record. The dom0 is
/// allow-list-checked (reusing the orchestrator's `xen_dom0s` set) BEFORE any SSH,
/// exactly as the storage RPCs are. A missing dom0 / a dom0 outside the allow-list
/// / a non-zero `xe` exit / a spawn failure each degrade to a `fail` record and
/// never panic.
fn run_snapshot(sched: &Schedule) -> RunRecord {
    let fail = |detail: String| RunRecord {
        sr: sched.sr.clone(),
        ok: false,
        ts: now_secs(),
        snapshot: String::new(),
        detail: detail_summary(&detail),
    };
    if sched.dom0.is_empty() {
        return fail("schedule has no dom0 to snapshot on".into());
    }
    // SECURITY: only ever SSH a dom0 in the orchestrator's configured allow-list.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == &sched.dom0)
    {
        return fail(format!("dom0 {} not in allowed set", sched.dom0));
    }
    let cmd = match snapshot_command(&sched.sr, SNAP_LABEL_PREFIX) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe(&key, &sched.dom0, &remote) {
        Ok(o) if o.status.success() => {
            let snapshot = String::from_utf8_lossy(&o.stdout).trim().to_string();
            RunRecord {
                sr: sched.sr.clone(),
                ok: true,
                ts: now_secs(),
                snapshot,
                detail: String::new(),
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let code = o
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string());
            fail(format!("xe vdi-snapshot exit {code}: {}", stderr.trim()))
        }
        Err(e) => fail(format!("ssh failed: {e}")),
    }
}

/// Enforce retention for one SR over SSH: list THIS SR's scheduler-made snapshots
/// (prefix-tagged only), select the oldest beyond `keep` via the pure
/// [`prune_targets`], and `xe vdi-destroy` each. Best-effort — every step degrades
/// to a skip on error and never panics. Returns the count destroyed (for logging).
fn enforce_retention(sched: &Schedule) -> usize {
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let Ok(list_cmd) = list_snapshots_command(&sched.sr, SNAP_LABEL_PREFIX) else {
        return 0;
    };
    let stdout = match ssh_xe(&key, &sched.dom0, &format!("xe {list_cmd}")) {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return 0,
    };
    let snaps = parse_snapshot_list(&stdout);
    let targets = prune_targets(&snaps, sched.retention);
    let mut destroyed = 0;
    for uuid in targets {
        let Ok(destroy) = destroy_command(&uuid) else {
            continue;
        };
        if let Ok(o) = ssh_xe(&key, &sched.dom0, &format!("xe {destroy}")) {
            if o.status.success() {
                destroyed += 1;
            }
        }
    }
    destroyed
}

/// Xe effect seam. Production delegates to the existing snapshot and retention
/// paths; tests count effects without touching Xen.
trait SnapEffects: Send + Sync {
    fn snapshot(&self, schedule: &Schedule) -> RunRecord;
    fn prune(&self, schedule: &Schedule) -> usize;
}

struct ProductionSnapEffects;

impl SnapEffects for ProductionSnapEffects {
    fn snapshot(&self, schedule: &Schedule) -> RunRecord {
        run_snapshot(schedule)
    }

    fn prune(&self, schedule: &Schedule) -> usize {
        enforce_retention(schedule)
    }
}

/// One scheduler pass. The durable read is completed before pending publication
/// retries or any xe effect. A completed snapshot result enters the in-memory
/// `pending` barrier before prune/publication, and new effects stop when that
/// ledger is full. A process crash before publication can still lose this state
/// and repeat the snapshot after restart.
fn run_pass(
    persist: &Persist,
    alerts_dir: &std::path::Path,
    reader: &dyn PassReader,
    effects: &dyn SnapEffects,
    publisher: &dyn RunPublisher,
    pending: &mut BTreeMap<String, RunRecord>,
) -> Result<(), String> {
    let mut staged = reader.stage(persist)?;

    // Retry completed-but-unpublished results first. A success is folded into
    // this pass immediately so the stale staged history cannot trigger a repeat.
    let pending_srs: Vec<String> = pending.keys().cloned().collect();
    for sr in pending_srs {
        let Some(record) = pending.get(&sr) else {
            continue;
        };
        match publisher.publish(persist, record) {
            Ok(()) => {
                staged.last_runs.insert(sr.clone(), record.ts);
                pending.remove(&sr);
            }
            Err(error) => tracing::warn!(
                sr,
                %error,
                "dc_snap_scheduler: pending run-history publication still unavailable"
            ),
        }
    }

    let now = now_secs();
    for sched in staged.schedules.values() {
        if pending.contains_key(&sched.sr) {
            continue;
        }
        let last = staged.last_runs.get(&sched.sr).copied();
        if !due(last, now, sched.interval_secs) {
            continue;
        }
        if pending.len() >= MAX_PENDING_RESULTS {
            tracing::warn!(
                limit = MAX_PENDING_RESULTS,
                "dc_snap_scheduler: pending result ledger full; deferring new effects"
            );
            break;
        }

        let rec = effects.snapshot(sched);
        // Install the same-worker completion guard before prune or publication.
        // If the Bus write fails, future passes in this process retry this result
        // instead of snapshotting. This guard is intentionally not crash-durable.
        pending.insert(sched.sr.clone(), rec.clone());
        if rec.ok {
            let n = effects.prune(sched);
            tracing::info!(sr = %sched.sr, snapshot = %rec.snapshot, pruned = n,
                "dc_snap_scheduler: snapshot taken + retention enforced");
        } else {
            tracing::warn!(sr = %sched.sr, detail = %rec.detail, "dc_snap_scheduler: snapshot failed");
            emit_alert(alerts_dir, &sched.sr, &rec.detail);
        }
        match publisher.publish(persist, &rec) {
            Ok(()) => {
                staged.last_runs.insert(sched.sr.clone(), rec.ts);
                pending.remove(&sched.sr);
            }
            Err(error) => tracing::warn!(
                sr = %sched.sr,
                %error,
                "dc_snap_scheduler: retaining completed result for publication retry"
            ),
        }
    }
    Ok(())
}

fn scheduler_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    scheduler_bus_root_or_system(override_root.or_else(mde_bus::default_data_dir))
}

fn scheduler_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn next_bus_retry_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL)
}

/// Bus open seam shared by startup recovery and per-tick reopening.
trait BusFactory: Send + Sync {
    fn open(&self, root: &std::path::Path) -> Result<Option<Persist>, String>;
}

struct PersistBusFactory;

impl BusFactory for PersistBusFactory {
    fn open(&self, root: &std::path::Path) -> Result<Option<Persist>, String> {
        Persist::open(root.to_path_buf())
            .map(Some)
            .map_err(|error| error.to_string())
    }
}

/// The supervised worker. Leader-gated (only the elected node snapshots +
/// publishes, so a multi-node mesh runs one snapshot per SR per interval) and
/// best-effort.
pub struct DcSnapSchedulerWorker {
    tick_interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
    alerts_dir: PathBuf,
    bus_root_override: Option<PathBuf>,
    bus_factory: Arc<dyn BusFactory>,
    reader: Arc<dyn PassReader>,
    effects: Arc<dyn SnapEffects>,
    publisher: Arc<dyn RunPublisher>,
    pending_results: BTreeMap<String, RunRecord>,
    #[cfg(test)]
    leader_override: Option<bool>,
}

impl DcSnapSchedulerWorker {
    /// Construct with production defaults (5 min tick, the shared leader lock
    /// under `workgroup_root`, the default Bus root + alert dir).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String, alerts_dir: PathBuf) -> Self {
        Self {
            tick_interval: TICK_INTERVAL,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            node_id,
            alerts_dir,
            bus_root_override: None,
            bus_factory: Arc::new(PersistBusFactory),
            reader: Arc::new(DurablePassReader),
            effects: Arc::new(ProductionSnapEffects),
            publisher: Arc::new(DurableRunPublisher),
            pending_results: BTreeMap::new(),
            #[cfg(test)]
            leader_override: None,
        }
    }

    /// Override the Bus root directory. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_tick_interval(mut self, interval: Duration) -> Self {
        self.tick_interval = interval;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_bus_factory(mut self, factory: Arc<dyn BusFactory>) -> Self {
        self.bus_factory = factory;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_reader(mut self, reader: Arc<dyn PassReader>) -> Self {
        self.reader = reader;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_effects(mut self, effects: Arc<dyn SnapEffects>) -> Self {
        self.effects = effects;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_publisher(mut self, publisher: Arc<dyn RunPublisher>) -> Self {
        self.publisher = publisher;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_leader(mut self, leader: bool) -> Self {
        self.leader_override = Some(leader);
        self
    }

    /// Only the directory leader runs the snapshots (no-fixed-center: any eligible
    /// node can be it, the elected one runs + publishes). Reuses the shared leader
    /// lock.
    fn is_leader(&self) -> bool {
        #[cfg(test)]
        if let Some(leader) = self.leader_override {
            return leader;
        }

        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
    }
}

#[async_trait::async_trait]
impl Worker for DcSnapSchedulerWorker {
    fn name(&self) -> &'static str {
        "dc_snap_scheduler"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = scheduler_bus_root(self.bus_root_override.clone());
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        loop {
            match self.bus_factory.open(&bus_root) {
                Ok(Some(_)) => break,
                Ok(None) => {
                    tracing::debug!("dc_snap_scheduler: Bus unavailable; startup will retry")
                }
                Err(error) => tracing::warn!(
                    %error,
                    "dc_snap_scheduler: Persist open failed; startup will retry"
                ),
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(retry_interval) => {}
            }
            retry_interval = next_bus_retry_interval(retry_interval);
        }

        loop {
            if self.is_leader() {
                // run_pass shells `ssh … xe vdi-snapshot/vdi-destroy` serially
                // across every due SR — potentially minutes of blocking work.
                // Run it OFF the runtime thread so it can neither pin a worker nor
                // starve the watchdog beat (mackesd-02 / WATCHDOG-2).
                let bus_root_tick = bus_root.clone();
                let alerts_dir = self.alerts_dir.clone();
                let bus_factory = Arc::clone(&self.bus_factory);
                let reader = Arc::clone(&self.reader);
                let effects = Arc::clone(&self.effects);
                let publisher = Arc::clone(&self.publisher);
                // Keep a pre-pass fallback outside the blocking task. A panic can
                // still lose a newly completed effect inside that task, but must
                // not discard pending results that existed before this pass.
                let pending_fallback = self.pending_results.clone();
                let mut pending = std::mem::take(&mut self.pending_results);
                match tokio::task::spawn_blocking(move || {
                    match bus_factory.open(&bus_root_tick) {
                        Ok(Some(persist)) => {
                            if let Err(error) = run_pass(
                                &persist,
                                &alerts_dir,
                                reader.as_ref(),
                                effects.as_ref(),
                                publisher.as_ref(),
                                &mut pending,
                            ) {
                                tracing::warn!(
                                    %error,
                                    "dc_snap_scheduler: authority staging failed; sweep deferred"
                                );
                            }
                        }
                        Ok(None) => tracing::debug!(
                            "dc_snap_scheduler: tick Bus unavailable; sweep deferred"
                        ),
                        Err(error) => tracing::warn!(
                            %error,
                            "dc_snap_scheduler: tick Persist open failed; sweep deferred"
                        ),
                    }
                    pending
                })
                .await
                {
                    Ok(pending) => self.pending_results = pending,
                    Err(error) => {
                        self.pending_results = pending_fallback;
                        tracing::warn!(
                            %error,
                            "dc_snap_scheduler: snapshot pass task join failed; restored pre-pass pending results"
                        );
                    }
                }
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.tick_interval) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    // ---- topics ----

    #[test]
    fn topics_format_under_the_dc_lanes() {
        assert_eq!(schedule_topic("sr-1"), "event/dc/snap-schedule/sr-1");
        assert_eq!(run_topic("sr-1"), "event/dc/snap-schedule-run/sr-1");
    }

    // ---- mackesd-02: bounded SSH `xe` invocation ----

    #[test]
    fn ssh_xe_timeout_is_generous_but_finite() {
        // A snapshot of a large SR legitimately needs a couple of minutes, so the
        // bound must be large — but finite so a stalled `xe` can't block a thread
        // forever (mackesd-02). It must also exceed the 8 s SSH connect cap.
        assert!(SSH_XE_TIMEOUT >= Duration::from_secs(60));
        assert!(SSH_XE_TIMEOUT > Duration::from_secs(8));
    }

    // ---- due-decision logic (cadence elapsed vs not) ----

    #[test]
    fn due_is_true_when_never_run() {
        assert!(due(None, 0, 86_400));
        assert!(due(None, 1_000_000, 86_400));
    }

    #[test]
    fn due_is_false_before_the_interval_elapses() {
        // Ran at t=1000, now t=1001, interval=daily → not yet due.
        assert!(!due(Some(1000), 1001, 86_400));
        // One second short of the interval → still not due.
        assert!(!due(Some(1000), 1000 + 86_399, 86_400));
    }

    #[test]
    fn due_is_true_once_the_interval_has_elapsed() {
        // Exactly the interval → due.
        assert!(due(Some(1000), 1000 + 86_400, 86_400));
        // Well past → due.
        assert!(due(Some(1000), 1000 + 200_000, 86_400));
    }

    #[test]
    fn due_handles_clock_skew_as_not_due() {
        // now earlier than last → saturating delta 0 → not due.
        assert!(!due(Some(5000), 1000, 86_400));
    }

    // ---- retention-prune selection ----

    #[test]
    fn prune_keeps_n_and_destroys_the_oldest_beyond_it() {
        // 5 scheduler snapshots, keep 2 → the 3 oldest are destroyed, the 2
        // newest survive. Times deliberately out of input order.
        let snaps = vec![
            ("a".to_string(), 100),
            ("b".to_string(), 500), // newest
            ("c".to_string(), 200),
            ("d".to_string(), 400), // 2nd newest
            ("e".to_string(), 300),
        ];
        let mut destroyed = prune_targets(&snaps, 2);
        destroyed.sort();
        // Newest two (b=500, d=400) survive; a,c,e are destroyed.
        assert_eq!(
            destroyed,
            vec!["a".to_string(), "c".to_string(), "e".to_string()]
        );
    }

    #[test]
    fn prune_keeps_everything_when_at_or_below_retention() {
        let snaps = vec![("a".to_string(), 100), ("b".to_string(), 200)];
        // Exactly the retention count → nothing to destroy.
        assert!(prune_targets(&snaps, 2).is_empty());
        // Fewer than retention → nothing to destroy.
        assert!(prune_targets(&snaps, 5).is_empty());
        // Empty input → nothing to destroy.
        assert!(prune_targets(&[], 3).is_empty());
    }

    #[test]
    fn prune_keep_one_destroys_all_but_the_newest() {
        let snaps = vec![
            ("old".to_string(), 100),
            ("new".to_string(), 300),
            ("mid".to_string(), 200),
        ];
        let destroyed = prune_targets(&snaps, 1);
        // Only the single newest ("new") survives.
        assert_eq!(destroyed.len(), 2);
        assert!(destroyed.contains(&"old".to_string()));
        assert!(destroyed.contains(&"mid".to_string()));
        assert!(!destroyed.contains(&"new".to_string()));
    }

    #[test]
    fn prune_keep_zero_is_treated_as_keep_one() {
        // Defensive: keep=0 must never destroy a freshly-made snapshot — it
        // behaves as keep=1.
        let snaps = vec![("a".to_string(), 100), ("b".to_string(), 200)];
        let destroyed = prune_targets(&snaps, 0);
        assert_eq!(destroyed, vec!["a".to_string()]); // only the newest (b) kept
    }

    #[test]
    fn prune_only_ever_names_inputs_it_was_given() {
        // The worker only feeds prune_targets its OWN prefix-tagged snapshots, so
        // every returned uuid is one of the inputs — never an operator snapshot.
        let snaps = vec![
            ("s1".to_string(), 1),
            ("s2".to_string(), 2),
            ("s3".to_string(), 3),
        ];
        for uuid in prune_targets(&snaps, 1) {
            assert!(["s1", "s2", "s3"].contains(&uuid.as_str()));
        }
    }

    // ---- snapshot-list parsing (feeds the retention selection) ----

    #[test]
    fn parse_snapshot_list_reads_uuid_and_time() {
        let out = "aaaa-1,20260625T12:00:00Z\nbbbb-2,20260624T12:00:00Z\n";
        let parsed = parse_snapshot_list(out);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "aaaa-1");
        assert_eq!(parsed[1].0, "bbbb-2");
        // The 25th sorts newer than the 24th.
        assert!(parsed[0].1 > parsed[1].1);
    }

    #[test]
    fn parse_snapshot_list_skips_garbage_and_blank_lines() {
        let out = "\ngood-1,20260625T12:00:00Z\nno-comma-line\n,emptyuuid\n";
        let parsed = parse_snapshot_list(out);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "good-1");
    }

    #[test]
    fn parse_snapshot_list_tolerates_extended_time_and_unparseable() {
        // Extended RFC3339 form parses; a junk time falls back to 0 (sorts oldest).
        let out = "ext-1,2026-06-25T12:00:00+00:00\njunk-1,not-a-time\n";
        let parsed = parse_snapshot_list(out);
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].1 > 0);
        assert_eq!(parsed[1].1, 0);
        // Pruning to keep 1 destroys the junk-time (oldest) one.
        let destroyed = prune_targets(&parsed, 1);
        assert_eq!(destroyed, vec!["junk-1".to_string()]);
    }

    // ---- schedule record (de)serialization ----

    #[test]
    fn schedule_parses_the_panel_record_shape() {
        // The exact JSON the Workbench Storage tab's snap_schedule_save writes.
        let body = r#"{"kind":"snap-schedule","id":"sr-1","sr":"sr-1","retention":3,"backup_target":"remote-sr","dom0":"172.20.0.9"}"#;
        let s = Schedule::parse(body).expect("valid schedule parses");
        assert_eq!(s.sr, "sr-1");
        assert_eq!(s.dom0, "172.20.0.9");
        assert_eq!(s.retention, 3);
        assert_eq!(s.backup_target, "remote-sr");
        // No cadence in the panel record → daily default.
        assert_eq!(s.interval_secs, DEFAULT_INTERVAL_SECS);
    }

    #[test]
    fn schedule_reads_an_explicit_cadence_when_present() {
        let body =
            r#"{"kind":"snap-schedule","sr":"sr-2","retention":2,"interval_secs":3600,"dom0":"h"}"#;
        let s = Schedule::parse(body).unwrap();
        assert_eq!(s.interval_secs, 3600);
        // The `cadence` alias is honored too.
        let body2 =
            r#"{"kind":"snap-schedule","sr":"sr-3","retention":1,"cadence":7200,"dom0":"h"}"#;
        assert_eq!(Schedule::parse(body2).unwrap().interval_secs, 7200);
    }

    #[test]
    fn schedule_rejects_garbage_and_invalid_records() {
        // Non-JSON.
        assert!(Schedule::parse("not json").is_none());
        // Wrong kind.
        assert!(Schedule::parse(r#"{"kind":"other","sr":"s","retention":1}"#).is_none());
        // Missing / empty sr.
        assert!(Schedule::parse(r#"{"kind":"snap-schedule","retention":1}"#).is_none());
        assert!(Schedule::parse(r#"{"kind":"snap-schedule","sr":"","retention":1}"#).is_none());
        // Zero / missing retention.
        assert!(Schedule::parse(r#"{"kind":"snap-schedule","sr":"s","retention":0}"#).is_none());
        assert!(Schedule::parse(r#"{"kind":"snap-schedule","sr":"s"}"#).is_none());
    }

    #[test]
    fn run_record_body_round_trips_status_and_fields() {
        let ok = RunRecord {
            sr: "sr-1".into(),
            ok: true,
            ts: 1_700_000_000,
            snapshot: "snap-9".into(),
            detail: String::new(),
        };
        let v: serde_json::Value = serde_json::from_str(&ok.body()).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["sr"], "sr-1");
        assert_eq!(v["ts"], 1_700_000_000_u64);
        assert_eq!(v["snapshot"], "snap-9");
        // The ts is recoverable for last-run tracking across restarts.
        assert_eq!(
            RunRecord::last_ts_from_body(&ok.body()),
            Some(1_700_000_000)
        );

        let fail = RunRecord {
            sr: "sr-2".into(),
            ok: false,
            ts: 1_700_000_500,
            snapshot: String::new(),
            detail: "ssh failed: timeout".into(),
        };
        let v: serde_json::Value = serde_json::from_str(&fail.body()).unwrap();
        assert_eq!(v["status"], "fail");
        assert_eq!(v["detail"], "ssh failed: timeout");
    }

    #[test]
    fn last_ts_from_body_is_none_for_garbage() {
        assert_eq!(RunRecord::last_ts_from_body("not json"), None);
        assert_eq!(RunRecord::last_ts_from_body(r#"{"status":"ok"}"#), None);
    }

    // ---- command builders reuse the storage injection guard ----

    #[test]
    fn snapshot_command_labels_with_the_scheduler_prefix() {
        let c = snapshot_command("5ab1-c0de", SNAP_LABEL_PREFIX).unwrap();
        assert!(c.contains("xe vdi-list sr-uuid=5ab1-c0de"));
        assert!(c.contains("xe vdi-snapshot uuid=$v"));
        assert!(c.contains(&format!("name-label={SNAP_LABEL_PREFIX}")));
    }

    #[test]
    fn command_builders_reject_injection() {
        // Same `[0-9a-fA-F-]` uuid guard the storage RPCs use.
        assert!(snapshot_command("sr;rm -rf /", SNAP_LABEL_PREFIX).is_err());
        assert!(snapshot_command("sr$(x)", SNAP_LABEL_PREFIX).is_err());
        assert!(snapshot_command("", SNAP_LABEL_PREFIX).is_err());
        assert!(list_snapshots_command("sr`x`", SNAP_LABEL_PREFIX).is_err());
        assert!(destroy_command("uuid;evil").is_err());
        // A label with shell metacharacters is rejected too.
        assert!(snapshot_command("5ab1-c0de", "bad;label").is_err());
    }

    #[test]
    fn list_and_destroy_commands_have_the_expected_shape() {
        let list = list_snapshots_command("5ab1-c0de", SNAP_LABEL_PREFIX).unwrap();
        assert_eq!(
            list,
            format!(
                "vdi-list sr-uuid=5ab1-c0de name-label={SNAP_LABEL_PREFIX} params=uuid,snapshot-time --minimal"
            )
        );
        // The uuid guard is hex+dash only (the storage RPCs' class), so the
        // fixture uses a hex uuid.
        assert_eq!(
            destroy_command("5ab1-c0de").unwrap(),
            "vdi-destroy uuid=5ab1-c0de"
        );
    }

    struct LateBusFactory {
        root: PathBuf,
        attempts: Arc<AtomicUsize>,
    }

    impl BusFactory for LateBusFactory {
        fn open(&self, _root: &std::path::Path) -> Result<Option<Persist>, String> {
            match self.attempts.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(None),
                1 => Err("injected unopenable Bus".into()),
                _ => Persist::open(self.root.clone())
                    .map(Some)
                    .map_err(|error| error.to_string()),
            }
        }
    }

    struct UnavailableBusFactory {
        attempts: Arc<AtomicUsize>,
    }

    impl BusFactory for UnavailableBusFactory {
        fn open(&self, _root: &std::path::Path) -> Result<Option<Persist>, String> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }

    #[derive(Default)]
    struct FakeEffects {
        snapshots: Mutex<Vec<String>>,
        prunes: Mutex<Vec<String>>,
    }

    impl SnapEffects for FakeEffects {
        fn snapshot(&self, schedule: &Schedule) -> RunRecord {
            self.snapshots.lock().unwrap().push(schedule.sr.clone());
            RunRecord {
                sr: schedule.sr.clone(),
                ok: true,
                ts: now_secs(),
                snapshot: format!("snapshot-{}", schedule.sr),
                detail: String::new(),
            }
        }

        fn prune(&self, schedule: &Schedule) -> usize {
            self.prunes.lock().unwrap().push(schedule.sr.clone());
            0
        }
    }

    struct HostileReader {
        mode: Arc<AtomicUsize>,
        enumeration_failures: Arc<AtomicUsize>,
        topic_failures: Arc<AtomicUsize>,
    }

    impl PassReader for HostileReader {
        fn stage(&self, persist: &Persist) -> Result<StagedPass, String> {
            match self.mode.load(Ordering::SeqCst) {
                0 => {
                    self.enumeration_failures.fetch_add(1, Ordering::SeqCst);
                    Err("injected list_topics failure".into())
                }
                1 => {
                    let topics = persist.list_topics().map_err(|error| error.to_string())?;
                    if let Some(topic) = topics
                        .iter()
                        .find(|topic| topic.starts_with(SCHEDULE_PREFIX))
                    {
                        persist
                            .list_since(topic, None)
                            .map_err(|error| error.to_string())?;
                    }
                    self.topic_failures.fetch_add(1, Ordering::SeqCst);
                    Err("injected later schedule-topic read failure".into())
                }
                _ => stage_pass(persist),
            }
        }
    }

    struct PanicOnceReader {
        attempts: Arc<AtomicUsize>,
    }

    impl PassReader for PanicOnceReader {
        fn stage(&self, _persist: &Persist) -> Result<StagedPass, String> {
            assert!(
                self.attempts.fetch_add(1, Ordering::SeqCst) != 0,
                "injected blocking-pass panic"
            );
            Err("defer after injected panic".into())
        }
    }

    struct GatePublisher {
        allow: Arc<AtomicBool>,
        attempts: Arc<AtomicUsize>,
    }

    impl RunPublisher for GatePublisher {
        fn publish(&self, persist: &Persist, record: &RunRecord) -> Result<(), String> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if !self.allow.load(Ordering::SeqCst) {
                return Err("injected publication failure".into());
            }
            DurableRunPublisher.publish(persist, record)
        }
    }

    fn write_schedule(persist: &Persist, sr: &str, interval_secs: u64) {
        let body = serde_json::json!({
            "kind": "snap-schedule",
            "sr": sr,
            "retention": 2,
            "interval_secs": interval_secs,
            "dom0": "test-dom0",
        })
        .to_string();
        persist
            .write(
                &schedule_topic(sr),
                mde_bus::hooks::config::Priority::Default,
                Some("snap-schedule"),
                Some(&body),
            )
            .unwrap();
    }

    fn write_history(persist: &Persist, sr: &str, ts: u64) {
        let record = RunRecord {
            sr: sr.to_string(),
            ok: true,
            ts,
            snapshot: "retained-snapshot".into(),
            detail: String::new(),
        };
        DurableRunPublisher.publish(persist, &record).unwrap();
    }

    fn worker_for_test(root: &std::path::Path) -> DcSnapSchedulerWorker {
        DcSnapSchedulerWorker::new(
            root.join("workgroup"),
            "test-node".into(),
            root.join("alerts"),
        )
        .with_bus_root(root.join("bus"))
        .with_tick_interval(Duration::from_millis(5))
        .with_leader(true)
    }

    #[tokio::test]
    async fn late_bus_replays_retained_authority_and_discovers_dynamic_schedules() {
        let dir = tempfile::tempdir().unwrap();
        let bus_root = dir.path().join("bus");
        let persist = Persist::open(bus_root.clone()).unwrap();
        write_schedule(&persist, "retained-due", 86_400);
        write_schedule(&persist, "retained-history", 1);
        write_history(&persist, "retained-history", u64::MAX);

        let attempts = Arc::new(AtomicUsize::new(0));
        let effects = Arc::new(FakeEffects::default());
        let mut worker = worker_for_test(dir.path())
            .with_bus_factory(Arc::new(LateBusFactory {
                root: bus_root.clone(),
                attempts: Arc::clone(&attempts),
            }))
            .with_effects(effects.clone());
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let snapshots = effects.snapshots.lock().unwrap().clone();
                if snapshots.iter().any(|sr| sr == "retained-due") {
                    break;
                }
                assert!(
                    !task.is_finished(),
                    "worker exited during late Bus recovery"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("retained due schedule must recover without restart");
        assert!(attempts.load(Ordering::SeqCst) >= 4);
        assert!(!effects
            .snapshots
            .lock()
            .unwrap()
            .iter()
            .any(|sr| sr == "retained-history"));

        write_schedule(&persist, "dynamic", 86_400);
        tokio::time::timeout(Duration::from_secs(3), async {
            while !effects
                .snapshots
                .lock()
                .unwrap()
                .iter()
                .any(|sr| sr == "dynamic")
            {
                assert!(
                    !task.is_finished(),
                    "worker exited before dynamic discovery"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("dynamic schedule topic must be discovered");
        tokio::time::sleep(Duration::from_millis(30)).await;
        let snapshots = effects.snapshots.lock().unwrap().clone();
        assert_eq!(
            snapshots.iter().filter(|sr| *sr == "retained-due").count(),
            1
        );
        assert_eq!(snapshots.iter().filter(|sr| *sr == "dynamic").count(), 1);

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("shutdown timeout")
            .expect("worker join")
            .expect("worker result");
    }

    #[tokio::test]
    async fn partial_reads_and_publication_failure_defer_without_duplicate_effects() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().join("bus")).unwrap();
        write_schedule(&persist, "read-a", 86_400);
        write_schedule(&persist, "read-b", 86_400);

        let mode = Arc::new(AtomicUsize::new(0));
        let enumeration_failures = Arc::new(AtomicUsize::new(0));
        let topic_failures = Arc::new(AtomicUsize::new(0));
        let effects = Arc::new(FakeEffects::default());
        let publish_allowed = Arc::new(AtomicBool::new(false));
        let publish_attempts = Arc::new(AtomicUsize::new(0));
        let mut worker = worker_for_test(dir.path())
            .with_reader(Arc::new(HostileReader {
                mode: Arc::clone(&mode),
                enumeration_failures: Arc::clone(&enumeration_failures),
                topic_failures: Arc::clone(&topic_failures),
            }))
            .with_effects(effects.clone())
            .with_publisher(Arc::new(GatePublisher {
                allow: Arc::clone(&publish_allowed),
                attempts: Arc::clone(&publish_attempts),
            }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::timeout(Duration::from_secs(2), async {
            while enumeration_failures.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(effects.snapshots.lock().unwrap().is_empty());

        mode.store(1, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(2), async {
            while topic_failures.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(effects.snapshots.lock().unwrap().is_empty());
        assert!(persist
            .list_since(&run_topic("read-a"), None)
            .unwrap()
            .is_empty());

        mode.store(2, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(2), async {
            while publish_attempts.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let snapshots = effects.snapshots.lock().unwrap().clone();
        assert_eq!(snapshots.iter().filter(|sr| *sr == "read-a").count(), 1);
        assert_eq!(snapshots.iter().filter(|sr| *sr == "read-b").count(), 1);
        assert!(persist
            .list_since(&run_topic("read-a"), None)
            .unwrap()
            .is_empty());

        publish_allowed.store(true, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(2), async {
            while persist
                .list_since(&run_topic("read-a"), None)
                .unwrap()
                .is_empty()
                || persist
                    .list_since(&run_topic("read-b"), None)
                    .unwrap()
                    .is_empty()
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let snapshots = effects.snapshots.lock().unwrap().clone();
        assert_eq!(snapshots.iter().filter(|sr| *sr == "read-a").count(), 1);
        assert_eq!(snapshots.iter().filter(|sr| *sr == "read-b").count(), 1);

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("shutdown timeout")
            .expect("worker join")
            .expect("worker result");
    }

    #[tokio::test]
    async fn system_bus_fallback_and_startup_retry_are_shutdown_aware() {
        assert_eq!(
            scheduler_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        let dir = tempfile::tempdir().unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut worker =
            worker_for_test(dir.path()).with_bus_factory(Arc::new(UnavailableBusFactory {
                attempts: Arc::clone(&attempts),
            }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );
        tokio::time::timeout(Duration::from_secs(2), async {
            while attempts.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            !task.is_finished(),
            "unavailable Bus must not end the worker"
        );
        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("shutdown must interrupt Bus retry")
            .expect("worker join")
            .expect("worker result");
    }

    #[tokio::test]
    async fn blocking_join_failure_restores_pre_pass_pending_results() {
        let dir = tempfile::tempdir().unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut worker = worker_for_test(dir.path()).with_reader(Arc::new(PanicOnceReader {
            attempts: Arc::clone(&attempts),
        }));
        worker.pending_results.insert(
            "already-pending".into(),
            RunRecord {
                sr: "already-pending".into(),
                ok: true,
                ts: 42,
                snapshot: "snapshot-before-pass".into(),
                detail: String::new(),
            },
        );
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let stop = tokio::spawn(async move {
            while attempts.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            shutdown_tx.send(true).unwrap();
        });

        worker
            .run(ShutdownToken::from_receiver(shutdown_rx))
            .await
            .expect("worker result");
        stop.await.expect("shutdown task");

        let restored = worker
            .pending_results
            .get("already-pending")
            .expect("pre-pass pending result must survive blocking-task join failure");
        assert_eq!(restored.snapshot, "snapshot-before-pass");
        assert_eq!(restored.ts, 42);
    }
}
