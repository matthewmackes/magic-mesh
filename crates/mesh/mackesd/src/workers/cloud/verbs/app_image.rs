//! WL-FUNC-018 — fail-closed admission of the signed App VM base image.
//!
//! The image store is replicated state, not live guest proof.  This module only
//! admits an image when the promoted version, manifest, artifact digest, and a
//! bounded detached-signature evidence envelope agree.  It deliberately does
//! not claim that a guest has booted or that a compositor is reachable.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::image_catalog::{images_dir, validate_manifest, ImageKind, ImageManifest};
use mackes_mesh_types::vdi_session::{
    AppVmLaunchRequest, AppVmRuntimeEvidence, AppVmRuntimeState, APP_VM_RUNTIME_TOPIC,
};
use mde_bus::persist::Persist;

pub(super) const APP_VM_IMAGE_NAME: &str = "app-vm-wayland-standard";
const PROMOTED_MARKER: &str = "PROMOTED";
const SHA_SIDECAR: &str = "image.sha256";
const ADMISSION_EVIDENCE: &str = "admission.json";
const SCHEMA_VERSION: u16 = 1;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_ID_BYTES: usize = 255;
const MAX_SIGNATURE_BYTES: usize = 4096;
/// A guest observation is a point-in-time readiness proof, not a lease.  A
/// resume/reconcile request must obtain a fresh observation rather than
/// treating an old `connected` record as proof that the guest is still alive.
pub(super) const APP_VM_RUNTIME_STALE_AFTER_MS: i64 = 5 * 60 * 1000;

/// The bounded, publisher-written evidence required before an App VM image is
/// selected.  `signature` is a detached signed-evidence reference; cryptographic
/// verification belongs to the release publisher/trust-store lane.  Presence is
/// still mandatory here, so an unsigned local build can never be admitted.
#[derive(Debug, Clone, Deserialize)]
struct AppVmImageEvidence {
    schema_version: u16,
    image_name: String,
    image_version: String,
    guest_profile: String,
    sha256: String,
    issued_at_ms: u64,
    expires_at_ms: u64,
    #[serde(default)]
    signature: Option<String>,
}

/// The only outcomes the typed App VM admission path exposes to callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AppVmImageAdmission {
    /// No complete image/evidence record is currently available.
    Unavailable(String),
    /// The record exists but has no detached signature reference.
    Unsigned(String),
    /// The record was once usable but its bounded freshness window is invalid.
    Stale(String),
    /// Local replicated evidence is internally consistent and currently fresh.
    Admitted { version: String },
}

impl AppVmImageAdmission {
    pub(super) fn reason(&self) -> String {
        match self {
            Self::Unavailable(reason) => format!("unavailable: {reason}"),
            Self::Unsigned(reason) => format!("unsigned: {reason}"),
            Self::Stale(reason) => format!("stale: {reason}"),
            Self::Admitted { version } => format!("admitted version {version}"),
        }
    }
}

/// The daemon-side result of checking the latest guest App VM observation.
/// Every non-ready outcome is intentionally typed so callers cannot turn a
/// missing or terminal record into an optimistic launch claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AppVmRuntimeAdmission {
    /// No runtime observation is available for this session.
    Missing(String),
    /// The latest observation is outside the bounded freshness window.
    Stale(String),
    /// The observation is well-formed but belongs to another VM/app/session.
    Mismatched(String),
    /// The guest reported a terminal failure for this app process.
    Terminal(String),
    /// The latest observation is fresh and identity-bound.
    Observed {
        state: AppVmRuntimeState,
        generation: u64,
    },
}

impl AppVmRuntimeAdmission {
    pub(super) fn reason(&self) -> String {
        match self {
            Self::Missing(reason) => format!("missing: {reason}"),
            Self::Stale(reason) => format!("stale: {reason}"),
            Self::Mismatched(reason) => format!("mismatched: {reason}"),
            Self::Terminal(reason) => format!("terminal: {reason}"),
            Self::Observed { state, generation } => {
                format!("observed {state:?} at generation {generation}")
            }
        }
    }

    pub(super) fn is_usable(&self) -> bool {
        matches!(
            self,
            Self::Observed {
                state: AppVmRuntimeState::Installing
                    | AppVmRuntimeState::StartingApp
                    | AppVmRuntimeState::Connected
                    | AppVmRuntimeState::Paused
                    | AppVmRuntimeState::Reconnecting,
                ..
            }
        )
    }
}

/// Read the newest persisted guest observation and bind it to the typed App VM
/// declaration.  The bus envelope timestamp supplies freshness; the guest
/// payload supplies only bounded identities/state and is never interpreted as
/// a command.  A malformed newest record is refused instead of falling back to
/// an older optimistic record.
pub(super) fn check_runtime_evidence(
    bus_root: Option<&Path>,
    request: &AppVmLaunchRequest,
    expected_vm_id: &str,
    now_ms: i64,
) -> AppVmRuntimeAdmission {
    let Some(bus_root) = bus_root else {
        return AppVmRuntimeAdmission::Missing("the runtime bus is unavailable".to_owned());
    };
    let persist = match Persist::open(bus_root.to_path_buf()) {
        Ok(persist) => persist,
        Err(error) => {
            return AppVmRuntimeAdmission::Missing(format!(
                "the runtime evidence store is unavailable: {error}"
            ));
        }
    };
    let messages = match persist.list_since(APP_VM_RUNTIME_TOPIC, None) {
        Ok(messages) => messages,
        Err(error) => {
            return AppVmRuntimeAdmission::Missing(format!(
                "the runtime evidence record cannot be read: {error}"
            ));
        }
    };
    if messages.is_empty() {
        return AppVmRuntimeAdmission::Missing(
            "no guest runtime observation has been published".to_owned(),
        );
    }

    // A shared runtime topic carries observations for multiple App VM
    // sessions. Search newest-first for this exact identity; an unrelated
    // session's fresh heartbeat must not block a valid resume here. Once a
    // record can be identified as belonging to this request, all freshness,
    // validation, and terminal-state checks remain fail-closed. Keep the
    // newest generation as the authority while scanning: a delayed
    // lower-generation row must not roll back to an older guest incarnation.
    let mut newest_matching = None;
    for message in &messages {
        let Some(body) = message.body.as_deref() else {
            return AppVmRuntimeAdmission::Mismatched(
                "guest runtime record has no typed body".to_owned(),
            );
        };
        if !crate::ipc::body_within_cap(Some(body)) {
            return AppVmRuntimeAdmission::Mismatched(
                "guest runtime record exceeds the bounded body limit".to_owned(),
            );
        }
        let evidence = match serde_json::from_str::<AppVmRuntimeEvidence>(body) {
            Ok(evidence) => evidence,
            Err(_) => {
                return AppVmRuntimeAdmission::Mismatched(
                    "guest runtime record is not typed JSON".to_owned(),
                );
            }
        };
        if evidence.validate().is_err() {
            return AppVmRuntimeAdmission::Mismatched(
                "guest runtime record has invalid bounded identities".to_owned(),
            );
        }
        if evidence.session_id != request.session_id || evidence.app_id != request.app_id {
            continue;
        }
        if evidence.vm_id != expected_vm_id {
            return AppVmRuntimeAdmission::Mismatched(format!(
                "matching session/app observation belongs to VM `{}`, not `{expected_vm_id}`",
                evidence.vm_id
            ));
        }
        if let Some((_, newest_generation)) = newest_matching {
            if evidence.generation < newest_generation {
                return AppVmRuntimeAdmission::Stale(
                    "guest runtime evidence regressed to an older generation".to_owned(),
                );
            }
            continue;
        }

        if message.ts_unix_ms > now_ms
            || now_ms.saturating_sub(message.ts_unix_ms) > APP_VM_RUNTIME_STALE_AFTER_MS
        {
            return AppVmRuntimeAdmission::Stale(format!(
                "matching guest observation is outside the {}ms freshness window",
                APP_VM_RUNTIME_STALE_AFTER_MS
            ));
        }
        if evidence.state == AppVmRuntimeState::Failed {
            return AppVmRuntimeAdmission::Terminal(
                evidence
                    .reason
                    .unwrap_or_else(|| "guest application reported failure".to_owned()),
            );
        }
        newest_matching = Some((evidence.state, evidence.generation));
    }

    newest_matching.map_or_else(
        || {
            AppVmRuntimeAdmission::Mismatched(format!(
                "no guest observation matches session `{}` and app `{}`",
                request.session_id, request.app_id
            ))
        },
        |(state, generation)| AppVmRuntimeAdmission::Observed { state, generation },
    )
}

/// Check the fixed App VM image selected by `AppVmProfile`.
pub(super) fn check(state_root: &Path, guest_profile: &str, now_ms: u64) -> AppVmImageAdmission {
    let Some(image_store) = real_dir(&images_dir(state_root), "image store") else {
        return AppVmImageAdmission::Unavailable("the App VM image store is absent".to_owned());
    };
    let Some(image_dir) = real_dir(
        &image_store.join(APP_VM_IMAGE_NAME),
        "App VM image name directory",
    ) else {
        return AppVmImageAdmission::Unavailable(
            "the fixed App VM image name is not available".to_owned(),
        );
    };
    let Some(promoted) = read_text(&image_dir.join(PROMOTED_MARKER), PROMOTED_MARKER) else {
        return AppVmImageAdmission::Unavailable(
            "no promoted App VM image version is present".to_owned(),
        );
    };
    let version = promoted.trim();
    if !is_safe_token(version) {
        return AppVmImageAdmission::Unavailable(
            "the promoted App VM version is invalid".to_owned(),
        );
    }

    let Some(version_dir) = real_dir(&image_dir.join(version), "promoted App VM image version")
    else {
        return AppVmImageAdmission::Unavailable(format!(
            "promoted version {version} has no image directory"
        ));
    };

    let Some(manifest) = read_manifest(&version_dir) else {
        return AppVmImageAdmission::Unavailable(format!(
            "promoted version {version} has no valid manifest"
        ));
    };
    if manifest.name != APP_VM_IMAGE_NAME
        || manifest.version != version
        || manifest.kind != ImageKind::Vm.as_str()
        || manifest.profile.as_deref() != Some("app_vm")
    {
        return AppVmImageAdmission::Unavailable(
            "promoted App VM manifest does not match the fixed profile".to_owned(),
        );
    }

    let Some(artifact) = find_artifact(&version_dir) else {
        return AppVmImageAdmission::Unavailable(format!(
            "promoted version {version} has no image artifact"
        ));
    };
    let actual_sha = match hash_file(&artifact) {
        Ok(sha) => sha,
        Err(reason) => {
            return AppVmImageAdmission::Unavailable(format!(
                "cannot verify promoted image artifact: {reason}"
            ));
        }
    };
    let Some(recorded_sha) = read_text(&version_dir.join(SHA_SIDECAR), SHA_SIDECAR) else {
        return AppVmImageAdmission::Unavailable(format!(
            "promoted version {version} has no image digest evidence"
        ));
    };
    let recorded_sha = recorded_sha.trim();
    if !is_sha256(recorded_sha) || recorded_sha != actual_sha {
        return AppVmImageAdmission::Unavailable(
            "promoted image digest evidence does not match the artifact".to_owned(),
        );
    }

    let evidence_path = version_dir.join(ADMISSION_EVIDENCE);
    let Some(raw_evidence) = read_text(&evidence_path, ADMISSION_EVIDENCE) else {
        return AppVmImageAdmission::Unsigned(format!(
            "promoted version {version} has no detached signature evidence"
        ));
    };
    let evidence = match serde_json::from_str::<AppVmImageEvidence>(&raw_evidence) {
        Ok(evidence) => evidence,
        Err(_) => {
            return AppVmImageAdmission::Unsigned(
                "signature evidence is malformed or not admitted".to_owned(),
            );
        }
    };
    if evidence
        .signature
        .as_deref()
        .is_none_or(|s| s.trim().is_empty())
    {
        return AppVmImageAdmission::Unsigned(
            "signature evidence has no detached signature reference".to_owned(),
        );
    }
    if evidence.schema_version != SCHEMA_VERSION
        || evidence.image_name != APP_VM_IMAGE_NAME
        || evidence.image_version != version
        || evidence.guest_profile != guest_profile
        || evidence.sha256 != actual_sha
        || !is_safe_text(&evidence.image_name, MAX_ID_BYTES)
        || !is_safe_text(&evidence.image_version, MAX_ID_BYTES)
        || !is_safe_text(&evidence.guest_profile, MAX_ID_BYTES)
        || !is_sha256(&evidence.sha256)
        || evidence
            .signature
            .as_deref()
            .is_none_or(|s| !is_safe_text(s, MAX_SIGNATURE_BYTES))
    {
        return AppVmImageAdmission::Unavailable(
            "signature evidence does not match the promoted App VM image".to_owned(),
        );
    }
    if evidence.issued_at_ms > now_ms || evidence.expires_at_ms <= now_ms {
        return AppVmImageAdmission::Stale(
            "signature evidence is outside its issued/expiry window".to_owned(),
        );
    }

    AppVmImageAdmission::Admitted {
        version: version.to_owned(),
    }
}

fn read_manifest(version_dir: &Path) -> Option<ImageManifest> {
    let raw = read_text(&version_dir.join("manifest.toml"), "image manifest")?;
    let manifest = toml::from_str::<ImageManifest>(&raw).ok()?;
    validate_manifest(&manifest).ok()?;
    Some(manifest)
}

fn read_text(path: &Path, label: &str) -> Option<String> {
    let file = open_regular(path)?;
    let metadata = file.metadata().ok()?;
    if metadata.len() > MAX_TEXT_BYTES as u64 {
        tracing::warn!(path = %path.display(), label, "App VM image evidence exceeded bound");
        return None;
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize + 1);
    file.take((MAX_TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_TEXT_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn open_regular(path: &Path) -> Option<std::fs::File> {
    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?
        .into()
    };
    #[cfg(not(unix))]
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    metadata.file_type().is_file().then_some(file)
}

fn real_dir(path: &Path, _label: &str) -> Option<PathBuf> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return None;
    }
    Some(path.to_path_buf())
}

fn is_safe_token(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

fn is_safe_text(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max_bytes
        && !value.chars().any(char::is_control)
        && !value.contains('/')
        && !value.contains('\\')
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = open_regular(path).ok_or_else(|| "artifact is unavailable".to_owned())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("read artifact: {error}"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn find_artifact(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(current).ok()?.flatten() {
            let path = entry.path();
            let file_type = entry.file_type().ok()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file()
                || !path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        matches!(ext, "qcow2" | "raw" | "iso" | "img" | "oci" | "tar")
                    })
            {
                continue;
            }
            let size = std::fs::symlink_metadata(&path).ok()?.len();
            if best.as_ref().is_none_or(|(best_size, _)| size > *best_size) {
                best = Some((size, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::vdi_session::AppVmRuntimeState;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::tempdir;

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    fn fixture(root: &Path, signature: Option<&str>, expires_at_ms: u64) {
        let version = "2026.07.31";
        let dir = images_dir(root).join(APP_VM_IMAGE_NAME).join(version);
        std::fs::create_dir_all(&dir).unwrap();
        let artifact = dir.join("disk.qcow2");
        std::fs::write(&artifact, b"app-vm-fixture").unwrap();
        let sha = hash_file(&artifact).unwrap();
        std::fs::write(dir.join(SHA_SIDECAR), &sha).unwrap();
        std::fs::write(
            dir.join("manifest.toml"),
            format!(
                "name = \"{APP_VM_IMAGE_NAME}\"\nkind = \"vm\"\nversion = \"{version}\"\nprofile = \"app_vm\"\n"
            ),
        )
        .unwrap();
        std::fs::write(
            images_dir(root)
                .join(APP_VM_IMAGE_NAME)
                .join(PROMOTED_MARKER),
            version,
        )
        .unwrap();
        let now = now();
        let evidence = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "image_name": APP_VM_IMAGE_NAME,
            "image_version": version,
            "guest_profile": "wayland-standard",
            "sha256": sha,
            "issued_at_ms": now.saturating_sub(1000),
            "expires_at_ms": expires_at_ms,
            "signature": signature,
        });
        std::fs::write(dir.join(ADMISSION_EVIDENCE), evidence.to_string()).unwrap();
    }

    fn runtime_request(session_id: &str, app_id: &str) -> AppVmLaunchRequest {
        AppVmLaunchRequest::new(
            app_id,
            "catalog-7",
            "wayland-standard",
            Vec::new(),
            session_id,
            true,
        )
        .unwrap()
    }

    fn publish_runtime(
        root: &Path,
        session_id: &str,
        app_id: &str,
        generation: u64,
        state: AppVmRuntimeState,
        reason: Option<&str>,
    ) -> mde_bus::persist::StoredMessage {
        let evidence = AppVmRuntimeEvidence {
            session_id: session_id.to_owned(),
            vm_id: "app-vm-1".to_owned(),
            app_id: app_id.to_owned(),
            generation,
            state,
            reason: reason.map(str::to_owned),
        };
        Persist::open(root.to_path_buf())
            .unwrap()
            .write(
                APP_VM_RUNTIME_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&serde_json::to_string(&evidence).unwrap()),
            )
            .unwrap()
    }

    #[test]
    fn missing_promoted_image_is_unavailable() {
        let root = tempdir().unwrap();
        assert!(matches!(
            check(root.path(), "wayland-standard", now()),
            AppVmImageAdmission::Unavailable(_)
        ));
    }

    #[test]
    fn unsigned_image_is_not_admitted() {
        let root = tempdir().unwrap();
        fixture(root.path(), None, now() + 60_000);
        assert!(matches!(
            check(root.path(), "wayland-standard", now()),
            AppVmImageAdmission::Unsigned(_)
        ));
    }

    #[test]
    fn expired_signature_evidence_is_stale() {
        let root = tempdir().unwrap();
        fixture(
            root.path(),
            Some("publisher-signature"),
            now().saturating_sub(1),
        );
        assert!(matches!(
            check(root.path(), "wayland-standard", now()),
            AppVmImageAdmission::Stale(_)
        ));
    }

    #[test]
    fn matching_signed_fresh_image_is_admitted() {
        let root = tempdir().unwrap();
        fixture(root.path(), Some("publisher-signature"), now() + 60_000);
        assert_eq!(
            check(root.path(), "wayland-standard", now()),
            AppVmImageAdmission::Admitted {
                version: "2026.07.31".to_owned()
            }
        );
    }

    #[test]
    fn runtime_evidence_missing_is_not_admitted() {
        let root = tempdir().unwrap();
        let result = check_runtime_evidence(
            Some(root.path()),
            &runtime_request("session-1", "org.example.Writer"),
            "app-vm-1",
            now() as i64,
        );
        assert!(matches!(result, AppVmRuntimeAdmission::Missing(_)));
    }

    #[test]
    fn runtime_evidence_requires_fresh_matching_identity() {
        let root = tempdir().unwrap();
        let message = publish_runtime(
            root.path(),
            "session-1",
            "org.example.Writer",
            4,
            AppVmRuntimeState::Connected,
            None,
        );
        let result = check_runtime_evidence(
            Some(root.path()),
            &runtime_request("session-1", "org.example.Writer"),
            "app-vm-1",
            message.ts_unix_ms,
        );
        assert_eq!(
            result,
            AppVmRuntimeAdmission::Observed {
                state: AppVmRuntimeState::Connected,
                generation: 4,
            }
        );
    }

    #[test]
    fn runtime_evidence_rejects_stale_and_mismatched_records() {
        let root = tempdir().unwrap();
        let message = publish_runtime(
            root.path(),
            "session-1",
            "org.example.Writer",
            4,
            AppVmRuntimeState::Connected,
            None,
        );
        assert!(matches!(
            check_runtime_evidence(
                Some(root.path()),
                &runtime_request("session-1", "org.example.Writer"),
                "app-vm-1",
                message
                    .ts_unix_ms
                    .saturating_add(APP_VM_RUNTIME_STALE_AFTER_MS + 1),
            ),
            AppVmRuntimeAdmission::Stale(_)
        ));
        assert!(matches!(
            check_runtime_evidence(
                Some(root.path()),
                &runtime_request("other-session", "org.example.Writer"),
                "app-vm-1",
                message.ts_unix_ms,
            ),
            AppVmRuntimeAdmission::Mismatched(_)
        ));
    }

    #[test]
    fn runtime_evidence_ignores_newer_heartbeat_for_another_session() {
        let root = tempdir().unwrap();
        let matching = publish_runtime(
            root.path(),
            "session-1",
            "org.example.Writer",
            4,
            AppVmRuntimeState::Connected,
            None,
        );
        publish_runtime(
            root.path(),
            "session-2",
            "org.example.Reader",
            9,
            AppVmRuntimeState::Connected,
            None,
        );

        assert_eq!(
            check_runtime_evidence(
                Some(root.path()),
                &runtime_request("session-1", "org.example.Writer"),
                "app-vm-1",
                matching.ts_unix_ms,
            ),
            AppVmRuntimeAdmission::Observed {
                state: AppVmRuntimeState::Connected,
                generation: 4,
            }
        );
    }

    #[test]
    fn runtime_evidence_rejects_a_late_lower_generation_row() {
        let root = tempdir().unwrap();
        let newer = publish_runtime(
            root.path(),
            "session-1",
            "org.example.Writer",
            9,
            AppVmRuntimeState::Connected,
            None,
        );
        publish_runtime(
            root.path(),
            "session-1",
            "org.example.Writer",
            8,
            AppVmRuntimeState::Reconnecting,
            None,
        );

        assert!(matches!(
            check_runtime_evidence(
                Some(root.path()),
                &runtime_request("session-1", "org.example.Writer"),
                "app-vm-1",
                newer.ts_unix_ms,
            ),
            AppVmRuntimeAdmission::Stale(reason)
                if reason.contains("regressed")
        ));
    }

    #[test]
    fn runtime_evidence_rejects_terminal_guest_failure() {
        let root = tempdir().unwrap();
        let message = publish_runtime(
            root.path(),
            "session-1",
            "org.example.Writer",
            4,
            AppVmRuntimeState::Failed,
            Some("application process exited"),
        );
        assert!(matches!(
            check_runtime_evidence(
                Some(root.path()),
                &runtime_request("session-1", "org.example.Writer"),
                "app-vm-1",
                message.ts_unix_ms,
            ),
            AppVmRuntimeAdmission::Terminal(_)
        ));
    }

    #[test]
    fn unavailable_runtime_evidence_cannot_admit_resume() {
        let root = tempdir().unwrap();
        let message = publish_runtime(
            root.path(),
            "session-1",
            "org.example.Writer",
            4,
            AppVmRuntimeState::Unavailable,
            Some("guest transport unavailable"),
        );
        let admission = check_runtime_evidence(
            Some(root.path()),
            &runtime_request("session-1", "org.example.Writer"),
            "app-vm-1",
            message.ts_unix_ms,
        );

        assert_eq!(
            admission,
            AppVmRuntimeAdmission::Observed {
                state: AppVmRuntimeState::Unavailable,
                generation: 4,
            }
        );
        assert!(
            !admission.is_usable(),
            "an explicit guest-unavailable observation must not become launch readiness"
        );
    }
}
