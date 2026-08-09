#!/usr/bin/env python3
"""Emit a six-node candidate manifest and source receipt from final RPM bytes."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import tempfile
from typing import Any


REVISION_RE = re.compile(r"[0-9a-f]{40}")
DIGEST_RE = re.compile(r"[0-9a-f]{64}")
ROLES = ("lighthouse", "workstation")
ROLE_PACKAGE = {
    "lighthouse": "magic-mesh-lighthouse",
    "workstation": "magic-mesh",
}
ROLE_BINARIES = {
    "lighthouse": {"mackesd": "./usr/bin/mackesd"},
    "workstation": {
        "mackesd": "./usr/bin/mackesd",
        "mde-shell-egui": "./usr/bin/mde-shell-egui",
    },
}
REQUIRED_PAYLOAD = {
    "lighthouse": {
        "/usr/bin/mackesd",
        "/usr/libexec/mackesd/provision-resource-publisher-credential",
        "/usr/libexec/mackesd/resource-publisher-hmac.conf",
        "/usr/lib/systemd/system/mcnf-resource-publisher-credential.service",
    },
    "workstation": {
        "/usr/bin/mackesd",
        "/usr/bin/mde-shell-egui",
        "/usr/libexec/mackesd/provision-resource-publisher-credential",
        "/usr/libexec/mackesd/resource-publisher-hmac.conf",
        "/usr/lib/systemd/system/mcnf-resource-publisher-credential.service",
    },
}


class CandidateError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise CandidateError(message)


def run(argv: list[str], *, input_bytes: bytes | None = None) -> bytes:
    try:
        completed = subprocess.run(
            argv,
            input=input_bytes,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except OSError as exc:
        fail(f"could not execute {argv[0]}: {exc}")
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        fail(f"{' '.join(argv)} failed: {detail or completed.returncode}")
    return completed.stdout


@dataclass(frozen=True)
class RpmSnapshot:
    path: Path
    original_name: str
    sha256: str
    size_bytes: int


def snapshot_rpm(path: Path, label: str, directory: Path) -> RpmSnapshot:
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    except OSError as exc:
        fail(f"{label} is unavailable: {exc}")
    temporary_descriptor = -1
    temporary_path = ""
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
            fail(f"{label} must be a regular, single-link file")
        temporary_descriptor, temporary_path = tempfile.mkstemp(
            prefix=".candidate-rpm.", suffix=".rpm", dir=directory
        )
        digest = hashlib.sha256()
        size = 0
        while True:
            chunk = os.read(descriptor, 1024 * 1024)
            if not chunk:
                break
            view = memoryview(chunk)
            while view:
                written = os.write(temporary_descriptor, view)
                if written <= 0:
                    fail(f"{label} snapshot write made no progress")
                view = view[written:]
            digest.update(chunk)
            size += len(chunk)
        os.fsync(temporary_descriptor)
        after = os.fstat(descriptor)
        identity = lambda value: (
            value.st_dev,
            value.st_ino,
            value.st_size,
            value.st_mtime_ns,
            value.st_ctime_ns,
        )
        if identity(before) != identity(after) or size != before.st_size:
            fail(f"{label} changed while its immutable snapshot was created")
        os.fchmod(temporary_descriptor, 0o400)
        return RpmSnapshot(Path(temporary_path), path.name, digest.hexdigest(), size)
    finally:
        os.close(descriptor)
        if temporary_descriptor >= 0:
            os.close(temporary_descriptor)


def verify_snapshot(snapshot: RpmSnapshot, role: str) -> None:
    try:
        raw = snapshot.path.read_bytes()
    except OSError as exc:
        fail(f"{role} private RPM snapshot is unavailable: {exc}")
    if len(raw) != snapshot.size_bytes or sha256(raw) != snapshot.sha256:
        fail(f"{role} private RPM snapshot changed during inspection")


def sha256(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def rpm_header(snapshot: RpmSnapshot, role: str) -> tuple[str, str]:
    query = "%{NAME} %{VERSION}-%{RELEASE}.%{ARCH}\\n%{PAYLOADDIGESTALGO} %{PAYLOADDIGEST}\\n"
    lines = run(["rpm", "-qp", "--qf", query, str(snapshot.path)]).decode("utf-8").splitlines()
    verify_snapshot(snapshot, role)
    if len(lines) != 2:
        fail(f"{role} RPM returned an incomplete identity header")
    package = lines[0]
    if package.split(" ", 1)[0] != ROLE_PACKAGE[role]:
        fail(f"{role} RPM package name is not {ROLE_PACKAGE[role]}")
    algorithm, separator, payload_digest = lines[1].partition(" ")
    if separator != " " or algorithm not in {"8", "SHA256", "sha256"}:
        fail(f"{role} RPM payload digest is not SHA-256")
    payload_digest = payload_digest.lower()
    if DIGEST_RE.fullmatch(payload_digest) is None:
        fail(f"{role} RPM payload digest is malformed")
    return package, payload_digest


def rpm_paths(snapshot: RpmSnapshot, role: str) -> set[str]:
    try:
        lines = run(["rpm", "-qlp", str(snapshot.path)]).decode("utf-8", errors="strict").splitlines()
    except UnicodeDecodeError:
        fail(f"{role} RPM file list is not UTF-8")
    verify_snapshot(snapshot, role)
    payload = set(lines)
    missing = sorted(REQUIRED_PAYLOAD[role] - payload)
    if missing:
        fail(f"{role} RPM omits required candidate payload: {', '.join(missing)}")
    return payload


def rpm_member(snapshot: RpmSnapshot, member: str, role: str) -> bytes:
    archive = run(["rpm2cpio", str(snapshot.path)])
    raw = run(["cpio", "--quiet", "--to-stdout", member], input_bytes=archive)
    verify_snapshot(snapshot, role)
    if not raw:
        fail(f"{role} RPM runtime binary is empty: {member}")
    return raw


def canonical_json(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def source_receipt(
    manifest_name: str,
    manifest_raw: bytes,
    revision: str,
    roles: dict[str, Any],
) -> dict[str, Any]:
    return {
        "candidate_manifest": {
            "path": manifest_name,
            "sha256": sha256(manifest_raw),
            "size_bytes": len(manifest_raw),
        },
        "kind": "mcnf-candidate-source-receipt-v1",
        "revision": revision,
        "roles": roles,
        "schema_version": 1,
    }


def atomic_write(path: Path, raw: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() or path.is_symlink():
        fail(f"refusing to overwrite output: {path}")
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def verify_checkout(repo: Path, revision: str) -> None:
    repo = repo.resolve(strict=True)
    head = run(["git", "-C", str(repo), "rev-parse", "--verify", "HEAD^{commit}"]).decode().strip()
    if head != revision:
        fail("source revision does not match the candidate checkout HEAD")
    run(["git", "-C", str(repo), "diff", "--quiet", "--ignore-submodules", "HEAD", "--"])
    untracked = run(["git", "-C", str(repo), "ls-files", "--others", "--exclude-standard"])
    if untracked:
        fail("candidate checkout contains untracked source files")


def validate_collector_schema(raw: bytes, revision: str, repo: Path) -> None:
    collector = repo / "install-helpers" / "collect-six-node-topology.py"
    if not collector.is_file() or collector.is_symlink():
        fail("six-node collector validator is unavailable")
    with tempfile.TemporaryDirectory(prefix="candidate-schema-") as directory:
        manifest = Path(directory) / "candidate-manifest.json"
        manifest.write_bytes(raw)
        run(
            [
                "python3",
                str(collector),
                "--validate-candidate-manifest",
                "--revision",
                revision,
                "--candidate-manifest",
                str(manifest),
            ]
        )


def describe_rpm(snapshot: RpmSnapshot, role: str) -> tuple[dict[str, Any], dict[str, Any]]:
    package, payload_digest = rpm_header(snapshot, role)
    rpm_paths(snapshot, role)
    binaries = {
        name: sha256(rpm_member(snapshot, member, role))
        for name, member in ROLE_BINARIES[role].items()
    }
    candidate = {
        "binaries": binaries,
        "package": package,
        "package_payload_sha256": payload_digest,
    }
    receipt = {
        **candidate,
        "role": role,
        "rpm": {
            "path": snapshot.original_name,
            "sha256": snapshot.sha256,
            "size_bytes": snapshot.size_bytes,
        },
    }
    return candidate, receipt


def self_test(repo: Path) -> None:
    revision = "a" * 40
    if REVISION_RE.fullmatch(revision) is None or REVISION_RE.fullmatch("b" * 64) is not None:
        fail("revision-width self-test failed")
    with tempfile.TemporaryDirectory(prefix="candidate-writer-self-test-") as directory:
        root = Path(directory)
        source = root / "fixture.rpm"
        source.write_bytes(b"fixed-rpm-fixture")
        snapshot = snapshot_rpm(source, "fixture RPM", root)
        source.write_bytes(b"changed-source-after-snapshot")
        verify_snapshot(snapshot, "fixture")
        snapshot.path.chmod(0o600)
        snapshot.path.write_bytes(b"mutated-private-snapshot")
        try:
            verify_snapshot(snapshot, "fixture")
        except CandidateError:
            pass
        else:
            fail("private snapshot mutation was accepted")
        role = lambda package, binaries: {
            "binaries": binaries,
            "package": package,
            "package_payload_sha256": "0" * 64,
        }
        manifest = {
            "kind": "mcnf-candidate-digest-manifest-v1",
            "revision": revision,
            "roles": {
                "lighthouse": role("magic-mesh-lighthouse 1-1.x86_64", {"mackesd": "1" * 64}),
                "workstation": role(
                    "magic-mesh 1-1.x86_64",
                    {"mackesd": "2" * 64, "mde-shell-egui": "3" * 64},
                ),
            },
            "schema_version": 1,
        }
        validate_collector_schema(canonical_json(manifest), revision, repo)
        manifest["unsupported"] = True
        try:
            validate_collector_schema(canonical_json(manifest), revision, repo)
        except CandidateError:
            pass
        else:
            fail("collector accepted an unsupported candidate-manifest field")
        receipt_raw = canonical_json(
            source_receipt("candidate-manifest.json", canonical_json(manifest), revision, {})
        )
        if receipt_raw.count(b'"schema_version":1') != 1:
            fail("source receipt emitted duplicate schema_version keys")
        checkout = root / "checkout"
        checkout.mkdir()
        run(["git", "-C", str(checkout), "init", "--quiet"])
        run(["git", "-C", str(checkout), "config", "user.name", "candidate-self-test"])
        run(["git", "-C", str(checkout), "config", "user.email", "candidate@example.invalid"])
        (checkout / "tracked").write_text("tracked\n", encoding="utf-8")
        run(["git", "-C", str(checkout), "add", "tracked"])
        run(["git", "-C", str(checkout), "commit", "--quiet", "-m", "fixture"])
        checkout_revision = run(
            ["git", "-C", str(checkout), "rev-parse", "HEAD"]
        ).decode().strip()
        verify_checkout(checkout, checkout_revision)
        (checkout / "untracked").write_text("untracked\n", encoding="utf-8")
        try:
            verify_checkout(checkout, checkout_revision)
        except CandidateError:
            pass
        else:
            fail("checkout with an untracked file was accepted")
    print(
        "write-candidate-manifest: self-test passed "
        "(revision width, clean checkout, full immutable snapshot, unique receipt fields, exact collector schema)"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--revision")
    parser.add_argument("--repo", type=Path)
    parser.add_argument("--workstation-rpm", type=Path)
    parser.add_argument("--lighthouse-rpm", type=Path)
    parser.add_argument("--out-dir", type=Path)
    args = parser.parse_args()
    if args.repo is None:
        fail("--repo is required")
    repo = args.repo.resolve(strict=True)
    if args.self_test:
        self_test(repo)
        return 0
    for name in ("revision", "workstation_rpm", "lighthouse_rpm", "out_dir"):
        if getattr(args, name) is None:
            fail(f"--{name.replace('_', '-')} is required")
    assert args.revision is not None
    assert args.workstation_rpm is not None
    assert args.lighthouse_rpm is not None
    assert args.out_dir is not None
    revision = args.revision.lower()
    if REVISION_RE.fullmatch(revision) is None:
        fail("--revision must be this repository's immutable 40-hex Git object ID")
    verify_checkout(repo, revision)
    with tempfile.TemporaryDirectory(prefix="candidate-rpm-snapshots-") as directory:
        snapshot_root = Path(directory)
        rpm_inputs = {
            "lighthouse": snapshot_rpm(args.lighthouse_rpm, "lighthouse RPM", snapshot_root),
            "workstation": snapshot_rpm(args.workstation_rpm, "workstation RPM", snapshot_root),
        }
        candidates: dict[str, Any] = {}
        receipts: dict[str, Any] = {}
        for role in ROLES:
            candidates[role], receipts[role] = describe_rpm(rpm_inputs[role], role)
    release_identities = {
        candidate["package"].split(" ", 1)[1] for candidate in candidates.values()
    }
    if len(release_identities) != 1:
        fail("candidate role RPMs do not share one version-release-architecture identity")
    manifest = {
        "kind": "mcnf-candidate-digest-manifest-v1",
        "revision": revision,
        "roles": candidates,
        "schema_version": 1,
    }
    manifest_raw = canonical_json(manifest)
    validate_collector_schema(manifest_raw, revision, repo)
    manifest_path = args.out_dir / "candidate-manifest.json"
    receipt_path = args.out_dir / "candidate-source-receipt.json"
    receipt = source_receipt(manifest_path.name, manifest_raw, revision, receipts)
    atomic_write(manifest_path, manifest_raw)
    atomic_write(receipt_path, canonical_json(receipt))
    print(f"write-candidate-manifest: wrote {manifest_path} and {receipt_path}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except CandidateError as exc:
        print(f"write-candidate-manifest: BLOCKED: {exc}", file=os.sys.stderr)
        raise SystemExit(2)
