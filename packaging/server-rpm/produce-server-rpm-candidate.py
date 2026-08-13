#!/usr/bin/env python3
"""Produce or reverify the governed headless Server RPM release candidate."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SHARED_PATH = ROOT / "packaging/app-vm/produce-rpm-candidate-manifest.py"
BUILD_IDENTITY_VERIFY = ROOT / "packaging/app-vm/verify-rpm-build-identity.py"
KIND = "mcnf-server-rpm-candidate-manifest"
ROLE = "server-rpm"
VARIANT = "magic-mesh-server/headless-workstation-v1"
REVISION = re.compile(r"[0-9a-f]{40}\Z")
FINGERPRINT = re.compile(r"[0-9A-F]{40,64}\Z")
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
NEVRA = re.compile(r"magic-mesh-server-(?:[0-9]+:)?[A-Za-z0-9][A-Za-z0-9._+~:-]*-[A-Za-z0-9][A-Za-z0-9._+~:-]*\.x86_64\Z")
MAX_MANIFEST = 1024 * 1024


def load_shared():
    spec = importlib.util.spec_from_file_location("mcnf_rpm_candidate_shared", SHARED_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("shared RPM candidate implementation cannot be loaded")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SHARED = load_shared()
Refusal = SHARED.Refusal


def exact_revision(value: str) -> str:
    if REVISION.fullmatch(value) is None or value == "0" * 40:
        raise Refusal("source revision must be one non-null lowercase Git object ID")
    return value


def exact_fingerprint(value: str) -> str:
    if FINGERPRINT.fullmatch(value) is None or set(value) == {"0"}:
        raise Refusal("expected signing fingerprint must be one non-null uppercase full fingerprint")
    return value


def immutable_regular(path: Path, label: str, maximum: int | None = None) -> os.stat_result:
    if path.is_symlink():
        raise Refusal(f"{label} must be a regular non-symlink file")
    try:
        metadata = os.stat(path, follow_symlinks=False)
    except OSError as exc:
        raise Refusal(f"{label} metadata is unavailable: {exc}") from exc
    if not path.is_file() or metadata.st_nlink != 1 or metadata.st_mode & 0o022:
        raise Refusal(f"{label} must be a single-link regular file not writable by group/other")
    if metadata.st_size <= 0 or (maximum is not None and metadata.st_size > maximum):
        raise Refusal(f"{label} violates its size bound")
    return metadata


def exact_json(path: Path) -> dict[str, object]:
    immutable_regular(path, "candidate manifest", MAX_MANIFEST)

    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise Refusal(f"candidate manifest contains duplicate field {key}")
            result[key] = value
        return result

    try:
        value = json.loads(
            path.read_text(encoding="utf-8"), object_pairs_hook=unique,
            parse_constant=lambda item: (_ for _ in ()).throw(Refusal(f"invalid JSON constant {item}")),
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise Refusal(f"candidate manifest is malformed: {exc}") from exc
    if not isinstance(value, dict):
        raise Refusal("candidate manifest must be exactly one JSON object")
    return value


def rpm_identity(path: Path, rpm_tool: str) -> tuple[str, str, str]:
    output = SHARED.run(
        [rpm_tool, "-qp", "--qf", "%{NAME}\t%{EPOCHNUM}\t%{VERSION}\t%{RELEASE}\t%{ARCH}\n%{PAYLOADDIGESTALGO}\t%{PAYLOADDIGEST}\n", "--", str(path)],
        "Server RPM identity inspection",
    )
    lines = output.splitlines()
    if len(lines) != 2 or len(lines[0].split("\t")) != 5 or len(lines[1].split("\t")) != 2:
        raise Refusal("Server RPM identity metadata is incomplete or ambiguous")
    name, epoch, version, release, architecture = lines[0].split("\t")
    algorithm, payload = lines[1].split("\t")
    if name != "magic-mesh-server" or architecture != "x86_64":
        raise Refusal("RPM is not the exact x86_64 magic-mesh-server variant")
    if not epoch.isdigit() or not version or not release or algorithm.upper() not in {"8", "SHA256"}:
        raise Refusal("Server RPM NEVRA or payload digest algorithm is unsupported")
    payload = payload.lower()
    if DIGEST.fullmatch(payload) is None or payload == "0" * 64:
        raise Refusal("Server RPM payload digest is malformed")
    prefix = "" if epoch == "0" else f"{epoch}:"
    nevra = f"{name}-{prefix}{version}-{release}.{architecture}"
    if NEVRA.fullmatch(nevra) is None:
        raise Refusal("Server RPM NEVRA is not canonical")
    return nevra, payload, version


def attest_build_identity(rpm: Path, revision: str, version: str, rpm2cpio: str, cpio: str) -> None:
    member = "./usr/bin/mackesd"
    archive = subprocess.Popen([rpm2cpio, str(rpm)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert archive.stdout is not None
    extract = subprocess.Popen(
        [cpio, "-i", "--quiet", "--to-stdout", "--", member], stdin=archive.stdout,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    archive.stdout.close()
    assert extract.stdout is not None
    identity = subprocess.run(
        [str(BUILD_IDENTITY_VERIFY), "--source-commit", revision, "--package-version", version, "--member", member],
        stdin=extract.stdout, capture_output=True, check=False,
    )
    extract.stdout.close()
    extract_error = extract.communicate()[1]
    archive_error = archive.communicate()[1]
    if archive.returncode or extract.returncode or identity.returncode:
        detail = (identity.stderr or extract_error or archive_error).decode(errors="replace").strip()
        raise Refusal(f"Server mackesd does not carry exactly the requested source revision: {detail}")


def validate_manifest(value: dict[str, object], revision: str, signer: str) -> dict[str, str]:
    fields = {"artifact", "build_identity", "kind", "release_role", "schema_version", "server_variant_identity", "signing_fingerprint"}
    if set(value) != fields or value.get("schema_version") != 1 or value.get("kind") != KIND:
        raise Refusal("Server candidate manifest identity or fields are not exact")
    if value.get("release_role") != ROLE or value.get("server_variant_identity") != VARIANT:
        raise Refusal("candidate does not bind the governed Server package role and variant")
    build = value.get("build_identity")
    if not isinstance(build, dict) or set(build) != {"source_revision"} or build.get("source_revision") != revision:
        raise Refusal("candidate source revision differs from the requested release revision")
    if value.get("signing_fingerprint") != signer:
        raise Refusal("candidate signer differs from the explicitly expected signing fingerprint")
    artifact = value.get("artifact")
    if not isinstance(artifact, dict) or set(artifact) != {"nevra", "payload_sha256", "rpm_sha256"}:
        raise Refusal("candidate artifact identity fields are not exact")
    typed = {key: item for key, item in artifact.items() if isinstance(item, str)}
    if len(typed) != 3 or NEVRA.fullmatch(typed["nevra"]) is None:
        raise Refusal("candidate Server NEVRA is malformed")
    for field in ("payload_sha256", "rpm_sha256"):
        if DIGEST.fullmatch(typed[field]) is None or typed[field] == "0" * 64:
            raise Refusal(f"candidate {field} is malformed")
    return typed


def reverify(args: argparse.Namespace, rpm: Path, manifest: Path) -> dict[str, object]:
    revision = exact_revision(args.source_revision)
    expected_signer = exact_fingerprint(args.expected_signing_fingerprint)
    immutable_regular(rpm, "Server RPM")
    immutable_regular(args.release_key, "governed release public key", MAX_MANIFEST)
    value = exact_json(manifest)
    artifact = validate_manifest(value, revision, expected_signer)
    with tempfile.TemporaryDirectory(prefix="server-rpm-reverify-") as raw:
        snapshot, whole = SHARED.snapshot_rpm(rpm, Path(raw))
        actual_signer = SHARED.verify_signature(snapshot, args.release_key, args.gpg, args.rpm_tool, args.rpmkeys)
        nevra, payload, version = rpm_identity(snapshot, args.rpm_tool)
        if actual_signer != expected_signer:
            raise Refusal("RPM signer differs from the explicitly expected signing fingerprint")
        if (whole, nevra, payload) != (artifact["rpm_sha256"], artifact["nevra"], artifact["payload_sha256"]):
            raise Refusal("Server RPM bytes or package identity differ from the companion manifest")
        attest_build_identity(snapshot, revision, version, args.rpm2cpio, args.cpio)
    return value


def produce(args: argparse.Namespace) -> dict[str, object]:
    revision = exact_revision(args.source_revision)
    expected_signer = exact_fingerprint(args.expected_signing_fingerprint)
    immutable_regular(args.rpm, "Server RPM")
    immutable_regular(args.release_key, "governed release public key", MAX_MANIFEST)
    if args.output.exists() or args.output.is_symlink() or args.output.parent.is_symlink():
        raise Refusal("output already exists or has substituted authority")
    parent = args.output.parent.resolve(strict=True)
    if parent.stat().st_mode & 0o022:
        raise Refusal("output parent must not be group/other writable")
    with tempfile.TemporaryDirectory(prefix="server-rpm-source-") as raw:
        snapshot, whole = SHARED.snapshot_rpm(args.rpm, Path(raw))
        signer = SHARED.verify_signature(snapshot, args.release_key, args.gpg, args.rpm_tool, args.rpmkeys)
        if signer != expected_signer:
            raise Refusal("RPM signer differs from the explicitly expected signing fingerprint")
        nevra, payload, version = rpm_identity(snapshot, args.rpm_tool)
        attest_build_identity(snapshot, revision, version, args.rpm2cpio, args.cpio)
        value = {
            "artifact": {"nevra": nevra, "payload_sha256": payload, "rpm_sha256": whole},
            "build_identity": {"source_revision": revision}, "kind": KIND,
            "release_role": ROLE, "schema_version": 1, "server_variant_identity": VARIANT,
            "signing_fingerprint": signer,
        }
        stage = Path(tempfile.mkdtemp(prefix=f".{args.output.name}.", dir=parent))
        try:
            stage.chmod(0o700)
            candidate = stage / "candidate.rpm"
            shutil.copyfile(snapshot, candidate)
            candidate.chmod(0o400)
            manifest = stage / "candidate-manifest.json"
            descriptor = os.open(manifest, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
            try:
                SHARED.write_all(descriptor, SHARED.canonical_json(value), "Server candidate manifest")
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
            reverify(args, candidate, manifest)
            SHARED.publish_noreplace(stage, args.output)
            stage = None
        finally:
            if stage is not None and stage.exists():
                shutil.rmtree(stage)
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("produce", "reverify"))
    parser.add_argument("--rpm", required=True, type=Path)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--expected-signing-fingerprint", required=True)
    parser.add_argument("--release-key", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--gpg", default="gpg", help=argparse.SUPPRESS)
    parser.add_argument("--rpm-tool", default="rpm", help=argparse.SUPPRESS)
    parser.add_argument("--rpmkeys", default="rpmkeys", help=argparse.SUPPRESS)
    parser.add_argument("--rpm2cpio", default="rpm2cpio", help=argparse.SUPPRESS)
    parser.add_argument("--cpio", default="cpio", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.mode == "produce":
        if args.output is None or args.manifest is not None:
            raise Refusal("produce requires --output and forbids --manifest")
        value = produce(args)
    else:
        if args.manifest is None or args.output is not None:
            raise Refusal("reverify requires --manifest and forbids --output")
        value = reverify(args, args.rpm, args.manifest)
    print(json.dumps(value, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, Refusal, RuntimeError) as exc:
        print(f"REFUSED[WL-CRIT-006/server-rpm-candidate]: {exc}", file=sys.stderr)
        raise SystemExit(2)
