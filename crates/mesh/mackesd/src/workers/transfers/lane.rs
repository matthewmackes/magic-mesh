//! TRANSFERS-1 — the injectable `LaneRunner` execution seam.
//!
//! The queue/ledger/verb spine owns lifecycle; it does NOT know how to move a byte.
//! Execution is delegated to a [`LaneRunner`] — the seam the per-protocol lanes
//! (TRANSFERS-2..6: sftp / rsync / wget / node / music) implement. TRANSFERS-1 ships
//! the trait plus [`GatedLaneRunner`], the **honest typed gate**: it runs no tool
//! and fabricates no success (§7) — it returns a [`LaneOutcome::Failed`] naming the
//! lane that has to land. TRANSFERS-2 adds [`TransferLaneRunner`], a production
//! dispatcher for the implemented lanes while keeping unfinished methods gated.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::job::Method;
use super::job::TransferJob;

/// Upper bound for one external transfer process. This is deliberately generous:
/// it prevents an immortal child while leaving real large downloads room to finish.
pub const HTTP_LANE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// Upper bound for one rsync process. Mirrors the HTTP lane's bounded-proc guard.
pub const RSYNC_LANE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// Upper bound for one sftp process. Mirrors the other bounded external lanes.
pub const SFTP_LANE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// Env override for the shared Navidrome library directory. Live media
/// lighthouses can point this at the rclone mount; tests and Workstations default
/// to the canonical workgroup-root music directory.
pub const MUSIC_LIBRARY_DIR_ENV: &str = "MDE_TRANSFERS_MUSIC_LIBRARY_DIR";
/// Env override for the shared mesh storage directory used by the node lane.
/// Production defaults to `/mnt/mesh-storage`; tests can point this at a tempdir.
pub const MESH_SHARE_DIR_ENV: &str = "MDE_TRANSFERS_MESH_SHARE_DIR";

static SFTP_BATCH_SEQ: AtomicU64 = AtomicU64::new(0);

/// What a lane reports when its run finishes.
///
/// There is no `Progress` variant here: live progress is written onto
/// [`TransferJob::progress`] by a lane as it runs; a `LaneOutcome` is the FINAL word
/// (the queue maps it to `Done`/`Failed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneOutcome {
    /// The move completed (and, when `policy.verify`, verified) — terminal `Done`.
    Done,
    /// The move ended in an honest failure — the reason is surfaced on the job's
    /// `error` (§7). Also the shape the [`GatedLaneRunner`] returns for every method.
    Failed {
        /// A human-readable, non-fabricated failure reason.
        error: String,
    },
}

impl LaneOutcome {
    /// Build a failure outcome from anything string-like.
    #[must_use]
    pub fn failed(error: impl Into<String>) -> Self {
        Self::Failed {
            error: error.into(),
        }
    }
}

/// Callback a lane uses to publish real progress parsed from its native tool.
#[derive(Clone)]
pub struct ProgressSink(Arc<dyn Fn(u8) + Send + Sync>);

impl ProgressSink {
    /// Build a sink from a callback.
    #[must_use]
    pub fn new(f: impl Fn(u8) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// A sink that drops progress reports.
    #[must_use]
    pub fn noop() -> Self {
        Self::new(|_| {})
    }

    /// Publish one parsed percentage.
    pub fn report(&self, pct: u8) {
        (self.0)(pct.min(99));
    }
}

/// The seam every lane implements.
///
/// `run` executes ONE job to completion (or honest failure) and returns its
/// [`LaneOutcome`]; the worker spawns it on a task and applies the outcome when it
/// finishes. A running task is aborted on `cancel`/`pause` (tokio task-abort → the
/// lane's `tokio::process` child is killed on drop), so lanes need no explicit cancel
/// channel in TRANSFERS-1.
#[async_trait::async_trait]
pub trait LaneRunner: Send + Sync {
    /// Execute `job` and report the outcome. Must not panic — a lane surfaces every
    /// failure as [`LaneOutcome::Failed`] with an honest reason.
    async fn run(&self, job: &TransferJob, progress: ProgressSink) -> LaneOutcome;
}

/// The TRANSFERS-1 default: no lane is wired yet, so every method fails honestly,
/// naming the lane that must land (§7 — never a fake success, never fake progress).
#[derive(Debug, Default, Clone, Copy)]
pub struct GatedLaneRunner;

#[async_trait::async_trait]
impl LaneRunner for GatedLaneRunner {
    async fn run(&self, job: &TransferJob, _progress: ProgressSink) -> LaneOutcome {
        LaneOutcome::failed(format!(
            "the `{}` transfer lane is not yet wired \u{2014} TRANSFERS-2..6 implement execution",
            job.method
        ))
    }
}

/// Production transfer dispatcher. Methods that are not backed by a lane remain
/// honestly gated instead of fabricating success.
#[derive(Debug, Default, Clone, Copy)]
pub struct TransferLaneRunner;

#[async_trait::async_trait]
impl LaneRunner for TransferLaneRunner {
    async fn run(&self, job: &TransferJob, progress: ProgressSink) -> LaneOutcome {
        match job.method {
            Method::Sftp => SftpLane::default().run(job, progress).await,
            Method::Http => HttpWgetLane::default().run(job, progress).await,
            Method::Rsync => RsyncLane::default().run(job, progress).await,
            Method::BrowserDownload => BrowserDownloadLane::default().run(job, progress).await,
            Method::Node => NodeLane::default().run(job, progress).await,
            Method::Music => MusicLibraryLane::default().run(job, progress).await,
        }
    }
}

/// The TRANSFERS-3 SFTP lane: bounded OpenSSH `sftp` in batch mode.
#[derive(Debug, Default, Clone, Copy)]
pub struct SftpLane;

impl SftpLane {
    fn plan(job: &TransferJob) -> Result<SftpPlan, String> {
        if job.method != Method::Sftp {
            return Err(format!("sftp lane cannot run `{}` jobs", job.method));
        }
        if job.source.as_bytes().contains(&0) || job.dest.as_bytes().contains(&0) {
            return Err("sftp lane rejects NUL bytes in source or destination".into());
        }
        let source_remote = parse_sftp_remote(&job.source)?;
        let dest_remote = parse_sftp_remote(&job.dest)?;
        let direction = match (source_remote, dest_remote) {
            (Some(remote), None) => {
                let local = PathBuf::from(job.dest.trim());
                if local.as_os_str().is_empty() {
                    return Err("sftp get requires a local destination path".into());
                }
                prepare_destination(&local)?;
                SftpDirection::Get { remote, local }
            }
            (None, Some(remote)) => {
                let local = PathBuf::from(job.source.trim());
                if local.as_os_str().is_empty() {
                    return Err("sftp put requires a local source path".into());
                }
                SftpDirection::Put { local, remote }
            }
            (Some(_), Some(_)) => {
                return Err("sftp lane requires exactly one remote endpoint".into());
            }
            (None, None) => {
                return Err(
                    "sftp lane requires either source or destination to be sftp:// or host:path"
                        .into(),
                );
            }
        };
        Ok(SftpPlan { direction })
    }
}

/// The TRANSFERS-2 HTTP lane: bounded `wget -c` with optional `--limit-rate`.
#[derive(Debug, Default, Clone, Copy)]
pub struct HttpWgetLane;

impl HttpWgetLane {
    fn plan(job: &TransferJob) -> Result<WgetPlan, String> {
        if job.method != Method::Http {
            return Err(format!("http lane cannot run `{}` jobs", job.method));
        }
        let source = job.source.trim();
        if !is_http_url(source) {
            return Err("http lane requires an http:// or https:// source URL".into());
        }
        if source.as_bytes().contains(&0) || job.dest.as_bytes().contains(&0) {
            return Err("http lane rejects NUL bytes in source or destination".into());
        }
        let dest = PathBuf::from(job.dest.trim());
        if dest.as_os_str().is_empty() {
            return Err("http lane requires a destination path".into());
        }
        prepare_destination(&dest)?;

        let mut args = vec![
            "--continue".to_string(),
            "--progress=dot:giga".to_string(),
            "--tries=3".to_string(),
            "--timeout=30".to_string(),
        ];
        if let Some(limit) = job
            .policy
            .bwlimit
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !valid_wget_rate(limit) {
                return Err(format!("invalid wget --limit-rate token `{limit}`"));
            }
            args.push(format!("--limit-rate={limit}"));
        }
        if dest.is_dir() {
            args.push("-P".into());
            args.push(dest.display().to_string());
        } else {
            args.push("-O".into());
            args.push(dest.display().to_string());
        }
        args.push(source.to_string());
        Ok(WgetPlan { args })
    }
}

/// The TRANSFERS-4 one-shot rsync lane: bounded `rsync --partial` with optional
/// `--bwlimit`. Recurring sync-pair scheduling lives in `sync_pair` + the worker
/// scheduler and feeds this lane through ordinary `Method::Rsync` jobs.
#[derive(Debug, Default, Clone, Copy)]
pub struct RsyncLane;

impl RsyncLane {
    fn plan(job: &TransferJob) -> Result<RsyncPlan, String> {
        if job.method != Method::Rsync {
            return Err(format!("rsync lane cannot run `{}` jobs", job.method));
        }
        let source = job.source.trim();
        let dest = job.dest.trim();
        if source.is_empty() || dest.is_empty() {
            return Err("rsync lane requires source and destination paths".into());
        }
        if source.as_bytes().contains(&0) || dest.as_bytes().contains(&0) {
            return Err("rsync lane rejects NUL bytes in source or destination".into());
        }
        if let Some(parent) = local_parent_for_dest(dest) {
            std::fs::create_dir_all(&parent).map_err(|e| {
                format!(
                    "could not create rsync destination parent {}: {e}",
                    parent.display()
                )
            })?;
        }
        let mut args = vec![
            "--archive".to_string(),
            "--partial".to_string(),
            "--info=progress2".to_string(),
        ];
        if let Some(limit) = job
            .policy
            .bwlimit
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !valid_rsync_bwlimit(limit) {
                return Err(format!("invalid rsync --bwlimit token `{limit}`"));
            }
            args.push(format!("--bwlimit={limit}"));
        }
        args.push(source.to_string());
        args.push(dest.to_string());
        Ok(RsyncPlan { args })
    }
}

/// The TRANSFERS-10 browser-output lane: move an already-materialized browser
/// download or scraper output through the same queue/ledger/verify surface.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrowserDownloadLane;

impl BrowserDownloadLane {
    fn plan(job: &TransferJob) -> Result<BrowserDownloadPlan, String> {
        if job.method != Method::BrowserDownload {
            return Err(format!(
                "browser-download lane cannot run `{}` jobs",
                job.method
            ));
        }
        let source = local_source_path(&job.source).ok_or_else(|| {
            "browser-download lane requires a local materialized source path".to_string()
        })?;
        if source.as_os_str().is_empty() {
            return Err("browser-download lane requires a source path".into());
        }
        if source.as_os_str().as_encoded_bytes().contains(&0) || job.dest.as_bytes().contains(&0) {
            return Err("browser-download lane rejects NUL bytes in source or destination".into());
        }
        let dest = PathBuf::from(job.dest.trim());
        if dest.as_os_str().is_empty() || job.dest.contains("://") {
            return Err("browser-download lane requires a local destination path".into());
        }
        prepare_destination(&dest)?;
        Ok(BrowserDownloadPlan { source, dest })
    }
}

/// The TRANSFERS-6 music lane: copy a local track into the shared Navidrome
/// library directory.
#[derive(Debug, Default, Clone, Copy)]
pub struct MusicLibraryLane;

impl MusicLibraryLane {
    fn plan(job: &TransferJob) -> Result<MusicPlan, String> {
        if job.method != Method::Music {
            return Err(format!("music lane cannot run `{}` jobs", job.method));
        }
        let source = local_source_path(&job.source)
            .ok_or_else(|| "music lane requires a local filesystem source path".to_string())?;
        if source.as_os_str().is_empty() {
            return Err("music lane requires a source path".into());
        }
        let file_name = source
            .file_name()
            .ok_or_else(|| "music lane source must name a file".to_string())?;
        if source.as_os_str().as_encoded_bytes().contains(&0) {
            return Err("music lane rejects NUL bytes in source".into());
        }
        let library_dir = music_library_dir(&job.dest);
        if library_dir.as_os_str().is_empty()
            || library_dir.as_os_str().as_encoded_bytes().contains(&0)
        {
            return Err("music lane requires a valid library directory".into());
        }
        let dest = library_dir.join(file_name);
        Ok(MusicPlan { source, dest })
    }
}

/// The TRANSFERS-5 node lane: stage a local file into the Syncthing-backed mesh
/// share so substrate replication carries it to the target peer.
#[derive(Debug, Default, Clone, Copy)]
pub struct NodeLane;

impl NodeLane {
    fn plan(job: &TransferJob) -> Result<NodePlan, String> {
        Self::plan_with_root(job, mesh_share_dir())
    }

    fn plan_with_root(job: &TransferJob, mesh_root: PathBuf) -> Result<NodePlan, String> {
        if job.method != Method::Node {
            return Err(format!("node lane cannot run `{}` jobs", job.method));
        }
        let source = local_source_path(&job.source)
            .ok_or_else(|| "node lane requires a local filesystem source path".to_string())?;
        if source.as_os_str().is_empty() {
            return Err("node lane requires a source path".into());
        }
        if source.as_os_str().as_encoded_bytes().contains(&0) {
            return Err("node lane rejects NUL bytes in source".into());
        }
        let file_name = source
            .file_name()
            .ok_or_else(|| "node lane source must name a file".to_string())?;
        let dest_dir = node_stage_dir(&job.dest, mesh_root)?;
        if dest_dir.as_os_str().is_empty() || dest_dir.as_os_str().as_encoded_bytes().contains(&0) {
            return Err("node lane requires a valid shared destination".into());
        }
        let dest = dest_dir.join(file_name);
        Ok(NodePlan {
            source,
            dest,
            target_peer: node_target_peer(&job.dest),
        })
    }
}

#[async_trait::async_trait]
impl LaneRunner for NodeLane {
    async fn run(&self, job: &TransferJob, progress: ProgressSink) -> LaneOutcome {
        let plan = match Self::plan(job) {
            Ok(plan) => plan,
            Err(e) => return LaneOutcome::failed(e),
        };
        copy_node_plan(plan, progress).await
    }
}

#[async_trait::async_trait]
impl LaneRunner for BrowserDownloadLane {
    async fn run(&self, job: &TransferJob, progress: ProgressSink) -> LaneOutcome {
        let plan = match Self::plan(job) {
            Ok(plan) => plan,
            Err(e) => return LaneOutcome::failed(e),
        };
        copy_materialized_file(
            "browser-download lane",
            &plan.source,
            resolve_dest_path(&plan.source, &plan.dest),
            progress,
        )
        .await
    }
}

async fn copy_node_plan(plan: NodePlan, progress: ProgressSink) -> LaneOutcome {
    copy_materialized_file("node lane", &plan.source, plan.dest, progress).await
}

async fn copy_materialized_file(
    lane: &str,
    source: &Path,
    dest: PathBuf,
    progress: ProgressSink,
) -> LaneOutcome {
    let meta = match tokio::fs::metadata(source).await {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return LaneOutcome::failed(format!("{lane} source must be a regular file")),
        Err(e) => {
            return LaneOutcome::failed(format!(
                "{lane} could not read source {}: {e}",
                source.display()
            ));
        }
    };
    if let Some(parent) = dest.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return LaneOutcome::failed(format!(
                "{lane} could not create destination directory {}: {e}",
                parent.display()
            ));
        }
    }
    match tokio::fs::copy(source, &dest).await {
        Ok(bytes) if bytes == meta.len() => {
            progress.report(99);
            LaneOutcome::Done
        }
        Ok(bytes) => LaneOutcome::failed(format!(
            "{lane} copied {bytes} bytes but source has {} bytes",
            meta.len()
        )),
        Err(e) => LaneOutcome::failed(format!(
            "{lane} copy {} -> {} failed: {e}",
            source.display(),
            dest.display()
        )),
    }
}

#[async_trait::async_trait]
impl LaneRunner for MusicLibraryLane {
    async fn run(&self, job: &TransferJob, progress: ProgressSink) -> LaneOutcome {
        let plan = match Self::plan(job) {
            Ok(plan) => plan,
            Err(e) => return LaneOutcome::failed(e),
        };
        let meta = match tokio::fs::metadata(&plan.source).await {
            Ok(meta) if meta.is_file() => meta,
            Ok(_) => return LaneOutcome::failed("music lane source must be a regular file"),
            Err(e) => {
                return LaneOutcome::failed(format!(
                    "music lane could not read source {}: {e}",
                    plan.source.display()
                ));
            }
        };
        if let Some(parent) = plan.dest.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return LaneOutcome::failed(format!(
                    "music lane could not create library directory {}: {e}",
                    parent.display()
                ));
            }
        }
        match tokio::fs::copy(&plan.source, &plan.dest).await {
            Ok(bytes) if bytes == meta.len() => {
                progress.report(99);
                LaneOutcome::Done
            }
            Ok(bytes) => LaneOutcome::failed(format!(
                "music lane copied {bytes} bytes but source has {} bytes",
                meta.len()
            )),
            Err(e) => LaneOutcome::failed(format!(
                "music lane copy {} -> {} failed: {e}",
                plan.source.display(),
                plan.dest.display()
            )),
        }
    }
}

#[async_trait::async_trait]
impl LaneRunner for SftpLane {
    async fn run(&self, job: &TransferJob, progress: ProgressSink) -> LaneOutcome {
        let plan = match Self::plan(job) {
            Ok(plan) => plan,
            Err(e) => return LaneOutcome::failed(redact_transfer_secret(&e)),
        };
        run_sftp_plan(&plan, progress, Path::new("sftp")).await
    }
}

async fn run_sftp_plan(plan: &SftpPlan, progress: ProgressSink, bin: &Path) -> LaneOutcome {
    run_sftp_plan_with_auth(plan, progress, bin, None, None).await
}

async fn run_sftp_plan_with_auth(
    plan: &SftpPlan,
    progress: ProgressSink,
    bin: &Path,
    identity_file: Option<&Path>,
    known_hosts_file: Option<&Path>,
) -> LaneOutcome {
    let batch = match write_sftp_batch(&plan.direction).await {
        Ok(batch) => batch,
        Err(e) => return LaneOutcome::failed(e),
    };
    let remote = plan.direction.remote();
    let mut cmd = Command::new(bin);
    cmd.arg("-B")
        .arg("32768")
        .arg("-oBatchMode=yes")
        .arg("-oStrictHostKeyChecking=accept-new")
        .arg("-b")
        .arg(&batch.path)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(identity_file) = identity_file {
        cmd.arg("-i").arg(identity_file);
    }
    if let Some(known_hosts_file) = known_hosts_file {
        cmd.arg("-oUserKnownHostsFile=".to_string() + &known_hosts_file.display().to_string());
    }
    if let Some(port) = remote.port {
        cmd.arg("-P").arg(port.to_string());
    }
    cmd.arg(remote.endpoint());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = tokio::fs::remove_file(&batch.path).await;
            return LaneOutcome::failed(format!("sftp spawn failed: {e}"));
        }
    };
    let stdout = child.stdout.take().map(|reader| {
        tokio::spawn(collect_output_with(
            reader,
            None,
            parse_sftp_progress_percent,
        ))
    });
    let stderr = child.stderr.take().map(|reader| {
        tokio::spawn(collect_output_with(
            reader,
            Some(progress),
            parse_sftp_progress_percent,
        ))
    });
    let status = match tokio::time::timeout(SFTP_LANE_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => {
            let _ = tokio::fs::remove_file(&batch.path).await;
            return LaneOutcome::failed(format!("sftp wait failed: {e}"));
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = tokio::fs::remove_file(&batch.path).await;
            return LaneOutcome::failed(format!(
                "sftp exceeded the {}s transfer timeout",
                SFTP_LANE_TIMEOUT.as_secs()
            ));
        }
    };
    let stdout = join_output(stdout).await;
    let stderr = join_output(stderr).await;
    let _ = tokio::fs::remove_file(&batch.path).await;
    if status.success() {
        LaneOutcome::Done
    } else {
        let err = process_tail(&stderr).or_else(|| process_tail(&stdout));
        LaneOutcome::failed(redact_transfer_secret(&format!(
            "sftp exited with status {}{}",
            status,
            err.map_or_else(String::new, |e| format!(": {e}"))
        )))
    }
}

#[async_trait::async_trait]
impl LaneRunner for RsyncLane {
    async fn run(&self, job: &TransferJob, progress: ProgressSink) -> LaneOutcome {
        let plan = match Self::plan(job) {
            Ok(plan) => plan,
            Err(e) => return LaneOutcome::failed(e),
        };
        let mut cmd = Command::new("rsync");
        cmd.args(&plan.args)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => return LaneOutcome::failed(format!("rsync spawn failed: {e}")),
        };
        let stdout = child.stdout.take().map(|reader| {
            tokio::spawn(collect_output_with(
                reader,
                Some(progress),
                parse_rsync_progress_percent,
            ))
        });
        let stderr = child.stderr.take().map(|reader| {
            tokio::spawn(collect_output_with(
                reader,
                None,
                parse_rsync_progress_percent,
            ))
        });
        let status = match tokio::time::timeout(RSYNC_LANE_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return LaneOutcome::failed(format!("rsync wait failed: {e}")),
            Err(_) => {
                let _ = child.kill().await;
                return LaneOutcome::failed(format!(
                    "rsync exceeded the {}s transfer timeout",
                    RSYNC_LANE_TIMEOUT.as_secs()
                ));
            }
        };
        let stdout = join_output(stdout).await;
        let stderr = join_output(stderr).await;
        if status.success() {
            LaneOutcome::Done
        } else {
            let err = process_tail(&stderr).or_else(|| process_tail(&stdout));
            LaneOutcome::failed(format!(
                "rsync exited with status {}{}",
                status,
                err.map_or_else(String::new, |e| format!(": {e}"))
            ))
        }
    }
}

#[async_trait::async_trait]
impl LaneRunner for HttpWgetLane {
    async fn run(&self, job: &TransferJob, progress: ProgressSink) -> LaneOutcome {
        let plan = match Self::plan(job) {
            Ok(plan) => plan,
            Err(e) => return LaneOutcome::failed(e),
        };
        let mut cmd = Command::new("wget");
        cmd.args(&plan.args)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => return LaneOutcome::failed(format!("wget spawn failed: {e}")),
        };
        let stderr = child
            .stderr
            .take()
            .map(|reader| tokio::spawn(collect_output(reader, Some(progress))));
        let stdout = child
            .stdout
            .take()
            .map(|reader| tokio::spawn(collect_output(reader, None)));
        let status = match tokio::time::timeout(HTTP_LANE_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return LaneOutcome::failed(format!("wget wait failed: {e}")),
            Err(_) => {
                let _ = child.kill().await;
                return LaneOutcome::failed(format!(
                    "wget exceeded the {}s transfer timeout",
                    HTTP_LANE_TIMEOUT.as_secs()
                ));
            }
        };
        let stderr = join_output(stderr).await;
        let stdout = join_output(stdout).await;
        if status.success() {
            LaneOutcome::Done
        } else {
            let err = process_tail(&stderr).or_else(|| process_tail(&stdout));
            LaneOutcome::failed(format!(
                "wget exited with status {}{}",
                status,
                err.map_or_else(String::new, |e| format!(": {e}"))
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WgetPlan {
    args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RsyncPlan {
    args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserDownloadPlan {
    source: PathBuf,
    dest: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SftpPlan {
    direction: SftpDirection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SftpDirection {
    Get { remote: SftpRemote, local: PathBuf },
    Put { local: PathBuf, remote: SftpRemote },
}

impl SftpDirection {
    fn remote(&self) -> &SftpRemote {
        match self {
            Self::Get { remote, .. } | Self::Put { remote, .. } => remote,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SftpRemote {
    user: Option<String>,
    host: String,
    port: Option<u16>,
    path: String,
}

impl SftpRemote {
    fn endpoint(&self) -> String {
        match self.user.as_deref() {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        }
    }
}

struct SftpBatch {
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MusicPlan {
    source: PathBuf,
    dest: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NodePlan {
    source: PathBuf,
    dest: PathBuf,
    target_peer: Option<String>,
}

fn is_http_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn valid_wget_rate(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn valid_rsync_bwlimit(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn parse_sftp_remote(raw: &str) -> Result<Option<SftpRemote>, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if let Some(rest) = s.strip_prefix("sftp://") {
        return parse_sftp_url_remote(rest).map(Some);
    }
    if s.contains("://") {
        return Ok(None);
    }
    if s.starts_with('/') || s.starts_with("./") || s.starts_with("../") {
        return Ok(None);
    }
    let Some((authority, path)) = s.split_once(':') else {
        return Ok(None);
    };
    if authority.is_empty() || path.is_empty() {
        return Err("sftp host:path endpoints require both host and path".into());
    }
    let (user, host) = split_sftp_user_host(authority)?;
    if host.is_empty() {
        return Err("sftp remote host is empty".into());
    }
    Ok(Some(SftpRemote {
        user,
        host: host.to_string(),
        port: None,
        path: path.to_string(),
    }))
}

fn parse_sftp_url_remote(rest: &str) -> Result<SftpRemote, String> {
    let (authority, path) = rest
        .split_once('/')
        .ok_or_else(|| "sftp:// endpoints require a remote path".to_string())?;
    if authority.is_empty() || path.is_empty() {
        return Err("sftp:// endpoints require both host and path".into());
    }
    let (user, host_port) = split_sftp_user_host(authority)?;
    let (host, port) = split_sftp_host_port(host_port)?;
    if host.is_empty() {
        return Err("sftp remote host is empty".into());
    }
    Ok(SftpRemote {
        user,
        host: host.to_string(),
        port,
        path: format!("/{path}"),
    })
}

fn split_sftp_user_host(authority: &str) -> Result<(Option<String>, &str), String> {
    if let Some((user, host)) = authority.rsplit_once('@') {
        if user.contains(':') {
            return Err(
                "sftp lane rejects password-bearing URLs; use key/agent credentials instead".into(),
            );
        }
        if user.is_empty() {
            return Err("sftp remote user is empty".into());
        }
        return Ok((Some(user.to_string()), host));
    }
    Ok((None, authority))
}

fn split_sftp_host_port(host_port: &str) -> Result<(&str, Option<u16>), String> {
    let Some((host, port)) = host_port.rsplit_once(':') else {
        return Ok((host_port, None));
    };
    if host.is_empty() || port.is_empty() {
        return Err("sftp port requires a host and numeric port".into());
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| "sftp port must be numeric".to_string())?;
    Ok((host, Some(port)))
}

async fn write_sftp_batch(direction: &SftpDirection) -> Result<SftpBatch, String> {
    let body = match direction {
        SftpDirection::Get { remote, local } => {
            format!(
                "get {} {}\n",
                sftp_batch_quote(&remote.path),
                sftp_batch_quote(&local.display().to_string())
            )
        }
        SftpDirection::Put { local, remote } => {
            format!(
                "put {} {}\n",
                sftp_batch_quote(&local.display().to_string()),
                sftp_batch_quote(&remote.path)
            )
        }
    };
    let seq = SFTP_BATCH_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "mde-transfer-sftp-{}-{}.batch",
        std::process::id(),
        seq
    ));
    tokio::fs::write(&path, body).await.map_err(|e| {
        format!(
            "sftp lane could not create batch file {}: {e}",
            path.display()
        )
    })?;
    Ok(SftpBatch { path })
}

fn sftp_batch_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if matches!(c, '\\' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn redact_transfer_secret(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for token in s.split_whitespace() {
        if let Some((scheme, rest)) = token.split_once("://") {
            if let Some((userinfo, tail)) = rest.split_once('@') {
                if userinfo.contains(':') {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(scheme);
                    out.push_str("://***:***@");
                    out.push_str(tail);
                    continue;
                }
            }
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(token);
    }
    out
}

fn local_parent_for_dest(dest: &str) -> Option<PathBuf> {
    if dest.contains(':') || dest.contains("://") {
        return None;
    }
    let path = PathBuf::from(dest);
    if path.exists() && path.is_dir() {
        return Some(path);
    }
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

fn local_source_path(raw: &str) -> Option<PathBuf> {
    let s = raw.trim();
    if s.is_empty()
        || s.contains("://")
        || (s.contains(':') && !s.starts_with('/') && !s.starts_with("./") && !s.starts_with("../"))
    {
        return None;
    }
    Some(PathBuf::from(s))
}

fn resolve_dest_path(source: &Path, dest: &Path) -> PathBuf {
    if dest.is_dir() {
        if let Some(file_name) = source.file_name() {
            return dest.join(file_name);
        }
    }
    dest.to_path_buf()
}

fn music_library_dir(dest: &str) -> PathBuf {
    let dest = dest.trim();
    if !dest.is_empty() && dest != "music-library" {
        return PathBuf::from(dest);
    }
    std::env::var(MUSIC_LIBRARY_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::default_qnm_shared_root().join("music-library"))
}

fn mesh_share_dir() -> PathBuf {
    std::env::var(MESH_SHARE_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::default_qnm_shared_root())
}

/// Resolve a node-lane destination into the local shared staging directory.
#[must_use]
pub fn node_dest_dir(dest: &str) -> Option<PathBuf> {
    node_stage_dir(dest, mesh_share_dir()).ok()
}

/// Resolve a node-lane destination against an explicit mesh root.
#[must_use]
pub fn node_dest_dir_with_root(dest: &str, mesh_root: &Path) -> Option<PathBuf> {
    node_stage_dir(dest, mesh_root.to_path_buf()).ok()
}

fn node_stage_dir(dest: &str, mesh_root: PathBuf) -> Result<PathBuf, String> {
    let dest = dest.trim();
    if dest.is_empty() || dest == "mesh-share:" || dest == "mesh-share" {
        return Ok(mesh_root);
    }
    if let Some(peer) = node_target_peer(dest) {
        return Ok(mesh_root.join(".transfers").join("node").join(peer));
    }
    if dest.contains("://") {
        return Err("node lane rejects URL destinations".into());
    }
    if dest.contains(':')
        && !dest.starts_with('/')
        && !dest.starts_with("./")
        && !dest.starts_with("../")
    {
        return Err("node lane requires a mesh-share path or node:<peer> destination".into());
    }
    Ok(PathBuf::from(dest))
}

fn node_target_peer(dest: &str) -> Option<String> {
    let raw = dest.trim().strip_prefix("node:")?.trim();
    let peer = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect::<String>();
    if peer.is_empty() { None } else { Some(peer) }
}

fn prepare_destination(dest: &Path) -> Result<(), String> {
    if dest.exists() {
        return Ok(());
    }
    let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(|e| {
        format!(
            "could not create destination parent {}: {e}",
            parent.display()
        )
    })
}

fn process_tail(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let tail = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())?
        .trim();
    Some(tail.chars().take(240).collect())
}

async fn collect_output<R>(reader: R, progress: Option<ProgressSink>) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    collect_output_with(reader, progress, parse_wget_progress_percent).await
}

async fn collect_output_with<R>(
    mut reader: R,
    progress: Option<ProgressSink>,
    parser: fn(&str) -> Option<u8>,
) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let mut out = Vec::new();
    let mut scan = String::new();
    let mut last = None;
    let mut buf = [0u8; 4096];
    loop {
        let Ok(n) = reader.read(&mut buf).await else {
            break;
        };
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        let Some(progress) = progress.as_ref() else {
            continue;
        };
        scan.push_str(&String::from_utf8_lossy(&buf[..n]));
        for part in scan.split(['\r', '\n']) {
            if let Some(pct) = parser(part) {
                if last.is_none_or(|prev| pct > prev) {
                    progress.report(pct);
                    last = Some(pct);
                }
            }
        }
        if scan.len() > 8192 {
            let keep_from = scan.len().saturating_sub(2048);
            scan.drain(..keep_from);
        }
    }
    out
}

async fn join_output(handle: Option<tokio::task::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    match handle {
        Some(handle) => handle.await.unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Parse coarse wget percentages from progress text. The current lane returns only
/// real percentages emitted by wget; callers decide how to persist/report them.
#[must_use]
pub fn parse_wget_progress_percent(line: &str) -> Option<u8> {
    for token in line.split_whitespace() {
        let Some(raw) = token.strip_suffix('%') else {
            continue;
        };
        let value = raw.trim_start_matches(|c: char| !c.is_ascii_digit());
        let parsed = value.parse::<u8>().ok()?;
        if parsed <= 100 {
            return Some(parsed);
        }
    }
    None
}

/// Parse rsync `--info=progress2` percentages from progress text.
#[must_use]
pub fn parse_rsync_progress_percent(line: &str) -> Option<u8> {
    for token in line.split_whitespace() {
        let Some(raw) = token.strip_suffix('%') else {
            continue;
        };
        if let Ok(parsed) = raw.parse::<u8>() {
            if parsed <= 100 {
                return Some(parsed);
            }
        }
    }
    None
}

/// Parse OpenSSH-style SFTP progress percentages when the client emits them.
#[must_use]
pub fn parse_sftp_progress_percent(line: &str) -> Option<u8> {
    parse_rsync_progress_percent(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::transfers::job::{Method, TransferPolicy};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command as StdCommand;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    #[tokio::test]
    async fn gated_lane_fails_every_method_honestly() {
        let lane = GatedLaneRunner;
        for m in Method::ALL {
            let job = TransferJob::new("/a", "/b", m, TransferPolicy::default());
            let outcome = lane.run(&job, ProgressSink::noop()).await;
            // The gate must never fake success (\u{a7}7).
            assert!(
                matches!(outcome, LaneOutcome::Failed { .. }),
                "the gate returned Done for {m}"
            );
            if let LaneOutcome::Failed { error } = outcome {
                assert!(error.contains(m.as_str()), "names the lane: {error}");
                assert!(
                    error.contains("TRANSFERS-2..6"),
                    "points at the lanes: {error}"
                );
            }
        }
    }

    #[test]
    fn http_plan_uses_resume_and_bwlimit() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out.bin");
        let job = TransferJob::new(
            "https://example.invalid/file.bin",
            dest.display().to_string(),
            Method::Http,
            TransferPolicy {
                bwlimit: Some("256k".into()),
                verify: false,
            },
        );
        let plan = HttpWgetLane::plan(&job).unwrap();
        assert!(plan.args.iter().any(|a| a == "--continue"));
        assert!(plan.args.iter().any(|a| a == "--limit-rate=256k"));
        assert!(
            plan.args
                .windows(2)
                .any(|w| w == ["-O", &dest.display().to_string()])
        );
    }

    #[test]
    fn http_plan_rejects_non_http_and_bad_bwlimit() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out.bin");
        let mut job = TransferJob::new(
            "file:///tmp/x",
            dest.display().to_string(),
            Method::Http,
            TransferPolicy::default(),
        );
        assert!(HttpWgetLane::plan(&job).unwrap_err().contains("http://"));
        job.source = "http://example.invalid/x".into();
        job.policy.bwlimit = Some("1m;rm".into());
        assert!(
            HttpWgetLane::plan(&job)
                .unwrap_err()
                .contains("invalid wget --limit-rate")
        );
    }

    #[test]
    fn wget_progress_parser_uses_real_percent_tokens_only() {
        assert_eq!(
            parse_wget_progress_percent(" 65536K .......... 42%  255K 1s"),
            Some(42)
        );
        assert_eq!(parse_wget_progress_percent("no percent here"), None);
        assert_eq!(parse_wget_progress_percent("101%"), None);
    }

    #[test]
    fn rsync_plan_uses_partial_progress_and_bwlimit() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let dest = tmp.path().join("dest");
        let job = TransferJob::new(
            source.display().to_string(),
            dest.display().to_string(),
            Method::Rsync,
            TransferPolicy {
                bwlimit: Some("512".into()),
                verify: false,
            },
        );
        let plan = RsyncLane::plan(&job).unwrap();
        assert!(plan.args.iter().any(|a| a == "--archive"));
        assert!(plan.args.iter().any(|a| a == "--partial"));
        assert!(plan.args.iter().any(|a| a == "--info=progress2"));
        assert!(plan.args.iter().any(|a| a == "--bwlimit=512"));
        assert_eq!(plan.args[plan.args.len() - 2], source.display().to_string());
        assert_eq!(plan.args[plan.args.len() - 1], dest.display().to_string());
    }

    #[test]
    fn rsync_plan_rejects_empty_paths_and_bad_bwlimit() {
        let mut job = TransferJob::new("", "/dest", Method::Rsync, TransferPolicy::default());
        assert!(RsyncLane::plan(&job).unwrap_err().contains("source"));
        job.source = "/source".into();
        job.policy.bwlimit = Some("1m;rm".into());
        assert!(
            RsyncLane::plan(&job)
                .unwrap_err()
                .contains("invalid rsync --bwlimit")
        );
    }

    #[test]
    fn rsync_progress_parser_reads_progress2_percentages_only() {
        assert_eq!(
            parse_rsync_progress_percent("      65,536  42%  600.00kB/s    0:00:10"),
            Some(42)
        );
        assert_eq!(
            parse_rsync_progress_percent("sending incremental file list"),
            None
        );
        assert_eq!(parse_rsync_progress_percent("101%"), None);
    }

    #[test]
    fn browser_download_plan_accepts_only_local_materialized_outputs() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("browser.tmp");
        let dest_dir = tmp.path().join("downloads");
        std::fs::write(&source, b"browser bytes").unwrap();
        std::fs::create_dir_all(&dest_dir).unwrap();
        let job = TransferJob::new(
            source.display().to_string(),
            dest_dir.display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );
        let plan = BrowserDownloadLane::plan(&job).unwrap();
        assert_eq!(plan.source, source);
        assert_eq!(plan.dest, dest_dir);

        let remote = TransferJob::new(
            "https://example.invalid/file.bin",
            tmp.path().join("out.bin").display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );
        assert!(
            BrowserDownloadLane::plan(&remote)
                .unwrap_err()
                .contains("local materialized source")
        );
    }

    #[tokio::test]
    async fn browser_download_lane_copies_to_selected_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("capture.png");
        let dest_dir = tmp.path().join("picked");
        std::fs::write(&source, b"png bytes").unwrap();
        std::fs::create_dir_all(&dest_dir).unwrap();
        let job = TransferJob::new(
            source.display().to_string(),
            dest_dir.display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        let outcome = BrowserDownloadLane
            .run(
                &job,
                ProgressSink::new(move |pct| sink_seen.lock().unwrap().push(pct)),
            )
            .await;
        assert_eq!(outcome, LaneOutcome::Done);
        assert_eq!(
            std::fs::read(dest_dir.join("capture.png")).unwrap(),
            b"png bytes"
        );
        assert!(seen.lock().unwrap().contains(&99));
    }

    #[test]
    fn sftp_plan_accepts_put_get_and_rejects_password_urls() {
        let tmp = tempfile::tempdir().unwrap();
        let local_source = tmp.path().join("upload.bin");
        let local_dest = tmp.path().join("download.bin");
        let remote_dest = tmp.path().join("remote-upload.bin");
        let put = TransferJob::new(
            local_source.display().to_string(),
            format!("alice@example.invalid:{}", remote_dest.display()),
            Method::Sftp,
            TransferPolicy::default(),
        );
        let plan = SftpLane::plan(&put).unwrap();
        match plan.direction {
            SftpDirection::Put { local, remote } => {
                assert_eq!(local, local_source);
                assert_eq!(remote.endpoint(), "alice@example.invalid");
                assert_eq!(remote.path, remote_dest.display().to_string());
                assert_eq!(remote.port, None);
            }
            other => panic!("expected put plan, got {other:?}"),
        }

        let get = TransferJob::new(
            "sftp://bob@example.invalid:2022/tmp/remote.bin",
            local_dest.display().to_string(),
            Method::Sftp,
            TransferPolicy::default(),
        );
        let plan = SftpLane::plan(&get).unwrap();
        match plan.direction {
            SftpDirection::Get { remote, local } => {
                assert_eq!(local, local_dest);
                assert_eq!(remote.endpoint(), "bob@example.invalid");
                assert_eq!(remote.path, "/tmp/remote.bin");
                assert_eq!(remote.port, Some(2022));
            }
            other => panic!("expected get plan, got {other:?}"),
        }

        let leaked = TransferJob::new(
            "sftp://bob:secret@example.invalid/tmp/remote.bin",
            tmp.path().join("out.bin").display().to_string(),
            Method::Sftp,
            TransferPolicy::default(),
        );
        let err = SftpLane::plan(&leaked).unwrap_err();
        assert!(err.contains("key/agent credentials"));
        assert!(!err.contains("secret"));
    }

    #[test]
    fn sftp_progress_parser_reads_percent_tokens() {
        assert_eq!(
            parse_sftp_progress_percent("remote.bin 65536 42% 1.0MB/s 00:01"),
            Some(42)
        );
        assert_eq!(parse_sftp_progress_percent("Connected to host"), None);
        assert_eq!(parse_sftp_progress_percent("101%"), None);
    }

    #[tokio::test]
    async fn sftp_lane_put_and_get_roundtrip_against_fixture_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = fixture_sftp_bin(tmp.path());

        let local_upload = tmp.path().join("upload.bin");
        let remote_upload = tmp.path().join("remote-upload.bin");
        std::fs::write(&local_upload, b"sftp upload").unwrap();
        let put = SftpLane::plan(&TransferJob::new(
            local_upload.display().to_string(),
            format!("fixture.invalid:{}", remote_upload.display()),
            Method::Sftp,
            TransferPolicy::default(),
        ))
        .unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        let outcome = run_sftp_plan(
            &put,
            ProgressSink::new(move |pct| sink_seen.lock().unwrap().push(pct)),
            &fake,
        )
        .await;
        assert_eq!(outcome, LaneOutcome::Done);
        assert_eq!(std::fs::read(&remote_upload).unwrap(), b"sftp upload");
        assert!(
            seen.lock().unwrap().contains(&42),
            "fixture sftp progress should be parsed"
        );

        let remote_download = tmp.path().join("remote-download.bin");
        let local_download = tmp.path().join("download.bin");
        std::fs::write(&remote_download, b"sftp download").unwrap();
        let get = SftpLane::plan(&TransferJob::new(
            format!("sftp://fixture.invalid{}", remote_download.display()),
            local_download.display().to_string(),
            Method::Sftp,
            TransferPolicy::default(),
        ))
        .unwrap();
        let outcome = run_sftp_plan(&get, ProgressSink::noop(), &fake).await;
        assert_eq!(outcome, LaneOutcome::Done);
        assert_eq!(std::fs::read(&local_download).unwrap(), b"sftp download");
    }

    #[tokio::test]
    async fn sftp_lane_put_and_get_roundtrip_against_fixture_sshd() {
        let Some(sshd_bin) = command_path("sshd") else {
            return;
        };
        let Some(sftp_bin) = command_path("sftp") else {
            return;
        };
        let Some(ssh_keygen_bin) = command_path("ssh-keygen") else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let host_key = tmp.path().join("ssh_host_ed25519_key");
        let client_key = tmp.path().join("client_ed25519");
        let authorized_keys = tmp.path().join("authorized_keys");
        let known_hosts = tmp.path().join("known_hosts");
        run_ok(
            StdCommand::new(&ssh_keygen_bin)
                .arg("-q")
                .arg("-t")
                .arg("ed25519")
                .arg("-N")
                .arg("")
                .arg("-f")
                .arg(&host_key),
        );
        run_ok(
            StdCommand::new(&ssh_keygen_bin)
                .arg("-q")
                .arg("-t")
                .arg("ed25519")
                .arg("-N")
                .arg("")
                .arg("-f")
                .arg(&client_key),
        );
        std::fs::copy(client_key.with_extension("pub"), &authorized_keys).unwrap();
        std::fs::write(&known_hosts, "").unwrap();

        let port = free_local_port();
        let config = tmp.path().join("sshd_config");
        std::fs::write(
            &config,
            format!(
                "\
Port {port}
ListenAddress 127.0.0.1
HostKey {}
AuthorizedKeysFile {}
PidFile {}
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
PubkeyAuthentication yes
PermitRootLogin no
UsePAM no
StrictModes no
LogLevel ERROR
Subsystem sftp internal-sftp
",
                host_key.display(),
                authorized_keys.display(),
                tmp.path().join("sshd.pid").display()
            ),
        )
        .unwrap();
        let sshd_log = tmp.path().join("sshd.log");
        let sshd_log_file = std::fs::File::create(&sshd_log).unwrap();
        let mut sshd = StdCommand::new(&sshd_bin)
            .arg("-D")
            .arg("-e")
            .arg("-f")
            .arg(&config)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(sshd_log_file))
            .spawn()
            .unwrap();
        if !wait_for_tcp("127.0.0.1", port, Duration::from_secs(5)) {
            let exited = sshd.try_wait().unwrap();
            let log = std::fs::read_to_string(&sshd_log).unwrap_or_default();
            let _ = sshd.kill();
            let _ = sshd.wait();
            panic!(
                "fixture sshd did not accept connections on port {port}; exited={exited:?}; log={log}"
            );
        }

        let user = std::env::var("USER").unwrap_or_else(|_| "mm".into());
        let remote_upload = tmp.path().join("sshd-upload.bin");
        let local_upload = tmp.path().join("local-upload.bin");
        std::fs::write(&local_upload, b"sshd upload").unwrap();
        let put = SftpLane::plan(&TransferJob::new(
            local_upload.display().to_string(),
            format!("sftp://{user}@127.0.0.1:{port}{}", remote_upload.display()),
            Method::Sftp,
            TransferPolicy::default(),
        ))
        .unwrap();
        let outcome = run_sftp_plan_with_auth(
            &put,
            ProgressSink::noop(),
            &sftp_bin,
            Some(&client_key),
            Some(&known_hosts),
        )
        .await;
        assert_eq!(outcome, LaneOutcome::Done);
        assert_eq!(std::fs::read(&remote_upload).unwrap(), b"sshd upload");

        let remote_download = tmp.path().join("sshd-download.bin");
        let local_download = tmp.path().join("local-download.bin");
        std::fs::write(&remote_download, b"sshd download").unwrap();
        let get = SftpLane::plan(&TransferJob::new(
            format!(
                "sftp://{user}@127.0.0.1:{port}{}",
                remote_download.display()
            ),
            local_download.display().to_string(),
            Method::Sftp,
            TransferPolicy::default(),
        ))
        .unwrap();
        let outcome = run_sftp_plan_with_auth(
            &get,
            ProgressSink::noop(),
            &sftp_bin,
            Some(&client_key),
            Some(&known_hosts),
        )
        .await;
        let _ = sshd.kill();
        let _ = sshd.wait();
        assert_eq!(outcome, LaneOutcome::Done);
        assert_eq!(std::fs::read(&local_download).unwrap(), b"sshd download");
    }

    #[test]
    fn music_plan_lands_source_basename_in_library_dir() {
        let job = TransferJob::new(
            "/music/input/song.wav",
            "/mnt/mesh-storage/music-library",
            Method::Music,
            TransferPolicy::default(),
        );
        let plan = MusicLibraryLane::plan(&job).unwrap();
        assert_eq!(plan.source, PathBuf::from("/music/input/song.wav"));
        assert_eq!(
            plan.dest,
            PathBuf::from("/mnt/mesh-storage/music-library/song.wav")
        );
    }

    #[test]
    fn music_plan_rejects_remote_sources() {
        let job = TransferJob::new(
            "host:/srv/song.wav",
            "/mnt/mesh-storage/music-library",
            Method::Music,
            TransferPolicy::default(),
        );
        assert!(
            MusicLibraryLane::plan(&job)
                .unwrap_err()
                .contains("local filesystem source")
        );
    }

    #[test]
    fn node_plan_stages_peer_destinations_under_mesh_share() {
        let mesh_root = PathBuf::from("/mnt/mesh-storage");
        let job = TransferJob::new(
            "/home/user/file.iso",
            "node:Oak-01",
            Method::Node,
            TransferPolicy::default(),
        );
        let plan = NodeLane::plan_with_root(&job, mesh_root).unwrap();
        assert_eq!(plan.source, PathBuf::from("/home/user/file.iso"));
        assert_eq!(
            plan.dest,
            PathBuf::from("/mnt/mesh-storage/.transfers/node/Oak-01/file.iso")
        );
        assert_eq!(plan.target_peer.as_deref(), Some("Oak-01"));
    }

    #[test]
    fn node_plan_rejects_remote_sources_and_ad_hoc_destinations() {
        let root = PathBuf::from("/mnt/mesh-storage");
        let mut job = TransferJob::new(
            "host:/srv/file.iso",
            "node:oak",
            Method::Node,
            TransferPolicy::default(),
        );
        assert!(
            NodeLane::plan_with_root(&job, root.clone())
                .unwrap_err()
                .contains("local filesystem source")
        );
        job.source = "/home/user/file.iso".into();
        job.dest = "sftp.example.com:/drop".into();
        assert!(
            NodeLane::plan_with_root(&job, root)
                .unwrap_err()
                .contains("node:<peer>")
        );
    }

    #[tokio::test]
    async fn http_lane_resumes_partial_download_against_fixture_server() {
        if StdCommand::new("wget").arg("--version").output().is_err() {
            return;
        }
        let body = (0..131_072).map(|n| (n % 251) as u8).collect::<Vec<_>>();
        let partial_len = 4096usize;
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("payload.bin");
        std::fs::write(&dest, &body[..partial_len]).unwrap();

        let (url, range_rx, join) = fixture_http_server(body.clone());
        let job = TransferJob::new(
            url,
            dest.display().to_string(),
            Method::Http,
            TransferPolicy {
                bwlimit: Some("1m".into()),
                verify: false,
            },
        );
        let outcome = HttpWgetLane.run(&job, ProgressSink::noop()).await;
        join.join().unwrap();
        assert_eq!(outcome, LaneOutcome::Done);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let range = range_rx.recv().unwrap_or_default();
        assert!(
            range.contains(&format!("bytes={partial_len}-")),
            "wget must resume from the partial file, got Range header `{range}`"
        );
    }

    #[tokio::test]
    async fn http_lane_bwlimit_caps_fixture_download_throughput() {
        if StdCommand::new("wget").arg("--version").output().is_err() {
            return;
        }
        let body = (0..32_768).map(|n| (n % 251) as u8).collect::<Vec<_>>();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("limited.bin");
        let (url, _range_rx, join) = fixture_http_server(body.clone());
        let job = TransferJob::new(
            url,
            dest.display().to_string(),
            Method::Http,
            TransferPolicy {
                bwlimit: Some("8k".into()),
                verify: false,
            },
        );
        let start = Instant::now();
        let outcome = HttpWgetLane.run(&job, ProgressSink::noop()).await;
        let elapsed = start.elapsed();
        join.join().unwrap();
        assert_eq!(outcome, LaneOutcome::Done);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(
            elapsed >= Duration::from_secs(2),
            "32 KiB at --limit-rate=8k should not complete as an uncapped local transfer; elapsed={elapsed:?}"
        );
    }

    #[tokio::test]
    async fn rsync_lane_round_trips_local_fixture_and_reports_progress() {
        if StdCommand::new("rsync").arg("--version").output().is_err() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let dest_dir = tmp.path().join("dest");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("payload.bin"), vec![7u8; 64 * 1024]).unwrap();
        let job = TransferJob::new(
            format!("{}/", source_dir.display()),
            dest_dir.display().to_string(),
            Method::Rsync,
            TransferPolicy {
                bwlimit: Some("2048".into()),
                verify: false,
            },
        );
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        let outcome = RsyncLane
            .run(
                &job,
                ProgressSink::new(move |pct| sink_seen.lock().unwrap().push(pct)),
            )
            .await;
        assert_eq!(outcome, LaneOutcome::Done);
        assert_eq!(
            std::fs::read(dest_dir.join("payload.bin")).unwrap(),
            vec![7u8; 64 * 1024]
        );
        assert!(
            seen.lock().unwrap().iter().any(|pct| *pct > 0),
            "rsync --info=progress2 should publish real progress"
        );
    }

    #[tokio::test]
    async fn music_lane_copies_track_into_shared_library() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("track.flac");
        let library = tmp.path().join("mesh-share").join("music-library");
        std::fs::write(&source, b"fake flac bytes").unwrap();
        let job = TransferJob::new(
            source.display().to_string(),
            library.display().to_string(),
            Method::Music,
            TransferPolicy::default(),
        );
        let outcome = MusicLibraryLane.run(&job, ProgressSink::noop()).await;
        assert_eq!(outcome, LaneOutcome::Done);
        assert_eq!(
            std::fs::read(library.join("track.flac")).unwrap(),
            b"fake flac bytes"
        );
    }

    #[tokio::test]
    async fn node_lane_stages_file_in_mesh_share_for_target_peer() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("payload.bin");
        let mesh_root = tmp.path().join("mesh-share");
        std::fs::write(&source, b"node payload").unwrap();
        let job = TransferJob::new(
            source.display().to_string(),
            "node:oak",
            Method::Node,
            TransferPolicy::default(),
        );
        let plan = NodeLane::plan_with_root(&job, mesh_root.clone()).unwrap();
        let outcome = copy_node_plan(plan, ProgressSink::noop()).await;
        assert_eq!(outcome, LaneOutcome::Done);
        assert_eq!(
            std::fs::read(mesh_root.join(".transfers/node/oak/payload.bin")).unwrap(),
            b"node payload"
        );
    }

    #[tokio::test]
    async fn surface_complete_gate_exercises_every_lane_against_fixtures() {
        if StdCommand::new("wget").arg("--version").output().is_err() {
            eprintln!("skipping surface-complete fixture: wget is not installed");
            return;
        }
        if StdCommand::new("rsync").arg("--version").output().is_err() {
            eprintln!("skipping surface-complete fixture: rsync is not installed");
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let mut completed = Vec::new();

        // HTTP covers resume: the destination starts with a partial file and the
        // fixture asserts wget requests the remaining byte range.
        let http_body = (0..65_536).map(|n| (n % 251) as u8).collect::<Vec<_>>();
        let http_dest = tmp.path().join("http.bin");
        let partial_len = 2048usize;
        std::fs::write(&http_dest, &http_body[..partial_len]).unwrap();
        let (http_url, range_rx, http_join) = fixture_http_server(http_body.clone());
        let http_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let http_sink = Arc::clone(&http_seen);
        let http_job = TransferJob::new(
            http_url,
            http_dest.display().to_string(),
            Method::Http,
            TransferPolicy::default(),
        );
        assert_eq!(
            HttpWgetLane
                .run(
                    &http_job,
                    ProgressSink::new(move |pct| http_sink.lock().unwrap().push(pct)),
                )
                .await,
            LaneOutcome::Done
        );
        http_join.join().unwrap();
        assert_eq!(std::fs::read(&http_dest).unwrap(), http_body);
        assert!(
            range_rx
                .recv()
                .unwrap_or_default()
                .contains(&format!("bytes={partial_len}-")),
            "HTTP lane did not resume from the partial file"
        );
        completed.push(Method::Http);

        // SFTP uses the same batch-mode execution path, with a fixture binary that
        // performs put/get locally and emits a real percent token.
        let sftp_bin = fixture_sftp_bin(tmp.path());
        let sftp_local = tmp.path().join("sftp-local.bin");
        let sftp_remote = tmp.path().join("sftp-remote.bin");
        std::fs::write(&sftp_local, b"sftp surface bytes").unwrap();
        let sftp_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sftp_sink = Arc::clone(&sftp_seen);
        let sftp_plan = SftpLane::plan(&TransferJob::new(
            sftp_local.display().to_string(),
            format!("fixture.invalid:{}", sftp_remote.display()),
            Method::Sftp,
            TransferPolicy::default(),
        ))
        .unwrap();
        assert_eq!(
            run_sftp_plan(
                &sftp_plan,
                ProgressSink::new(move |pct| sftp_sink.lock().unwrap().push(pct)),
                &sftp_bin,
            )
            .await,
            LaneOutcome::Done
        );
        assert_eq!(std::fs::read(&sftp_remote).unwrap(), b"sftp surface bytes");
        assert!(sftp_seen.lock().unwrap().contains(&42));
        completed.push(Method::Sftp);

        // rsync covers the delta/mirror lane and native progress parser.
        let rsync_source = tmp.path().join("rsync-source");
        let rsync_dest = tmp.path().join("rsync-dest");
        std::fs::create_dir_all(&rsync_source).unwrap();
        std::fs::write(rsync_source.join("mirror.txt"), vec![9u8; 64 * 1024]).unwrap();
        let rsync_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let rsync_sink = Arc::clone(&rsync_seen);
        let rsync_job = TransferJob::new(
            format!("{}/", rsync_source.display()),
            rsync_dest.display().to_string(),
            Method::Rsync,
            TransferPolicy::default(),
        );
        assert_eq!(
            RsyncLane
                .run(
                    &rsync_job,
                    ProgressSink::new(move |pct| rsync_sink.lock().unwrap().push(pct)),
                )
                .await,
            LaneOutcome::Done
        );
        assert_eq!(
            std::fs::read(rsync_dest.join("mirror.txt")).unwrap(),
            vec![9u8; 64 * 1024]
        );
        assert!(rsync_seen.lock().unwrap().iter().any(|pct| *pct > 0));
        completed.push(Method::Rsync);

        // Browser download/scrape outputs are already materialized and handed to
        // Transfers for the move.
        let browser_source = tmp.path().join("browser-output.json");
        let browser_dest_dir = tmp.path().join("browser-picked");
        std::fs::write(&browser_source, br#"{"kind":"scrape"}"#).unwrap();
        std::fs::create_dir_all(&browser_dest_dir).unwrap();
        let browser_job = TransferJob::new(
            browser_source.display().to_string(),
            browser_dest_dir.display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );
        assert_eq!(
            BrowserDownloadLane
                .run(&browser_job, ProgressSink::noop())
                .await,
            LaneOutcome::Done
        );
        assert_eq!(
            std::fs::read(browser_dest_dir.join("browser-output.json")).unwrap(),
            br#"{"kind":"scrape"}"#
        );
        completed.push(Method::BrowserDownload);

        // Node stages through the shared mesh root.
        let node_source = tmp.path().join("node.bin");
        let mesh_root = tmp.path().join("mesh-share");
        std::fs::write(&node_source, b"node surface bytes").unwrap();
        let node_job = TransferJob::new(
            node_source.display().to_string(),
            "node:oak",
            Method::Node,
            TransferPolicy::default(),
        );
        let node_plan = NodeLane::plan_with_root(&node_job, mesh_root.clone()).unwrap();
        assert_eq!(
            copy_node_plan(node_plan, ProgressSink::noop()).await,
            LaneOutcome::Done
        );
        assert_eq!(
            std::fs::read(mesh_root.join(".transfers/node/oak/node.bin")).unwrap(),
            b"node surface bytes"
        );
        completed.push(Method::Node);

        // Music lands in the shared library path.
        let music_source = tmp.path().join("track.flac");
        let music_library = tmp.path().join("music-library");
        std::fs::write(&music_source, b"music surface bytes").unwrap();
        let music_job = TransferJob::new(
            music_source.display().to_string(),
            music_library.display().to_string(),
            Method::Music,
            TransferPolicy::default(),
        );
        assert_eq!(
            MusicLibraryLane.run(&music_job, ProgressSink::noop()).await,
            LaneOutcome::Done
        );
        assert_eq!(
            std::fs::read(music_library.join("track.flac")).unwrap(),
            b"music surface bytes"
        );
        completed.push(Method::Music);

        assert_eq!(completed.len(), Method::ALL.len());
        for method in Method::ALL {
            assert!(completed.contains(&method), "missing {method} from gate");
        }
    }

    fn fixture_http_server(
        body: Vec<u8>,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let join = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            let range = request
                .lines()
                .find_map(|line| line.strip_prefix("Range:"))
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            let start = range
                .strip_prefix("bytes=")
                .and_then(|r| r.split_once('-').map(|(n, _)| n))
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(0)
                .min(body.len());
            let _ = tx.send(range);
            if start > 0 {
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                    body.len() - start,
                    start,
                    body.len() - 1,
                    body.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(&body[start..]).unwrap();
            } else {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        (format!("http://{addr}/payload.bin"), rx, join)
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut data = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            if data.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&data).into_owned()
    }

    fn fixture_sftp_bin(root: &Path) -> PathBuf {
        let bin = root.join("fixture-sftp");
        std::fs::write(
            &bin,
            r#"#!/bin/sh
batch=
while [ "$#" -gt 0 ]; do
  case "$1" in
    -b) batch="$2"; shift 2 ;;
    *) shift ;;
  esac
done
if [ -z "$batch" ]; then
  echo "missing batch file" >&2
  exit 7
fi
eval "set -- $(cat "$batch")"
cmd="$1"
src="$2"
dst="$3"
echo "payload.bin 65536 42% 1.0MB/s 00:01" >&2
case "$cmd" in
  put|get) cp "$src" "$dst" ;;
  *) echo "unknown command: $cmd" >&2; exit 9 ;;
esac
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&bin).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin, perms).unwrap();
        bin
    }

    fn run_ok(cmd: &mut StdCommand) {
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "command failed: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn command_path(name: &str) -> Option<PathBuf> {
        let output = StdCommand::new("sh")
            .arg("-c")
            .arg(format!("command -v {name}"))
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8(output.stdout).ok()?;
        Some(PathBuf::from(path.trim()))
    }

    fn free_local_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    fn wait_for_tcp(host: &str, port: u16, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if TcpStream::connect((host, port)).is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}
