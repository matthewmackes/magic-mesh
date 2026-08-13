#!/usr/bin/env python3
"""Produce or verify the governed Lighthouse RPM input for the Browser VM."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
APP_HELPER = ROOT / "packaging/app-vm/produce-rpm-candidate-manifest.py"
BUILD_IDENTITY_VERIFY = ROOT / "packaging/app-vm/verify-rpm-build-identity.py"
TARGET = "mcnf-browser-vm/browser-vm-chromium-v1"
VARIANT = "magic-mesh-lighthouse/thin-control-plane-v1"
KIND = "mcnf-browser-vm-lighthouse-rpm-candidate-manifest"
REVISION = re.compile(r"[0-9a-f]{40}\Z")
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
FINGERPRINT = re.compile(r"[0-9A-F]{40,64}\Z")
NEVRA = re.compile(r"magic-mesh-lighthouse-(?:[0-9]+:)?[A-Za-z0-9][A-Za-z0-9._+~:-]*-[A-Za-z0-9][A-Za-z0-9._+~:-]*\.x86_64\Z")


def load_shared():
    spec = importlib.util.spec_from_file_location("mcnf_app_rpm_candidate", APP_HELPER)
    if spec is None or spec.loader is None:
        raise RuntimeError("shared RPM candidate helper cannot be loaded")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SHARED = load_shared()
Refusal = SHARED.Refusal


def exact_json(path: Path) -> dict[str, object]:
    if path.is_symlink() or not path.is_file():
        raise Refusal("candidate manifest must be a regular non-symlink file")
    metadata = os.stat(path, follow_symlinks=False)
    if metadata.st_size <= 0 or metadata.st_size > 1024 * 1024 or metadata.st_mode & 0o022:
        raise Refusal("candidate manifest must be bounded and not group/other writable")

    def object_hook(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise Refusal(f"candidate manifest contains duplicate field {key}")
            value[key] = item
        return value

    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=object_hook)
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise Refusal(f"candidate manifest cannot be read exactly: {exc}") from exc
    if not isinstance(value, dict):
        raise Refusal("candidate manifest must be one JSON object")
    return value


def rpm_identity(path: Path, rpm_tool: str) -> tuple[str, str, str]:
    output = SHARED.run(
        [rpm_tool, "-qp", "--qf", "%{NAME}\t%{EPOCHNUM}\t%{VERSION}\t%{RELEASE}\t%{ARCH}\n%{PAYLOADDIGESTALGO}\t%{PAYLOADDIGEST}\n", "--", str(path)],
        "Lighthouse RPM identity inspection",
    )
    lines = output.splitlines()
    if len(lines) != 2 or len(lines[0].split("\t")) != 5 or len(lines[1].split("\t")) != 2:
        raise Refusal("Lighthouse RPM identity metadata is incomplete or ambiguous")
    name, epoch, version, release, architecture = lines[0].split("\t")
    algorithm, payload = lines[1].split("\t")
    if name != "magic-mesh-lighthouse" or architecture != "x86_64":
        raise Refusal("RPM is not the exact x86_64 Lighthouse variant")
    if not epoch.isdigit() or not version or not release:
        raise Refusal("Lighthouse RPM NEVRA is malformed")
    if algorithm.upper() not in {"8", "SHA256"}:
        raise Refusal("Lighthouse RPM payload digest is not SHA-256")
    payload = payload.lower()
    if DIGEST.fullmatch(payload) is None or payload == "0" * 64:
        raise Refusal("Lighthouse RPM payload digest is malformed")
    prefix = "" if epoch == "0" else f"{epoch}:"
    nevra = f"{name}-{prefix}{version}-{release}.{architecture}"
    if NEVRA.fullmatch(nevra) is None:
        raise Refusal("Lighthouse RPM NEVRA is not canonical")
    return nevra, payload, version


def attest_build_identity(rpm: Path, revision: str, version: str, rpm2cpio: str, cpio: str) -> None:
    member = "./usr/bin/mackesd"
    first = subprocess.Popen([rpm2cpio, str(rpm)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert first.stdout is not None
    second = subprocess.Popen(
        [cpio, "-i", "--quiet", "--to-stdout", "--", member],
        stdin=first.stdout, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    first.stdout.close()
    assert second.stdout is not None
    identity = subprocess.run(
        [str(BUILD_IDENTITY_VERIFY), "--source-commit", revision,
         "--package-version", version, "--member", member],
        stdin=second.stdout, capture_output=True, check=False,
    )
    second.stdout.close()
    second_stderr = second.communicate()[1]
    first_stderr = first.communicate()[1]
    if first.returncode != 0 or second.returncode != 0 or identity.returncode != 0:
        detail = (identity.stderr or second_stderr or first_stderr).decode(errors="replace").strip()
        raise Refusal(f"installed mackesd does not carry exactly the requested source revision: {detail}")


def validate_manifest(value: dict[str, object], revision: str) -> tuple[dict[str, str], str]:
    expected = {"artifact", "browser_target_identity", "build_identity", "kind", "lighthouse_variant_identity", "schema_version", "signing_fingerprint"}
    if set(value) != expected or value.get("schema_version") != 1 or value.get("kind") != KIND:
        raise Refusal("Lighthouse candidate manifest identity or fields are not exact")
    if value.get("browser_target_identity") != TARGET or value.get("lighthouse_variant_identity") != VARIANT:
        raise Refusal("candidate does not bind the immutable Browser/Lighthouse variant boundary")
    build = value.get("build_identity")
    artifact = value.get("artifact")
    if not isinstance(build, dict) or set(build) != {"source_revision"} or build.get("source_revision") != revision:
        raise Refusal("candidate source revision does not match the requested Browser build")
    if not isinstance(artifact, dict) or set(artifact) != {"nevra", "payload_sha256", "rpm_sha256"}:
        raise Refusal("candidate artifact identity fields are not exact")
    typed = {key: item for key, item in artifact.items() if isinstance(item, str)}
    if len(typed) != 3 or NEVRA.fullmatch(typed["nevra"]) is None:
        raise Refusal("candidate Lighthouse NEVRA is malformed")
    for field in ("payload_sha256", "rpm_sha256"):
        if DIGEST.fullmatch(typed[field]) is None or typed[field] == "0" * 64:
            raise Refusal(f"candidate {field} is malformed")
    signer = value.get("signing_fingerprint")
    if not isinstance(signer, str) or FINGERPRINT.fullmatch(signer) is None:
        raise Refusal("candidate signing fingerprint is malformed")
    return typed, signer


def verify(args: argparse.Namespace, rpm: Path, manifest: Path) -> dict[str, object]:
    revision = args.source_revision
    if REVISION.fullmatch(revision) is None or revision == "0" * 40:
        raise Refusal("source revision must be one non-null lowercase Git object ID")
    if args.release_key.is_symlink() or not args.release_key.is_file():
        raise Refusal("governed release public key must be a regular non-symlink file")
    key = args.release_key.resolve(strict=True)
    if key.stat().st_mode & 0o022:
        raise Refusal("governed release public key must not be group/other writable")
    value = exact_json(manifest)
    artifact, expected_signer = validate_manifest(value, revision)
    with tempfile.TemporaryDirectory(prefix="browser-lighthouse-verify-") as raw:
        snapshot, whole = SHARED.snapshot_rpm(rpm, Path(raw))
        signer = SHARED.verify_signature(snapshot, key, args.gpg, args.rpm_tool, args.rpmkeys)
        nevra, payload, version = rpm_identity(snapshot, args.rpm_tool)
        if (whole, signer, nevra, payload) != (artifact["rpm_sha256"], expected_signer, artifact["nevra"], artifact["payload_sha256"]):
            raise Refusal("signed Lighthouse RPM bytes or governed identity do not match the manifest")
        attest_build_identity(snapshot, revision, version, args.rpm2cpio, args.cpio)
    return value


def produce(args: argparse.Namespace) -> dict[str, object]:
    revision = args.source_revision
    if REVISION.fullmatch(revision) is None or revision == "0" * 40:
        raise Refusal("source revision must be one non-null lowercase Git object ID")
    if args.release_key.is_symlink() or not args.release_key.is_file():
        raise Refusal("governed release public key must be a regular non-symlink file")
    key = args.release_key.resolve(strict=True)
    if key.stat().st_mode & 0o022:
        raise Refusal("governed release public key must not be group/other writable")
    parent = args.output.parent.resolve(strict=True)
    if parent.stat().st_mode & 0o022 or args.output.exists() or args.output.is_symlink():
        raise Refusal("output authority is writable, substituted, or already exists")
    with tempfile.TemporaryDirectory(prefix="browser-lighthouse-candidate-") as raw:
        snapshot, whole = SHARED.snapshot_rpm(args.rpm, Path(raw))
        signer = SHARED.verify_signature(snapshot, key, args.gpg, args.rpm_tool, args.rpmkeys)
        nevra, payload, version = rpm_identity(snapshot, args.rpm_tool)
        attest_build_identity(snapshot, revision, version, args.rpm2cpio, args.cpio)
        value = {
            "artifact": {"nevra": nevra, "payload_sha256": payload, "rpm_sha256": whole},
            "browser_target_identity": TARGET,
            "build_identity": {"source_revision": revision},
            "kind": KIND,
            "lighthouse_variant_identity": VARIANT,
            "schema_version": 1,
            "signing_fingerprint": signer,
        }
        stage = Path(tempfile.mkdtemp(prefix=f".{args.output.name}.", dir=parent))
        try:
            stage.chmod(0o700)
            manifest = stage / "candidate-manifest.json"
            descriptor = os.open(manifest, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
            try:
                SHARED.write_all(descriptor, SHARED.canonical_json(value), "Lighthouse candidate manifest")
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
            verify(args, snapshot, manifest)
            SHARED.publish_noreplace(stage, args.output)
            stage = None
        finally:
            if stage is not None and stage.exists():
                import shutil
                shutil.rmtree(stage)
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("produce", "verify"))
    parser.add_argument("--rpm", required=True, type=Path)
    parser.add_argument("--source-revision", required=True)
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
            raise Refusal("verify requires --manifest and forbids --output")
        value = verify(args, args.rpm, args.manifest)
    print(json.dumps(value, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, Refusal, RuntimeError) as exc:
        print(f"REFUSED[WL-ARCH-008/lighthouse-rpm-candidate]: {exc}", file=sys.stderr)
        raise SystemExit(2)
