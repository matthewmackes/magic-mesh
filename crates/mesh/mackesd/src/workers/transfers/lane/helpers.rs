//! TRANSFERS lane fs/io + validation helpers — the leaf argument-validation, sftp
//! remote parsing, path/destination resolution, child-process output plumbing and
//! progress parsers split out of the `lane` worker god-module (pure relocation, no
//! behaviour change).
//!
//! `use super::*` pulls in the parent `lane` module's std/lane imports and the plan
//! structs these helpers ride; the node destination + progress parsers stay `pub`
//! (the queue engine + external callers use them), the rest are `pub(super)`.

use super::*;

pub(super) fn valid_wget_rate(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

pub(super) fn valid_rsync_bwlimit(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

pub(super) fn parse_sftp_remote(raw: &str) -> Result<Option<SftpRemote>, String> {
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

pub(super) fn parse_sftp_url_remote(rest: &str) -> Result<SftpRemote, String> {
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

pub(super) fn split_sftp_user_host(authority: &str) -> Result<(Option<String>, &str), String> {
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

pub(super) fn split_sftp_host_port(host_port: &str) -> Result<(&str, Option<u16>), String> {
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

pub(super) async fn write_sftp_batch(direction: &SftpDirection) -> Result<SftpBatch, String> {
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

pub(super) fn sftp_batch_quote(s: &str) -> String {
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

pub(super) fn redact_transfer_secret(s: &str) -> String {
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

pub(super) fn local_parent_for_dest(dest: &str) -> Option<PathBuf> {
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

pub(super) fn local_source_path(raw: &str) -> Option<PathBuf> {
    let s = raw.trim();
    if s.is_empty()
        || s.contains("://")
        || (s.contains(':') && !s.starts_with('/') && !s.starts_with("./") && !s.starts_with("../"))
    {
        return None;
    }
    Some(PathBuf::from(s))
}

pub(super) fn resolve_dest_path(source: &Path, dest: &Path) -> PathBuf {
    if dest.is_dir() {
        if let Some(file_name) = source.file_name() {
            return dest.join(file_name);
        }
    }
    dest.to_path_buf()
}

pub(super) fn music_library_dir(dest: &str) -> PathBuf {
    let dest = dest.trim();
    if !dest.is_empty() && dest != "music-library" {
        return PathBuf::from(dest);
    }
    std::env::var(MUSIC_LIBRARY_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::default_qnm_shared_root().join("music-library"))
}

pub(super) fn mesh_share_dir() -> PathBuf {
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

pub(super) fn node_stage_dir(dest: &str, mesh_root: PathBuf) -> Result<PathBuf, String> {
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

pub(super) fn node_target_peer(dest: &str) -> Option<String> {
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

pub(super) fn prepare_destination(dest: &Path) -> Result<(), String> {
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

pub(super) fn process_tail(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let tail = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())?
        .trim();
    Some(tail.chars().take(240).collect())
}

pub(super) async fn collect_output<R>(reader: R, progress: Option<ProgressSink>) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    collect_output_with(reader, progress, parse_wget_progress_percent).await
}

pub(super) async fn collect_output_with<R>(
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

pub(super) async fn join_output(handle: Option<tokio::task::JoinHandle<Vec<u8>>>) -> Vec<u8> {
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
