#!/usr/bin/env python3
"""Validate one complete, artifact-bound Browser VM live acceptance record.

The Browser VM has deliberately separate evidence producers for framebuffer
presentation/input/reconnect, guest runtime state, Chromium media decode, and
long-run performance.  This helper is the promotion boundary that binds those
records to one source commit and image digest and additionally requires a
real, sample-backed audio record plus a separate operator confirmation that
the bound after-reconnect playback was heard at the physical seat.  Digital
PCM never substitutes for that human observation.  The verifier never probes
a target and never turns a reachable port, a guest-local fixture, or a
manually asserted counter into a pass.

Usage:
  verify-browser-vm-live-acceptance.py validate acceptance.json
  verify-browser-vm-live-acceptance.py assemble BUNDLE --output acceptance.json \
    --source-commit <git-sha> --image-digest sha256:<64-hex> --transport rdp
  verify-browser-vm-live-acceptance.py --self-test
"""

from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import sys
import tempfile
from types import ModuleType
from typing import Any, NoReturn


SCHEMA_VERSION = 1
MAX_FILE_BYTES = 8 * 1024 * 1024
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
IMAGE_DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
MAX_AGE_SECONDS = 24 * 60 * 60
UTC_TIMESTAMP_RE = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"
)
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
BOUNDED_LABEL_RE = re.compile(r"^[^\x00-\x1f\x7f]{1,128}$")
CREDENTIAL_FIELD_RE = re.compile(
    r"(?:pass(?:word|phrase)?|secret|token|ticket|credential|bearer|"
    r"api[_-]?key|access[_-]?key|private[_-]?key|cookie|authorization|"
    r"identity[_-]?file|pem)",
    re.IGNORECASE,
)

EXPECTED_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "profile",
        "image",
        "source_commit",
        "image_digest",
        "status",
        "source",
        "transport",
        "deployment_evidence",
        "vdi_evidence",
        "runtime_evidence",
        "media_evidence",
        "performance_evidence",
        "audio_evidence",
        "physical_audibility_evidence",
        "recorded_at",
    }
)
ARTIFACT_FIELDS = frozenset({"path", "sha256"})
PHYSICAL_AUDIBILITY_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "profile",
        "image",
        "source_commit",
        "image_digest",
        "status",
        "source",
        "transport",
        "target_host",
        "seat_id",
        "sink_id",
        "observation_method",
        "audibility",
        "operator_confirmation",
        "audio_manifest_sha256",
        "after_reconnect_playback_sha256",
        "confirmed_at",
        "recorded_at",
    }
)
EXPECTED_AUDIO_CLAIMS = {
    "playback": "sample-backed",
    "capture": "sample-backed",
    "recovery": "sample-backed-after-observed-reconnect",
    "scope": "digital-pcm-path-only",
    "physical_audibility": "operator-confirmation-required",
    "production_audio_acceptance": "not-proven-by-this-validator",
}
EVIDENCE_KINDS = {
    "browser_vm_deployment_receipt": "deployment_evidence",
    "browser_vm_runtime_evidence": "runtime_evidence",
    "browser_vm_media_probe": "media_evidence",
    "browser_vm_performance": "performance_evidence",
    "browser_vm_live_audio_samples": "audio_evidence",
    "browser_vm_physical_audibility_confirmation": "physical_audibility_evidence",
}
ASSEMBLED_FIELDS = (
    "deployment_evidence",
    "vdi_evidence",
    "runtime_evidence",
    "media_evidence",
    "performance_evidence",
    "audio_evidence",
    "physical_audibility_evidence",
)


class EvidenceError(ValueError):
    """The supplied record cannot support Browser VM live acceptance."""


def fail(message: str) -> NoReturn:
    raise EvidenceError(message)


def reject_json_constant(value: str) -> NoReturn:
    fail(f"non-finite JSON number is not allowed: {value}")


def reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def read_json(path: Path) -> Any:
    try:
        stat_result = path.lstat()
    except OSError as exc:
        fail(f"evidence file is not readable: {exc}")
    if os.path.islink(path) or not path.is_file():
        fail("evidence file must be a regular non-symlink file")
    if stat_result.st_mode & 0o077 or stat_result.st_mode & 0o111:
        fail("evidence file must be private and non-executable")
    if stat_result.st_size > MAX_FILE_BYTES:
        fail(f"evidence file exceeds {MAX_FILE_BYTES} bytes")
    try:
        raw = path.read_bytes()
        return json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=reject_duplicate_keys,
            parse_constant=reject_json_constant,
        )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(f"malformed evidence JSON: {exc}")


def reject_credential_fields(value: Any, location: str = "root") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if not isinstance(key, str):
                fail(f"field name at {location} is not a string")
            if CREDENTIAL_FIELD_RE.search(key):
                fail(f"credential-shaped field is not allowed: {location}.{key}")
            reject_credential_fields(child, f"{location}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            reject_credential_fields(child, f"{location}[{index}]")


def require_string(data: dict[str, Any], field: str, pattern: re.Pattern[str]) -> str:
    value = data.get(field)
    if not isinstance(value, str) or pattern.fullmatch(value) is None:
        fail(f"{field} is malformed")
    return value


def validate_timestamp(value: Any, field: str = "recorded_at") -> datetime:
    if not isinstance(value, str) or UTC_TIMESTAMP_RE.fullmatch(value) is None:
        fail(f"{field} must use second-precision UTC form YYYY-MM-DDTHH:MM:SSZ")
    try:
        recorded = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(
            tzinfo=timezone.utc
        )
    except ValueError as exc:
        fail(f"{field} is not a real UTC timestamp: {exc}")
    if recorded.timestamp() > datetime.now(timezone.utc).timestamp() + 300:
        fail(f"{field} is too far in the future")
    return recorded


def validate_fresh_timestamp(value: Any, field: str) -> datetime:
    if not isinstance(value, str) or not value.endswith("Z"):
        fail(f"{field} must be a UTC timestamp")
    try:
        recorded = datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as exc:
        fail(f"{field} is not a valid UTC timestamp: {exc}")
    age = (datetime.now(timezone.utc) - recorded).total_seconds()
    if age < -300 or age > MAX_AGE_SECONDS:
        fail(f"{field} is stale or from the future (age_seconds={age:.0f})")
    return recorded


def load_validator(name: str, filename: str) -> ModuleType:
    path = Path(__file__).with_name(filename)
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        fail(f"validator module is unavailable: {filename}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_validators() -> dict[str, ModuleType]:
    """Load the repository-owned evidence validators without injectable seams."""
    return {
        "vdi_evidence": load_validator("browser_vdi_proof", "verify-vdi-live-proof.py"),
        "deployment_evidence": load_validator(
            "browser_deployment_receipt", "verify-browser-vm-deployment.py"
        ),
        "runtime_evidence": load_validator(
            "browser_runtime_evidence", "verify-browser-vm-runtime-evidence.py"
        ),
        "media_evidence": load_validator(
            "browser_media_evidence", "verify-browser-vm-media-evidence.py"
        ),
        "performance_evidence": load_validator(
            "browser_performance_evidence", "verify-browser-vm-performance.py"
        ),
        "audio_evidence": load_validator(
            "browser_live_audio_samples", "verify-browser-vm-live-audio.py"
        ),
    }


def artifact_path(
    descriptor: Any,
    field: str,
    artifact_root: Path,
) -> Path:
    if not isinstance(descriptor, dict) or frozenset(descriptor) != ARTIFACT_FIELDS:
        fail(f"{field} must contain exactly path and sha256")
    path_text = descriptor.get("path")
    if (
        not isinstance(path_text, str)
        or not path_text
        or Path(path_text).is_absolute()
        or ".." in Path(path_text).parts
    ):
        fail(f"{field}.path must be a relative path inside the bundle")
    digest = descriptor.get("sha256")
    if not isinstance(digest, str) or SHA256_RE.fullmatch(digest) is None:
        fail(f"{field}.sha256 must be 64 lowercase hex characters")
    root = artifact_root.resolve()
    candidate = root / path_text
    if candidate.is_symlink():
        fail(f"{field}.path must be a regular non-symlink file")
    resolved = candidate.resolve()
    try:
        resolved.relative_to(root)
    except ValueError as exc:
        raise EvidenceError(f"{field}.path escapes the evidence bundle") from exc
    try:
        stat_result = resolved.lstat()
    except OSError as exc:
        fail(f"{field}.path is not readable: {exc}")
    if resolved.is_symlink() or not resolved.is_file():
        fail(f"{field}.path must be a regular non-symlink file")
    if stat_result.st_size > MAX_FILE_BYTES:
        fail(f"{field}.path exceeds {MAX_FILE_BYTES} bytes")
    actual = hashlib.sha256(resolved.read_bytes()).hexdigest()
    if actual != digest:
        fail(f"{field}.sha256 does not match the artifact")
    return resolved


def artifact_descriptor(path: Path, artifact_root: Path) -> dict[str, str]:
    root = artifact_root.resolve()
    if path.is_symlink():
        fail("assembled evidence must not be a symlink")
    resolved = path.resolve()
    try:
        relative = resolved.relative_to(root)
    except ValueError as exc:
        raise EvidenceError("assembled evidence escapes the evidence bundle") from exc
    if not resolved.is_file():
        fail("assembled evidence must be a regular file")
    return {
        "path": relative.as_posix(),
        "sha256": hashlib.sha256(resolved.read_bytes()).hexdigest(),
    }


def resolve_assembly_paths(bundle_dir: Path, output: Path) -> tuple[Path, Path]:
    try:
        root_stat = bundle_dir.lstat()
    except OSError as exc:
        fail(f"bundle directory is not readable: {exc}")
    if bundle_dir.is_symlink() or not bundle_dir.is_dir():
        fail("bundle directory must be a real non-symlink directory")
    if root_stat.st_mode & 0o022:
        fail("bundle directory must not be writable by group or other")
    root = bundle_dir.resolve()
    requested = output if output.is_absolute() else root / output
    if requested.is_symlink():
        fail("output manifest must not be a symlink")
    resolved_output = requested.resolve(strict=False)
    try:
        resolved_output.relative_to(root)
    except ValueError as exc:
        raise EvidenceError("output manifest escapes the evidence bundle") from exc
    if resolved_output.exists():
        fail("output manifest already exists")
    parent = resolved_output.parent
    if parent.is_symlink() or not parent.is_dir():
        fail("output manifest parent must be an existing non-symlink directory")
    try:
        parent.resolve().relative_to(root)
    except ValueError as exc:
        raise EvidenceError("output manifest parent escapes the evidence bundle") from exc
    return root, resolved_output


def discover_json_artifacts(root: Path, output: Path) -> list[tuple[Path, dict[str, Any]]]:
    """Read every in-bundle JSON file after rejecting links and special entries."""
    paths: list[Path] = []

    def walk(directory: Path) -> None:
        try:
            entries = sorted(os.scandir(directory), key=lambda entry: entry.name)
        except OSError as exc:
            fail(f"bundle directory is not readable: {exc}")
        for entry in entries:
            path = Path(entry.path)
            if entry.is_symlink():
                fail(f"bundle contains a symlink: {path.relative_to(root)}")
            if entry.is_dir(follow_symlinks=False):
                walk(path)
            elif entry.is_file(follow_symlinks=False):
                if path.suffix == ".json" and path != output:
                    paths.append(path)
            else:
                fail(f"bundle contains a non-regular entry: {path.relative_to(root)}")

    walk(root)
    discovered: list[tuple[Path, dict[str, Any]]] = []
    for path in paths:
        data = read_json(path)
        if not isinstance(data, dict):
            fail(f"JSON artifact is not an object: {path.relative_to(root)}")
        reject_credential_fields(data, path.relative_to(root).as_posix())
        discovered.append((path, data))
    return discovered


def classify_candidate(data: dict[str, Any]) -> str | None:
    kind = data.get("kind")
    if isinstance(kind, str) and kind in EVIDENCE_KINDS:
        return EVIDENCE_KINDS[kind]
    # VDI proof predates the kind discriminator. Keep this signature narrow so
    # unrelated JSON records cannot be upgraded into VDI evidence.
    if kind is None and {
        "schema_version",
        "source_commit",
        "image_digest",
        "status",
        "protocol",
        "target",
        "probe",
        "recorded_at",
    }.issubset(data):
        return "vdi_evidence"
    return None


def validate_assembly_candidates(
    discovered: list[tuple[Path, dict[str, Any]]],
    *,
    artifact_root: Path,
) -> dict[str, list[tuple[Path, dict[str, Any]]]]:
    loaded = load_validators()
    candidates = {field: [] for field in ASSEMBLED_FIELDS}
    physical_candidates: list[tuple[Path, dict[str, Any]]] = []
    for path, data in discovered:
        field = classify_candidate(data)
        if field is None:
            continue
        if field == "physical_audibility_evidence":
            if frozenset(data) != PHYSICAL_AUDIBILITY_FIELDS:
                fail(
                    "physical_audibility_evidence candidate has missing or unexpected "
                    f"fields: {path.relative_to(artifact_root)}"
                )
            physical_candidates.append((path, data))
            continue
        try:
            if field == "vdi_evidence":
                loaded[field].validate_evidence(data)
            elif field == "audio_evidence":
                loaded[field].validate_document(data, path.parent)
            else:
                result = loaded[field].validate_document(data)
                if result.get("status") != "validated":
                    fail(f"{field} candidate did not validate")
        except Exception as exc:
            fail(
                f"invalid {field} candidate {path.relative_to(artifact_root)}: {exc}"
            )
        candidates[field].append((path, data))

    # Physical listening evidence is relational. Validate each candidate using
    # the unique already-validated deployment/audio pair named by its own
    # immutable provenance, target, transport, and audio-manifest digest.
    for path, data in physical_candidates:
        deployment_contexts = [
            item
            for item in candidates["deployment_evidence"]
            if item[1].get("source_commit") == data.get("source_commit")
            and item[1].get("image_digest") == data.get("image_digest")
            and item[1].get("target_host") == data.get("target_host")
        ]
        audio_contexts = [
            item
            for item in candidates["audio_evidence"]
            if item[1].get("source_commit") == data.get("source_commit")
            and item[1].get("image_digest") == data.get("image_digest")
            and item[1].get("transport") == data.get("transport")
            and artifact_descriptor(item[0], artifact_root)["sha256"]
            == data.get("audio_manifest_sha256")
        ]
        contexts = [
            (deployment, audio)
            for deployment in deployment_contexts
            for audio in audio_contexts
        ]
        if len(contexts) != 1:
            fail(
                "physical_audibility_evidence candidate does not name exactly one "
                f"validated deployment/audio context: {path.relative_to(artifact_root)}"
            )
        deployment, audio = contexts[0]
        audio_descriptor = artifact_descriptor(audio[0], artifact_root)
        try:
            validate_physical_audibility(
                data,
                transport=data.get("transport"),
                source_commit=data.get("source_commit"),
                image_digest=data.get("image_digest"),
                target_host=deployment[1].get("target_host"),
                audio_descriptor=audio_descriptor,
                audio_data=audio[1],
            )
        except Exception as exc:
            fail(
                "invalid physical_audibility_evidence candidate "
                f"{path.relative_to(artifact_root)}: {exc}"
            )
        candidates["physical_audibility_evidence"].append((path, data))
    return candidates


def candidate_transport(field: str, data: dict[str, Any]) -> Any:
    if field == "vdi_evidence":
        return data.get("protocol")
    if field in {
        "runtime_evidence",
        "performance_evidence",
        "audio_evidence",
        "physical_audibility_evidence",
    }:
        return data.get("transport")
    # Deployment and guest-local media schemas have no transport field; their
    # immutable provenance is bound to the selected transport by the composite.
    return "rdp"


def select_candidate(
    candidates: dict[str, list[tuple[Path, dict[str, Any]]]],
    field: str,
    *,
    source_commit: str,
    image_digest: str,
    transport: str,
) -> tuple[Path, dict[str, Any]]:
    matches = [
        item
        for item in candidates[field]
        if item[1].get("source_commit") == source_commit
        and item[1].get("image_digest") == image_digest
        and candidate_transport(field, item[1]) == transport
    ]
    if len(matches) != 1:
        fail(
            f"{field}: expected exactly one validated record for requested "
            f"provenance and transport, found {len(matches)}"
        )
    return matches[0]


def deterministic_recorded_at(selected: dict[str, tuple[Path, dict[str, Any]]]) -> str:
    timestamps: list[datetime] = []
    for field, (_, data) in selected.items():
        values = [data.get("recorded_at")]
        if field == "physical_audibility_evidence":
            values.append(data.get("confirmed_at"))
        for value in values:
            if not isinstance(value, str) or not value.endswith("Z"):
                fail(f"{field} has no usable UTC evidence timestamp")
            try:
                parsed = datetime.fromisoformat(value[:-1] + "+00:00")
            except ValueError as exc:
                fail(f"{field} has an invalid UTC evidence timestamp: {exc}")
            timestamps.append(parsed)
    latest = max(timestamps)
    if latest.microsecond:
        latest = latest.replace(microsecond=0) + timedelta(seconds=1)
    return latest.strftime("%Y-%m-%dT%H:%M:%SZ")


def atomic_write_manifest(path: Path, manifest: dict[str, Any]) -> None:
    raw = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        try:
            # A same-directory hard link publishes the fully-fsynced inode in
            # one operation and, unlike replace(), cannot clobber a path that
            # races into existence after the preflight check.
            os.link(temporary, path, follow_symlinks=False)
        except FileExistsError:
            fail("output manifest appeared during atomic assembly")
        temporary.unlink()
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if temporary.exists():
            temporary.unlink()


def assemble(
    bundle_dir: Path,
    output: Path,
    *,
    source_commit: str,
    image_digest: str,
    transport: str,
) -> dict[str, Any]:
    if COMMIT_RE.fullmatch(source_commit) is None or source_commit == "0" * 40:
        fail("requested source_commit must be a non-null 40-character revision")
    if (
        IMAGE_DIGEST_RE.fullmatch(image_digest) is None
        or image_digest == "sha256:" + "0" * 64
    ):
        fail("requested image_digest must be a non-null immutable sha256 digest")
    if transport != "rdp":
        fail("R1 assembly requires the rdp transport")
    root, output_path = resolve_assembly_paths(bundle_dir, output)
    discovered = discover_json_artifacts(root, output_path)
    candidates = validate_assembly_candidates(discovered, artifact_root=root)
    selected = {
        field: select_candidate(
            candidates,
            field,
            source_commit=source_commit,
            image_digest=image_digest,
            transport=transport,
        )
        for field in ASSEMBLED_FIELDS
    }
    descriptors = {
        field: artifact_descriptor(path, root)
        for field, (path, _) in selected.items()
    }
    manifest = {
        "schema_version": SCHEMA_VERSION,
        "kind": "browser_vm_live_acceptance",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "source_commit": source_commit,
        "image_digest": image_digest,
        "status": "passed",
        "source": "live-browser-vm-acceptance",
        "transport": transport,
        **descriptors,
        "recorded_at": deterministic_recorded_at(selected),
    }
    # Validate before publication, then validate the exact bytes at the final
    # path. Assembly only selects and hashes existing proof; it never upgrades it.
    validate(manifest, artifact_root=root)
    atomic_write_manifest(output_path, manifest)
    try:
        composite = validate(read_json(output_path), artifact_root=root)
    except Exception:
        output_path.unlink(missing_ok=True)
        raise
    return {
        "status": "assembled",
        "manifest": output_path.relative_to(root).as_posix(),
        "sha256": hashlib.sha256(output_path.read_bytes()).hexdigest(),
        "composite": composite,
    }


def validate_physical_audibility(
    data: Any,
    *,
    transport: str,
    source_commit: str,
    image_digest: str,
    target_host: str,
    audio_descriptor: dict[str, Any],
    audio_data: dict[str, Any],
) -> dict[str, Any]:
    if not isinstance(data, dict) or frozenset(data) != PHYSICAL_AUDIBILITY_FIELDS:
        fail("physical audibility evidence has missing or unexpected fields")
    reject_credential_fields(data, "physical_audibility")
    if data["schema_version"] != SCHEMA_VERSION or isinstance(
        data["schema_version"], bool
    ):
        fail("physical audibility evidence schema_version is invalid")
    if data["kind"] != "browser_vm_physical_audibility_confirmation":
        fail("physical audibility evidence kind is invalid")
    if data["profile"] != "browser-vm-chromium" or data["image"] != "browser-vm-chromium":
        fail("physical audibility evidence is not bound to browser-vm-chromium")
    if data["source_commit"] != source_commit or data["image_digest"] != image_digest:
        fail("physical audibility evidence provenance does not match acceptance")
    if data["status"] != "observed":
        fail("physical audibility evidence is not observed")
    if data["source"] != "operator-physical-listening":
        fail("physical audibility evidence is not an operator listening observation")
    if data["transport"] != transport:
        fail("physical audibility evidence transport does not match acceptance")
    if data["target_host"] != target_host:
        fail("physical audibility evidence target_host does not match deployment")
    seat_id = require_string(data, "seat_id", BOUNDED_LABEL_RE)
    sink_id = require_string(data, "sink_id", BOUNDED_LABEL_RE)
    if seat_id.strip() != seat_id or sink_id.strip() != sink_id:
        fail("physical audibility seat_id and sink_id must not have edge whitespace")
    if data["observation_method"] != "human-listening-at-physical-seat":
        fail("physical audibility requires human listening at the physical seat")
    if data["audibility"] != "audible" or data["operator_confirmation"] is not True:
        fail("physical audibility lacks an explicit audible operator confirmation")

    manifest_digest = require_string(data, "audio_manifest_sha256", SHA256_RE)
    if manifest_digest != audio_descriptor.get("sha256"):
        fail("physical audibility is not bound to the validated audio manifest")
    after_playback = [
        capture
        for capture in audio_data.get("captures", [])
        if capture.get("phase") == "after-recovery"
        and capture.get("direction") == "playback"
    ]
    if len(after_playback) != 1:
        fail("validated audio has no unique after-reconnect playback capture")
    playback_digest = require_string(
        data, "after_reconnect_playback_sha256", SHA256_RE
    )
    if playback_digest != after_playback[0].get("sha256"):
        fail("physical audibility is not bound to after-reconnect playback samples")

    confirmed = validate_timestamp(data["confirmed_at"], "physical_audibility.confirmed_at")
    recorded = validate_timestamp(data["recorded_at"], "physical_audibility.recorded_at")
    validate_fresh_timestamp(data["confirmed_at"], "physical_audibility.confirmed_at")
    validate_fresh_timestamp(data["recorded_at"], "physical_audibility.recorded_at")
    after_playback_at = validate_timestamp(
        after_playback[0].get("captured_at"),
        "audio.after_reconnect_playback.captured_at",
    )
    if confirmed < after_playback_at:
        fail("physical audibility was not confirmed after reconnect playback")
    if recorded < confirmed or recorded - confirmed > timedelta(minutes=5):
        fail("physical audibility confirmation was not recorded within five minutes")
    return {
        "status": "operator-confirmed",
        "scope": "physical-speaker-output",
        "observation_method": data["observation_method"],
        "seat_id": seat_id,
        "sink_id": sink_id,
        "target_host": target_host,
        "confirmed_at": data["confirmed_at"],
        "audio_manifest_sha256": manifest_digest,
        "after_reconnect_playback_sha256": playback_digest,
    }


def validate(
    bundle: Any,
    *,
    artifact_root: Path,
) -> dict[str, Any]:
    if not isinstance(bundle, dict):
        fail("acceptance root must be one JSON object")
    reject_credential_fields(bundle)
    if frozenset(bundle) != EXPECTED_FIELDS:
        missing = EXPECTED_FIELDS - frozenset(bundle)
        extra = frozenset(bundle) - EXPECTED_FIELDS
        if missing:
            fail(f"acceptance is missing fields: {', '.join(sorted(missing))}")
        fail(f"acceptance has unexpected fields: {', '.join(sorted(extra))}")
    if bundle["schema_version"] != SCHEMA_VERSION or isinstance(bundle["schema_version"], bool):
        fail(f"schema_version must be integer {SCHEMA_VERSION}")
    if bundle["kind"] != "browser_vm_live_acceptance":
        fail("kind is not browser_vm_live_acceptance")
    if bundle["profile"] != "browser-vm-chromium" or bundle["image"] != "browser-vm-chromium":
        fail("acceptance is not bound to browser-vm-chromium")
    if bundle["status"] != "passed":
        fail("Browser VM live acceptance is not passed")
    if bundle["source"] != "live-browser-vm-acceptance":
        fail("source is not the approved live Browser VM acceptance harness")
    source_commit = require_string(bundle, "source_commit", COMMIT_RE)
    image_digest = require_string(bundle, "image_digest", IMAGE_DIGEST_RE)
    if source_commit == "0" * 40:
        fail("source_commit must not be the null revision")
    if image_digest == "sha256:" + "0" * 64:
        fail("image_digest must not be the null digest")
    transport = bundle["transport"]
    if transport != "rdp":
        fail("transport must be rdp for R1 Chromium promotion")
    acceptance_recorded_at = validate_timestamp(bundle["recorded_at"])
    validate_fresh_timestamp(bundle["recorded_at"], "recorded_at")

    # Promotion always loads the repository validators itself.  An injectable
    # validator map would let a caller replace the PCM analyzer with a stub and
    # recreate the legacy counter-only bypass.
    loaded = load_validators()

    deployment_path = artifact_path(
        bundle["deployment_evidence"], "deployment_evidence", artifact_root
    )
    deployment_data = read_json(deployment_path)
    try:
        deployment_result = loaded["deployment_evidence"].validate_document(deployment_data)
    except Exception as exc:
        fail(f"deployment_evidence is invalid: {exc}")
    if deployment_result.get("status") != "validated":
        fail("deployment_evidence does not prove an attached running guest")
    if deployment_data.get("source_commit") != source_commit:
        fail("deployment_evidence source_commit does not match acceptance")
    if deployment_data.get("image_digest") != image_digest:
        fail("deployment_evidence image_digest does not match acceptance")

    vdi_path = artifact_path(bundle["vdi_evidence"], "vdi_evidence", artifact_root)
    vdi_data = read_json(vdi_path)
    try:
        loaded["vdi_evidence"].validate_evidence(vdi_data)
    except Exception as exc:
        fail(f"vdi_evidence is invalid: {exc}")
    if vdi_data.get("status") != "observed":
        fail("vdi_evidence must be observed")
    if vdi_data.get("source_commit") != source_commit:
        fail("vdi_evidence source_commit does not match acceptance")
    if vdi_data.get("image_digest") != image_digest:
        fail("vdi_evidence image_digest does not match acceptance")
    validate_fresh_timestamp(vdi_data.get("recorded_at"), "vdi_evidence.recorded_at")
    if vdi_data.get("protocol") != transport:
        fail("vdi_evidence protocol does not match acceptance transport")
    if vdi_data.get("input_observation") not in {"echoed", "unchanged"}:
        fail("vdi_evidence lacks a focused input observation")
    if transport == "rdp" and vdi_data.get("reconnect_observation") != "tier-reconnected":
        fail("RDP vdi_evidence lacks tier reconnect observation")

    runtime_path = artifact_path(
        bundle["runtime_evidence"], "runtime_evidence", artifact_root
    )
    runtime_data = read_json(runtime_path)
    try:
        runtime_result = loaded["runtime_evidence"].validate_document(runtime_data)
    except Exception as exc:
        fail(f"runtime_evidence is invalid: {exc}")
    if runtime_result.get("status") != "validated":
        fail("runtime_evidence does not prove connected guest wiring")
    if runtime_data.get("transport") != transport:
        fail("runtime_evidence transport does not match acceptance")
    if runtime_data.get("source_commit") != source_commit:
        fail("runtime_evidence source_commit does not match acceptance")
    if runtime_data.get("image_digest") != image_digest:
        fail("runtime_evidence image_digest does not match acceptance")
    validate_fresh_timestamp(runtime_data.get("recorded_at"), "runtime_evidence.recorded_at")
    if runtime_data.get("transport_health") != "connected":
        fail("runtime_evidence does not prove a connected guest")
    if runtime_data.get("gpu_status") != "passed":
        fail("runtime_evidence.gpu_status does not prove guest GPU video readiness")
    if runtime_data.get("audio_status") != "wired":
        fail("runtime_evidence does not prove guest audio endpoints are wired")

    media_path = artifact_path(bundle["media_evidence"], "media_evidence", artifact_root)
    media_data = read_json(media_path)
    try:
        media_result = loaded["media_evidence"].validate_document(media_data)
    except Exception as exc:
        fail(f"media_evidence is invalid: {exc}")
    if media_result.get("status") != "validated":
        fail("media_evidence does not prove guest Chromium decode")
    if media_data.get("source_commit") != source_commit:
        fail("media_evidence source_commit does not match acceptance")
    if media_data.get("image_digest") != image_digest:
        fail("media_evidence image_digest does not match acceptance")
    validate_fresh_timestamp(media_data.get("recorded_at"), "media_evidence.recorded_at")
    if media_data.get("video_dropped_frames") != 0:
        fail("media_evidence contains dropped video frames")

    performance_path = artifact_path(
        bundle["performance_evidence"], "performance_evidence", artifact_root
    )
    performance_data = read_json(performance_path)
    try:
        performance_result = loaded["performance_evidence"].validate_document(performance_data)
    except Exception as exc:
        fail(f"performance_evidence is invalid: {exc}")
    if performance_result.get("status") != "validated":
        fail("performance_evidence does not pass Browser VM acceptance")
    if performance_data.get("source_commit") != source_commit:
        fail("performance_evidence source_commit does not match acceptance")
    if performance_data.get("image_digest") != image_digest:
        fail("performance_evidence image_digest does not match acceptance")
    if performance_data.get("domain_uuid") != deployment_result.get("domain_uuid"):
        fail("performance_evidence domain_uuid does not match deployment_evidence")
    validate_fresh_timestamp(
        performance_data.get("recorded_at"), "performance_evidence.recorded_at"
    )

    audio_path = artifact_path(bundle["audio_evidence"], "audio_evidence", artifact_root)
    audio_data = read_json(audio_path)
    reject_credential_fields(audio_data, "audio")
    try:
        audio_result = loaded["audio_evidence"].validate_document(
            audio_data, audio_path.parent
        )
    except Exception as exc:
        fail(f"audio_evidence sample validation failed: {exc}")
    if audio_result.get("status") != "validated":
        fail("audio_evidence sample validator did not validate the evidence")
    if audio_result.get("evidence_class") != "browser_vm_sample_backed_audio":
        fail("audio_evidence is not sample-backed Browser VM audio")
    if audio_result.get("claims") != EXPECTED_AUDIO_CLAIMS:
        fail("audio_evidence claims do not preserve the digital-only boundary")
    if audio_data.get("source_commit") != source_commit:
        fail("audio_evidence source_commit does not match acceptance")
    if audio_data.get("image_digest") != image_digest:
        fail("audio_evidence image_digest does not match acceptance")
    if audio_data.get("transport") != transport:
        fail("audio_evidence transport does not match acceptance")
    if audio_result.get("source_commit") != source_commit:
        fail("audio_evidence validation result source_commit does not match acceptance")
    if audio_result.get("image_digest") != image_digest:
        fail("audio_evidence validation result image_digest does not match acceptance")
    if audio_result.get("transport") != transport:
        fail("audio_evidence validation result transport does not match acceptance")
    validate_fresh_timestamp(audio_data.get("recorded_at"), "audio_evidence.recorded_at")

    physical_path = artifact_path(
        bundle["physical_audibility_evidence"],
        "physical_audibility_evidence",
        artifact_root,
    )
    physical_data = read_json(physical_path)
    physical_result = validate_physical_audibility(
        physical_data,
        transport=transport,
        source_commit=source_commit,
        image_digest=image_digest,
        target_host=deployment_data.get("target_host"),
        audio_descriptor=bundle["audio_evidence"],
        audio_data=audio_data,
    )
    physical_recorded_at = validate_timestamp(
        physical_data.get("recorded_at"), "physical_audibility.recorded_at"
    )
    if acceptance_recorded_at < physical_recorded_at:
        fail("acceptance was recorded before physical audibility confirmation")

    return {
        "status": "validated",
        "live_proof": "observed",
        "evidence_class": "browser_vm_live_acceptance",
        "profile": bundle["profile"],
        "source_commit": source_commit,
        "image_digest": image_digest,
        "transport": transport,
        "claims": {
            "deployed_guest": "observed",
            "guest_frame": "observed",
            "focused_input": "observed",
            "transport_reconnect": "observed",
            "guest_gpu_video": "observed",
            "guest_chromium_decode": "observed",
            "guest_audio_samples": "validated-before-and-after-reconnect",
            "physical_audibility": "operator-confirmed-at-physical-seat",
            "production_audio_acceptance": "sample-and-physical-evidence-validated",
            "performance": "observed",
        },
        "audio": audio_result,
        "physical_audibility": physical_result,
    }


def _legacy_audio_summary() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "kind": "browser_vm_live_audio",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "source_commit": "0123456789abcdef0123456789abcdef01234567",
        "image_digest": "sha256:" + "a" * 64,
        "status": "observed",
        "source": "live-browser-vm-audio",
        "transport": "rdp",
        "pcm_bytes": 192_000,
        "playback_samples": 480,
        "capture_samples": 480,
        "dropouts": 0,
        "recovery_observed": True,
        "recorded_at": "2020-01-02T03:04:05Z",
    }


def _physical_audibility_fixture(
    *,
    source_commit: str,
    image_digest: str,
    target_host: str,
    audio_manifest_sha256: str,
    after_reconnect_playback_sha256: str,
    confirmed_at: str,
    recorded_at: str,
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "kind": "browser_vm_physical_audibility_confirmation",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "source_commit": source_commit,
        "image_digest": image_digest,
        "status": "observed",
        "source": "operator-physical-listening",
        "transport": "rdp",
        "target_host": target_host,
        "seat_id": "Dell",
        "sink_id": "alsa_output.pci-0000_00_1f.3.analog-stereo",
        "observation_method": "human-listening-at-physical-seat",
        "audibility": "audible",
        "operator_confirmation": True,
        "audio_manifest_sha256": audio_manifest_sha256,
        "after_reconnect_playback_sha256": after_reconnect_playback_sha256,
        "confirmed_at": confirmed_at,
        "recorded_at": recorded_at,
    }


def _write_artifact(root: Path, relative: str, value: Any) -> dict[str, str]:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    path.chmod(0o600)
    return {"path": relative, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


def _rewrite_json_artifact(
    root: Path,
    bundle: dict[str, Any],
    field: str,
    mutate: Any,
) -> dict[str, Any]:
    descriptor = bundle[field]
    path = root / descriptor["path"]
    data = json.loads(path.read_text(encoding="utf-8"))
    mutate(data)
    path.write_text(json.dumps(data, sort_keys=True) + "\n", encoding="utf-8")
    path.chmod(0o600)
    descriptor["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
    return data


def _fixture(root: Path) -> dict[str, Any]:
    source_commit = "0123456789abcdef0123456789abcdef01234567"
    image_digest = "sha256:" + "a" * 64
    vdi = load_validator("fixture_vdi_proof", "verify-vdi-live-proof.py")
    runtime = load_validator("fixture_runtime_evidence", "verify-browser-vm-runtime-evidence.py")
    media = load_validator("fixture_media_evidence", "verify-browser-vm-media-evidence.py")
    performance = load_validator("fixture_performance_evidence", "verify-browser-vm-performance.py")
    deployment = load_validator("fixture_browser_deployment", "verify-browser-vm-deployment.py")
    audio = load_validator("fixture_live_audio_samples", "verify-browser-vm-live-audio.py")
    rdp_log = (
        "live: FRAME OK 1024x768 rects=1 fnv1a64=0x0123456789abcdef distinct_colors=42\n"
        "live: INPUT ECHOED — framebuffer changed after keystroke "
        "(before=0x0123456789abcdef after=0xfedcba9876543210)\n"
        "live: RECONNECTED tier=Compressed desktop=1024x768\n"
        "live: TIER FRAME OK 1024x768 rects=1 fnv1a64=0xfedcba9876543210 distinct_colors=43\n"
    )
    vdi_data = vdi.make_evidence("rdp", "127.0.0.1:13389", 0, rdp_log, source_commit, image_digest)
    runtime_data = runtime.make_valid()
    runtime_data.update(
        {
            "transport": "rdp",
            "gpu_status": "passed",
            "source_commit": source_commit,
            "image_digest": image_digest,
        }
    )
    media_data = media.valid_record()
    media_data.update({"source_commit": source_commit, "image_digest": image_digest})
    performance_data = performance.valid_record()
    performance_data.update({"source_commit": source_commit, "image_digest": image_digest})
    now = datetime.now(timezone.utc).replace(microsecond=0)
    recorded_at = now.strftime("%Y-%m-%dT%H:%M:%SZ")
    deployment_data = {
        "schema_version": 2,
        "kind": "browser_vm_deployment_receipt",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "status": "observed",
        "source": "deploy-image.sh",
        "target_host": "127.0.0.1",
        "node_hostname": "fixture-node",
        "domain_name": "browser-vm",
        "domain_uuid": "01234567-89ab-4cde-8fab-0123456789ab",
        "domain_state": "running",
        "remote_image": "/var/lib/libvirt/images/browser-vm-chromium.qcow2",
        "remote_image_format": "qcow2",
        "attached_disk": "/var/lib/libvirt/images/browser-vm-r1-overlay.qcow2",
        "attached_disk_format": "qcow2",
        "backing_image": "/var/lib/libvirt/images/browser-vm-chromium.qcow2",
        "backing_chain_depth": 1,
        "source_commit": source_commit,
        "image_digest": image_digest,
        "remote_image_digest": image_digest,
        "recorded_at": recorded_at,
    }
    deployment.validate_document(deployment_data)
    performance_data["domain_uuid"] = deployment_data["domain_uuid"]
    runtime_data["recorded_at"] = recorded_at
    media_data["recorded_at"] = recorded_at
    performance_data["recorded_at"] = recorded_at
    audio_root = root / "evidence" / "audio"
    audio_data = audio.fixture(audio_root, now - timedelta(seconds=10))
    audio_data["source_commit"] = source_commit
    audio_data["image_digest"] = image_digest
    audio_descriptor = _write_artifact(
        root, "evidence/audio/audio-samples.json", audio_data
    )
    after_reconnect_playback = [
        capture
        for capture in audio_data["captures"]
        if capture["phase"] == "after-recovery" and capture["direction"] == "playback"
    ]
    assert len(after_reconnect_playback) == 1
    physical_data = _physical_audibility_fixture(
        source_commit=source_commit,
        image_digest=image_digest,
        target_host=deployment_data["target_host"],
        audio_manifest_sha256=audio_descriptor["sha256"],
        after_reconnect_playback_sha256=after_reconnect_playback[0]["sha256"],
        confirmed_at=(now - timedelta(seconds=5)).strftime("%Y-%m-%dT%H:%M:%SZ"),
        recorded_at=(now - timedelta(seconds=4)).strftime("%Y-%m-%dT%H:%M:%SZ"),
    )
    descriptors = {
        "deployment_evidence": _write_artifact(root, "evidence/deployment.json", deployment_data),
        "vdi_evidence": _write_artifact(root, "evidence/vdi.json", vdi_data),
        "runtime_evidence": _write_artifact(root, "evidence/runtime.json", runtime_data),
        "media_evidence": _write_artifact(root, "evidence/media.json", media_data),
        "performance_evidence": _write_artifact(
            root, "evidence/performance.json", performance_data
        ),
        "audio_evidence": audio_descriptor,
        "physical_audibility_evidence": _write_artifact(
            root, "evidence/physical-audibility.json", physical_data
        ),
    }
    return {
        "schema_version": 1,
        "kind": "browser_vm_live_acceptance",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "status": "passed",
        "source": "live-browser-vm-acceptance",
        "source_commit": source_commit,
        "image_digest": image_digest,
        "transport": "rdp",
        **descriptors,
        "recorded_at": recorded_at,
    }


def self_test() -> None:
    positive = 0
    negative = 0
    with __import__("tempfile").TemporaryDirectory(prefix="browser-vm-acceptance-") as temporary:
        root = Path(temporary)
        bundle = _fixture(root)
        result = validate(bundle, artifact_root=root)
        assert result["status"] == "validated"
        assert (
            result["claims"]["guest_audio_samples"]
            == "validated-before-and-after-reconnect"
        )
        assert (
            result["claims"]["physical_audibility"]
            == "operator-confirmed-at-physical-seat"
        )
        assert (
            result["claims"]["production_audio_acceptance"]
            == "sample-and-physical-evidence-validated"
        )
        assert result["audio"]["claims"] == EXPECTED_AUDIO_CLAIMS
        assert result["physical_audibility"]["status"] == "operator-confirmed"
        positive += 1

        def fresh() -> dict[str, Any]:
            return json.loads(json.dumps(_fixture(root)))

        def expect_rejected(
            candidate: dict[str, Any], needle: str, label: str
        ) -> None:
            nonlocal negative
            try:
                validate(candidate, artifact_root=root)
            except EvidenceError as exc:
                assert needle in str(exc), (label, needle, exc)
                negative += 1
            else:
                raise AssertionError(f"accepted invalid acceptance record: {label}")

        candidate = fresh()
        candidate["performance_evidence"]["sha256"] = "0" * 64
        expect_rejected(candidate, "does not match", "performance artifact digest")

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "runtime_evidence",
            lambda data: data.update({"gpu_status": "unavailable"}),
        )
        expect_rejected(candidate, "gpu_status", "runtime GPU bypass")

        candidate = fresh()
        candidate["source_commit"] = "f" * 40
        expect_rejected(candidate, "source_commit", "acceptance provenance mismatch")

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "performance_evidence",
            lambda data: data.update(
                {"domain_uuid": "77777777-7777-4777-8777-777777777777"}
            ),
        )
        expect_rejected(
            candidate,
            "performance_evidence domain_uuid",
            "performance/deployment identity mismatch",
        )

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "audio_evidence",
            lambda data: data.update({"source_commit": "f" * 40}),
        )
        expect_rejected(
            candidate, "audio_evidence source_commit", "audio provenance mismatch"
        )

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "audio_evidence",
            lambda data: data.update({"image_digest": "sha256:" + "b" * 64}),
        )
        expect_rejected(
            candidate, "audio_evidence image_digest", "audio image mismatch"
        )

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "audio_evidence",
            lambda data: data.update({"transport": "sunshine"}),
        )
        expect_rejected(candidate, "audio_evidence transport", "audio transport mismatch")

        def stale_audio(data: dict[str, Any]) -> None:
            data["disconnect_observed_at"] = "2020-01-02T03:04:03Z"
            data["reconnect_observed_at"] = "2020-01-02T03:04:04Z"
            data["recorded_at"] = "2020-01-02T03:04:07Z"
            for capture in data["captures"]:
                if capture["phase"] == "before-recovery":
                    second = 1 if capture["direction"] == "playback" else 2
                else:
                    second = 5 if capture["direction"] == "playback" else 6
                capture["captured_at"] = f"2020-01-02T03:04:0{second}Z"

        candidate = fresh()
        _rewrite_json_artifact(root, candidate, "audio_evidence", stale_audio)
        expect_rejected(candidate, "stale", "stale sample evidence")

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "audio_evidence",
            lambda data: (data.clear(), data.update(_legacy_audio_summary())),
        )
        expect_rejected(
            candidate,
            "missing or unexpected fields",
            "legacy counter-only audio summary",
        )

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "audio_evidence",
            lambda data: data.update({"physical_audibility": "observed"}),
        )
        expect_rejected(
            candidate,
            "missing or unexpected fields",
            "sample-manifest physical-audibility overclaim",
        )

        candidate = fresh()
        audio_path = root / candidate["audio_evidence"]["path"]
        raw_audio = audio_path.read_text(encoding="utf-8").rstrip()
        audio_path.write_text(
            raw_audio[:-1] + ', "kind": "browser_vm_live_audio_samples"}\n',
            encoding="utf-8",
        )
        audio_path.chmod(0o600)
        candidate["audio_evidence"]["sha256"] = hashlib.sha256(
            audio_path.read_bytes()
        ).hexdigest()
        expect_rejected(candidate, "duplicate JSON field: kind", "duplicate audio field")

        candidate = fresh()
        audio_path = root / candidate["audio_evidence"]["path"]
        audio_data = json.loads(audio_path.read_text(encoding="utf-8"))
        after_playback = next(
            capture
            for capture in audio_data["captures"]
            if capture["phase"] == "after-recovery"
            and capture["direction"] == "playback"
        )
        sample_path = audio_path.parent / after_playback["path"]
        audio_validator = load_validator(
            "fixture_silent_audio_samples", "verify-browser-vm-live-audio.py"
        )
        after_playback["sha256"] = audio_validator.write_tone(
            sample_path, after_playback["expected_tone_hz"], silent=True
        )
        audio_path.write_text(
            json.dumps(audio_data, sort_keys=True) + "\n", encoding="utf-8"
        )
        audio_path.chmod(0o600)
        candidate["audio_evidence"]["sha256"] = hashlib.sha256(
            audio_path.read_bytes()
        ).hexdigest()
        expect_rejected(candidate, "non-silent", "silent sample-validation bypass")

        candidate = fresh()
        candidate.pop("physical_audibility_evidence")
        expect_rejected(
            candidate,
            "physical_audibility_evidence",
            "missing physical audibility confirmation",
        )

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "physical_audibility_evidence",
            lambda data: data.update({"observation_method": "digital-pcm-analysis"}),
        )
        expect_rejected(
            candidate,
            "human listening",
            "digital-only physical-audibility overclaim",
        )

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "physical_audibility_evidence",
            lambda data: data.update({"digital_only_claim": True}),
        )
        expect_rejected(
            candidate,
            "missing or unexpected fields",
            "unknown physical audibility field",
        )

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "physical_audibility_evidence",
            lambda data: data.update({"transport": "sunshine"}),
        )
        expect_rejected(
            candidate,
            "transport does not match",
            "physical audibility transport mismatch",
        )

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "physical_audibility_evidence",
            lambda data: data.update({"source_commit": "f" * 40}),
        )
        expect_rejected(
            candidate,
            "provenance does not match",
            "physical audibility provenance mismatch",
        )

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "physical_audibility_evidence",
            lambda data: data.update(
                {
                    "confirmed_at": "2020-01-02T03:04:05Z",
                    "recorded_at": "2020-01-02T03:04:06Z",
                }
            ),
        )
        expect_rejected(candidate, "stale", "stale physical audibility evidence")

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "physical_audibility_evidence",
            lambda data: data.update({"audio_manifest_sha256": "0" * 64}),
        )
        expect_rejected(
            candidate,
            "validated audio manifest",
            "physical confirmation audio-manifest mismatch",
        )

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "physical_audibility_evidence",
            lambda data: data.update(
                {"after_reconnect_playback_sha256": "0" * 64}
            ),
        )
        expect_rejected(
            candidate,
            "after-reconnect playback samples",
            "physical confirmation playback-sample mismatch",
        )

        candidate = fresh()
        _rewrite_json_artifact(
            root,
            candidate,
            "physical_audibility_evidence",
            lambda data: data.update({"operator_confirmation": False}),
        )
        expect_rejected(
            candidate,
            "explicit audible operator confirmation",
            "missing operator confirmation",
        )

        candidate = fresh()
        candidate["legacy_audio_summary"] = _legacy_audio_summary()
        expect_rejected(candidate, "unexpected fields", "unknown acceptance field")

        candidate = fresh()
        duplicate_path = root / "duplicate-acceptance.json"
        raw_acceptance = json.dumps(candidate, sort_keys=True)
        duplicate_path.write_text(
            raw_acceptance[:-1] + ', "transport": "rdp"}\n', encoding="utf-8"
        )
        duplicate_path.chmod(0o600)
        try:
            read_json(duplicate_path)
        except EvidenceError as exc:
            assert "duplicate JSON field: transport" in str(exc), exc
            negative += 1
        else:
            raise AssertionError("accepted a duplicate top-level acceptance field")

        def assembly_case(name: str) -> tuple[Path, dict[str, Any]]:
            case_root = root / name
            case_root.mkdir(mode=0o700)
            return case_root, _fixture(case_root)

        case_root, case_bundle = assembly_case("assemble-happy")
        assembled = assemble(
            case_root,
            Path("acceptance.json"),
            source_commit=case_bundle["source_commit"],
            image_digest=case_bundle["image_digest"],
            transport="rdp",
        )
        assembled_path = case_root / assembled["manifest"]
        assembled_data = read_json(assembled_path)
        assert assembled["status"] == "assembled"
        assert assembled["composite"]["status"] == "validated"
        assert assembled_path.stat().st_mode & 0o777 == 0o600
        assert assembled_data["recorded_at"] >= json.loads(
            (case_root / case_bundle["physical_audibility_evidence"]["path"]).read_text(
                encoding="utf-8"
            )
        )["confirmed_at"]
        positive += 1

        def expect_assembly_rejected(
            case_root: Path,
            case_bundle: dict[str, Any],
            needle: str,
            label: str,
        ) -> None:
            nonlocal negative
            try:
                assemble(
                    case_root,
                    Path("acceptance.json"),
                    source_commit=case_bundle["source_commit"],
                    image_digest=case_bundle["image_digest"],
                    transport="rdp",
                )
            except EvidenceError as exc:
                assert needle in str(exc), (label, needle, exc)
                assert not (case_root / "acceptance.json").exists()
                negative += 1
            else:
                raise AssertionError(f"assembled invalid evidence bundle: {label}")

        case_root, case_bundle = assembly_case("assemble-mismatch")
        runtime_path = case_root / case_bundle["runtime_evidence"]["path"]
        runtime_data = json.loads(runtime_path.read_text(encoding="utf-8"))
        runtime_data["source_commit"] = "f" * 40
        runtime_path.write_text(
            json.dumps(runtime_data, sort_keys=True) + "\n", encoding="utf-8"
        )
        runtime_path.chmod(0o600)
        expect_assembly_rejected(
            case_root,
            case_bundle,
            "runtime_evidence: expected exactly one",
            "provenance mismatch",
        )

        case_root, case_bundle = assembly_case("assemble-ambiguity")
        performance_path = case_root / case_bundle["performance_evidence"]["path"]
        duplicate_performance = json.loads(
            performance_path.read_text(encoding="utf-8")
        )
        _write_artifact(
            case_root, "evidence/performance-duplicate.json", duplicate_performance
        )
        expect_assembly_rejected(
            case_root,
            case_bundle,
            "performance_evidence: expected exactly one",
            "ambiguous performance evidence",
        )

        case_root, case_bundle = assembly_case("assemble-symlink")
        (case_root / "evidence" / "deployment-link.json").symlink_to(
            case_root / case_bundle["deployment_evidence"]["path"]
        )
        expect_assembly_rejected(
            case_root, case_bundle, "contains a symlink", "symlinked evidence"
        )

        case_root, case_bundle = assembly_case("assemble-no-physical")
        (case_root / case_bundle["physical_audibility_evidence"]["path"]).unlink()
        expect_assembly_rejected(
            case_root,
            case_bundle,
            "physical_audibility_evidence: expected exactly one",
            "missing physical confirmation",
        )

        case_root, case_bundle = assembly_case("assemble-credential")
        vdi_path = case_root / case_bundle["vdi_evidence"]["path"]
        vdi_data = json.loads(vdi_path.read_text(encoding="utf-8"))
        vdi_data["access_token"] = "must-never-enter-proof"
        vdi_path.write_text(
            json.dumps(vdi_data, sort_keys=True) + "\n", encoding="utf-8"
        )
        vdi_path.chmod(0o600)
        expect_assembly_rejected(
            case_root,
            case_bundle,
            "credential-shaped field",
            "credential-bearing evidence",
        )

        case_root, case_bundle = assembly_case("assemble-path-escape")
        audio_path = case_root / case_bundle["audio_evidence"]["path"]
        audio_data = json.loads(audio_path.read_text(encoding="utf-8"))
        audio_data["captures"][0]["path"] = "../../../outside.wav"
        audio_path.write_text(
            json.dumps(audio_data, sort_keys=True) + "\n", encoding="utf-8"
        )
        audio_path.chmod(0o600)
        expect_assembly_rejected(
            case_root,
            case_bundle,
            "within the evidence directory",
            "audio sample path escape",
        )

    print(
        "verify-browser-vm-live-acceptance: self-test passed "
        f"({positive} positive, {negative} negative cases)"
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", choices=("validate", "assemble"))
    parser.add_argument("path", nargs="?", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--source-commit")
    parser.add_argument("--image-digest")
    parser.add_argument("--transport", choices=("rdp",))
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.self_test:
            if any(
                value is not None
                for value in (
                    args.command,
                    args.path,
                    args.output,
                    args.source_commit,
                    args.image_digest,
                    args.transport,
                )
            ):
                parser.error("--self-test does not accept assembly or validation arguments")
            self_test()
            return 0
        if args.command == "validate" and args.path is not None:
            if any(
                value is not None
                for value in (
                    args.output,
                    args.source_commit,
                    args.image_digest,
                    args.transport,
                )
            ):
                parser.error("validate does not accept assembly arguments")
            result = validate(read_json(args.path), artifact_root=args.path.parent)
        elif args.command == "assemble" and args.path is not None:
            if any(
                value is None
                for value in (
                    args.output,
                    args.source_commit,
                    args.image_digest,
                    args.transport,
                )
            ):
                parser.error(
                    "assemble requires --output, --source-commit, --image-digest, "
                    "and --transport rdp"
                )
            result = assemble(
                args.path,
                args.output,
                source_commit=args.source_commit,
                image_digest=args.image_digest,
                transport=args.transport,
            )
        else:
            parser.error("use validate, assemble, or --self-test")
        print(json.dumps(result, sort_keys=True))
        return 0
    except EvidenceError as exc:
        print(f"verify-browser-vm-live-acceptance: rejected: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
