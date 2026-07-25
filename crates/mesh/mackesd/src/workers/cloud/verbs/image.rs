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

use crate::image_catalog::{ImageKind, ImageManifest, images_dir, load_manifests, record_manifest};

use super::super::{CloudWorker, path_key};

/// The disk builder binary — bootc image-mode → osbuild bridge. Produces the golden
/// qcow2 from a bootc container image (the same tool `packaging/bootc/build-image.sh`
/// shells for its disk lane).
const IMAGE_BUILDER_BIN: &str = "bootc-image-builder";

/// The default bootc container image the golden disk is built from (overridable per
/// request via the `image` field). Matches the local bootc build tag.
const DEFAULT_BOOTC_IMAGE: &str = "localhost/magic-mesh-bootc:latest";

/// Bound an untrusted image reference before it becomes a builder argv value.
///
/// This is deliberately a lexical OCI-reference check, not a registry lookup:
/// the mesh may use private, air-gapped, or local registries. It accepts the
/// normal `name[:tag][@algorithm:hex-digest]` forms while refusing shell/control
/// characters, path-shaped names, and option-shaped values before auth/replay.
const MAX_IMAGE_REFERENCE_BYTES: usize = 4096;

/// Bound the complete verb body before serde can walk or allocate from it.
const MAX_IMAGE_REQUEST_BYTES: usize = 64 * 1024;

/// Bound text that is copied into the provider-neutral reply envelope.
const MAX_REPLY_TEXT_BYTES: usize = 4096;

/// Keep a failed builder from turning its diagnostic stream into an unbounded
/// raw-log bus payload.
const MAX_RAW_LOG_BYTES: usize = 64 * 1024;

/// Marker and sidecar files are replicated state, not trusted local constants.
const MAX_STORE_TEXT_BYTES: usize = 4096;

/// Keep a hostile image store from turning one list reply into an unbounded
/// roster. The order remains the catalog's existing newest-first order.
const MAX_IMAGE_ROWS: usize = 256;

const TEXT_TRUNCATION_MARKER: &str = "\n[output truncated]";

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

/// Validate the caller-controlled positional image passed to
/// `bootc-image-builder`.
fn validate_image_reference(reference: &str) -> Result<&str, String> {
    if reference.is_empty() {
        return Err("image reference must not be empty".to_string());
    }
    if reference.len() > MAX_IMAGE_REFERENCE_BYTES {
        return Err(format!(
            "image reference exceeds {MAX_IMAGE_REFERENCE_BYTES} bytes"
        ));
    }
    if reference.starts_with('-') {
        return Err("image reference must not begin with `-`".to_string());
    }
    if !reference.is_ascii()
        || reference
            .chars()
            .any(|c| c.is_ascii_control() || c.is_ascii_whitespace())
    {
        return Err("image reference contains whitespace or control characters".to_string());
    }

    let (name, digest) = match reference.split_once('@') {
        Some((name, digest)) => (name, Some(digest)),
        None => (reference, None),
    };
    if name.is_empty() || name.contains('@') {
        return Err("image reference has an invalid name".to_string());
    }

    if let Some(digest) = digest {
        let Some((algorithm, encoded)) = digest.split_once(':') else {
            return Err("image reference has an invalid digest".to_string());
        };
        if algorithm.is_empty()
            || !algorithm
                .chars()
                .enumerate()
                .all(|(i, c)| c.is_ascii_lowercase() || (i > 0 && c.is_ascii_digit()))
            || encoded.len() < 32
            || !encoded.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err("image reference has an invalid digest".to_string());
        }
    }

    // A colon after the final slash is the optional tag. A colon before the
    // slash belongs to a registry port and is checked below.
    let tag_start = name
        .rfind(':')
        .filter(|colon| name.rfind('/').is_none_or(|slash| *colon > slash));
    let (name, tag) = match tag_start {
        Some(colon) => (&name[..colon], Some(&name[colon + 1..])),
        None => (name, None),
    };
    if let Some(tag) = tag {
        if tag.is_empty()
            || tag.len() > 128
            || !tag
                .chars()
                .enumerate()
                .all(|(i, c)| c.is_ascii_alphanumeric() || (i > 0 && matches!(c, '_' | '.' | '-')))
        {
            return Err("image reference has an invalid tag".to_string());
        }
    }

    let components: Vec<&str> = name.split('/').collect();
    if components.iter().any(|component| component.is_empty()) {
        return Err("image reference has an empty path component".to_string());
    }
    if components
        .iter()
        .skip(1)
        .any(|component| !valid_image_name_component(component))
    {
        return Err("image reference has an invalid repository path".to_string());
    }
    if !valid_registry_component(components[0]) {
        return Err("image reference has an invalid registry or repository name".to_string());
    }

    Ok(reference)
}

fn valid_image_name_component(component: &str) -> bool {
    let bytes = component.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_registry_component(component: &str) -> bool {
    if let Some(close) = component.strip_prefix('[').and_then(|rest| rest.find(']')) {
        let close = close + 1;
        let host = &component[1..close];
        let port = &component[close + 1..];
        return !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
            && (port.is_empty() || (port.starts_with(':') && valid_registry_port(&port[1..])));
    }

    let Some(colon) = component.rfind(':') else {
        return valid_image_name_component(component);
    };
    component[..colon].contains(':') == false
        && valid_image_name_component(&component[..colon])
        && valid_registry_port(&component[colon + 1..])
}

fn valid_registry_port(port: &str) -> bool {
    !port.is_empty()
        && port.len() <= 5
        && port.chars().all(|c| c.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0)
}

/// Route an `image-build` request to its sub-action handler.
pub(crate) fn handle(w: &CloudWorker, verb_name: &str, raw: &str) -> CloudReply {
    if raw.len() > MAX_IMAGE_REQUEST_BYTES {
        return reject(
            verb_name,
            format!("image-build request exceeds {MAX_IMAGE_REQUEST_BYTES} bytes"),
        );
    }
    let body: ImageBuildBody = match serde_json::from_str(raw.trim()) {
        Ok(body) => body,
        Err(error) => {
            return reject(
                verb_name,
                format!(
                    "invalid image-build request: {}",
                    bounded_text(&error.to_string(), MAX_REPLY_TEXT_BYTES)
                ),
            );
        }
    };
    match body
        .action
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("build")
    {
        "list" => list(w, verb_name),
        "build" => build(w, verb_name, &body, raw),
        "promote" => promote(w, verb_name, &body, raw),
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
        verb: reply_verb(verb_name),
        images: Some(load_rows(&w.state_root)),
        ..Default::default()
    }
}

/// `build` — shell the disk builder, verify + hash the artifact, and record it into
/// the Syncthing image store (manifest + SHA256 sidecar). Armed-gated.
fn build(w: &CloudWorker, verb_name: &str, body: &ImageBuildBody, raw: &str) -> CloudReply {
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
    if let Err(e) = path_key::segment("image name", &name) {
        return reject(verb_name, e);
    }
    if let Err(e) = path_key::segment("image version", &version) {
        return reject(verb_name, e);
    }
    let image_ref = body
        .image
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_BOOTC_IMAGE);
    if let Err(e) = validate_image_reference(image_ref) {
        return reject(verb_name, e);
    }

    // The armed-token gate — a build without a valid capability stages nothing.
    let target = format!("build:{name}@{version}");
    let verdict = w.consume_armed_token(
        body.armed_token.as_deref(),
        verb_name,
        body.node.trim(),
        &target,
        raw,
    );
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
    // Keep the caller-controlled image reference positional. Without `--`, a
    // reference beginning with `-` could be reinterpreted by the builder as a
    // second option (for example a second `--output`), despite using literal
    // argv rather than a shell.
    let args = [
        "--type", "qcow2", "--rootfs", "xfs", "--output", &out_str, "--", image_ref,
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
        verb: reply_verb(verb_name),
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
fn promote(w: &CloudWorker, verb_name: &str, body: &ImageBuildBody, raw: &str) -> CloudReply {
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
    if let Err(e) = path_key::segment("image name", &name) {
        return reject(verb_name, e);
    }
    if let Err(e) = path_key::segment("image version", version) {
        return reject(verb_name, e);
    }

    let target = format!("promote:{name}@{version}");
    let verdict = w.consume_armed_token(
        body.armed_token.as_deref(),
        verb_name,
        body.node.trim(),
        &target,
        raw,
    );
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
        Ok(Some(recorded)) if recorded.trim() != actual => {
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
        Ok(None) => {
            if let Err(e) = std::fs::write(version_dir.join(SHA_SIDECAR), &actual) {
                return error(verb_name, format!("record SHA256 sidecar: {e}"));
            }
        }
        Ok(Some(_)) => {}
        Err(e) => return error(verb_name, format!("read SHA256 sidecar: {e}")),
    }

    let marker = images_dir(&w.state_root).join(&name).join(PROMOTED_MARKER);
    if let Err(e) = std::fs::write(&marker, version) {
        return error(verb_name, format!("set promotion marker: {e}"));
    }
    CloudReply {
        ok: true,
        verb: reply_verb(verb_name),
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
        if rows.len() >= MAX_IMAGE_ROWS {
            break;
        }
        if path_key::segment("image name", &m.name).is_err() {
            continue;
        }
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
            sha256: read_sha(root, &chosen.name, &chosen.version)
                .ok()
                .flatten()
                .unwrap_or_default(),
            promoted: promoted_ver.as_deref() == Some(chosen.version.as_str()),
        });
    }
    rows
}

/// Read the promotion marker for `name` (the active-base version), if set.
fn read_promoted(root: &Path, name: &str) -> Option<String> {
    path_key::segment("image name", name).ok()?;
    let value = read_bounded_text(
        &images_dir(root).join(name).join(PROMOTED_MARKER),
        "promotion marker",
    )
    .ok()??;
    let value = value.trim();
    path_key::segment("promoted version", value).ok()?;
    Some(value.to_string())
}

/// Read the recorded SHA256 sidecar for `name@version`, if present.
fn read_sha(root: &Path, name: &str, version: &str) -> Result<Option<String>, String> {
    path_key::segment("image name", name)?;
    path_key::segment("image version", version)?;
    let Some(value) = read_bounded_text(
        &images_dir(root).join(name).join(version).join(SHA_SIDECAR),
        "SHA256 sidecar",
    )?
    else {
        return Ok(None);
    };
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("SHA256 sidecar is not a 64-character hexadecimal digest".to_string());
    }
    Ok(Some(value.to_string()))
}

/// Read replicated text with a hard byte ceiling. The metadata check avoids a
/// large allocation in the usual case; `take(cap + 1)` also closes the race where
/// a file grows after metadata was read. Missing files retain the old optional
/// semantics, while malformed/oversized present files fail closed.
fn read_bounded_text(path: &Path, label: &str) -> Result<Option<String>, String> {
    use std::io::Read as _;

    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {label}: {error}")),
    };
    let declared_len = file
        .metadata()
        .map_err(|error| format!("stat {label}: {error}"))?
        .len();
    if declared_len > MAX_STORE_TEXT_BYTES as u64 {
        return Err(format!("{label} exceeds {MAX_STORE_TEXT_BYTES} bytes"));
    }
    let mut bytes = Vec::with_capacity((declared_len as usize).saturating_add(1));
    file.take((MAX_STORE_TEXT_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read {label}: {error}"))?;
    if bytes.len() > MAX_STORE_TEXT_BYTES {
        return Err(format!("{label} exceeds {MAX_STORE_TEXT_BYTES} bytes"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| format!("{label} is not valid UTF-8"))
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

fn now_ms_u64() -> u64 {
    u64::try_from(super::super::now_ms()).unwrap_or(0)
}

/// An honest gate reply (the backend/tool isn't in a state to serve this verb).
fn gated(verb_name: &str, why: impl Into<String>) -> CloudReply {
    let why = why.into();
    CloudReply {
        ok: false,
        verb: reply_verb(verb_name),
        gated: Some(bounded_text(&why, MAX_REPLY_TEXT_BYTES)),
        ..Default::default()
    }
}

/// An honest rejection (a malformed / underspecified request).
fn reject(verb_name: &str, why: String) -> CloudReply {
    CloudReply {
        ok: false,
        verb: reply_verb(verb_name),
        error: Some(bounded_text(&why, MAX_REPLY_TEXT_BYTES)),
        ..Default::default()
    }
}

/// An honest backend failure.
fn error(verb_name: &str, why: String) -> CloudReply {
    CloudReply {
        ok: false,
        verb: reply_verb(verb_name),
        error: Some(bounded_text(&why, MAX_REPLY_TEXT_BYTES)),
        ..Default::default()
    }
}

/// An honest backend failure carrying the tool's raw output behind the shell's
/// expandable raw-log.
fn error_with_log(verb_name: &str, why: String, log: String) -> CloudReply {
    CloudReply {
        ok: false,
        verb: reply_verb(verb_name),
        error: Some(bounded_text(&why, MAX_REPLY_TEXT_BYTES)),
        raw_log: Some(bounded_text(&log, MAX_RAW_LOG_BYTES)).filter(|s| !s.is_empty()),
        ..Default::default()
    }
}

fn pick_log(stdout: &str, stderr: &str) -> String {
    let selected = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    bounded_text(selected, MAX_RAW_LOG_BYTES)
}

fn reply_verb(verb_name: &str) -> String {
    bounded_text(verb_name, MAX_REPLY_TEXT_BYTES)
}

/// Truncate on a UTF-8 boundary while retaining a visible diagnostic marker.
fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let keep = max_bytes.saturating_sub(TEXT_TRUNCATION_MARKER.len());
    let mut end = keep.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = String::with_capacity(end + TEXT_TRUNCATION_MARKER.len());
    bounded.push_str(&value[..end]);
    bounded.push_str(TEXT_TRUNCATION_MARKER);
    bounded
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::super::gate::{ArmedToken, HmacTokenSigner};
    use super::super::super::runner::fake::FakeRunner;
    use super::super::super::{CloudWorker, now_ms};
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

    fn token(body: &str, action: &str, name: &str, version: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);
        let nonce = format!(
            "nonce-image-{}-{}",
            std::process::id(),
            NEXT_NONCE.fetch_add(1, Ordering::Relaxed)
        );
        ArmedToken::mint(
            &signer(),
            &nonce,
            now_ms().saturating_add(super::super::super::MAX_AUTH_TTL_MS),
            "image-build",
            "me",
            &format!("{action}:{name}@{version}"),
            &mackes_mesh_types::cloud::cloud_request_digest(body).unwrap(),
        )
        .encode()
    }

    fn armed_request(mut body: serde_json::Value) -> String {
        let action = body["action"].as_str().unwrap_or("build").to_string();
        let name = body["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| {
                body["delivery_type"]
                    .as_str()
                    .map(|dtype| format!("{dtype}-golden"))
            })
            .unwrap_or_default();
        let version = body["version"]
            .as_str()
            .filter(|version| !version.is_empty())
            .unwrap_or("latest")
            .to_string();
        let raw = body.to_string();
        body["armed_token"] = serde_json::Value::String(token(&raw, &action, &name, &version));
        body.to_string()
    }

    #[test]
    fn build_shells_the_builder_hashes_and_records_into_the_syncthing_store() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(tmp.path(), runner.clone());
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me", "action": "build", "delivery_type": "desktop_vm", "version": "1.0"
        }));
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
        let separator = calls[0]
            .1
            .iter()
            .position(|a| a == "--")
            .expect("image reference is after the end-of-options separator");
        assert_eq!(
            calls[0].1.get(separator + 1).map(String::as_str),
            Some(DEFAULT_BOOTC_IMAGE)
        );
        // The manifest landed in the SAME image_catalog store the existing lane uses.
        let manifests = load_manifests(tmp.path());
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].version, "1.0");
        assert_eq!(manifests[0].kind, "vm");
        // The SHA256 sidecar replicates alongside the image.
        assert!(
            read_sha(tmp.path(), "desktop_vm-golden", "1.0")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn option_shaped_image_reference_is_rejected_before_auth_replay_or_runner() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(tmp.path(), runner.clone());
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me",
            "action": "build",
            "name": "gold",
            "version": "1",
            "image": "--output=/tmp/should-not-be-an-output"
        }));

        let first = w.handle("image-build", &raw);
        assert!(!first.ok);
        assert!(first.gated.is_none(), "invalid input must not reach auth");
        assert!(
            first
                .error
                .as_deref()
                .is_some_and(|error| error.contains("must not begin with `-`"))
        );

        // Replaying the exact request reaches the same pre-auth rejection: the
        // armed nonce was never claimed, and the runner was never dispatched.
        let second = w.handle("image-build", &raw);
        assert_eq!(second.error, first.error);
        assert!(second.gated.is_none());
        assert!(runner.tool_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn oversized_request_is_rejected_before_deserialization_auth_or_runner() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = staged_worker(tmp.path(), runner.clone());
        let raw = format!(
            r#"{{"schema_version":1,"node":"me","action":"build","name":"{}"}}"#,
            "x".repeat(MAX_IMAGE_REQUEST_BYTES)
        );

        // Call this verb seam directly: the shared cloud envelope has its own
        // equal-or-smaller RPC cap and would reject the body before the
        // image-specific boundary can be exercised.
        let reply = handle(&w, "image-build", &raw);
        assert!(!reply.ok);
        assert!(reply.gated.is_none(), "oversized input must fail closed");
        assert!(
            reply
                .error
                .as_deref()
                .is_some_and(|error| error.contains("request exceeds"))
        );
        assert!(
            reply
                .error
                .as_deref()
                .is_some_and(|error| error.len() <= MAX_REPLY_TEXT_BYTES)
        );
        assert!(runner.tool_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn replicated_marker_and_sidecar_caps_are_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(tmp.path(), runner);
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me", "action": "build", "name": "gold", "version": "1.0"
        }));
        assert!(w.handle("image-build", &raw).ok);

        let image_dir = images_dir(tmp.path()).join("gold");
        std::fs::write(
            image_dir.join(PROMOTED_MARKER),
            "2".repeat(MAX_STORE_TEXT_BYTES + 1),
        )
        .unwrap();
        std::fs::write(
            image_dir.join("1.0").join(SHA_SIDECAR),
            "a".repeat(MAX_STORE_TEXT_BYTES + 1),
        )
        .unwrap();

        let listed = w
            .handle(
                "image-build",
                r#"{"schema_version":1,"node":"me","action":"list"}"#,
            )
            .images
            .expect("roster");
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].promoted, "an oversized marker is not trusted");
        assert!(
            listed[0].sha256.is_empty(),
            "an oversized sidecar is not surfaced"
        );

        let promote_raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me", "action": "promote", "name": "gold", "version": "1.0"
        }));
        let reply = w.handle("image-build", &promote_raw);
        assert!(!reply.ok);
        assert!(
            reply
                .error
                .as_deref()
                .is_some_and(|error| error.contains("sidecar"))
        );
        assert!(
            reply
                .error
                .as_deref()
                .is_some_and(|error| error.len() <= MAX_REPLY_TEXT_BYTES)
        );
    }

    #[test]
    fn image_roster_and_diagnostics_are_bounded_before_reply() {
        let tmp = tempfile::tempdir().unwrap();
        for index in 0..=MAX_IMAGE_ROWS {
            record_manifest(
                &ImageManifest {
                    name: format!("gold-{index:03}"),
                    kind: ImageKind::Vm.as_str().to_string(),
                    version: "1".to_string(),
                    built_at_ms: Some(index as u64),
                    size_bytes: None,
                    profile: None,
                },
                tmp.path(),
            )
            .unwrap();
        }

        let reply = list(
            &armed_worker(tmp.path(), Arc::new(FakeRunner::default())),
            "image-build",
        );
        let rows = reply.images.expect("roster");
        assert_eq!(rows.len(), MAX_IMAGE_ROWS);
        assert!(rows.iter().all(|row| row.name.len() <= 255));

        let huge = "é".repeat(MAX_RAW_LOG_BYTES + 32);
        let huge_verb = "v".repeat(MAX_REPLY_TEXT_BYTES + 32);
        let diagnostic = error_with_log(
            &huge_verb,
            "e".repeat(MAX_REPLY_TEXT_BYTES + 32),
            pick_log(&huge, ""),
        );
        assert!(diagnostic.verb.len() <= MAX_REPLY_TEXT_BYTES);
        assert!(
            diagnostic
                .error
                .as_deref()
                .is_some_and(|error| error.len() <= MAX_REPLY_TEXT_BYTES)
        );
        assert!(
            diagnostic
                .raw_log
                .as_deref()
                .is_some_and(|log| log.len() <= MAX_RAW_LOG_BYTES)
        );
        assert!(
            diagnostic
                .raw_log
                .as_deref()
                .is_some_and(|log| log.is_char_boundary(log.len()))
        );
    }

    #[test]
    fn build_without_a_token_stages_and_builds_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = staged_worker(tmp.path(), runner.clone());
        let reply = w.handle(
            "image-build",
            r#"{"schema_version":1,"node":"me","action":"build","name":"gold","version":"1"}"#,
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
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me", "action": "build", "name": "gold", "version": "1"
        }));
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
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me", "action": "build", "name": "gold", "version": "1"
        }));
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
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me", "action": "build", "name": "gold", "version": "1"
        }));
        let reply = w.handle("image-build", &raw);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("no image artifact"));
    }

    #[test]
    fn service_container_build_is_routed_to_container_deploy() {
        let tmp = tempfile::tempdir().unwrap();
        let w = armed_worker(tmp.path(), Arc::new(FakeRunner::default()));
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me", "action": "build", "delivery_type": "service_container"
        }));
        let reply = w.handle("image-build", &raw);
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("container-deploy"));
    }

    #[test]
    fn build_and_promote_reject_path_keys_before_touching_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        let absolute = outside.to_string_lossy();
        let w = armed_worker(tmp.path(), Arc::new(FakeRunner::default()));

        for raw in [
            armed_request(serde_json::json!({
                "schema_version": 1,
                "node": "me", "action": "build", "name": absolute.as_ref(), "version": "1"
            })),
            armed_request(serde_json::json!({
                "schema_version": 1,
                "node": "me", "action": "build", "name": "gold", "version": "../escape"
            })),
            armed_request(serde_json::json!({
                "schema_version": 1,
                "node": "me", "action": "promote", "name": "gold", "version": "/tmp/escape"
            })),
        ] {
            let reply = w.handle("image-build", &raw);
            assert!(!reply.ok);
            assert!(reply.error.unwrap().contains("path-safe"));
        }
        assert!(!outside.exists());
    }

    #[test]
    fn list_reads_the_roster_and_prefers_the_promoted_version() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(tmp.path(), runner);
        // Build two versions of the same golden image.
        for v in ["1.0", "2.0"] {
            let raw = armed_request(serde_json::json!({
                "schema_version": 1,
                "node": "me", "action": "build", "name": "gold", "version": v
            }));
            assert!(w.handle("image-build", &raw).ok);
        }
        // list needs no token, but it is still scoped to the selected node so
        // only that node's replicated image store answers.
        let rows = w
            .handle(
                "image-build",
                r#"{"schema_version":1,"node":"me","action":"list"}"#,
            )
            .images
            .expect("roster");
        assert_eq!(rows.len(), 1, "one row per image name");
        assert!(!rows[0].promoted, "nothing promoted yet");

        // Promote 1.0 → list now reflects that version + the promoted flag.
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me", "action": "promote", "name": "gold", "version": "1.0"
        }));
        let pr = w.handle("image-build", &raw);
        assert!(pr.ok, "gated:{:?} err:{:?}", pr.gated, pr.error);
        let rows = w
            .handle(
                "image-build",
                r#"{"schema_version":1,"node":"me","action":"list"}"#,
            )
            .images
            .unwrap();
        assert!(rows[0].promoted, "the promoted version is flagged");
    }

    #[test]
    fn promote_refuses_on_a_sha256_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let w = armed_worker(tmp.path(), Arc::new(FakeRunner::default()));
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me", "action": "build", "name": "gold", "version": "1.0"
        }));
        assert!(w.handle("image-build", &raw).ok);
        // Corrupt the replicated artifact after the recorded hash.
        let artifact =
            find_artifact(&images_dir(tmp.path()).join("gold").join("1.0")).expect("artifact");
        std::fs::write(&artifact, b"tampered-bytes").unwrap();
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me", "action": "promote", "name": "gold", "version": "1.0"
        }));
        let reply = w.handle("image-build", &raw);
        assert!(!reply.ok, "a mismatched image must not promote");
        assert!(reply.error.unwrap().contains("mismatch"));
    }

    #[test]
    fn an_unknown_action_is_an_honest_rejection() {
        let tmp = tempfile::tempdir().unwrap();
        let w = armed_worker(tmp.path(), Arc::new(FakeRunner::default()));
        let reply = w.handle(
            "image-build",
            r#"{"schema_version":1,"node":"me","action":"frobnicate"}"#,
        );
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("unknown image-build action"));
    }
}
