#!/usr/bin/python3
"""Fail-closed Surface acceptance promotion verifier for CRIT-006 input."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any


SCHEMA_VERSION = 1
KIND = "mcnf-surface-acceptance-promotion-input"
MAX_FILE_BYTES = 512 * 1024
MAX_RECORD_AGE = timedelta(days=7)
MAX_PREFLIGHT_AGE = timedelta(hours=24)
CAMERA_PROOF_MAX_AGE_MS = 90_000
CAMERA_PROOF_FUTURE_SKEW_MS = 5_000
REVISION = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
STACK_PACKAGES = {
    "kernel-surface", "iptsd", "libwacom-surface", "surface-control", "surface-secureboot"
}
REQUIRED_EVIDENCE = {
    "audio-microphone": (
        ("speaker",), ("microphone",), ("headphone",), ("bluetooth",),
    ),
    "suspend-s0ix": (
        ("suspend",), ("resume",), ("s0ix",), ("wi-fi", "wifi"), ("bluetooth",), ("mesh",),
    ),
    "reboot-upgrade": (
        ("cold boot",), ("reboot",), ("upgrade",), ("rollback",), ("secure boot", "secure-boot"),
    ),
}
REQUIRED_COLLECTOR_ARTIFACTS = {
    "audio.json", "power.json", "radios.json", "services.json", "camera-proof.json"
}


class PromotionError(RuntimeError):
    pass


def strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise PromotionError(f"duplicate JSON field: {key}")
        value[key] = item
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_json(path: Path) -> Any:
    try:
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode) or not (0 < info.st_size <= MAX_FILE_BYTES):
            raise PromotionError(f"input is not a bounded regular file: {path.name}")
        with path.open("rb") as stream:
            data = stream.read(MAX_FILE_BYTES + 1)
        if len(data) > MAX_FILE_BYTES:
            raise PromotionError(f"input exceeds the governed size: {path.name}")
        return json.loads(data, object_pairs_hook=strict_object)
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as exc:
        raise PromotionError(f"invalid bounded JSON: {path.name}") from exc


def parse_time(value: Any, field: str) -> datetime:
    if not isinstance(value, str) or re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z", value) is None:
        raise PromotionError(f"{field} is not an exact UTC seconds timestamp")
    try:
        return datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as exc:
        raise PromotionError(f"{field} is not a real timestamp") from exc


def require_fresh(value: datetime, now: datetime, maximum: timedelta, field: str) -> None:
    if value > now + timedelta(minutes=5) or now - value > maximum:
        raise PromotionError(f"{field} is stale or future-dated")


def run_verifier(argv: list[str], timeout: int = 120) -> None:
    try:
        result = subprocess.run(
            argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, env={"PATH": "/usr/sbin:/usr/bin", "LANG": "C", "LC_ALL": "C"},
            timeout=timeout, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise PromotionError(f"governed verifier unavailable or timed out: {Path(argv[0]).name}") from exc
    if result.returncode != 0:
        raise PromotionError(f"governed verifier refused input: {Path(argv[0]).name}")


def validate_preflight(path: Path, manifest_sha256: str, revision: str, now: datetime) -> dict[str, Any]:
    value = load_json(path)
    expected = {
        "schema_version", "kind", "verdict", "read_only", "target", "local_revision",
        "artifact_manifest", "collector_and_physical_proof", "access",
        "physical_proof_performed", "blockers",
    }
    if not isinstance(value, dict) or set(value) != expected or value.get("schema_version") != 1:
        raise PromotionError("deployment preflight schema is invalid")
    if value.get("kind") != "mcnf-surface-pro56-deployment-preflight" or value.get("verdict") != "ready" or value.get("read_only") is not True or value.get("blockers") != []:
        raise PromotionError("deployment preflight is not a governed ready result")
    target = value.get("target")
    if target != {"seat": "Surface", "expected_generation": 6, "addresses_redacted": True}:
        raise PromotionError("deployment preflight is not for the canonical Pro 6 seat")
    local = value.get("local_revision")
    if not isinstance(local, dict) or local.get("status") != "ready" or local.get("tracked_checkout_clean") is not True or local.get("revision") != revision:
        raise PromotionError("deployment preflight revision is dirty, unavailable, or mismatched")
    artifacts = value.get("artifact_manifest")
    if not isinstance(artifacts, dict) or artifacts.get("status") != "ready" or artifacts.get("manifest_status") != "ready" or artifacts.get("signature_verification") != "passed" or artifacts.get("manifest_sha256") != manifest_sha256:
        raise PromotionError("deployment preflight does not bind the ready signed-stack manifest")
    collector = value.get("collector_and_physical_proof")
    if not isinstance(collector, dict) or collector.get("status") != "ready" or collector.get("physical_acceptance_claimed") is not False:
        raise PromotionError("deployment preflight collector contract is unavailable or makes a manual claim")
    access = value.get("access")
    if not isinstance(access, list) or not any(isinstance(row, dict) and isinstance(row.get("remote"), dict) and row["remote"].get("status") == "ready" and row["remote"].get("revision_matches_local") is True and row["remote"].get("collector_hash_matches_local") is True for row in access):
        raise PromotionError("deployment preflight has no exact admitted remote revision/collector binding")
    mtime = datetime.fromtimestamp(path.stat().st_mtime, tz=timezone.utc)
    require_fresh(mtime, now, MAX_PREFLIGHT_AGE, "deployment preflight")
    return {"sha256": sha256_file(path), "mtime_utc": mtime.isoformat(timespec="seconds").replace("+00:00", "Z")}


def stack_identity(manifest: Path) -> dict[str, Any]:
    value = load_json(manifest)
    if not isinstance(value, dict) or value.get("schema_version") != 2 or value.get("kind") != "mcnf-surface-stack-provenance" or value.get("status") != "ready" or value.get("blockers") != []:
        raise PromotionError("Surface signed-stack candidate is not ready")
    packages = value.get("packages")
    if not isinstance(packages, list) or {row.get("name") for row in packages if isinstance(row, dict)} != STACK_PACKAGES:
        raise PromotionError("Surface signed-stack package set is incomplete or foreign")
    result = {}
    for row in packages:
        if row.get("availability") != "ready" or row.get("blocker") is not None or not isinstance(row.get("rpm"), dict):
            raise PromotionError("Surface signed-stack package row is not ready")
        rpm = row["rpm"]
        if not isinstance(rpm.get("nevra"), str) or not isinstance(rpm.get("sha256"), str) or SHA256.fullmatch(rpm["sha256"]) is None:
            raise PromotionError("Surface signed-stack package identity is incomplete")
        result[row["name"]] = {"nevra": rpm["nevra"], "sha256": rpm["sha256"], "signing_fingerprint": rpm.get("signing_fingerprint")}
    return {"manifest_sha256": sha256_file(manifest), "packages": result, "signing_key": value.get("signing_key"), "target": value.get("target")}


def bundle_package_identity(bundle: Path) -> tuple[dict[str, str], dict[str, str]]:
    manifest = load_json(bundle / "manifest.json")
    release = load_json(bundle / "release-packages.json")
    if not isinstance(manifest, dict) or not isinstance(release, dict) or release.get("status") != "ok":
        raise PromotionError("collector bundle package inventory is incomplete")
    packages = release.get("data", {}).get("packages", [])
    stack: dict[str, str] = {}
    magic_mesh: dict[str, str] | None = None
    for row in packages if isinstance(packages, list) else []:
        if not isinstance(row, dict) or row.get("status") != "installed" or not isinstance(row.get("nevra"), list) or len(row["nevra"]) != 1:
            continue
        item = row["nevra"][0]
        if not isinstance(item, dict) or set(item) != {"name", "epoch", "version", "release", "arch"}:
            continue
        epoch = "" if item["epoch"] in {"0", "(none)"} else f"{item['epoch']}:"
        nevra = f"{item['name']}-{epoch}{item['version']}-{item['release']}.{item['arch']}"
        if item["name"] in STACK_PACKAGES:
            stack[item["name"]] = nevra
        elif item["name"] == "magic-mesh":
            magic_mesh = {"nevra": nevra}
    if set(stack) != STACK_PACKAGES or magic_mesh is None:
        raise PromotionError("deployed Surface or magic-mesh package identity is incomplete")
    return stack, magic_mesh


def validate_required_evidence(record: dict[str, Any], bundle: Path) -> dict[str, str]:
    observations = record.get("observations")
    if not isinstance(observations, list):
        raise PromotionError("physical record observations are unavailable")
    by_check = {row.get("check"): row for row in observations if isinstance(row, dict)}
    evidence_hashes = {}
    for check, term_groups in REQUIRED_EVIDENCE.items():
        row = by_check.get(check)
        if not isinstance(row, dict) or row.get("performed") is not True or row.get("outcome") != "pass" or row.get("limitations") != []:
            raise PromotionError(f"required physical evidence did not pass: {check}")
        text = row.get("observation")
        if not isinstance(text, str):
            raise PromotionError(f"required physical evidence text is unavailable: {check}")
        validate_evidence_text(check, text, term_groups)
        evidence_hashes[check] = hashlib.sha256(json.dumps(row, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    artifact_rows = record.get("collector_bundle", {}).get("artifacts", [])
    bound = {row.get("file"): row.get("sha256") for row in artifact_rows if isinstance(row, dict)}
    manifest = load_json(bundle / "manifest.json")
    statuses = {row.get("file"): row.get("status") for row in manifest.get("artifacts", []) if isinstance(row, dict)}
    for name in REQUIRED_COLLECTOR_ARTIFACTS:
        actual = sha256_file(bundle / name)
        if statuses.get(name) != "ok" or bound.get(name) != actual:
            raise PromotionError(f"required collector evidence is incomplete or hash-mismatched: {name}")
        evidence_hashes[name] = actual
    return evidence_hashes


def validate_evidence_text(
    check: str, text: str, term_groups: tuple[tuple[str, ...], ...]
) -> None:
    normalized = text.lower().replace("–", "-").replace("—", "-")
    if any(not any(term in normalized for term in alternatives) for alternatives in term_groups):
        raise PromotionError(f"required physical evidence is incomplete: {check}")


def validate_camera_proof_binding(
    record: dict[str, Any], bundle: Path, generation: int, captured: datetime
) -> dict[str, Any]:
    artifact = load_json(bundle / "camera-proof.json")
    if not isinstance(artifact, dict) or set(artifact) != {"schema_version", "status", "data"}:
        raise PromotionError("camera proof artifact schema is invalid")
    if artifact.get("schema_version") != 1 or artifact.get("status") != "ok":
        raise PromotionError("camera proof artifact is not a successful observation")
    proof = artifact.get("data")
    required = {
        "topic", "node", "model", "generation", "completed_at_ms", "outcome",
        "result_sha256", "frame_bytes_retained", "device_identifier_retained",
        "request_identifier_retained",
    }
    expected_model = "Surface Pro 6" if generation == 6 else "Surface Pro 5"
    if not isinstance(proof, dict) or set(proof) != required:
        raise PromotionError("camera proof projection schema is invalid")
    node = proof.get("node")
    if (
        not isinstance(node, str)
        or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}", node) is None
        or proof.get("topic") != f"state/hardware/surface/{node}/camera-proof"
        or proof.get("model") != expected_model
        or proof.get("generation") != generation
        or proof.get("outcome") != "passed"
        or not isinstance(proof.get("result_sha256"), str)
        or SHA256.fullmatch(proof["result_sha256"]) is None
        or proof.get("frame_bytes_retained") is not False
        or proof.get("device_identifier_retained") is not False
        or proof.get("request_identifier_retained") is not False
    ):
        raise PromotionError("camera proof identity, outcome, or privacy binding is invalid")
    completed = proof.get("completed_at_ms")
    captured_ms = int(captured.timestamp() * 1000)
    if not isinstance(completed, int) or isinstance(completed, bool) or completed <= 0:
        raise PromotionError("camera proof completion timestamp is invalid")
    if completed > captured_ms + CAMERA_PROOF_FUTURE_SKEW_MS or captured_ms - completed > CAMERA_PROOF_MAX_AGE_MS:
        raise PromotionError("camera proof was not fresh at collection")
    artifact_sha256 = sha256_file(bundle / "camera-proof.json")
    expected_record = {**proof, "artifact_sha256": artifact_sha256}
    if record.get("camera_proof") != expected_record:
        raise PromotionError("physical record does not hash-bind the exact camera proof")
    return {
        "artifact_sha256": artifact_sha256,
        "result_sha256": proof["result_sha256"],
        "node": node,
        "model": expected_model,
        "generation": generation,
        "completed_at_ms": completed,
        "outcome": "passed",
        "frame_bytes_retained": False,
        "device_identifier_retained": False,
        "request_identifier_retained": False,
    }


def validate_seat(
    record_path: Path, bundle: Path, generation: int, revision: str,
    stack_packages: dict[str, dict[str, Any]], now: datetime,
) -> dict[str, Any]:
    record = load_json(record_path)
    if not isinstance(record, dict) or record.get("kind") != "mcnf-surface-pro56-physical-acceptance" or record.get("record_status") != "complete" or record.get("acceptance_verdict") != "accepted" or record.get("hardware_mutated_by_recorder") is not False:
        raise PromotionError("physical record is not a governed accepted result")
    surface = record.get("surface")
    expected_model = "Surface Pro 6" if generation == 6 else "Surface Pro"
    if not isinstance(surface, dict) or surface.get("generation") != generation or surface.get("model") != expected_model:
        raise PromotionError("physical record has a foreign model or generation")
    if generation == 6 and (record.get("seat_label") != "Surface" or record.get("prior_pro6_record_sha256") is not None):
        raise PromotionError("canonical Pro 6 physical record binding is invalid")
    if generation == 5 and surface.get("sku") not in {"Surface_Pro_1796", "Surface_Pro_1807"}:
        raise PromotionError("Pro 5 physical record has a foreign SKU")
    if record.get("revision") != revision or record.get("revision_provenance") != "operator-declared exact deployment revision":
        raise PromotionError("physical record revision is missing or mismatched")
    recorded = parse_time(record.get("recorded_at_utc"), "physical recorded_at_utc")
    captured = parse_time(record.get("collector_bundle", {}).get("captured_at_utc"), "collector captured_at_utc")
    require_fresh(recorded, now, MAX_RECORD_AGE, "physical record")
    require_fresh(captured, now, MAX_RECORD_AGE, "collector evidence")
    if captured > recorded:
        raise PromotionError("physical and collector evidence timestamps are misordered")
    camera_proof = validate_camera_proof_binding(record, bundle, generation, captured)
    stack_deployed, magic_mesh = bundle_package_identity(bundle)
    expected_nevras = {name: row["nevra"] for name, row in stack_packages.items()}
    if stack_deployed != expected_nevras:
        raise PromotionError("deployed Surface package identity differs from signed candidate")
    record_nevra = record.get("collector_bundle", {}).get("magic_mesh_nevra")
    if not isinstance(record_nevra, list) or len(record_nevra) != 1 or not isinstance(record_nevra[0], dict) or set(record_nevra[0]) != {"name", "epoch", "version", "release", "arch"} or any(not isinstance(value, str) for value in record_nevra[0].values()):
        raise PromotionError("physical record lacks exact magic-mesh package identity")
    if record_nevra[0].get("name") != "magic-mesh" or magic_mesh["nevra"] != f"magic-mesh-{'' if record_nevra[0]['epoch'] in {'0', '(none)'} else record_nevra[0]['epoch'] + ':'}{record_nevra[0]['version']}-{record_nevra[0]['release']}.{record_nevra[0]['arch']}":
        raise PromotionError("physical record and collector magic-mesh package identities differ")
    if record.get("collector_bundle", {}).get("manifest_sha256") != sha256_file(bundle / "manifest.json"):
        raise PromotionError("physical record collector-manifest hash mismatches its bundle")
    return {
        "record_sha256": sha256_file(record_path),
        "bundle_manifest_sha256": sha256_file(bundle / "manifest.json"),
        "seat_label": record["seat_label"], "generation": generation,
        "model": surface["model"], "sku": surface.get("sku"),
        "revision": revision, "magic_mesh_nevra": magic_mesh["nevra"],
        "recorded_at_utc": record["recorded_at_utc"],
        "camera_proof": camera_proof,
        "required_evidence": validate_required_evidence(record, bundle),
    }


def write_new(path: Path, value: dict[str, Any]) -> None:
    if path.exists() or path.is_symlink():
        raise PromotionError("promotion output already exists; verifier never overwrites")
    encoded = (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n").encode("ascii")
    if len(encoded) > MAX_FILE_BYTES:
        raise PromotionError("promotion output exceeds the governed size")
    parent = path.parent.resolve(strict=True)
    destination = parent / path.name
    old_umask = os.umask(0o077)
    temporary: str | None = None
    try:
        descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.tmp-", dir=parent)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(encoded); stream.flush(); os.fsync(stream.fileno())
        os.chmod(temporary, 0o600)
        try:
            # Hard-link publication is an atomic no-clobber operation. Unlike
            # replace(), it cannot overwrite an output another verifier won
            # between the initial check and publication.
            os.link(temporary, destination)
        except FileExistsError as exc:
            raise PromotionError(
                "promotion output already exists; verifier never overwrites"
            ) from exc
        os.unlink(temporary); temporary = None
        directory = os.open(parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        os.umask(old_umask)
        if temporary is not None:
            try: os.unlink(temporary)
            except FileNotFoundError: pass


def self_test() -> int:
    now = datetime.now(timezone.utc)
    base = {
        "audio-microphone": "Speaker microphone headphone and Bluetooth audio all exercised",
        "suspend-s0ix": "Suspend resume S0ix Wi-Fi Bluetooth and mesh recovery all exercised",
        "reboot-upgrade": "Cold boot reboot upgrade rollback and secure-boot recovery all exercised",
    }
    observations = [{"check": check, "performed": True, "outcome": "pass", "limitations": [], "observation": text} for check, text in base.items()]
    record = {"observations": observations, "collector_bundle": {"artifacts": []}}
    for check, groups in REQUIRED_EVIDENCE.items():
        validate_evidence_text(check, base[check], groups)
    hostile = json.loads(json.dumps(record))
    hostile["observations"][1]["observation"] = "Suspend and resume only"
    try:
        validate_evidence_text("suspend-s0ix", hostile["observations"][1]["observation"], REQUIRED_EVIDENCE["suspend-s0ix"])
    except PromotionError: pass
    else: raise AssertionError("accepted incomplete suspend/network evidence")
    try: json.loads('{"status":"ready","status":"blocked"}', object_pairs_hook=strict_object)
    except PromotionError: pass
    else: raise AssertionError("accepted duplicate promotion input field")
    try: require_fresh(now - timedelta(days=8), now, MAX_RECORD_AGE, "test record")
    except PromotionError: pass
    else: raise AssertionError("accepted stale physical evidence")
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        captured_ms = int(now.timestamp() * 1000)
        proof = {
            "topic": "state/hardware/surface/surface-6/camera-proof",
            "node": "surface-6", "model": "Surface Pro 6", "generation": 6,
            "completed_at_ms": captured_ms - 1, "outcome": "passed",
            "result_sha256": "a" * 64, "frame_bytes_retained": False,
            "device_identifier_retained": False, "request_identifier_retained": False,
        }
        proof_path = root / "camera-proof.json"
        proof_path.write_text(json.dumps({"schema_version": 1, "status": "ok", "data": proof}) + "\n")
        camera_record = {"camera_proof": {**proof, "artifact_sha256": sha256_file(proof_path)}}
        assert validate_camera_proof_binding(camera_record, root, 6, now)["outcome"] == "passed"
        for hostile in (
            {},
            {"camera_proof": {**camera_record["camera_proof"], "result_sha256": "b" * 64}},
            {"camera_proof": {**camera_record["camera_proof"], "frame_bytes_retained": True}},
        ):
            try: validate_camera_proof_binding(hostile, root, 6, now)
            except PromotionError: pass
            else: raise AssertionError("accepted missing, mismatched, or identifying camera proof binding")
        stale = {**proof, "completed_at_ms": captured_ms - CAMERA_PROOF_MAX_AGE_MS - 1}
        proof_path.write_text(json.dumps({"schema_version": 1, "status": "ok", "data": stale}) + "\n")
        stale_record = {"camera_proof": {**stale, "artifact_sha256": sha256_file(proof_path)}}
        try: validate_camera_proof_binding(stale_record, root, 6, now)
        except PromotionError: pass
        else: raise AssertionError("accepted stale camera proof")
        output = Path(directory) / "promotion.json"
        write_new(output, {"verdict": "ready"})
        original = output.read_bytes()
        try: write_new(output, {"verdict": "replaced"})
        except PromotionError: pass
        else: raise AssertionError("overwrote an existing promotion output")
        if output.read_bytes() != original or stat.S_IMODE(output.stat().st_mode) != 0o600:
            raise AssertionError("promotion publication changed bytes or mode")
    print("verify-surface-acceptance-promotion: self-test passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--artifact-dir", type=Path)
    parser.add_argument("--preflight", type=Path)
    parser.add_argument("--pro6-record", type=Path)
    parser.add_argument("--pro6-bundle", type=Path)
    parser.add_argument("--pro5-record", type=Path)
    parser.add_argument("--pro5-bundle", type=Path)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    required = (args.manifest, args.artifact_dir, args.preflight, args.pro6_record, args.pro6_bundle, args.out)
    if any(value is None for value in required):
        parser.error("manifest, artifact-dir, preflight, Pro 6 record/bundle, and out are required")
    if (args.pro5_record is None) != (args.pro5_bundle is None):
        raise PromotionError("optional Pro 5 record and bundle must be supplied together")
    root = Path(__file__).resolve().parent.parent
    stack_verifier = root / "install-helpers/verify-surface-stack.sh"
    record_verifier = root / "install-helpers/record-surface-physical-acceptance.py"
    run_verifier([str(stack_verifier), "--manifest", str(args.manifest), "--artifact-dir", str(args.artifact_dir)])
    stack = stack_identity(args.manifest)
    pro6_record = load_json(args.pro6_record)
    revision = pro6_record.get("revision") if isinstance(pro6_record, dict) else None
    if not isinstance(revision, str) or REVISION.fullmatch(revision) is None:
        raise PromotionError("Pro 6 record lacks an exact revision")
    preflight = validate_preflight(args.preflight, stack["manifest_sha256"], revision, datetime.now(timezone.utc))
    run_verifier([str(record_verifier), "validate", "--bundle", str(args.pro6_bundle), "--record", str(args.pro6_record)])
    now = datetime.now(timezone.utc)
    pro6 = validate_seat(args.pro6_record, args.pro6_bundle, 6, revision, stack["packages"], now)
    pro5 = None
    if args.pro5_record is not None:
        run_verifier([
            str(record_verifier), "validate", "--bundle", str(args.pro5_bundle), "--record", str(args.pro5_record),
            "--prior-pro6-bundle", str(args.pro6_bundle), "--prior-pro6-record", str(args.pro6_record),
        ])
        pro5 = validate_seat(args.pro5_record, args.pro5_bundle, 5, revision, stack["packages"], now)
        if load_json(args.pro5_record).get("prior_pro6_record_sha256") != pro6["record_sha256"]:
            raise PromotionError("Pro 5 record does not hash-bind the accepted Pro 6 record")
        if pro5["magic_mesh_nevra"] != pro6["magic_mesh_nevra"]:
            raise PromotionError("Pro 5 and Pro 6 deployment package identities differ")
    output = {
        "schema_version": SCHEMA_VERSION, "kind": KIND, "verdict": "ready",
        "checked_at_utc": now.isoformat(timespec="seconds").replace("+00:00", "Z"),
        "signed_stack": stack, "deployment_preflight": preflight,
        "deployed_revision": revision, "canonical_pro6": pro6, "optional_pro5": pro5,
        "manual_override": False, "hardware_mutated_by_verifier": False,
        "promotion_scope": "Surface acceptance input for WL-CRIT-006; not a release promotion action",
    }
    write_new(args.out, output)
    print(f"Surface acceptance promotion input ready: {args.out}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PromotionError as exc:
        print(f"verify-surface-acceptance-promotion: {exc}", file=sys.stderr)
        raise SystemExit(2)
