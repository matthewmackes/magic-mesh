//! Workloads U6 — the `image-build` verb: build / list / promote the per-delivery-
//! type **golden images** the workloads run on.
//!
//! A golden image is a bootc image-mode disk built by `bootc-image-builder` (which
//! drives osbuild under the hood) and landed in the mesh's **Syncthing-replicated
//! image store** — the exact same `<workgroup>/images/<name>/<version>/` lane the
//! [`crate::image_catalog`] / [`crate::image_build`] pipeline already uses (W53/W55),
//! so a built base replicates to every peer with no egress (the airgap distribution
//! lane). This unit does NOT regress that lane: it records the same
//! [`ImageManifest`](crate::image_catalog::ImageManifest) through
//! [`record_manifest`](crate::image_catalog::record_manifest), and ADDS the SHA256
//! content hash the [`ImageRow`] contract carries as a sidecar in the same versioned
//! dir (so it replicates alongside the image) plus a `promote`-time re-verification
//! that refuses to promote a replicated image whose bytes no longer match.
//!
//! Three sub-actions ride the one `image-build` verb (`action` in the body):
//! - `list`   — the golden-image roster (a READ; no armed token needed).
//! - `build`  — shell the disk builder, hash + record the artifact (armed).
//! - `promote`— re-verify the SHA256 and mark a version the active base (armed).
//!
//! Honest by construction (§7): a missing tool, a failed build, or a hash mismatch
//! is a truthful gate/error — never a fabricated success and never an invented row.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use mackes_mesh_types::cloud::{CloudReply, DeliveryType, ImageRow};

use crate::image_catalog::{images_dir, load_manifests, record_manifest, ImageKind, ImageManifest};

use super::super::{gate, CloudWorker};

/// The disk builder binary — bootc image-mode → osbuild bridge. Produces the golden
/// qcow2 from a bootc container image (the same tool `packaging/bootc/build-image.sh`
/// shells for its disk lane).
const IMAGE_BUILDER_BIN: &str = "bootc-image-builder";

/// The default bootc container image the golden disk is built from (overridable per
/// request via the `image` field). Matches the local bootc build tag.
const DEFAULT_BOOTC_IMAGE: &str = "localhost/magic-mesh-bootc:latest";

/// The SHA256 sidecar written next to `manifest.toml` in each versioned dir — the
/// verified content hash the Syncthing lane checks + the [`ImageRow`] surfaces.
const SHA_SIDECAR: &str = "image.sha256";

/// The promotion marker at `<images>/<name>/PROMOTED` naming the active-base version.
const PROMOTED_MARKER: &str = "PROMOTED";

/// The parsed `image-build` request body (the verb-specific fields off the wire).
#[derive(Debug, Clone, Default, Deserialize)]
struct ImageBuildBody {
    /// The placement node (the armed-token binding + the drain's placement key).
    #[serde(default)]
    node: String,
    /// `build` (default) | `list` | `promote`.
    #[serde(default)]
    action: Option<String>,
    /// The image name; defaults to `<delivery_type>-golden` when a delivery type is
    /// given.
    #[serde(default)]
    name: Option<String>,
    /// The image version (defaults to `latest` for a build).
    #[serde(default)]
    version: Option<String>,
    /// The delivery type this golden image is for (stamped into the manifest profile
    /// + the default name).
    #[serde(default)]
    delivery_type: Option<DeliveryType>,
    /// Override the bootc container image ref the disk is built from.
    #[serde(default)]
    image: Option<String>,
    /// The armed-token capability authorizing a live build/promote.
    #[serde(default)]
    armed_token: Option<String>,
}

impl ImageBuildBody {
    /// The resolved image name: an explicit `name`, else `<delivery_type>-golden`.
    fn resolved_name(&self) -> Option<String> {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| self.delivery_type.map(|d| format!("{}-golden", d.as_str())))
    }

    /// The resolved version, defaulting to `latest`.
    fn resolved_version(&self) -> String {
        self.version
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("latest")
            .to_string()
    }
}

/// Route an `image-build` request to its sub-action handler.
pub(crate) fn handle(w: &CloudWorker, verb_name: &str, raw: &str) -> CloudReply {
    let body: ImageBuildBody = serde_json::from_str(raw.trim()).unwrap_or_default();
    match body
        .action
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("build")
    {
        "list" => list(w, verb_name),
        "build" => build(w, verb_name, &body),
        "promote" => promote(w, verb_name, &body),
        other => reject(
            verb_name,
            format!("unknown image-build action `{other}` (expected build|list|promote)"),
        ),
    }
}

/// `list` — the golden-image roster read from the Syncthing-replicated image store
/// (one row per image name, showing the promoted version when set, else the newest).
fn list(w: &CloudWorker, verb_name: &str) -> CloudReply {
    CloudReply {
        ok: true,
        verb: verb_name.to_string(),
        images: Some(load_rows(&w.state_root)),
        ..Default::default()
    }
}

/// `build` — shell the disk builder, verify + hash the artifact, and record it into
/// the Syncthing image store (manifest + SHA256 sidecar). Armed-gated.
fn build(w: &CloudWorker, verb_name: &str, body: &ImageBuildBody) -> CloudReply {
    // A container workload has no golden VM disk — it ships via container-deploy (U7).
    if matches!(body.delivery_type, Some(DeliveryType::ServiceContainer)) {
        return gated(
            verb_name,
            "service_container workloads are shipped via container-deploy (U7), not image-build",
        );
    }
    let Some(name) = body.resolved_name() else {
        return reject(
            verb_name,
            "image-build `build` requires a `name` (or a `delivery_type` to derive one)"
                .to_string(),
        );
    };
    let version = body.resolved_version();

    // The armed-token gate — a build without a valid capability stages nothing.
    let verdict = verify(w, verb_name, &body.node, body.armed_token.as_deref());
    if !verdict.is_valid() {
        return gated(
            verb_name,
            format!(
                "live image build is gated ({}) — nothing built",
                verdict.reason()
            ),
        );
    }

    let out_dir = images_dir(&w.state_root).join(&name).join(&version);
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        return error(
            verb_name,
            format!("prepare image dir {}: {e}", out_dir.display()),
        );
    }

    // The bootc image-mode → osbuild disk build. `bootc-image-builder` writes the
    // qcow2 under `--output`; a spawn failure means the tool is absent (honest gate).
    let out_str = out_dir.to_string_lossy().into_owned();
    let image_ref = body
        .image
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_BOOTC_IMAGE);
    let args = [
        "--type", "qcow2", "--rootfs", "xfs", "--output", &out_str, image_ref,
    ];
    match w.runner.run_tool(IMAGE_BUILDER_BIN, &args) {
        Err(spawn) => {
            return gated(
                verb_name,
                format!("golden-image tool `{IMAGE_BUILDER_BIN}` unavailable: {spawn}"),
            );
        }
        Ok(run) if !run.ok => {
            return error_with_log(
                verb_name,
                format!("`{IMAGE_BUILDER_BIN}` failed to build `{name}@{version}`"),
                pick_log(&run.stdout, &run.stderr),
            );
        }
        Ok(_) => {}
    }

    // Verify the artifact actually landed (a tool that lies about success still fails
    // here) and hash it — the SHA256 the Syncthing lane checks.
    let Some(artifact) = find_artifact(&out_dir) else {
        return error(
            verb_name,
            format!(
                "`{IMAGE_BUILDER_BIN}` reported success but produced no image artifact under {}",
                out_dir.display()
            ),
        );
    };
    let (sha256, size) = match hash_file(&artifact) {
        Ok(v) => v,
        Err(e) => return error(verb_name, format!("hash golden image: {e}")),
    };

    // Record into the SAME Syncthing-replicated store the existing image lane uses,
    // then write the SHA256 sidecar alongside it so it replicates with the image.
    let manifest = ImageManifest {
        name: name.clone(),
        kind: ImageKind::Vm.as_str().to_string(),
        version: version.clone(),
        built_at_ms: Some(now_ms_u64()),
        size_bytes: Some(size),
        profile: body.delivery_type.map(|d| d.as_str().to_string()),
    };
    if let Err(e) = record_manifest(&manifest, &w.state_root) {
        return error(verb_name, format!("record image manifest: {e}"));
    }
    if let Err(e) = std::fs::write(out_dir.join(SHA_SIDECAR), &sha256) {
        return error(verb_name, format!("write SHA256 sidecar: {e}"));
    }

    let promoted = read_promoted(&w.state_root, &name).as_deref() == Some(version.as_str());
    CloudReply {
        ok: true,
        verb: verb_name.to_string(),
        images: Some(vec![ImageRow {
            name,
            sha256,
            promoted,
        }]),
        ..Default::default()
    }
}

/// `promote` — re-verify a version's SHA256 against its recorded sidecar, then mark
/// it the active base. Armed-gated; a mismatch (a corrupted/tampered replicated
/// image) refuses the promotion.
fn promote(w: &CloudWorker, verb_name: &str, body: &ImageBuildBody) -> CloudReply {
    let Some(name) = body.resolved_name() else {
        return reject(
            verb_name,
            "image-build `promote` requires a `name`".to_string(),
        );
    };
    let Some(version) = body
        .version
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return reject(
            verb_name,
            "image-build `promote` requires a `version`".to_string(),
        );
    };

    let verdict = verify(w, verb_name, &body.node, body.armed_token.as_deref());
    if !verdict.is_valid() {
        return gated(
            verb_name,
            format!(
                "image promotion is gated ({}) — nothing promoted",
                verdict.reason()
            ),
        );
    }

    let version_dir = images_dir(&w.state_root).join(&name).join(version);
    if !version_dir.join("manifest.toml").is_file() {
        return reject(
            verb_name,
            format!("no such image version to promote: {name}@{version}"),
        );
    }
    let Some(artifact) = find_artifact(&version_dir) else {
        return error(
            verb_name,
            format!("image {name}@{version} has no artifact to verify"),
        );
    };
    let (actual, _) = match hash_file(&artifact) {
        Ok(v) => v,
        Err(e) => return error(verb_name, format!("verify golden image: {e}")),
    };
    // The SHA256 verification: the recorded sidecar must match the artifact's bytes.
    match read_sha(&w.state_root, &name, version) {
        Some(recorded) if recorded.trim() != actual => {
            return error(
                verb_name,
                format!(
                    "refusing to promote {name}@{version}: SHA256 mismatch (recorded {}…, actual {}…) — replicated image failed verification",
                    &recorded.trim().chars().take(12).collect::<String>(),
                    &actual.chars().take(12).collect::<String>(),
                ),
            );
        }
        // No sidecar (a legacy build) — record the verified hash now as the baseline.
        None => {
            if let Err(e) = std::fs::write(version_dir.join(SHA_SIDECAR), &actual) {
                return error(verb_name, format!("record SHA256 sidecar: {e}"));
            }
        }
        _ => {}
    }

    let marker = images_dir(&w.state_root).join(&name).join(PROMOTED_MARKER);
    if let Err(e) = std::fs::write(&marker, version) {
        return error(verb_name, format!("set promotion marker: {e}"));
    }
    CloudReply {
        ok: true,
        verb: verb_name.to_string(),
        images: Some(vec![ImageRow {
            name,
            sha256: actual,
            promoted: true,
        }]),
        ..Default::default()
    }
}

// ─────────────────────────── the image store (Syncthing lane) ───────────────────────────

/// Fold the Syncthing-replicated manifest store into one [`ImageRow`] per image name
/// — the promoted version when set (and present), else the newest.
fn load_rows(root: &Path) -> Vec<ImageRow> {
    let manifests = load_manifests(root); // newest-first
    let mut seen = std::collections::BTreeSet::new();
    let mut rows = Vec::new();
    for m in &manifests {
        if !seen.insert(m.name.clone()) {
            continue;
        }
        let promoted_ver = read_promoted(root, &m.name);
        // Prefer the promoted version's row when the marker names an existing build.
        let chosen = promoted_ver
            .as_deref()
            .and_then(|pv| {
                manifests
                    .iter()
                    .find(|x| x.name == m.name && x.version == pv)
            })
            .unwrap_or(m);
        rows.push(ImageRow {
            name: chosen.name.clone(),
            sha256: read_sha(root, &chosen.name, &chosen.version).unwrap_or_default(),
            promoted: promoted_ver.as_deref() == Some(chosen.version.as_str()),
        });
    }
    rows
}

/// Read the promotion marker for `name` (the active-base version), if set.
fn read_promoted(root: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(images_dir(root).join(name).join(PROMOTED_MARKER))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Read the recorded SHA256 sidecar for `name@version`, if present.
fn read_sha(root: &Path, name: &str, version: &str) -> Option<String> {
    std::fs::read_to_string(images_dir(root).join(name).join(version).join(SHA_SIDECAR))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The produced disk artifact under `dir`: the largest file (recursively) with a
/// recognized image extension. `None` when the builder produced nothing (honest —
/// never a fabricated artifact path).
fn find_artifact(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let is_image = p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| matches!(x, "qcow2" | "raw" | "iso" | "img" | "oci" | "tar"));
            if !is_image {
                continue;
            }
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            if best.as_ref().is_none_or(|(b, _)| size > *b) {
                best = Some((size, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Stream a file through SHA256, returning `(hex_digest, byte_len)`.
fn hash_file(path: &Path) -> Result<(String, u64), String> {
    use std::io::Read as _;
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hex(&hasher.finalize()), total))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ─────────────────────────── small shared helpers ───────────────────────────

/// The armed-token verdict for `(verb, node)` under this worker's signer.
fn verify(w: &CloudWorker, verb_name: &str, node: &str, token: Option<&str>) -> gate::TokenVerdict {
    gate::verify_token(
        token,
        verb_name,
        node.trim(),
        super::super::now_ms(),
        w.signer.as_ref(),
    )
}

fn now_ms_u64() -> u64 {
    u64::try_from(super::super::now_ms()).unwrap_or(0)
}

/// An honest gate reply (the backend/tool isn't in a state to serve this verb).
fn gated(verb_name: &str, why: impl Into<String>) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        gated: Some(why.into()),
        ..Default::default()
    }
}

/// An honest rejection (a malformed / underspecified request).
fn reject(verb_name: &str, why: String) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        error: Some(why),
        ..Default::default()
    }
}

/// An honest backend failure.
fn error(verb_name: &str, why: String) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        error: Some(why),
        ..Default::default()
    }
}

/// An honest backend failure carrying the tool's raw output behind the shell's
/// expandable raw-log.
fn error_with_log(verb_name: &str, why: String, log: String) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        error: Some(why),
        raw_log: Some(log).filter(|s| !s.is_empty()),
        ..Default::default()
    }
}

fn pick_log(stdout: &str, stderr: &str) -> String {
    if stderr.trim().is_empty() {
        stdout.trim().to_string()
    } else {
        stderr.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::super::gate::{ArmedToken, HmacTokenSigner};
    use super::super::super::runner::fake::FakeRunner;
    use super::super::super::{now_ms, CloudWorker};
    use super::*;

    const KEY: &[u8] = b"test-mesh-arming-key";

    fn signer() -> HmacTokenSigner {
        HmacTokenSigner::new(KEY.to_vec())
    }

    /// A worker rooted at `root` (its Syncthing image store) with the test arming key.
    fn armed_worker(root: &Path, runner: Arc<FakeRunner>) -> CloudWorker {
        CloudWorker::new("me".into(), "peer:me".into(), root.to_path_buf())
            .with_runner(runner)
            .with_signer(Arc::new(signer()))
            .with_bus_root(None)
    }

    /// A worker with no arming key — every build/promote stages honestly.
    fn staged_worker(root: &Path, runner: Arc<FakeRunner>) -> CloudWorker {
        CloudWorker::new("me".into(), "peer:me".into(), root.to_path_buf())
            .with_runner(runner)
            .with_bus_root(None)
    }

    fn token(verb: &str) -> String {
        ArmedToken::mint(
            &signer(),
            "nonce-12345678",
            now_ms() + 3_600_000,
            verb,
            "me",
        )
        .encode()
    }

    #[test]
    fn build_shells_the_builder_hashes_and_records_into_the_syncthing_store() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(tmp.path(), runner.clone());
        let raw = format!(
            r#"{{"node":"me","action":"build","delivery_type":"desktop_vm","version":"1.0","armed_token":"{}"}}"#,
            token("image-build")
        );
        let reply = w.handle("image-build", &raw);
        assert!(reply.ok, "gated:{:?} err:{:?}", reply.gated, reply.error);
        let rows = reply.images.expect("images");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "desktop_vm-golden");
        assert_eq!(rows[0].sha256.len(), 64, "a real SHA256 hex was recorded");
        assert!(!rows[0].promoted, "a fresh build is not auto-promoted");
        // The builder ran with the golden-disk pipeline.
        let calls = runner.tool_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "bootc-image-builder");
        assert!(calls[0].1.iter().any(|a| a == "qcow2"));
        // The manifest landed in the SAME image_catalog store the existing lane uses.
        let manifests = load_manifests(tmp.path());
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].version, "1.0");
        assert_eq!(manifests[0].kind, "vm");
        // The SHA256 sidecar replicates alongside the image.
        assert!(read_sha(tmp.path(), "desktop_vm-golden", "1.0").is_some());
    }

    #[test]
    fn build_without_a_token_stages_and_builds_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = staged_worker(tmp.path(), runner.clone());
        let reply = w.handle(
            "image-build",
            r#"{"node":"me","action":"build","name":"gold","version":"1"}"#,
        );
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("gated"));
        assert!(
            runner.tool_calls.lock().unwrap().is_empty(),
            "a staged build never shells the builder"
        );
        assert!(load_manifests(tmp.path()).is_empty(), "nothing recorded");
    }

    #[test]
    fn build_with_the_tool_absent_is_honestly_gated_not_faked() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner {
            tool_absent: true,
            ..Default::default()
        });
        let w = armed_worker(tmp.path(), runner);
        let raw = format!(
            r#"{{"node":"me","action":"build","name":"gold","version":"1","armed_token":"{}"}}"#,
            token("image-build")
        );
        let reply = w.handle("image-build", &raw);
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("unavailable"));
        assert!(load_manifests(tmp.path()).is_empty());
    }

    #[test]
    fn a_build_tool_failure_is_an_honest_error_and_records_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner {
            tool_fail: true,
            ..Default::default()
        });
        let w = armed_worker(tmp.path(), runner);
        let raw = format!(
            r#"{{"node":"me","action":"build","name":"gold","version":"1","armed_token":"{}"}}"#,
            token("image-build")
        );
        let reply = w.handle("image-build", &raw);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("failed"));
        assert!(load_manifests(tmp.path()).is_empty());
    }

    #[test]
    fn a_successful_run_with_no_artifact_is_an_honest_error() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner {
            tool_no_artifact: true,
            ..Default::default()
        });
        let w = armed_worker(tmp.path(), runner);
        let raw = format!(
            r#"{{"node":"me","action":"build","name":"gold","version":"1","armed_token":"{}"}}"#,
            token("image-build")
        );
        let reply = w.handle("image-build", &raw);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("no image artifact"));
    }

    #[test]
    fn service_container_build_is_routed_to_container_deploy() {
        let tmp = tempfile::tempdir().unwrap();
        let w = armed_worker(tmp.path(), Arc::new(FakeRunner::default()));
        let raw = format!(
            r#"{{"node":"me","action":"build","delivery_type":"service_container","armed_token":"{}"}}"#,
            token("image-build")
        );
        let reply = w.handle("image-build", &raw);
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("container-deploy"));
    }

    #[test]
    fn list_reads_the_roster_and_prefers_the_promoted_version() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(tmp.path(), runner);
        // Build two versions of the same golden image.
        for v in ["1.0", "2.0"] {
            let raw = format!(
                r#"{{"node":"me","action":"build","name":"gold","version":"{v}","armed_token":"{}"}}"#,
                token("image-build")
            );
            assert!(w.handle("image-build", &raw).ok);
        }
        // list (a read) needs no token.
        let rows = w
            .handle("image-build", r#"{"action":"list"}"#)
            .images
            .expect("roster");
        assert_eq!(rows.len(), 1, "one row per image name");
        assert!(!rows[0].promoted, "nothing promoted yet");

        // Promote 1.0 → list now reflects that version + the promoted flag.
        let raw = format!(
            r#"{{"node":"me","action":"promote","name":"gold","version":"1.0","armed_token":"{}"}}"#,
            token("image-build")
        );
        let pr = w.handle("image-build", &raw);
        assert!(pr.ok, "gated:{:?} err:{:?}", pr.gated, pr.error);
        let rows = w
            .handle("image-build", r#"{"action":"list"}"#)
            .images
            .unwrap();
        assert!(rows[0].promoted, "the promoted version is flagged");
    }

    #[test]
    fn promote_refuses_on_a_sha256_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let w = armed_worker(tmp.path(), Arc::new(FakeRunner::default()));
        let raw = format!(
            r#"{{"node":"me","action":"build","name":"gold","version":"1.0","armed_token":"{}"}}"#,
            token("image-build")
        );
        assert!(w.handle("image-build", &raw).ok);
        // Corrupt the replicated artifact after the recorded hash.
        let artifact =
            find_artifact(&images_dir(tmp.path()).join("gold").join("1.0")).expect("artifact");
        std::fs::write(&artifact, b"tampered-bytes").unwrap();
        let raw = format!(
            r#"{{"node":"me","action":"promote","name":"gold","version":"1.0","armed_token":"{}"}}"#,
            token("image-build")
        );
        let reply = w.handle("image-build", &raw);
        assert!(!reply.ok, "a mismatched image must not promote");
        assert!(reply.error.unwrap().contains("mismatch"));
    }

    #[test]
    fn an_unknown_action_is_an_honest_rejection() {
        let tmp = tempfile::tempdir().unwrap();
        let w = armed_worker(tmp.path(), Arc::new(FakeRunner::default()));
        let reply = w.handle("image-build", r#"{"action":"frobnicate"}"#);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("unknown image-build action"));
    }
}
