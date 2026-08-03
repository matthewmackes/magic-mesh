#!/usr/bin/env python3
"""Validate one complete, artifact-bound Browser VM live acceptance record.

The Browser VM has deliberately separate evidence producers for framebuffer
presentation/input/reconnect, guest runtime state, Chromium media decode, and
long-run performance.  This helper is the promotion boundary that binds those
records to one source commit and image digest and additionally requires a
real, sample-backed audio record.  It never probes a target and never turns a
reachable port, a guest-local fixture, or a manually asserted counter into a
pass.

Usage:
  verify-browser-vm-live-acceptance.py validate acceptance.json
  verify-browser-vm-live-acceptance.py --self-test
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import sys
from types import ModuleType
from typing import Any, NoReturn


SCHEMA_VERSION = 1
MAX_FILE_BYTES = 8 * 1024 * 1024
MAX_COUNTER = 10_000_000_000
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
IMAGE_DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
MAX_AGE_SECONDS = 24 * 60 * 60
UTC_TIMESTAMP_RE = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"
)
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
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
        "vdi_evidence",
        "runtime_evidence",
        "media_evidence",
        "performance_evidence",
        "audio_evidence",
        "recorded_at",
    }
)
ARTIFACT_FIELDS = frozenset({"path", "sha256"})
AUDIO_FIELDS = frozenset(
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
        "pcm_bytes",
        "playback_samples",
        "capture_samples",
        "dropouts",
        "recovery_observed",
        "recorded_at",
    }
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


def require_uint(data: dict[str, Any], field: str) -> int:
    value = data.get(field)
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= MAX_COUNTER:
        fail(f"{field} must be an integer between 0 and {MAX_COUNTER}")
    return value


def validate_timestamp(value: Any, field: str = "recorded_at") -> None:
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


def validate_fresh_timestamp(value: Any, field: str) -> None:
    if not isinstance(value, str) or not value.endswith("Z"):
        fail(f"{field} must be a UTC timestamp")
    try:
        recorded = datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as exc:
        fail(f"{field} is not a valid UTC timestamp: {exc}")
    age = (datetime.now(timezone.utc) - recorded).total_seconds()
    if age < -300 or age > MAX_AGE_SECONDS:
        fail(f"{field} is stale or from the future (age_seconds={age:.0f})")


def load_validator(name: str, filename: str) -> ModuleType:
    path = Path(__file__).with_name(filename)
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        fail(f"validator module is unavailable: {filename}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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


def validate_audio(data: Any, transport: str) -> dict[str, Any]:
    if not isinstance(data, dict) or frozenset(data) != AUDIO_FIELDS:
        fail("audio evidence has missing or unexpected fields")
    reject_credential_fields(data, "audio")
    if data["schema_version"] != SCHEMA_VERSION or isinstance(data["schema_version"], bool):
        fail("audio evidence schema_version is invalid")
    if data["kind"] != "browser_vm_live_audio":
        fail("audio evidence kind is not browser_vm_live_audio")
    if data["profile"] != "browser-vm-chromium" or data["image"] != "browser-vm-chromium":
        fail("audio evidence is not bound to browser-vm-chromium")
    source_commit = require_string(data, "source_commit", COMMIT_RE)
    image_digest = require_string(data, "image_digest", IMAGE_DIGEST_RE)
    if source_commit == "0" * 40 or image_digest == "sha256:" + "0" * 64:
        fail("audio evidence provenance must not use null values")
    if data["status"] != "observed":
        fail("audio evidence is not observed")
    if data["source"] != "live-browser-vm-audio":
        fail("audio evidence source is not the approved live capture")
    if data["transport"] != transport:
        fail("audio evidence transport does not match the acceptance transport")
    pcm = require_uint(data, "pcm_bytes")
    playback = require_uint(data, "playback_samples")
    capture = require_uint(data, "capture_samples")
    dropouts = require_uint(data, "dropouts")
    if not isinstance(data["recovery_observed"], bool):
        fail("audio evidence recovery_observed must be boolean")
    if pcm == 0 or playback == 0 or capture == 0:
        fail("audio evidence requires PCM, playback, and capture samples")
    if dropouts != 0:
        fail("audio evidence contains dropouts")
    if not data["recovery_observed"]:
        fail("audio evidence lacks recovery observation")
    validate_fresh_timestamp(data["recorded_at"], "audio.recorded_at")
    return {
        "status": "observed",
        "pcm_bytes": pcm,
        "playback_samples": playback,
        "capture_samples": capture,
        "dropouts": dropouts,
        "source_commit": source_commit,
        "image_digest": image_digest,
    }


def validate(
    bundle: Any,
    *,
    artifact_root: Path,
    validators: dict[str, ModuleType] | None = None,
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
    if transport not in {"rdp", "spice"}:
        fail("transport must be rdp or spice")
    validate_timestamp(bundle["recorded_at"])
    validate_fresh_timestamp(bundle["recorded_at"], "recorded_at")

    loaded = validators or {
        "vdi": load_validator("browser_vdi_proof", "verify-vdi-live-proof.py"),
        "runtime": load_validator("browser_runtime_evidence", "verify-browser-vm-runtime-evidence.py"),
        "media": load_validator("browser_media_evidence", "verify-browser-vm-media-evidence.py"),
        "performance": load_validator("browser_performance_evidence", "verify-browser-vm-performance.py"),
    }

    vdi_path = artifact_path(bundle["vdi_evidence"], "vdi_evidence", artifact_root)
    vdi_data = read_json(vdi_path)
    try:
        loaded["vdi"].validate_evidence(vdi_data)
    except Exception as exc:
        fail(f"vdi_evidence is invalid: {exc}")
    if vdi_data.get("status") != "observed":
        fail("vdi_evidence must be observed")
    if vdi_data.get("source_commit") != source_commit:
        fail("vdi_evidence source_commit does not match acceptance")
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
        runtime_result = loaded["runtime"].validate_document(runtime_data)
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
        media_result = loaded["media"].validate_document(media_data)
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
        performance_result = loaded["performance"].validate_document(performance_data)
    except Exception as exc:
        fail(f"performance_evidence is invalid: {exc}")
    if performance_result.get("status") != "validated":
        fail("performance_evidence does not pass Browser VM acceptance")
    if performance_data.get("source_commit") != source_commit:
        fail("performance_evidence source_commit does not match acceptance")
    if performance_data.get("image_digest") != image_digest:
        fail("performance_evidence image_digest does not match acceptance")
    validate_fresh_timestamp(
        performance_data.get("recorded_at"), "performance_evidence.recorded_at"
    )

    audio_path = artifact_path(bundle["audio_evidence"], "audio_evidence", artifact_root)
    audio_data = read_json(audio_path)
    audio_result = validate_audio(audio_data, transport)

    return {
        "status": "validated",
        "live_proof": "observed",
        "evidence_class": "browser_vm_live_acceptance",
        "profile": bundle["profile"],
        "source_commit": source_commit,
        "image_digest": image_digest,
        "transport": transport,
        "claims": {
            "guest_frame": "observed",
            "focused_input": "observed",
            "transport_reconnect": "observed",
            "guest_gpu_video": "observed",
            "guest_chromium_decode": "observed",
            "guest_audio": "observed",
            "performance": "observed",
        },
        "audio": audio_result,
    }


def _valid_audio() -> dict[str, Any]:
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


def _write_artifact(root: Path, relative: str, value: Any) -> dict[str, str]:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    path.chmod(0o600)
    return {"path": relative, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


def _fixture(root: Path) -> dict[str, Any]:
    source_commit = "0123456789abcdef0123456789abcdef01234567"
    image_digest = "sha256:" + "a" * 64
    vdi = load_validator("fixture_vdi_proof", "verify-vdi-live-proof.py")
    runtime = load_validator("fixture_runtime_evidence", "verify-browser-vm-runtime-evidence.py")
    media = load_validator("fixture_media_evidence", "verify-browser-vm-media-evidence.py")
    performance = load_validator("fixture_performance_evidence", "verify-browser-vm-performance.py")
    rdp_log = (
        "live: FRAME OK 1024x768 rects=1 fnv1a64=0x0123456789abcdef distinct_colors=42\n"
        "live: INPUT sent OK; framebuffer UNCHANGED after keystroke "
        "(fnv1a64=0x0123456789abcdef)\n"
        "live: RECONNECTED tier=Compressed desktop=1024x768\n"
        "live: TIER FRAME OK 1024x768 rects=1 fnv1a64=0xfedcba9876543210 distinct_colors=43\n"
    )
    vdi_data = vdi.make_evidence("rdp", "127.0.0.1:13389", 0, rdp_log, source_commit)
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
    audio_data = _valid_audio()
    recorded_at = datetime.now(timezone.utc).replace(microsecond=0).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    runtime_data["recorded_at"] = recorded_at
    media_data["recorded_at"] = recorded_at
    performance_data["recorded_at"] = recorded_at
    audio_data["recorded_at"] = recorded_at
    descriptors = {
        "vdi_evidence": _write_artifact(root, "evidence/vdi.json", vdi_data),
        "runtime_evidence": _write_artifact(root, "evidence/runtime.json", runtime_data),
        "media_evidence": _write_artifact(root, "evidence/media.json", media_data),
        "performance_evidence": _write_artifact(
            root, "evidence/performance.json", performance_data
        ),
        "audio_evidence": _write_artifact(root, "evidence/audio.json", audio_data),
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
        "recorded_at": datetime.now(timezone.utc).replace(microsecond=0).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        ),
    }


def self_test() -> None:
    positive = 0
    negative = 0
    with __import__("tempfile").TemporaryDirectory(prefix="browser-vm-acceptance-") as temporary:
        root = Path(temporary)
        bundle = _fixture(root)
        result = validate(bundle, artifact_root=root)
        assert result["status"] == "validated"
        assert result["claims"]["guest_audio"] == "observed"
        positive += 1

        for mutation, needle in (
            (
                lambda value: value["performance_evidence"].update(
                    {"sha256": "0" * 64}
                ),
                "does not match",
            ),
            (
                lambda value: json.loads(
                    (root / value["runtime_evidence"]["path"]).read_text()
                ).update({"gpu_status": "unavailable"}),
                "gpu_status",
            ),
            (
                lambda value: value.update({"source_commit": "f" * 40}),
                "source_commit",
            ),
        ):
            candidate = json.loads(json.dumps(bundle))
            if needle == "gpu_status":
                runtime_path = root / candidate["runtime_evidence"]["path"]
                runtime_data = json.loads(runtime_path.read_text())
                runtime_data["gpu_status"] = "unavailable"
                runtime_path.write_text(json.dumps(runtime_data, sort_keys=True) + "\n")
                runtime_path.chmod(0o600)
                candidate["runtime_evidence"]["sha256"] = hashlib.sha256(
                    runtime_path.read_bytes()
                ).hexdigest()
            else:
                mutation(candidate)
            try:
                validate(candidate, artifact_root=root)
            except EvidenceError as exc:
                assert needle in str(exc), (needle, exc)
                negative += 1
            else:
                raise AssertionError(f"accepted invalid acceptance record: {needle}")

    print(
        "verify-browser-vm-live-acceptance: self-test passed "
        f"({positive} positive, {negative} negative cases)"
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", choices=("validate",))
    parser.add_argument("path", nargs="?", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.self_test:
            if args.command is not None or args.path is not None:
                parser.error("--self-test does not accept a command or path")
            self_test()
            return 0
        if args.command != "validate" or args.path is None:
            parser.error("use validate acceptance.json or --self-test")
        result = validate(read_json(args.path), artifact_root=args.path.parent)
        print(json.dumps(result, sort_keys=True))
        return 0
    except EvidenceError as exc:
        print(f"verify-browser-vm-live-acceptance: rejected: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
