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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::job::Method;
use super::job::TransferJob;

mod helpers;

use helpers::*;
pub use helpers::{
    node_dest_dir, node_dest_dir_with_root, parse_rsync_progress_percent,
    parse_sftp_progress_percent, parse_wget_progress_percent,
};

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

#[cfg(test)]
mod tests;
