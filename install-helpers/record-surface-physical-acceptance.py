#!/usr/bin/python3
"""Record and validate explicit Surface Pro 5/6 physical acceptance observations."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any


SCHEMA_VERSION = 1
MAX_FILE_BYTES = 512 * 1024
MAX_BUNDLE_BYTES = 4 * 1024 * 1024
CAMERA_PROOF_MAX_AGE_MS = 90_000
CAMERA_PROOF_FUTURE_SKEW_MS = 5_000
RECORD_KIND = "mcnf-surface-pro56-physical-acceptance"
REVISION = re.compile(r"^[0-9a-f]{40}$")
OBSERVER = re.compile(r"^[A-Za-z0-9][A-Za-z0-9@._-]{1,127}$")
TIMESTAMP = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
EXPECTED_FILES = (
    "identity.json", "release-packages.json", "kernel-modules.json", "iptsd.json",
    "input.json", "buttons-storage.json", "sam-iio.json", "drm.json", "cameras.json", "camera-proof.json",
    "radios.json", "firmware.json", "audio.json", "power.json", "services.json",
)
COLLECTOR_MANUAL_CHECKS = (
    "ten-finger touch accuracy and edge gestures",
    "pen hover, pressure, eraser, and palm rejection",
    "Type Cover attach, detach, keyboard, touchpad, and backlight",
    "power and volume buttons",
    "microSD insertion, read, eject, and reinsertion",
    "portrait and landscape rotation with correct touch transform",
    "camera privacy indication observed; one-frame proof passes, discards the frame immediately, and retains no image or device identifier",
    "speaker, microphone, headphone, and Bluetooth audio judgement",
    "suspend/resume, S0ix residency delta, Wi-Fi, Bluetooth, and mesh recovery",
    "cold boot, reboot, upgrade, rollback, and secure-boot recovery",
    "internal and external DRM modes, scaling, rotation, hotplug, and atomic modesetting",
    "fingerprint reader availability and authentication judgement without recording biometric data",
)
CHECKS = (
    "touch", "pen", "type-cover", "buttons", "microsd", "rotation",
    "camera-privacy", "audio-microphone", "suspend-s0ix", "reboot-upgrade",
    "drm-modes", "fingerprint",
)
OUTCOMES = {"pass", "fail", "blocked", "unsupported"}
SECRET = re.compile(r"(?i)\b(?:bearer|token|password|secret|private[-_ ]?key)\s*[:=]\s*\S+")
IPV4 = re.compile(r"(?<![0-9])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![0-9])")
MAC = re.compile(r"(?i)\b(?:[0-9a-f]{2}:){5}[0-9a-f]{2}\b")


class RecordError(RuntimeError):
    pass


def strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise RecordError(f"duplicate JSON field: {key}")
        value[key] = item
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def bounded_regular(path: Path, maximum: int = MAX_FILE_BYTES) -> bytes:
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        raise RecordError(f"input is not a regular non-symlink file: {path.name}")
    if info.st_size <= 0 or info.st_size > maximum:
        raise RecordError(f"input has an invalid bounded size: {path.name}")
    with path.open("rb") as stream:
        data = stream.read(maximum + 1)
    if len(data) > maximum:
        raise RecordError(f"input exceeds {maximum} bytes: {path.name}")
    return data


def load_json(path: Path) -> Any:
    try:
        return json.loads(bounded_regular(path), object_pairs_hook=strict_object)
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as exc:
        raise RecordError(f"invalid bounded JSON input: {path.name}") from exc


def parse_time(value: Any, field: str) -> datetime:
    if not isinstance(value, str) or TIMESTAMP.fullmatch(value) is None:
        raise RecordError(f"{field} must be an exact UTC seconds timestamp")
    try:
        return datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as exc:
        raise RecordError(f"{field} is not a real timestamp") from exc


def governed_text(value: Any, field: str, maximum: int = 2048) -> str:
    if not isinstance(value, str) or not (8 <= len(value) <= maximum):
        raise RecordError(f"{field} must contain 8-{maximum} characters")
    if value != value.strip() or any(ord(char) < 32 or ord(char) == 127 for char in value):
        raise RecordError(f"{field} contains whitespace or control characters outside the contract")
    if SECRET.search(value) or IPV4.search(value) or MAC.search(value):
        raise RecordError(f"{field} contains a prohibited credential or network identifier")
    return value


def validate_camera_proof(value: Any, generation: int, captured: datetime) -> dict[str, Any]:
    required = {
        "topic", "node", "model", "generation", "completed_at_ms", "outcome",
        "result_sha256", "frame_bytes_retained", "device_identifier_retained",
        "request_identifier_retained",
    }
    if not isinstance(value, dict) or set(value) != required:
        raise RecordError("camera proof projection schema is invalid")
    node = value.get("node")
    expected_model = "Surface Pro 6" if generation == 6 else "Surface Pro 5"
    if (
        not isinstance(node, str)
        or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}", node) is None
        or value.get("topic") != f"state/hardware/surface/{node}/camera-proof"
        or value.get("model") != expected_model
        or value.get("generation") != generation
        or value.get("outcome") != "passed"
        or not isinstance(value.get("result_sha256"), str)
        or SHA256.fullmatch(value["result_sha256"]) is None
        or value.get("frame_bytes_retained") is not False
        or value.get("device_identifier_retained") is not False
        or value.get("request_identifier_retained") is not False
    ):
        raise RecordError("camera proof identity, outcome, or privacy binding is invalid")
    completed = value.get("completed_at_ms")
    if not isinstance(completed, int) or isinstance(completed, bool) or completed <= 0:
        raise RecordError("camera proof completion timestamp is invalid")
    captured_ms = int(captured.timestamp() * 1000)
    if completed > captured_ms + CAMERA_PROOF_FUTURE_SKEW_MS or captured_ms - completed > CAMERA_PROOF_MAX_AGE_MS:
        raise RecordError("camera proof was not fresh at collection")
    return value


def load_bundle(bundle: Path) -> dict[str, Any]:
    if bundle.is_symlink() or not bundle.is_dir():
        raise RecordError("collector bundle must be a real directory")
    paths = list(bundle.iterdir())
    if sorted(path.name for path in paths) != sorted((*EXPECTED_FILES, "manifest.json")):
        raise RecordError("collector bundle contains missing or unknown files")
    if sum(path.lstat().st_size for path in paths) > MAX_BUNDLE_BYTES:
        raise RecordError("collector bundle exceeds the governed size")
    manifest = load_json(bundle / "manifest.json")
    expected_manifest = {
        "schema_version", "collector", "seat_label", "captured_at_utc",
        "expected_surface_pro_generation", "collection_scope", "collection_verdict",
        "incomplete_probes", "physical_acceptance_claimed", "manual_acceptance_required",
        "artifacts",
    }
    if not isinstance(manifest, dict) or set(manifest) != expected_manifest:
        raise RecordError("collector manifest schema is invalid")
    if manifest["schema_version"] != 1 or manifest["physical_acceptance_claimed"] is not False:
        raise RecordError("collector manifest makes an invalid physical-acceptance claim")
    if manifest["manual_acceptance_required"] != list(COLLECTOR_MANUAL_CHECKS):
        raise RecordError("collector manual-check contract is unknown")
    generation = manifest["expected_surface_pro_generation"]
    if generation not in (5, 6):
        raise RecordError("collector generation is invalid")
    captured = parse_time(manifest["captured_at_utc"], "collector captured_at_utc")
    collector = manifest["collector"]
    if not isinstance(collector, dict) or not isinstance(collector.get("sha256"), str) or SHA256.fullmatch(collector["sha256"]) is None:
        raise RecordError("collector provenance is invalid")
    if manifest["collection_verdict"] not in {"complete", "incomplete"} or not isinstance(manifest["incomplete_probes"], list):
        raise RecordError("collector verdict is invalid")
    artifacts = manifest["artifacts"]
    if not isinstance(artifacts, list) or len(artifacts) != len(EXPECTED_FILES):
        raise RecordError("collector artifact manifest is incomplete")
    artifact_binding = []
    seen: set[str] = set()
    observed_incomplete = []
    camera_proof: dict[str, Any] | None = None
    for item in artifacts:
        if not isinstance(item, dict) or set(item) != {"file", "bytes", "sha256", "status"}:
            raise RecordError("collector artifact descriptor is invalid")
        name = item["file"]
        if name not in EXPECTED_FILES or name in seen:
            raise RecordError("collector artifact is unknown or duplicated")
        if item["status"] not in {"ok", "error", "unavailable"}:
            raise RecordError("collector artifact status is invalid")
        seen.add(name)
        path = bundle / name
        data = bounded_regular(path)
        if item["bytes"] != len(data) or item["sha256"] != hashlib.sha256(data).hexdigest():
            raise RecordError(f"collector artifact integrity mismatch: {name}")
        try:
            document = json.loads(data, object_pairs_hook=strict_object)
        except (UnicodeError, json.JSONDecodeError, ValueError) as exc:
            raise RecordError(f"collector artifact JSON is invalid: {name}") from exc
        if not isinstance(document, dict) or document.get("schema_version") != 1 or document.get("status") != item["status"]:
            raise RecordError(f"collector artifact schema is invalid: {name}")
        if item["status"] != "ok":
            observed_incomplete.append(name)
        elif name == "camera-proof.json":
            camera_proof = validate_camera_proof(document.get("data"), generation, captured)
        artifact_binding.append({"file": name, "bytes": len(data), "sha256": item["sha256"]})
    if manifest["incomplete_probes"] != sorted(observed_incomplete) or manifest["collection_verdict"] != ("incomplete" if observed_incomplete else "complete"):
        raise RecordError("collector incomplete-probe verdict is inconsistent")
    identity = load_json(bundle / "identity.json")
    if identity.get("status") != "ok" or not isinstance(identity.get("data"), dict):
        raise RecordError("collector identity is incomplete")
    identity_data = identity["data"]
    if identity_data.get("manufacturer") != "Microsoft Corporation" or identity_data.get("expected_generation") != generation or identity_data.get("detected_generation") != generation:
        raise RecordError("collector identity contradicts its generation")
    model = identity_data.get("product_name")
    sku = identity_data.get("product_sku")
    if generation == 6:
        if model != "Surface Pro 6" or manifest["seat_label"] != "Surface":
            raise RecordError("Pro 6 acceptance requires the canonical Surface seat and exact model")
    elif model != "Surface Pro" or sku not in {"Surface_Pro_1796", "Surface_Pro_1807"} or manifest["seat_label"] == "Surface":
        raise RecordError("Pro 5 acceptance requires an exact Pro 5 SKU and distinct seat")
    release = load_json(bundle / "release-packages.json")
    packages = release.get("data", {}).get("packages", []) if isinstance(release, dict) else []
    magic_mesh_nevra = next(
        (row.get("nevra") for row in packages if isinstance(row, dict) and row.get("name") == "magic-mesh" and row.get("status") == "installed"),
        None,
    )
    if not isinstance(magic_mesh_nevra, list) or not magic_mesh_nevra:
        raise RecordError("collector lacks the installed magic-mesh package identity")
    if manifest["collection_verdict"] == "complete" and camera_proof is None:
        raise RecordError("complete collector bundle lacks a successful fresh camera proof")
    camera_artifact_sha256 = next(
        (item["sha256"] for item in artifact_binding if item["file"] == "camera-proof.json"),
        None,
    )
    return {
        "manifest": manifest,
        "captured": captured,
        "binding": {
            "manifest_sha256": sha256_file(bundle / "manifest.json"),
            "collector_sha256": collector["sha256"],
            "magic_mesh_nevra": magic_mesh_nevra,
            "artifacts": sorted(artifact_binding, key=lambda item: item["file"]),
        },
        "seat": manifest["seat_label"],
        "generation": generation,
        "model": model,
        "sku": sku,
        "collection_verdict": manifest["collection_verdict"],
        "incomplete_probes": manifest["incomplete_probes"],
        "camera_proof": (
            {**camera_proof, "artifact_sha256": camera_artifact_sha256}
            if camera_proof is not None
            else None
        ),
    }


def validate_observations(value: Any, captured: datetime, now: datetime) -> list[dict[str, Any]]:
    if not isinstance(value, list) or len(value) != len(CHECKS):
        raise RecordError("observations must contain every governed check exactly once")
    seen: set[str] = set()
    validated = []
    for item in value:
        expected = {"check", "performed", "outcome", "observed_at_utc", "observation", "limitations"}
        if not isinstance(item, dict) or set(item) != expected:
            raise RecordError("observation schema contains missing or unknown fields")
        check = item["check"]
        if check not in CHECKS or check in seen:
            raise RecordError("observation check is unknown or duplicated")
        seen.add(check)
        outcome = item["outcome"]
        performed = item["performed"]
        if outcome not in OUTCOMES or not isinstance(performed, bool):
            raise RecordError(f"{check} has an invalid performed/outcome value")
        if (outcome in {"pass", "fail"}) != performed:
            raise RecordError(f"{check} has contradictory performed/outcome values")
        limitations = item["limitations"]
        if not isinstance(limitations, list) or len(limitations) > 8:
            raise RecordError(f"{check} limitations must be a bounded list")
        limitations = [governed_text(text, f"{check} limitation", 512) for text in limitations]
        if outcome == "pass" and limitations:
            raise RecordError(f"{check} cannot pass while declaring limitations")
        if outcome != "pass" and not limitations:
            raise RecordError(f"{check} non-pass outcome requires an explicit limitation")
        observed = parse_time(item["observed_at_utc"], f"{check} observed_at_utc")
        if observed < captured or observed > now + timedelta(minutes=5):
            raise RecordError(f"{check} timestamp predates collection or is in the future")
        validated.append({
            "check": check, "performed": performed, "outcome": outcome,
            "observed_at_utc": item["observed_at_utc"],
            "observation": governed_text(item["observation"], f"{check} observation"),
            "limitations": limitations,
        })
    return sorted(validated, key=lambda item: CHECKS.index(item["check"]))


def build_record(bundle: dict[str, Any], observations: dict[str, Any], prior_sha256: str | None) -> dict[str, Any]:
    if not isinstance(observations, dict) or set(observations) != {"schema_version", "revision", "observer", "observations"}:
        raise RecordError("operator observation document schema is invalid")
    if observations["schema_version"] != SCHEMA_VERSION or not isinstance(observations["revision"], str) or REVISION.fullmatch(observations["revision"]) is None:
        raise RecordError("operator observations require an exact 40-character revision")
    if not isinstance(observations["observer"], str) or OBSERVER.fullmatch(observations["observer"]) is None:
        raise RecordError("operator identifier is invalid")
    now = datetime.now(timezone.utc)
    checks = validate_observations(observations["observations"], bundle["captured"], now)
    accepted = (
        bundle["collection_verdict"] == "complete"
        and bundle["camera_proof"] is not None
        and all(item["outcome"] == "pass" for item in checks)
    )
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": RECORD_KIND,
        "recorded_at_utc": now.isoformat(timespec="seconds").replace("+00:00", "Z"),
        "seat_label": bundle["seat"],
        "surface": {"generation": bundle["generation"], "model": bundle["model"], "sku": bundle["sku"]},
        "revision": observations["revision"],
        "revision_provenance": "operator-declared exact deployment revision",
        "observer": observations["observer"],
        "collector_bundle": {
            **bundle["binding"],
            "captured_at_utc": bundle["manifest"]["captured_at_utc"],
            "collection_verdict": bundle["collection_verdict"],
            "incomplete_probes": bundle["incomplete_probes"],
        },
        "camera_proof": bundle["camera_proof"],
        "prior_pro6_record_sha256": prior_sha256,
        "observations": checks,
        "record_status": "complete",
        "acceptance_verdict": "accepted" if accepted else "not-accepted",
        "limitations": sorted({text for item in checks for text in item["limitations"]}),
        "hardware_mutated_by_recorder": False,
    }


def validate_record(record_path: Path, bundle_path: Path, prior_path: Path | None = None) -> dict[str, Any]:
    record = load_json(record_path)
    bundle = load_bundle(bundle_path)
    expected = {
        "schema_version", "kind", "recorded_at_utc", "seat_label", "surface", "revision", "revision_provenance",
        "observer", "collector_bundle", "prior_pro6_record_sha256", "observations",
        "camera_proof", "record_status", "acceptance_verdict", "limitations", "hardware_mutated_by_recorder",
    }
    if not isinstance(record, dict) or set(record) != expected or record.get("schema_version") != SCHEMA_VERSION or record.get("kind") != RECORD_KIND:
        raise RecordError("physical acceptance record schema is invalid")
    operator = {"schema_version": 1, "revision": record["revision"], "observer": record["observer"], "observations": record["observations"]}
    prior_sha = sha256_file(prior_path) if prior_path is not None else None
    rebuilt = build_record(bundle, operator, prior_sha)
    for key in expected - {"recorded_at_utc"}:
        if record.get(key) != rebuilt.get(key):
            raise RecordError(f"physical acceptance record binding mismatch: {key}")
    recorded = parse_time(record["recorded_at_utc"], "recorded_at_utc")
    latest_observation = max(parse_time(item["observed_at_utc"], "observed_at_utc") for item in record["observations"])
    if recorded < latest_observation or recorded > datetime.now(timezone.utc) + timedelta(minutes=5):
        raise RecordError("record timestamp predates an observation or is in the future")
    if bundle["generation"] == 6 and (prior_path is not None or record["prior_pro6_record_sha256"] is not None):
        raise RecordError("Pro 6 record cannot have a prior Pro 6 dependency")
    if bundle["generation"] == 5:
        if prior_path is None or record["prior_pro6_record_sha256"] != prior_sha:
            raise RecordError("Pro 5 record requires the exact prior Pro 6 record")
    return record


def validate_prior_pro6(record_path: Path, bundle_path: Path) -> str:
    prior = validate_record(record_path, bundle_path)
    if (
        prior["surface"]["generation"] != 6
        or prior["seat_label"] != "Surface"
        or prior["record_status"] != "complete"
        or prior["acceptance_verdict"] != "accepted"
    ):
        raise RecordError("prior record is not an accepted canonical Pro 6 acceptance record")
    return sha256_file(record_path)


def write_new(path: Path, value: dict[str, Any]) -> None:
    if path.exists() or path.is_symlink():
        raise RecordError("output already exists; recorder never overwrites evidence")
    parent = path.parent.resolve(strict=True)
    destination = parent / path.name
    if destination.exists() or destination.is_symlink():
        raise RecordError("resolved output already exists; recorder never overwrites evidence")
    encoded = (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n").encode("ascii")
    if len(encoded) > MAX_FILE_BYTES:
        raise RecordError("physical acceptance record exceeds the governed size")
    old_umask = os.umask(0o077)
    temporary: str | None = None
    try:
        descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.tmp-", dir=parent)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, destination)
        temporary = None
    finally:
        os.umask(old_umask)
        if temporary is not None:
            try: os.unlink(temporary)
            except FileNotFoundError: pass


def self_test() -> int:
    captured = datetime(2026, 8, 9, tzinfo=timezone.utc)
    now = datetime(2026, 8, 10, tzinfo=timezone.utc)
    rows = [{
        "check": check, "performed": True, "outcome": "pass",
        "observed_at_utc": "2026-08-09T01:00:00Z",
        "observation": f"Operator explicitly exercised {check}", "limitations": [],
    } for check in CHECKS]
    assert len(validate_observations(rows, captured, now)) == len(CHECKS)
    for hostile in (
        rows[:-1], rows + [rows[0]],
        [{**row, "outcome": "pass", "performed": False} if index == 0 else row for index, row in enumerate(rows)],
        [{**row, "outcome": "blocked", "performed": False, "limitations": []} if index == 0 else row for index, row in enumerate(rows)],
    ):
        try: validate_observations(hostile, captured, now)
        except RecordError: pass
        else: raise AssertionError("accepted incomplete, duplicate, or contradictory observations")
    try: json.loads('{"check":"touch","check":"pen"}', object_pairs_hook=strict_object)
    except RecordError: pass
    else: raise AssertionError("accepted duplicate JSON field")
    captured_ms = int(captured.timestamp() * 1000)
    proof = {
        "topic": "state/hardware/surface/surface-6/camera-proof",
        "node": "surface-6",
        "model": "Surface Pro 6",
        "generation": 6,
        "completed_at_ms": captured_ms - 1,
        "outcome": "passed",
        "result_sha256": "a" * 64,
        "frame_bytes_retained": False,
        "device_identifier_retained": False,
        "request_identifier_retained": False,
    }
    assert validate_camera_proof(proof, 6, captured) == proof
    for update in (
        {"model": "Surface Pro 5"},
        {"generation": 5},
        {"outcome": "failed"},
        {"result_sha256": "bad"},
        {"frame_bytes_retained": True},
        {"completed_at_ms": captured_ms - CAMERA_PROOF_MAX_AGE_MS - 1},
        {"completed_at_ms": captured_ms + CAMERA_PROOF_FUTURE_SKEW_MS + 1},
        {"device_id": "/dev/video0"},
    ):
        candidate = dict(proof)
        candidate.update(update)
        try: validate_camera_proof(candidate, 6, captured)
        except RecordError: pass
        else: raise AssertionError("accepted absent, stale, mismatched, or identifying camera proof")
    bound_proof = {**proof, "artifact_sha256": "b" * 64}
    bundle = {
        "captured": captured,
        "seat": "Surface",
        "generation": 6,
        "model": "Surface Pro 6",
        "sku": "Surface_Pro_6",
        "collection_verdict": "complete",
        "incomplete_probes": [],
        "manifest": {"captured_at_utc": "2026-08-09T00:00:00Z"},
        "binding": {
            "manifest_sha256": "c" * 64,
            "collector_sha256": "d" * 64,
            "magic_mesh_nevra": [{
                "name": "magic-mesh", "epoch": "0", "version": "1",
                "release": "1", "arch": "x86_64",
            }],
            "artifacts": [{"file": "camera-proof.json", "bytes": 1, "sha256": "b" * 64}],
        },
        "camera_proof": bound_proof,
    }
    accepted = build_record(
        bundle,
        {
            "schema_version": 1,
            "revision": "a" * 40,
            "observer": "operator-test",
            "observations": rows,
        },
        None,
    )
    if accepted["acceptance_verdict"] != "accepted" or accepted["camera_proof"] != bound_proof:
        raise AssertionError("accepted record did not cryptographically bind the camera proof")
    print("record-surface-physical-acceptance: self-test passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    subparsers = parser.add_subparsers(dest="command")
    record_parser = subparsers.add_parser("record")
    record_parser.add_argument("--bundle", type=Path, required=True)
    record_parser.add_argument("--observations", type=Path, required=True)
    record_parser.add_argument("--out", type=Path, required=True)
    record_parser.add_argument("--prior-pro6-record", type=Path)
    record_parser.add_argument("--prior-pro6-bundle", type=Path)
    validate_parser = subparsers.add_parser("validate")
    validate_parser.add_argument("--bundle", type=Path, required=True)
    validate_parser.add_argument("--record", type=Path, required=True)
    validate_parser.add_argument("--prior-pro6-record", type=Path)
    validate_parser.add_argument("--prior-pro6-bundle", type=Path)
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if args.command not in {"record", "validate"}:
        parser.error("record or validate is required")
    bundle = load_bundle(args.bundle)
    prior_args = (args.prior_pro6_record, args.prior_pro6_bundle)
    if (prior_args[0] is None) != (prior_args[1] is None):
        raise RecordError("prior Pro 6 record and bundle must be supplied together")
    prior_sha = validate_prior_pro6(*prior_args) if prior_args[0] is not None else None
    if bundle["generation"] == 5 and prior_sha is None:
        raise RecordError("Pro 5 recording requires a completed prior Pro 6 record and bundle")
    if bundle["generation"] == 6 and prior_sha is not None:
        raise RecordError("Pro 6 recording cannot depend on a prior Pro 6 record")
    if args.command == "record":
        value = build_record(bundle, load_json(args.observations), prior_sha)
        write_new(args.out, value)
        print(f"physical acceptance record written: {args.out}")
        print(f"acceptance_verdict={value['acceptance_verdict']}")
        return 0 if value["acceptance_verdict"] == "accepted" else 3
    value = validate_record(args.record, args.bundle, args.prior_pro6_record)
    print(f"physical acceptance record valid: {args.record}")
    print(f"acceptance_verdict={value['acceptance_verdict']}")
    return 0 if value["acceptance_verdict"] == "accepted" else 3


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RecordError as exc:
        print(f"record-surface-physical-acceptance: {exc}", file=sys.stderr)
        raise SystemExit(2)
