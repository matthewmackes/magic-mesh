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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::job::Method;
use super::job::TransferJob;
use super::job::TransferPolicy;

/// Upper bound for one external transfer process. This is deliberately generous:
/// it prevents an immortal child while leaving real large downloads room to finish.
pub const HTTP_LANE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// Upper bound for one rsync process. Mirrors the HTTP lane's bounded-proc guard.
pub const RSYNC_LANE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// Upper bound for one sftp process. Mirrors the other bounded external lanes.
pub const SFTP_LANE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// Keep browser HLS package expansion bounded: large live/DVR manifests can be
/// effectively unbounded, and Transfers must never turn one click into a crawl.
const BROWSER_HLS_MAX_PLAYLISTS: usize = 16;
const BROWSER_HLS_MAX_ASSETS: usize = 256;
/// Keep browser DASH package expansion bounded for the same reason as HLS.
const BROWSER_DASH_MAX_ASSETS: usize = 256;
/// Keep Browser scrape crawl execution bounded. The Browser shell already emits
/// a depth-1 handoff manifest; the daemon must not turn that into an open crawl.
const BROWSER_SCRAPE_CRAWL_MAX_TARGETS: usize = 128;
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
        if let Some(request) = browser_media_download_request(&source)? {
            if browser_media_request_is_hls(&request) {
                let (package_dir, manifest_filename) =
                    hls_package_destination(&dest, &request.suggested_filename);
                return Ok(BrowserDownloadPlan::FetchHlsPackage {
                    manifest_url: request.asset_url,
                    package_dir,
                    manifest_filename,
                });
            }
            if browser_media_request_is_dash(&request) {
                let (package_dir, manifest_filename) =
                    dash_package_destination(&dest, &request.suggested_filename);
                return Ok(BrowserDownloadPlan::FetchDashPackage {
                    manifest_url: request.asset_url,
                    package_dir,
                    manifest_filename,
                });
            }
            let resolved_dest = if dest.is_dir() {
                dest.join(&request.suggested_filename)
            } else {
                dest
            };
            return Ok(BrowserDownloadPlan::FetchMedia {
                asset_url: request.asset_url,
                dest: resolved_dest,
            });
        }
        if let Some(request) = browser_scrape_crawl_request(&source)? {
            if !request.targets.is_empty() {
                let original_dest = resolve_dest_path(&source, &dest);
                let package_dir = scrape_crawl_package_destination(&original_dest);
                return Ok(BrowserDownloadPlan::FetchScrapeCrawlPackage {
                    source,
                    dest: original_dest,
                    page_url: request.page_url,
                    package_dir,
                    targets: request.targets,
                });
            }
        }
        Ok(BrowserDownloadPlan::CopyMaterialized { source, dest })
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
        match plan {
            BrowserDownloadPlan::CopyMaterialized { source, dest } => {
                copy_materialized_file(
                    "browser-download lane",
                    &source,
                    resolve_dest_path(&source, &dest),
                    progress,
                )
                .await
            }
            BrowserDownloadPlan::FetchMedia { asset_url, dest } => {
                fetch_http_child(&job.id, &job.policy, &asset_url, &dest, progress).await
            }
            BrowserDownloadPlan::FetchHlsPackage {
                manifest_url,
                package_dir,
                manifest_filename,
            } => {
                fetch_hls_package(
                    &job.id,
                    &job.policy,
                    &manifest_url,
                    &package_dir,
                    &manifest_filename,
                    progress,
                )
                .await
            }
            BrowserDownloadPlan::FetchDashPackage {
                manifest_url,
                package_dir,
                manifest_filename,
            } => {
                fetch_dash_package(
                    &job.id,
                    &job.policy,
                    &manifest_url,
                    &package_dir,
                    &manifest_filename,
                    progress,
                )
                .await
            }
            BrowserDownloadPlan::FetchScrapeCrawlPackage {
                source,
                dest,
                page_url,
                package_dir,
                targets,
            } => {
                fetch_scrape_crawl_package(
                    &job.id,
                    &job.policy,
                    &source,
                    &dest,
                    &page_url,
                    &package_dir,
                    &targets,
                    progress,
                )
                .await
            }
        }
    }
}

async fn copy_node_plan(plan: NodePlan, progress: ProgressSink) -> LaneOutcome {
    copy_materialized_file("node lane", &plan.source, plan.dest, progress).await
}

async fn fetch_http_child(
    job_id: &str,
    policy: &TransferPolicy,
    url: &str,
    dest: &Path,
    progress: ProgressSink,
) -> LaneOutcome {
    let mut http_job = TransferJob::new(
        url.to_string(),
        dest.display().to_string(),
        Method::Http,
        policy.clone(),
    );
    http_job.id = job_id.to_string();
    HttpWgetLane.run(&http_job, progress).await
}

async fn fetch_hls_package(
    job_id: &str,
    policy: &TransferPolicy,
    manifest_url: &str,
    package_dir: &Path,
    manifest_filename: &str,
    progress: ProgressSink,
) -> LaneOutcome {
    if let Err(e) = tokio::fs::create_dir_all(package_dir).await {
        return LaneOutcome::failed(format!(
            "browser HLS package could not create {}: {e}",
            package_dir.display()
        ));
    }

    let mut used_filenames = BTreeSet::new();
    let root_filename = unique_browser_package_filename(manifest_filename, &mut used_filenames);
    let mut pending = VecDeque::from([(manifest_url.to_string(), root_filename.clone(), 0usize)]);
    let mut seen_urls = BTreeSet::new();
    let mut url_paths = BTreeMap::from([(manifest_url.to_string(), root_filename.clone())]);
    let mut playlist_bodies = Vec::new();
    let mut items = Vec::new();
    let mut playlist_count = 0usize;
    let mut asset_count = 0usize;

    while let Some((playlist_url, filename, depth)) = pending.pop_front() {
        if !seen_urls.insert(playlist_url.clone()) {
            continue;
        }
        playlist_count += 1;
        if playlist_count > BROWSER_HLS_MAX_PLAYLISTS {
            return LaneOutcome::failed(format!(
                "browser HLS package exceeded the {BROWSER_HLS_MAX_PLAYLISTS} playlist limit"
            ));
        }
        let playlist_path = package_dir.join(&filename);
        let outcome = fetch_http_child(
            job_id,
            policy,
            &playlist_url,
            &playlist_path,
            progress.clone(),
        )
        .await;
        if let LaneOutcome::Failed { error } = outcome {
            return LaneOutcome::failed(format!(
                "browser HLS package playlist fetch failed for {playlist_url}: {error}"
            ));
        }
        items.push(HlsPackageItem {
            kind: "playlist",
            url: playlist_url.clone(),
            path: filename.clone(),
        });
        let body = match tokio::fs::read_to_string(&playlist_path).await {
            Ok(body) => body,
            Err(e) => {
                return LaneOutcome::failed(format!(
                    "browser HLS package could not read playlist {}: {e}",
                    playlist_path.display()
                ));
            }
        };
        playlist_bodies.push((playlist_url.clone(), filename.clone(), body.clone()));
        for reference in hls_playlist_references(&body) {
            let child_url = match resolve_hls_child_url(&playlist_url, &reference.uri) {
                Ok(url) => url,
                Err(e) => return LaneOutcome::failed(e),
            };
            if is_hls_manifest_url(&child_url) {
                if depth + 1 >= BROWSER_HLS_MAX_PLAYLISTS {
                    return LaneOutcome::failed(
                        "browser HLS package exceeded nested playlist depth".to_string(),
                    );
                }
                if seen_urls.contains(&child_url) {
                    continue;
                }
                let child_filename = unique_browser_package_filename(
                    &hls_url_filename(&child_url),
                    &mut used_filenames,
                );
                url_paths.insert(child_url.clone(), child_filename.clone());
                pending.push_back((child_url, child_filename, depth + 1));
                continue;
            }
            if !seen_urls.insert(child_url.clone()) {
                continue;
            }
            asset_count += 1;
            if asset_count > BROWSER_HLS_MAX_ASSETS {
                return LaneOutcome::failed(format!(
                    "browser HLS package exceeded the {BROWSER_HLS_MAX_ASSETS} asset limit"
                ));
            }
            let filename =
                unique_browser_package_filename(&hls_url_filename(&child_url), &mut used_filenames);
            url_paths.insert(child_url.clone(), filename.clone());
            let dest = package_dir.join(&filename);
            let outcome =
                fetch_http_child(job_id, policy, &child_url, &dest, progress.clone()).await;
            if let LaneOutcome::Failed { error } = outcome {
                return LaneOutcome::failed(format!(
                    "browser HLS package asset fetch failed for {child_url}: {error}"
                ));
            }
            items.push(HlsPackageItem {
                kind: reference.item_kind(),
                url: child_url,
                path: filename,
            });
        }
        let rough = ((playlist_count + asset_count).min(99) as u8).min(99);
        progress.report(rough);
    }

    for (playlist_url, filename, body) in playlist_bodies {
        let rewritten = match rewrite_hls_playlist_to_local(&playlist_url, &body, &url_paths) {
            Ok(body) => body,
            Err(e) => return LaneOutcome::failed(e),
        };
        let path = package_dir.join(&filename);
        if let Err(e) = tokio::fs::write(&path, rewritten).await {
            return LaneOutcome::failed(format!(
                "browser HLS package could not write offline playlist {}: {e}",
                path.display()
            ));
        }
    }

    let manifest = serde_json::json!({
        "op": "browser_hls_download_package",
        "source": "browser_download_lane",
        "root_url": manifest_url,
        "root_playlist_path": root_filename,
        "offline_rewrite_status": "completed",
        "playlist_count": playlist_count,
        "asset_count": asset_count,
        "items": items
            .iter()
            .map(|item| serde_json::json!({
                "kind": item.kind,
                "url": item.url,
                "path": item.path,
            }))
            .collect::<Vec<_>>(),
    });
    let manifest_path = package_dir.join("browser-hls-package.json");
    let body = match serde_json::to_vec_pretty(&manifest) {
        Ok(body) => body,
        Err(e) => return LaneOutcome::failed(format!("browser HLS package encode failed: {e}")),
    };
    if let Err(e) = tokio::fs::write(&manifest_path, body).await {
        return LaneOutcome::failed(format!(
            "browser HLS package could not write {}: {e}",
            manifest_path.display()
        ));
    }
    progress.report(99);
    LaneOutcome::Done
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HlsPackageItem {
    kind: &'static str,
    url: String,
    path: String,
}

async fn fetch_dash_package(
    job_id: &str,
    policy: &TransferPolicy,
    manifest_url: &str,
    package_dir: &Path,
    manifest_filename: &str,
    progress: ProgressSink,
) -> LaneOutcome {
    if let Err(e) = tokio::fs::create_dir_all(package_dir).await {
        return LaneOutcome::failed(format!(
            "browser DASH package could not create {}: {e}",
            package_dir.display()
        ));
    }

    let mut used_filenames = BTreeSet::new();
    let root_filename = unique_browser_package_filename(manifest_filename, &mut used_filenames);
    let manifest_path = package_dir.join(&root_filename);
    let outcome = fetch_http_child(
        job_id,
        policy,
        manifest_url,
        &manifest_path,
        progress.clone(),
    )
    .await;
    if let LaneOutcome::Failed { error } = outcome {
        return LaneOutcome::failed(format!(
            "browser DASH package MPD fetch failed for {manifest_url}: {error}"
        ));
    }
    let body = match tokio::fs::read_to_string(&manifest_path).await {
        Ok(body) => body,
        Err(e) => {
            return LaneOutcome::failed(format!(
                "browser DASH package could not read MPD {}: {e}",
                manifest_path.display()
            ));
        }
    };
    let mut items = vec![DashPackageItem {
        kind: "mpd",
        url: manifest_url.to_string(),
        path: root_filename.clone(),
    }];
    let mut seen_urls = BTreeSet::from([manifest_url.to_string()]);
    let mut url_paths = BTreeMap::from([(manifest_url.to_string(), root_filename.clone())]);
    let references = match dash_mpd_references(manifest_url, &body) {
        Ok(references) => references,
        Err(e) => return LaneOutcome::failed(e),
    };
    let mut asset_count = 0usize;
    for reference in references {
        if !seen_urls.insert(reference.url.clone()) {
            continue;
        }
        asset_count += 1;
        if asset_count > BROWSER_DASH_MAX_ASSETS {
            return LaneOutcome::failed(format!(
                "browser DASH package exceeded the {BROWSER_DASH_MAX_ASSETS} asset limit"
            ));
        }
        let filename =
            unique_browser_package_filename(&hls_url_filename(&reference.url), &mut used_filenames);
        url_paths.insert(reference.url.clone(), filename.clone());
        let dest = package_dir.join(&filename);
        let outcome =
            fetch_http_child(job_id, policy, &reference.url, &dest, progress.clone()).await;
        if let LaneOutcome::Failed { error } = outcome {
            return LaneOutcome::failed(format!(
                "browser DASH package asset fetch failed for {}: {error}",
                reference.url
            ));
        }
        items.push(DashPackageItem {
            kind: reference.kind,
            url: reference.url,
            path: filename,
        });
        progress.report(asset_count.min(99) as u8);
    }

    let rewritten = match rewrite_dash_mpd_to_local(manifest_url, &body, &url_paths) {
        Ok(body) => body,
        Err(e) => return LaneOutcome::failed(e),
    };
    if let Err(e) = tokio::fs::write(&manifest_path, rewritten).await {
        return LaneOutcome::failed(format!(
            "browser DASH package could not write offline MPD {}: {e}",
            manifest_path.display()
        ));
    }

    let package = serde_json::json!({
        "op": "browser_dash_download_package",
        "source": "browser_download_lane",
        "root_url": manifest_url,
        "root_mpd_path": root_filename,
        "offline_rewrite_status": "completed",
        "asset_count": asset_count,
        "items": items
            .iter()
            .map(|item| serde_json::json!({
                "kind": item.kind,
                "url": item.url,
                "path": item.path,
            }))
            .collect::<Vec<_>>(),
    });
    let package_path = package_dir.join("browser-dash-package.json");
    let body = match serde_json::to_vec_pretty(&package) {
        Ok(body) => body,
        Err(e) => return LaneOutcome::failed(format!("browser DASH package encode failed: {e}")),
    };
    if let Err(e) = tokio::fs::write(&package_path, body).await {
        return LaneOutcome::failed(format!(
            "browser DASH package could not write {}: {e}",
            package_path.display()
        ));
    }
    progress.report(99);
    LaneOutcome::Done
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DashPackageItem {
    kind: &'static str,
    url: String,
    path: String,
}

async fn fetch_scrape_crawl_package(
    job_id: &str,
    policy: &TransferPolicy,
    source: &Path,
    dest: &Path,
    page_url: &str,
    package_dir: &Path,
    targets: &[BrowserScrapeCrawlTarget],
    progress: ProgressSink,
) -> LaneOutcome {
    let copy_outcome = copy_materialized_file(
        "browser scrape crawl",
        source,
        dest.to_path_buf(),
        ProgressSink::noop(),
    )
    .await;
    if let LaneOutcome::Failed { error } = copy_outcome {
        return LaneOutcome::failed(error);
    }
    if let Err(e) = tokio::fs::create_dir_all(package_dir).await {
        return LaneOutcome::failed(format!(
            "browser scrape crawl package could not create {}: {e}",
            package_dir.display()
        ));
    }

    let mut used_filenames = BTreeSet::new();
    let mut items = Vec::new();
    for (idx, target) in targets
        .iter()
        .take(BROWSER_SCRAPE_CRAWL_MAX_TARGETS)
        .enumerate()
    {
        let filename =
            unique_browser_package_filename(&hls_url_filename(&target.url), &mut used_filenames);
        let target_path = package_dir.join(&filename);
        let outcome =
            fetch_http_child(job_id, policy, &target.url, &target_path, progress.clone()).await;
        if let LaneOutcome::Failed { error } = outcome {
            return LaneOutcome::failed(format!(
                "browser scrape crawl target fetch failed for {}: {error}",
                target.url
            ));
        }
        items.push(ScrapeCrawlPackageItem {
            url: target.url.clone(),
            source: target.source.clone(),
            resource: target.resource.clone(),
            depth: target.depth,
            path: filename,
        });
        let rough = (((idx + 1) * 99) / targets.len().max(1)) as u8;
        progress.report(rough);
    }

    let package = serde_json::json!({
        "op": "browser_scrape_crawl_package",
        "source": "browser_download_lane",
        "page_url": page_url,
        "execution_status": "completed",
        "max_depth": 1,
        "target_count": items.len(),
        "original_export": dest
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("browser-active-page-scrape.json"),
        "items": items
            .iter()
            .map(|item| serde_json::json!({
                "url": item.url,
                "source": item.source,
                "resource": item.resource,
                "depth": item.depth,
                "path": item.path,
            }))
            .collect::<Vec<_>>(),
    });
    let package_path = package_dir.join("browser-scrape-crawl-package.json");
    let body = match serde_json::to_vec_pretty(&package) {
        Ok(body) => body,
        Err(e) => {
            return LaneOutcome::failed(format!("browser scrape crawl package encode failed: {e}"));
        }
    };
    if let Err(e) = tokio::fs::write(&package_path, body).await {
        return LaneOutcome::failed(format!(
            "browser scrape crawl package could not write {}: {e}",
            package_path.display()
        ));
    }
    progress.report(99);
    LaneOutcome::Done
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScrapeCrawlPackageItem {
    url: String,
    source: String,
    resource: String,
    depth: u8,
    path: String,
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
enum BrowserDownloadPlan {
    CopyMaterialized {
        source: PathBuf,
        dest: PathBuf,
    },
    FetchMedia {
        asset_url: String,
        dest: PathBuf,
    },
    FetchHlsPackage {
        manifest_url: String,
        package_dir: PathBuf,
        manifest_filename: String,
    },
    FetchDashPackage {
        manifest_url: String,
        package_dir: PathBuf,
        manifest_filename: String,
    },
    FetchScrapeCrawlPackage {
        source: PathBuf,
        dest: PathBuf,
        page_url: String,
        package_dir: PathBuf,
        targets: Vec<BrowserScrapeCrawlTarget>,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserMediaDownloadRequest {
    asset_url: String,
    suggested_filename: String,
    kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserScrapeCrawlRequest {
    page_url: String,
    targets: Vec<BrowserScrapeCrawlTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserScrapeCrawlTarget {
    url: String,
    source: String,
    resource: String,
    depth: u8,
}

fn browser_media_download_request(
    source: &Path,
) -> Result<Option<BrowserMediaDownloadRequest>, String> {
    if source.extension().and_then(|ext| ext.to_str()) != Some("json")
        || !source
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".download.json"))
    {
        return Ok(None);
    }
    let body = std::fs::read(source).map_err(|e| {
        format!(
            "browser media request {} could not be read: {e}",
            source.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
        format!(
            "browser media request {} is not JSON: {e}",
            source.display()
        )
    })?;
    if value.get("op").and_then(serde_json::Value::as_str) != Some("browser_media_download_request")
    {
        return Ok(None);
    }
    let asset_url = value
        .get("asset_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| is_http_url(url))
        .ok_or_else(|| {
            "browser media request requires an http:// or https:// asset_url".to_string()
        })?
        .to_owned();
    if asset_url.as_bytes().contains(&0) {
        return Err("browser media request rejects NUL bytes in asset_url".to_string());
    }
    let suggested = value
        .get("suggested_filename")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("browser-media");
    let suggested_filename = safe_browser_download_filename(suggested);
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .map(|kind| kind.to_ascii_lowercase());
    Ok(Some(BrowserMediaDownloadRequest {
        asset_url,
        suggested_filename,
        kind,
    }))
}

fn browser_scrape_crawl_request(
    source: &Path,
) -> Result<Option<BrowserScrapeCrawlRequest>, String> {
    if source.extension().and_then(|ext| ext.to_str()) != Some("json") {
        return Ok(None);
    }
    if !source
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("mde-browser-scrape-"))
    {
        return Ok(None);
    }
    let body = std::fs::read(source).map_err(|e| {
        format!(
            "browser scrape crawl request {} could not be read: {e}",
            source.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
        format!(
            "browser scrape crawl request {} is not JSON: {e}",
            source.display()
        )
    })?;
    if value.get("op").and_then(serde_json::Value::as_str) != Some("browser_active_page_scrape") {
        return Ok(None);
    }
    if value
        .get("crawl_execution_status")
        .and_then(serde_json::Value::as_str)
        != Some("not_started")
    {
        return Ok(None);
    }
    let page_url = value
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| is_http_url(url))
        .ok_or_else(|| {
            "browser scrape crawl request requires an http:// or https:// page url".to_string()
        })?
        .to_owned();
    if page_url.as_bytes().contains(&0) {
        return Err("browser scrape crawl request rejects NUL bytes in page url".to_string());
    }
    let page = reqwest::Url::parse(&page_url)
        .map_err(|e| format!("browser scrape crawl page URL is invalid: {e}"))?;
    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();
    if let Some(items) = value
        .get("crawl_manifest")
        .and_then(serde_json::Value::as_array)
    {
        for item in items.iter() {
            if targets.len() >= BROWSER_SCRAPE_CRAWL_MAX_TARGETS {
                break;
            }
            if item.get("same_origin").and_then(serde_json::Value::as_bool) != Some(true) {
                continue;
            }
            let Some(raw_url) = item.get("url").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let url = raw_url.trim();
            if !is_http_url(url) || url.as_bytes().contains(&0) {
                continue;
            }
            if !same_origin_url(&page, url) {
                continue;
            }
            if !seen.insert(url.to_owned()) {
                continue;
            }
            targets.push(BrowserScrapeCrawlTarget {
                url: url.to_owned(),
                source: item
                    .get("source")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|source| !source.is_empty())
                    .unwrap_or("crawl_manifest")
                    .chars()
                    .take(80)
                    .collect(),
                resource: item
                    .get("resource")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|resource| !resource.is_empty())
                    .unwrap_or("document")
                    .chars()
                    .take(80)
                    .collect(),
                depth: 1,
            });
        }
    }
    Ok(Some(BrowserScrapeCrawlRequest { page_url, targets }))
}

fn browser_media_request_is_hls(request: &BrowserMediaDownloadRequest) -> bool {
    request.kind.as_deref() == Some("hls")
        || is_hls_manifest_url(&request.asset_url)
        || request
            .suggested_filename
            .to_ascii_lowercase()
            .ends_with(".m3u8")
}

fn browser_media_request_is_dash(request: &BrowserMediaDownloadRequest) -> bool {
    request.kind.as_deref() == Some("dash")
        || is_dash_manifest_url(&request.asset_url)
        || request
            .suggested_filename
            .to_ascii_lowercase()
            .ends_with(".mpd")
}

fn same_origin_url(page: &reqwest::Url, candidate: &str) -> bool {
    let Ok(candidate) = reqwest::Url::parse(candidate) else {
        return false;
    };
    page.scheme().eq_ignore_ascii_case(candidate.scheme())
        && page.host_str().map(str::to_ascii_lowercase)
            == candidate.host_str().map(str::to_ascii_lowercase)
        && page.port_or_known_default() == candidate.port_or_known_default()
}

fn is_hls_manifest_url(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(Iterator::last)
                .map(str::to_ascii_lowercase)
        })
        .is_some_and(|leaf| leaf.ends_with(".m3u8"))
}

fn is_dash_manifest_url(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(Iterator::last)
                .map(str::to_ascii_lowercase)
        })
        .is_some_and(|leaf| leaf.ends_with(".mpd"))
}

fn hls_package_destination(dest: &Path, suggested_filename: &str) -> (PathBuf, String) {
    let manifest_filename = safe_browser_download_filename(suggested_filename);
    let stem = Path::new(&manifest_filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(safe_browser_download_filename)
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "browser-media".to_string());
    if dest.is_dir() {
        return (dest.join(format!("{stem}.hls")), manifest_filename);
    }
    let package_dir = dest.with_extension("hls");
    let filename = dest
        .file_name()
        .and_then(|name| name.to_str())
        .map(safe_browser_download_filename)
        .filter(|name| !name.is_empty())
        .unwrap_or(manifest_filename);
    (package_dir, filename)
}

fn dash_package_destination(dest: &Path, suggested_filename: &str) -> (PathBuf, String) {
    let manifest_filename = safe_browser_download_filename(suggested_filename);
    let stem = Path::new(&manifest_filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(safe_browser_download_filename)
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "browser-media".to_string());
    if dest.is_dir() {
        return (dest.join(format!("{stem}.dash")), manifest_filename);
    }
    let package_dir = dest.with_extension("dash");
    let filename = dest
        .file_name()
        .and_then(|name| name.to_str())
        .map(safe_browser_download_filename)
        .filter(|name| !name.is_empty())
        .unwrap_or(manifest_filename);
    (package_dir, filename)
}

fn scrape_crawl_package_destination(original_dest: &Path) -> PathBuf {
    if original_dest.is_dir() {
        return original_dest.join("browser-scrape.crawl");
    }
    let stem = original_dest
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(safe_browser_download_filename)
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "browser-scrape".to_string());
    match original_dest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent.join(format!("{stem}.crawl")),
        None => PathBuf::from(format!("{stem}.crawl")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HlsReference {
    uri: String,
    attr: bool,
}

impl HlsReference {
    fn item_kind(&self) -> &'static str {
        if self.attr {
            "hls-asset"
        } else {
            "segment"
        }
    }
}

fn hls_playlist_references(body: &str) -> Vec<HlsReference> {
    let mut refs = Vec::new();
    for line in body.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            refs.extend(
                hls_uri_attributes(line)
                    .into_iter()
                    .map(|uri| HlsReference { uri, attr: true }),
            );
        } else {
            refs.push(HlsReference {
                uri: line.to_string(),
                attr: false,
            });
        }
    }
    refs
}

fn hls_uri_attributes(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(idx) = rest.find("URI=") {
        rest = &rest[idx + 4..];
        let (uri, next) = if let Some(quoted) = rest.strip_prefix('"') {
            match quoted.split_once('"') {
                Some((uri, tail)) => (uri, tail),
                None => break,
            }
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        };
        let uri = uri.trim();
        if !uri.is_empty() {
            out.push(uri.to_string());
        }
        rest = next;
    }
    out
}

fn rewrite_hls_playlist_to_local(
    base_url: &str,
    body: &str,
    url_paths: &BTreeMap<String, String>,
) -> Result<String, String> {
    let mut out = String::new();
    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            out.push_str(raw_line);
        } else if line.starts_with('#') {
            out.push_str(&rewrite_hls_uri_attributes(base_url, raw_line, url_paths)?);
        } else {
            let resolved = resolve_hls_child_url(base_url, line)?;
            if let Some(path) = url_paths.get(&resolved) {
                out.push_str(path);
            } else {
                out.push_str(raw_line);
            }
        }
        out.push('\n');
    }
    Ok(out)
}

fn rewrite_hls_uri_attributes(
    base_url: &str,
    line: &str,
    url_paths: &BTreeMap<String, String>,
) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = line;
    while let Some(idx) = rest.find("URI=") {
        out.push_str(&rest[..idx + 4]);
        rest = &rest[idx + 4..];
        if let Some(quoted) = rest.strip_prefix('"') {
            let Some(end) = quoted.find('"') else {
                out.push('"');
                out.push_str(quoted);
                return Ok(out);
            };
            let raw_uri = &quoted[..end];
            out.push('"');
            out.push_str(&rewrite_hls_uri_value(base_url, raw_uri, url_paths)?);
            out.push('"');
            rest = &quoted[end + 1..];
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            let raw_uri = &rest[..end];
            out.push_str(&rewrite_hls_uri_value(base_url, raw_uri, url_paths)?);
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

fn rewrite_hls_uri_value(
    base_url: &str,
    raw_uri: &str,
    url_paths: &BTreeMap<String, String>,
) -> Result<String, String> {
    let trimmed = raw_uri.trim();
    if trimmed.is_empty() {
        return Ok(raw_uri.to_string());
    }
    let resolved = resolve_hls_child_url(base_url, trimmed)?;
    Ok(url_paths
        .get(&resolved)
        .cloned()
        .unwrap_or_else(|| raw_uri.to_string()))
}

fn resolve_hls_child_url(base_url: &str, child: &str) -> Result<String, String> {
    let base = reqwest::Url::parse(base_url)
        .map_err(|e| format!("browser HLS package base URL is invalid: {e}"))?;
    let child = child.trim();
    if child.as_bytes().contains(&0) {
        return Err("browser HLS package rejects NUL bytes in child URI".to_string());
    }
    let resolved = base
        .join(child)
        .map_err(|e| format!("browser HLS package child URI `{child}` is invalid: {e}"))?;
    if !matches!(resolved.scheme(), "http" | "https") {
        return Err("browser HLS package only follows http:// or https:// child URIs".to_string());
    }
    Ok(resolved.to_string())
}

fn hls_url_filename(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back())
                .map(safe_browser_download_filename)
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "browser-media".to_string())
}

fn unique_browser_package_filename(raw: &str, used: &mut BTreeSet<String>) -> String {
    let filename = safe_browser_download_filename(raw);
    if used.insert(filename.clone()) {
        return filename;
    }
    let path = Path::new(&filename);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("browser-media");
    let ext = path.extension().and_then(|ext| ext.to_str());
    for idx in 2..=BROWSER_HLS_MAX_ASSETS + BROWSER_HLS_MAX_PLAYLISTS + 2 {
        let candidate = match ext {
            Some(ext) if !ext.is_empty() => format!("{stem}-{idx}.{ext}"),
            _ => format!("{stem}-{idx}"),
        };
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    format!("browser-media-{}", used.len() + 1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DashReference {
    kind: &'static str,
    url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DashRepresentation {
    id: String,
    bandwidth: String,
}

fn dash_mpd_references(manifest_url: &str, body: &str) -> Result<Vec<DashReference>, String> {
    let manifest_base = reqwest::Url::parse(manifest_url)
        .map_err(|e| format!("browser DASH package MPD URL is invalid: {e}"))?;
    let base_urls = dash_base_urls(body);
    let bases = if base_urls.is_empty() {
        vec![manifest_base]
    } else {
        base_urls
            .iter()
            .map(|base| resolve_dash_child_url(manifest_base.as_str(), base))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|url| reqwest::Url::parse(&url).ok())
            .collect::<Vec<_>>()
    };
    let bases = if bases.is_empty() {
        vec![reqwest::Url::parse(manifest_url)
            .map_err(|e| format!("browser DASH package MPD URL is invalid: {e}"))?]
    } else {
        bases
    };
    let representations = dash_representations(body);
    let numbers = dash_segment_numbers(body);
    let mut out = Vec::new();

    for source in dash_source_urls(body) {
        for base in &bases {
            out.push(DashReference {
                kind: "dash-asset",
                url: resolve_dash_child_url(base.as_str(), &source)?,
            });
        }
    }

    for tag in xml_tags_named(body, "SegmentTemplate") {
        let start_number = xml_attr(&tag, "startNumber")
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(1);
        let numbers = if numbers.is_empty() {
            vec![start_number]
        } else {
            numbers.clone()
        };
        let init = xml_attr(&tag, "initialization");
        let media = xml_attr(&tag, "media");
        for representation in &representations {
            for base in &bases {
                if let Some(init) = init.as_deref() {
                    let uri = dash_expand_template(init, representation, start_number);
                    out.push(DashReference {
                        kind: "dash-init",
                        url: resolve_dash_child_url(base.as_str(), &uri)?,
                    });
                }
                if let Some(media) = media.as_deref() {
                    for number in &numbers {
                        let uri = dash_expand_template(media, representation, *number);
                        out.push(DashReference {
                            kind: "dash-segment",
                            url: resolve_dash_child_url(base.as_str(), &uri)?,
                        });
                    }
                }
            }
        }
    }

    Ok(out)
}

fn dash_base_urls(body: &str) -> Vec<String> {
    xml_element_texts(body, "BaseURL")
        .into_iter()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect()
}

fn dash_source_urls(body: &str) -> Vec<String> {
    xml_tags(body)
        .into_iter()
        .filter_map(|tag| xml_attr(&tag, "sourceURL"))
        .filter(|url| !url.trim().is_empty())
        .collect()
}

fn dash_representations(body: &str) -> Vec<DashRepresentation> {
    let reps = xml_tags_named(body, "Representation")
        .into_iter()
        .map(|tag| DashRepresentation {
            id: xml_attr(&tag, "id").unwrap_or_else(|| "representation".to_string()),
            bandwidth: xml_attr(&tag, "bandwidth").unwrap_or_else(|| "0".to_string()),
        })
        .collect::<Vec<_>>();
    if reps.is_empty() {
        vec![DashRepresentation {
            id: "representation".to_string(),
            bandwidth: "0".to_string(),
        }]
    } else {
        reps
    }
}

fn dash_segment_numbers(body: &str) -> Vec<u64> {
    let mut numbers = Vec::new();
    let mut next = xml_tags_named(body, "SegmentTemplate")
        .into_iter()
        .find_map(|tag| xml_attr(&tag, "startNumber"))
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(1);
    for tag in xml_tags_named(body, "S") {
        let repeat = xml_attr(&tag, "r")
            .and_then(|r| r.parse::<i64>().ok())
            .map_or(1usize, |r| {
                if r < 0 {
                    1
                } else {
                    (r as usize).saturating_add(1)
                }
            });
        for _ in 0..repeat {
            if numbers.len() >= BROWSER_DASH_MAX_ASSETS {
                return numbers;
            }
            numbers.push(next);
            next = next.saturating_add(1);
        }
    }
    numbers
}

fn dash_expand_template(
    template: &str,
    representation: &DashRepresentation,
    number: u64,
) -> String {
    let with_rep = template
        .replace("$RepresentationID$", &representation.id)
        .replace("$Bandwidth$", &representation.bandwidth);
    dash_expand_number_token(&with_rep, number)
}

fn dash_expand_number_token(template: &str, number: u64) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("$Number") {
        out.push_str(&rest[..start]);
        let token_rest = &rest[start + 1..];
        let Some(end) = token_rest.find('$') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &token_rest[..end];
        if let Some(width) = token
            .strip_prefix("Number%0")
            .and_then(|raw| raw.strip_suffix('d'))
            .and_then(|raw| raw.parse::<usize>().ok())
        {
            out.push_str(&format!("{number:0width$}"));
        } else {
            out.push_str(&number.to_string());
        }
        rest = &token_rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn resolve_dash_child_url(base_url: &str, child: &str) -> Result<String, String> {
    let base = reqwest::Url::parse(base_url)
        .map_err(|e| format!("browser DASH package base URL is invalid: {e}"))?;
    let child = xml_unescape(child.trim());
    if child.as_bytes().contains(&0) {
        return Err("browser DASH package rejects NUL bytes in child URI".to_string());
    }
    let resolved = base
        .join(&child)
        .map_err(|e| format!("browser DASH package child URI `{child}` is invalid: {e}"))?;
    if !matches!(resolved.scheme(), "http" | "https") {
        return Err("browser DASH package only follows http:// or https:// child URIs".to_string());
    }
    Ok(resolved.to_string())
}

fn rewrite_dash_mpd_to_local(
    manifest_url: &str,
    body: &str,
    url_paths: &BTreeMap<String, String>,
) -> Result<String, String> {
    let base_urls = dash_base_urls(body);
    let manifest_base = reqwest::Url::parse(manifest_url)
        .map_err(|e| format!("browser DASH package MPD URL is invalid: {e}"))?;
    let mut bases = if base_urls.is_empty() {
        vec![manifest_base]
    } else {
        base_urls
            .iter()
            .map(|base| resolve_dash_child_url(manifest_base.as_str(), base))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|url| reqwest::Url::parse(&url).ok())
            .collect::<Vec<_>>()
    };
    if bases.is_empty() {
        bases.push(
            reqwest::Url::parse(manifest_url)
                .map_err(|e| format!("browser DASH package MPD URL is invalid: {e}"))?,
        );
    }
    let representations = dash_representations(body);
    let numbers = dash_segment_numbers(body);
    let mut replacements = BTreeMap::new();

    for source in dash_source_urls(body) {
        for base in &bases {
            let resolved = resolve_dash_child_url(base.as_str(), &source)?;
            if let Some(path) = url_paths.get(&resolved) {
                replacements.insert(source.clone(), path.clone());
            }
        }
    }

    for tag in xml_tags_named(body, "SegmentTemplate") {
        let start_number = xml_attr(&tag, "startNumber")
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(1);
        let numbers = if numbers.is_empty() {
            vec![start_number]
        } else {
            numbers.clone()
        };
        if let Some(init) = xml_attr(&tag, "initialization") {
            let rewrite_template = dash_local_asset_template(
                &init,
                &representations,
                start_number,
                &bases,
                url_paths,
            )?;
            if let Some(template) = rewrite_template {
                replacements.insert(init, template);
            }
        }
        if let Some(media) = xml_attr(&tag, "media") {
            let rewrite_template =
                dash_local_segment_template(&media, &representations, &numbers, &bases, url_paths)?;
            if let Some(template) = rewrite_template {
                replacements.insert(media, template);
            }
        }
    }

    let mut rewritten = rewrite_xml_element_texts(body, "BaseURL", "");
    for (from, to) in replacements {
        rewritten = rewritten.replace(&from, &xml_escape_attr(&to));
    }
    Ok(rewritten)
}

fn dash_local_asset_template(
    template: &str,
    representations: &[DashRepresentation],
    number: u64,
    bases: &[reqwest::Url],
    url_paths: &BTreeMap<String, String>,
) -> Result<Option<String>, String> {
    let Some(first_representation) = representations.first() else {
        return Ok(None);
    };
    let first_uri = dash_expand_template(template, first_representation, number);
    let mut first_path = None;
    for base in bases {
        let resolved = resolve_dash_child_url(base.as_str(), &first_uri)?;
        if let Some(path) = url_paths.get(&resolved) {
            first_path = Some(path.clone());
            break;
        }
    }
    let Some(first_path) = first_path else {
        return Ok(None);
    };
    Ok(Some(dash_localize_template_tokens(
        template,
        first_path,
        first_representation,
        number,
    )))
}

fn dash_local_segment_template(
    template: &str,
    representations: &[DashRepresentation],
    numbers: &[u64],
    bases: &[reqwest::Url],
    url_paths: &BTreeMap<String, String>,
) -> Result<Option<String>, String> {
    let Some(first_representation) = representations.first() else {
        return Ok(None);
    };
    let first_number = numbers.first().copied().unwrap_or(1);
    let first_uri = dash_expand_template(template, first_representation, first_number);
    let mut first_path = None;
    for base in bases {
        let resolved = resolve_dash_child_url(base.as_str(), &first_uri)?;
        if let Some(path) = url_paths.get(&resolved) {
            first_path = Some(path.clone());
            break;
        }
    }
    let Some(first_path) = first_path else {
        return Ok(None);
    };
    Ok(Some(dash_localize_template_tokens(
        template,
        first_path,
        first_representation,
        first_number,
    )))
}

fn dash_localize_template_tokens(
    source_template: &str,
    mut local_template: String,
    first_representation: &DashRepresentation,
    first_number: u64,
) -> String {
    if let Some(token) = dash_template_number_token(source_template) {
        let raw_number = if let Some(width) = token.width {
            format!("{first_number:0width$}")
        } else {
            first_number.to_string()
        };
        if local_template.contains(&raw_number) {
            local_template = local_template.replacen(&raw_number, token.raw, 1);
        }
    }
    if source_template.contains("$RepresentationID$")
        && local_template.contains(&first_representation.id)
    {
        local_template = local_template.replacen(&first_representation.id, "$RepresentationID$", 1);
    }
    if source_template.contains("$Bandwidth$")
        && local_template.contains(&first_representation.bandwidth)
    {
        local_template = local_template.replacen(&first_representation.bandwidth, "$Bandwidth$", 1);
    }
    local_template
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DashNumberToken<'a> {
    raw: &'a str,
    width: Option<usize>,
}

fn dash_template_number_token(template: &str) -> Option<DashNumberToken<'_>> {
    let start = template.find("$Number")?;
    let rest = &template[start..];
    let end = rest[1..].find('$')? + 2;
    let raw = &rest[..end];
    let inner = &raw[1..raw.len().saturating_sub(1)];
    let width = inner
        .strip_prefix("Number%0")
        .and_then(|raw| raw.strip_suffix('d'))
        .and_then(|raw| raw.parse::<usize>().ok());
    Some(DashNumberToken { raw, width })
}

fn rewrite_xml_element_texts(body: &str, name: &str, replacement: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    let open = format!("<{name}");
    let close = format!("</{name}>");
    while let Some(start) = rest.find(&open) {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(open_end) = rest.find('>') else {
            out.push_str(rest);
            return out;
        };
        let head_end = open_end + 1;
        out.push_str(&rest[..head_end]);
        rest = &rest[head_end..];
        let Some(close_start) = rest.find(&close) else {
            out.push_str(rest);
            return out;
        };
        out.push_str(replacement);
        out.push_str(&rest[close_start..close_start + close.len()]);
        rest = &rest[close_start + close.len()..];
    }
    out.push_str(rest);
    out
}

fn xml_escape_attr(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_tags(body: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        if rest.starts_with('/') || rest.starts_with('!') || rest.starts_with('?') {
            if let Some(end) = rest.find('>') {
                rest = &rest[end + 1..];
                continue;
            }
            break;
        }
        let Some(end) = rest.find('>') else {
            break;
        };
        tags.push(rest[..end].trim().trim_end_matches('/').trim().to_string());
        rest = &rest[end + 1..];
    }
    tags
}

fn xml_tags_named(body: &str, name: &str) -> Vec<String> {
    xml_tags(body)
        .into_iter()
        .filter(|tag| {
            tag == name
                || tag
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.chars().next().is_some_and(char::is_whitespace))
        })
        .collect()
}

fn xml_attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    loop {
        let idx = rest.find(name)?;
        rest = &rest[idx + name.len()..];
        if !rest.trim_start().starts_with('=') {
            continue;
        }
        rest = rest.trim_start();
        rest = rest.strip_prefix('=')?.trim_start();
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        rest = &rest[quote.len_utf8()..];
        let end = rest.find(quote)?;
        return Some(xml_unescape(&rest[..end]));
    }
}

fn xml_element_texts(body: &str, name: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = body;
    let open = format!("<{name}");
    let close = format!("</{name}>");
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        let Some(open_end) = rest.find('>') else {
            break;
        };
        rest = &rest[open_end + 1..];
        let Some(close_start) = rest.find(&close) else {
            break;
        };
        values.push(xml_unescape(&rest[..close_start]));
        rest = &rest[close_start + close.len()..];
    }
    values
}

fn xml_unescape(raw: &str) -> String {
    raw.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn safe_browser_download_filename(raw: &str) -> String {
    let leaf = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    let mut out = String::new();
    let mut last_dash = false;
    for ch in leaf.chars() {
        let next = if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            last_dash = false;
            Some(ch)
        } else if !last_dash {
            last_dash = true;
            Some('-')
        } else {
            None
        };
        if let Some(ch) = next {
            out.push(ch);
        }
        if out.len() >= 128 {
            break;
        }
    }
    let out = out.trim_matches(['.', '-', '_']);
    if out.is_empty() {
        "browser-media".to_string()
    } else {
        out.to_string()
    }
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
    if peer.is_empty() {
        None
    } else {
        Some(peer)
    }
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
    use std::collections::BTreeMap;
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
        assert!(plan
            .args
            .windows(2)
            .any(|w| w == ["-O", &dest.display().to_string()]));
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
        assert!(HttpWgetLane::plan(&job)
            .unwrap_err()
            .contains("invalid wget --limit-rate"));
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
        assert!(RsyncLane::plan(&job)
            .unwrap_err()
            .contains("invalid rsync --bwlimit"));
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
        assert_eq!(
            plan,
            BrowserDownloadPlan::CopyMaterialized {
                source: source.clone(),
                dest: dest_dir.clone()
            }
        );

        let remote = TransferJob::new(
            "https://example.invalid/file.bin",
            tmp.path().join("out.bin").display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );
        assert!(BrowserDownloadLane::plan(&remote)
            .unwrap_err()
            .contains("local materialized source"));
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
    fn browser_download_plan_promotes_media_request_files_to_http_fetches() {
        let tmp = tempfile::tempdir().unwrap();
        let request = tmp.path().join("asset.download.json");
        let dest_dir = tmp.path().join("picked");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(
            &request,
            serde_json::json!({
                "op": "browser_media_download_request",
                "asset_url": "https://media.example.test/video/poster image.jpg",
                "suggested_filename": "../poster image.jpg",
                "ignore_blocking": true,
                "rename_strategy": "auto_rename_by_url_hint",
            })
            .to_string(),
        )
        .unwrap();
        let job = TransferJob::new(
            request.display().to_string(),
            dest_dir.display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );

        let plan = BrowserDownloadLane::plan(&job).unwrap();

        assert_eq!(
            plan,
            BrowserDownloadPlan::FetchMedia {
                asset_url: "https://media.example.test/video/poster image.jpg".to_string(),
                dest: dest_dir.join("poster-image.jpg")
            }
        );
    }

    #[test]
    fn browser_download_plan_promotes_hls_requests_to_package_fetches() {
        let tmp = tempfile::tempdir().unwrap();
        let request = tmp.path().join("asset.download.json");
        let dest_dir = tmp.path().join("picked");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(
            &request,
            serde_json::json!({
                "op": "browser_media_download_request",
                "asset_url": "https://media.example.test/video/master.m3u8",
                "suggested_filename": "../master playlist.m3u8",
                "kind": "hls",
                "ignore_blocking": true,
                "rename_strategy": "auto_rename_by_url_hint",
            })
            .to_string(),
        )
        .unwrap();
        let job = TransferJob::new(
            request.display().to_string(),
            dest_dir.display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );

        let plan = BrowserDownloadLane::plan(&job).unwrap();

        assert_eq!(
            plan,
            BrowserDownloadPlan::FetchHlsPackage {
                manifest_url: "https://media.example.test/video/master.m3u8".to_string(),
                package_dir: dest_dir.join("master-playlist.hls"),
                manifest_filename: "master-playlist.m3u8".to_string()
            }
        );
    }

    #[test]
    fn browser_download_plan_promotes_dash_requests_to_package_fetches() {
        let tmp = tempfile::tempdir().unwrap();
        let request = tmp.path().join("asset.download.json");
        let dest_dir = tmp.path().join("picked");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(
            &request,
            serde_json::json!({
                "op": "browser_media_download_request",
                "asset_url": "https://media.example.test/video/manifest.mpd",
                "suggested_filename": "../dash manifest.mpd",
                "kind": "dash",
                "ignore_blocking": true,
                "rename_strategy": "auto_rename_by_url_hint",
            })
            .to_string(),
        )
        .unwrap();
        let job = TransferJob::new(
            request.display().to_string(),
            dest_dir.display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );

        let plan = BrowserDownloadLane::plan(&job).unwrap();

        assert_eq!(
            plan,
            BrowserDownloadPlan::FetchDashPackage {
                manifest_url: "https://media.example.test/video/manifest.mpd".to_string(),
                package_dir: dest_dir.join("dash-manifest.dash"),
                manifest_filename: "dash-manifest.mpd".to_string()
            }
        );
    }

    #[test]
    fn browser_download_plan_promotes_scrape_exports_to_crawl_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let request = tmp.path().join("mde-browser-scrape-1-example.json");
        let dest_dir = tmp.path().join("picked");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(
            &request,
            serde_json::json!({
                "op": "browser_active_page_scrape",
                "url": "https://example.test/articles/root.html",
                "crawl_execution_status": "not_started",
                "crawl_manifest": [
                    {
                        "url": "https://example.test/articles/related.html",
                        "source": "telemetry",
                        "resource": "xhr",
                        "same_origin": true,
                        "depth": 1
                    },
                    {
                        "url": "https://example.test/articles/related.html",
                        "source": "dom_link",
                        "resource": "document",
                        "same_origin": true,
                        "depth": 1
                    },
                    {
                        "url": "https://elsewhere.test/",
                        "source": "dom_link",
                        "resource": "document",
                        "same_origin": false,
                        "depth": 1
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let job = TransferJob::new(
            request.display().to_string(),
            dest_dir.display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );

        let plan = BrowserDownloadLane::plan(&job).unwrap();

        assert_eq!(
            plan,
            BrowserDownloadPlan::FetchScrapeCrawlPackage {
                source: request.clone(),
                dest: dest_dir.join("mde-browser-scrape-1-example.json"),
                page_url: "https://example.test/articles/root.html".to_string(),
                package_dir: dest_dir.join("mde-browser-scrape-1-example.crawl"),
                targets: vec![BrowserScrapeCrawlTarget {
                    url: "https://example.test/articles/related.html".to_string(),
                    source: "telemetry".to_string(),
                    resource: "xhr".to_string(),
                    depth: 1,
                }]
            }
        );
    }

    #[tokio::test]
    async fn browser_download_lane_fetches_media_request_assets() {
        if StdCommand::new("wget").arg("--version").output().is_err() {
            return;
        }
        let body = b"#EXTM3U\n#EXT-X-VERSION:3\n".to_vec();
        let (url, _range_rx, join) = fixture_http_server(body.clone());
        let tmp = tempfile::tempdir().unwrap();
        let request = tmp.path().join("asset.download.json");
        let dest_dir = tmp.path().join("picked");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(
            &request,
            serde_json::json!({
                "op": "browser_media_download_request",
                "asset_url": url,
                "suggested_filename": "poster.jpg",
                "kind": "image",
                "allowed_by_page_filter": false,
                "ignore_blocking": true,
                "rename_strategy": "auto_rename_by_url_hint",
            })
            .to_string(),
        )
        .unwrap();
        let job = TransferJob::new(
            request.display().to_string(),
            dest_dir.display().to_string(),
            Method::BrowserDownload,
            TransferPolicy {
                bwlimit: None,
                verify: false,
            },
        );

        let outcome = BrowserDownloadLane.run(&job, ProgressSink::noop()).await;
        join.join().unwrap();

        assert_eq!(outcome, LaneOutcome::Done);
        assert_eq!(std::fs::read(dest_dir.join("poster.jpg")).unwrap(), body);
    }

    #[test]
    fn hls_playlist_parser_finds_child_playlists_segments_and_uri_attributes() {
        let body = "\
#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=1200000
variant/main.m3u8
#EXT-X-MAP:URI=\"init.mp4\"
#EXT-X-KEY:METHOD=AES-128,URI=\"../keys/key.bin\"
#EXTINF:4,
seg-1.ts
";
        let refs = hls_playlist_references(body);
        assert_eq!(
            refs.iter().map(|r| r.uri.as_str()).collect::<Vec<_>>(),
            vec![
                "variant/main.m3u8",
                "init.mp4",
                "../keys/key.bin",
                "seg-1.ts"
            ]
        );
        assert_eq!(
            resolve_hls_child_url(
                "https://cdn.example.test/path/master.m3u8",
                "../keys/key.bin"
            )
            .unwrap(),
            "https://cdn.example.test/keys/key.bin"
        );
        let mut paths = BTreeMap::new();
        paths.insert(
            "https://cdn.example.test/path/variant/main.m3u8".to_string(),
            "main.m3u8".to_string(),
        );
        paths.insert(
            "https://cdn.example.test/path/init.mp4".to_string(),
            "init.mp4".to_string(),
        );
        paths.insert(
            "https://cdn.example.test/keys/key.bin".to_string(),
            "key.bin".to_string(),
        );
        paths.insert(
            "https://cdn.example.test/path/seg-1.ts".to_string(),
            "seg-1.ts".to_string(),
        );
        let rewritten = rewrite_hls_playlist_to_local(
            "https://cdn.example.test/path/master.m3u8",
            body,
            &paths,
        )
        .unwrap();
        assert!(rewritten.contains("\nmain.m3u8\n"));
        assert!(rewritten.contains("URI=\"init.mp4\""));
        assert!(rewritten.contains("URI=\"key.bin\""));
        assert!(rewritten.contains("\nseg-1.ts\n"));
        assert!(!rewritten.contains("variant/main.m3u8"));
        assert!(!rewritten.contains("../keys/key.bin"));
    }

    #[tokio::test]
    async fn browser_download_lane_expands_hls_playlist_packages() {
        if StdCommand::new("wget").arg("--version").output().is_err() {
            return;
        }
        let master = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1200000\nvariant/main.m3u8\n".to_vec();
        let variant = b"#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXT-X-KEY:METHOD=AES-128,URI=\"../keys/key.bin\"\n#EXTINF:4,\nseg-1.ts\n#EXTINF:4,\nmedia/seg-2.ts\n".to_vec();
        let (base_url, requests_rx, join) = fixture_http_routes_server(vec![
            ("/video/master.m3u8", master.clone()),
            ("/video/variant/main.m3u8", variant.clone()),
            ("/video/variant/init.mp4", b"init".to_vec()),
            ("/video/keys/key.bin", b"key".to_vec()),
            ("/video/variant/seg-1.ts", b"seg1".to_vec()),
            ("/video/variant/media/seg-2.ts", b"seg2".to_vec()),
        ]);
        let tmp = tempfile::tempdir().unwrap();
        let request = tmp.path().join("asset.download.json");
        let dest_dir = tmp.path().join("picked");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(
            &request,
            serde_json::json!({
                "op": "browser_media_download_request",
                "asset_url": format!("{base_url}/video/master.m3u8"),
                "suggested_filename": "master.m3u8",
                "kind": "hls",
                "ignore_blocking": true,
                "rename_strategy": "auto_rename_by_url_hint",
            })
            .to_string(),
        )
        .unwrap();
        let job = TransferJob::new(
            request.display().to_string(),
            dest_dir.display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );

        let outcome = BrowserDownloadLane.run(&job, ProgressSink::noop()).await;
        let requests = requests_rx.recv().unwrap();
        join.join().unwrap();

        assert_eq!(outcome, LaneOutcome::Done);
        let package = dest_dir.join("master.hls");
        let master_playlist =
            std::fs::read_to_string(package.join("master.m3u8")).expect("read rewritten master");
        let variant_playlist =
            std::fs::read_to_string(package.join("main.m3u8")).expect("read rewritten variant");
        assert!(master_playlist.contains("\nmain.m3u8\n"));
        assert!(!master_playlist.contains("variant/main.m3u8"));
        assert!(variant_playlist.contains("URI=\"init.mp4\""));
        assert!(variant_playlist.contains("URI=\"key.bin\""));
        assert!(variant_playlist.contains("\nseg-1.ts\n"));
        assert!(variant_playlist.contains("\nseg-2.ts\n"));
        assert!(!variant_playlist.contains("../keys/key.bin"));
        assert!(!variant_playlist.contains("media/seg-2.ts"));
        assert_eq!(std::fs::read(package.join("init.mp4")).unwrap(), b"init");
        assert_eq!(std::fs::read(package.join("key.bin")).unwrap(), b"key");
        assert_eq!(std::fs::read(package.join("seg-1.ts")).unwrap(), b"seg1");
        assert_eq!(std::fs::read(package.join("seg-2.ts")).unwrap(), b"seg2");
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(package.join("browser-hls-package.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["op"], "browser_hls_download_package");
        assert_eq!(manifest["offline_rewrite_status"], "completed");
        assert_eq!(manifest["root_playlist_path"], "master.m3u8");
        assert_eq!(manifest["playlist_count"], 2);
        assert_eq!(manifest["asset_count"], 4);
        assert!(requests.contains(&"/video/master.m3u8".to_string()));
        assert!(requests.contains(&"/video/variant/main.m3u8".to_string()));
        assert!(requests.contains(&"/video/variant/media/seg-2.ts".to_string()));
    }

    #[test]
    fn dash_mpd_parser_expands_baseurl_templates_and_timeline() {
        let body = r#"
<MPD>
  <Period>
    <AdaptationSet>
      <BaseURL>media/</BaseURL>
      <Representation id="v1" bandwidth="1200000">
        <SegmentTemplate initialization="init-$RepresentationID$.mp4"
          media="chunk-$RepresentationID$-$Number%05d$.m4s" startNumber="7">
          <SegmentTimeline>
            <S d="2000" r="1" />
            <S d="2000" />
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>
"#;

        let refs =
            dash_mpd_references("https://cdn.example.test/video/manifest.mpd", body).unwrap();

        assert_eq!(
            refs,
            vec![
                DashReference {
                    kind: "dash-init",
                    url: "https://cdn.example.test/video/media/init-v1.mp4".to_string()
                },
                DashReference {
                    kind: "dash-segment",
                    url: "https://cdn.example.test/video/media/chunk-v1-00007.m4s".to_string()
                },
                DashReference {
                    kind: "dash-segment",
                    url: "https://cdn.example.test/video/media/chunk-v1-00008.m4s".to_string()
                },
                DashReference {
                    kind: "dash-segment",
                    url: "https://cdn.example.test/video/media/chunk-v1-00009.m4s".to_string()
                }
            ]
        );
        let mut paths = BTreeMap::new();
        paths.insert(
            "https://cdn.example.test/video/media/init-v1.mp4".to_string(),
            "init-v1.mp4".to_string(),
        );
        paths.insert(
            "https://cdn.example.test/video/media/chunk-v1-00007.m4s".to_string(),
            "chunk-v1-00007.m4s".to_string(),
        );
        let rewritten =
            rewrite_dash_mpd_to_local("https://cdn.example.test/video/manifest.mpd", body, &paths)
                .unwrap();
        assert!(rewritten.contains("<BaseURL></BaseURL>"));
        assert!(rewritten.contains("initialization=\"init-$RepresentationID$.mp4\""));
        assert!(rewritten.contains("media=\"chunk-$RepresentationID$-$Number%05d$.m4s\""));
        assert!(!rewritten.contains("<BaseURL>media/</BaseURL>"));
    }

    #[tokio::test]
    async fn browser_download_lane_expands_dash_mpd_packages() {
        if StdCommand::new("wget").arg("--version").output().is_err() {
            return;
        }
        let mpd = br#"<MPD>
  <Period>
    <AdaptationSet>
      <BaseURL>media/</BaseURL>
      <Representation id="v1" bandwidth="1200000">
        <SegmentTemplate initialization="init-$RepresentationID$.mp4"
          media="chunk-$RepresentationID$-$Number$.m4s" startNumber="3">
          <SegmentTimeline><S d="2" r="1"/></SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>
"#
        .to_vec();
        let (base_url, requests_rx, join) = fixture_http_routes_server(vec![
            ("/video/manifest.mpd", mpd.clone()),
            ("/video/media/init-v1.mp4", b"init".to_vec()),
            ("/video/media/chunk-v1-3.m4s", b"seg3".to_vec()),
            ("/video/media/chunk-v1-4.m4s", b"seg4".to_vec()),
        ]);
        let tmp = tempfile::tempdir().unwrap();
        let request = tmp.path().join("asset.download.json");
        let dest_dir = tmp.path().join("picked");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(
            &request,
            serde_json::json!({
                "op": "browser_media_download_request",
                "asset_url": format!("{base_url}/video/manifest.mpd"),
                "suggested_filename": "manifest.mpd",
                "kind": "dash",
                "ignore_blocking": true,
                "rename_strategy": "auto_rename_by_url_hint",
            })
            .to_string(),
        )
        .unwrap();
        let job = TransferJob::new(
            request.display().to_string(),
            dest_dir.display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );

        let outcome = BrowserDownloadLane.run(&job, ProgressSink::noop()).await;
        let requests = requests_rx.recv().unwrap();
        join.join().unwrap();

        assert_eq!(outcome, LaneOutcome::Done);
        let package = dest_dir.join("manifest.dash");
        let rewritten_mpd =
            std::fs::read_to_string(package.join("manifest.mpd")).expect("read rewritten MPD");
        assert!(rewritten_mpd.contains("<BaseURL></BaseURL>"));
        assert!(rewritten_mpd.contains("initialization=\"init-$RepresentationID$.mp4\""));
        assert!(rewritten_mpd.contains("media=\"chunk-$RepresentationID$-$Number$.m4s\""));
        assert!(!rewritten_mpd.contains("<BaseURL>media/</BaseURL>"));
        assert_eq!(std::fs::read(package.join("init-v1.mp4")).unwrap(), b"init");
        assert_eq!(
            std::fs::read(package.join("chunk-v1-3.m4s")).unwrap(),
            b"seg3"
        );
        assert_eq!(
            std::fs::read(package.join("chunk-v1-4.m4s")).unwrap(),
            b"seg4"
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(package.join("browser-dash-package.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["op"], "browser_dash_download_package");
        assert_eq!(manifest["offline_rewrite_status"], "completed");
        assert_eq!(manifest["root_mpd_path"], "manifest.mpd");
        assert_eq!(manifest["asset_count"], 3);
        assert!(requests.contains(&"/video/manifest.mpd".to_string()));
        assert!(requests.contains(&"/video/media/init-v1.mp4".to_string()));
        assert!(requests.contains(&"/video/media/chunk-v1-4.m4s".to_string()));
    }

    #[tokio::test]
    async fn browser_download_lane_executes_scrape_crawl_packages() {
        if StdCommand::new("wget").arg("--version").output().is_err() {
            return;
        }
        let (base_url, requests_rx, join) = fixture_http_routes_server(vec![
            ("/articles/related.html", b"<html>related</html>".to_vec()),
            ("/articles/part-2.html", b"<html>part two</html>".to_vec()),
        ]);
        let tmp = tempfile::tempdir().unwrap();
        let request = tmp.path().join("mde-browser-scrape-2-example.json");
        let dest_dir = tmp.path().join("picked");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(
            &request,
            serde_json::json!({
                "op": "browser_active_page_scrape",
                "url": format!("{base_url}/articles/root.html"),
                "crawl_execution_status": "not_started",
                "crawl_manifest": [
                    {
                        "url": format!("{base_url}/articles/related.html"),
                        "source": "telemetry",
                        "resource": "xhr",
                        "same_origin": true,
                        "depth": 1
                    },
                    {
                        "url": format!("{base_url}/articles/part-2.html"),
                        "source": "dom_link",
                        "resource": "document",
                        "same_origin": true,
                        "depth": 1
                    },
                    {
                        "url": "https://elsewhere.test/",
                        "source": "dom_link",
                        "resource": "document",
                        "same_origin": false,
                        "depth": 1
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let job = TransferJob::new(
            request.display().to_string(),
            dest_dir.display().to_string(),
            Method::BrowserDownload,
            TransferPolicy::default(),
        );

        let outcome = BrowserDownloadLane.run(&job, ProgressSink::noop()).await;
        let requests = requests_rx.recv().unwrap();
        join.join().unwrap();

        assert_eq!(outcome, LaneOutcome::Done);
        assert!(
            dest_dir.join("mde-browser-scrape-2-example.json").exists(),
            "original scrape JSON export is still copied"
        );
        let package = dest_dir.join("mde-browser-scrape-2-example.crawl");
        assert_eq!(
            std::fs::read(package.join("related.html")).unwrap(),
            b"<html>related</html>"
        );
        assert_eq!(
            std::fs::read(package.join("part-2.html")).unwrap(),
            b"<html>part two</html>"
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(package.join("browser-scrape-crawl-package.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["op"], "browser_scrape_crawl_package");
        assert_eq!(manifest["execution_status"], "completed");
        assert_eq!(manifest["target_count"], 2);
        assert!(requests.contains(&"/articles/related.html".to_string()));
        assert!(requests.contains(&"/articles/part-2.html".to_string()));
        assert!(!requests.iter().any(|path| path.contains("elsewhere")));
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
        assert!(MusicLibraryLane::plan(&job)
            .unwrap_err()
            .contains("local filesystem source"));
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
        assert!(NodeLane::plan_with_root(&job, root.clone())
            .unwrap_err()
            .contains("local filesystem source"));
        job.source = "/home/user/file.iso".into();
        job.dest = "sftp.example.com:/drop".into();
        assert!(NodeLane::plan_with_root(&job, root)
            .unwrap_err()
            .contains("node:<peer>"));
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

    fn fixture_http_routes_server(
        routes: Vec<(&'static str, Vec<u8>)>,
    ) -> (String, mpsc::Receiver<Vec<String>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let routes = routes.into_iter().collect::<BTreeMap<_, _>>();
        let expected = routes.len();
        let (tx, rx) = mpsc::channel();
        let join = thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..expected {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                seen.push(path.clone());
                if let Some(body) = routes.get(path.as_str()) {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(head.as_bytes()).unwrap();
                    stream.write_all(body).unwrap();
                } else {
                    let body = b"not found";
                    let head = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(head.as_bytes()).unwrap();
                    stream.write_all(body).unwrap();
                }
            }
            let _ = tx.send(seen);
        });
        (format!("http://{addr}"), rx, join)
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
