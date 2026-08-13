#!/usr/bin/env python3
"""Produce and revalidate the governed App VM base-image receipt."""

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
from pathlib import Path

EXIT_REFUSED = 2
MAX_DOCUMENT = 16 * 1024 * 1024
MAX_RECEIPT = 8192
DIGEST_RE = re.compile(r"sha256:[0-9a-f]{64}\Z")
REVISION_RE = re.compile(r"[0-9a-f]{40}\Z")
TARGET = "mcnf-app-vm/wayland-standard-v1"
PROFILE = "wayland-standard"
KIND = "mcnf-app-vm-base-image-receipt"
MEDIA_TYPES = {
    "application/vnd.docker.distribution.manifest.v2+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.oci.image.index.v1+json",
}


class Refusal(Exception):
    pass


def exact_json(raw: bytes, label: str) -> dict[str, object]:
    def pairs(items: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in items:
            if key in result:
                raise Refusal(f"{label} contains duplicate field {key}")
            result[key] = value
        return result

    try:
        value = json.loads(raw.decode("utf-8"), object_pairs_hook=pairs)
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise Refusal(f"{label} is not exact JSON: {exc}") from exc
    if not isinstance(value, dict):
        raise Refusal(f"{label} must be one JSON object")
    return value


def validate_identity(repo: Path, revision: str, epoch: str) -> None:
    if not REVISION_RE.fullmatch(revision) or not epoch.isdigit():
        raise Refusal("source revision or commit epoch is malformed")
    actual = subprocess.run(
        ["git", "-C", str(repo), "show", "-s", "--format=%ct", revision],
        text=True, capture_output=True, timeout=15, check=False,
    )
    if actual.returncode or actual.stdout.strip() != epoch:
        raise Refusal("source revision and commit epoch do not identify one commit")


def inspect_registry(skopeo: Path, reference: str, architecture: str) -> tuple[str, str, str | None]:
    if not reference or any(ch.isspace() for ch in reference) or reference.startswith("docker://"):
        raise Refusal("registry reference must be one bounded transport-free reference")
    command = [str(skopeo), "inspect", "--raw", f"docker://{reference}"]
    result = subprocess.run(command, capture_output=True, timeout=30, check=False)
    if result.returncode:
        raise Refusal("registry manifest inspection failed")
    if not result.stdout or len(result.stdout) > MAX_DOCUMENT:
        raise Refusal("registry manifest document is empty or exceeds 16 MiB")
    document = exact_json(result.stdout, "registry manifest")
    media_type = document.get("mediaType")
    if media_type not in MEDIA_TYPES:
        raise Refusal("registry media type is absent or unsupported")
    platform_digest: str | None = None
    if media_type.endswith("manifest.list.v2+json") or media_type.endswith("image.index.v1+json"):
        matches: list[str] = []
        manifests = document.get("manifests")
        if not isinstance(manifests, list):
            raise Refusal("registry index omits manifests")
        for entry in manifests:
            if not isinstance(entry, dict) or not isinstance(entry.get("platform"), dict):
                continue
            platform = entry["platform"]
            digest = entry.get("digest")
            if platform.get("os") == "linux" and platform.get("architecture") == architecture:
                if not isinstance(digest, str) or not DIGEST_RE.fullmatch(digest):
                    raise Refusal("matching platform digest is malformed")
                matches.append(digest)
        if len(matches) != 1:
            raise Refusal("registry index must contain exactly one matching Linux architecture")
        platform_digest = matches[0]
    else:
        meta = subprocess.run(
            [str(skopeo), "inspect", "--format", "{{.Architecture}}", f"docker://{reference}"],
            text=True, capture_output=True, timeout=30, check=False,
        )
        if meta.returncode or meta.stdout.strip() != architecture:
            raise Refusal("registry manifest architecture does not match")
    resolved = "sha256:" + hashlib.sha256(result.stdout).hexdigest()
    pinned = reference.rsplit("@", 1)[1] if "@" in reference else None
    if pinned is not None and pinned != resolved:
        raise Refusal("reference digest does not match registry manifest bytes")
    return resolved, str(media_type), platform_digest


def receipt_bytes(value: dict[str, object]) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def publish(path: Path, payload: bytes) -> None:
    if path.exists() or path.is_symlink():
        raise Refusal("receipt output already exists")
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".app-vm-base.", dir=path.parent)
    try:
        os.fchmod(fd, 0o400)
        with os.fdopen(fd, "wb", closefd=True) as stream:
            stream.write(payload); stream.flush(); os.fsync(stream.fileno())
        os.link(temporary, path)
        os.unlink(temporary)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(directory)
        finally: os.close(directory)
    finally:
        try: os.unlink(temporary)
        except FileNotFoundError: pass


def read_receipt(path: Path) -> dict[str, object]:
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
        raise Refusal("receipt must be a regular non-symlink file")
    if info.st_size <= 0 or info.st_size > MAX_RECEIPT or info.st_mode & 0o022:
        raise Refusal("receipt is empty, oversized, or writable by group/other")
    return exact_json(path.read_bytes(), "receipt")


def expected_fields(args: argparse.Namespace) -> dict[str, object]:
    return {
        "architecture": args.architecture,
        "app_vm_profile": PROFILE,
        "app_vm_target": TARGET,
        "commit_epoch": int(args.commit_epoch),
        "image_reference": args.image_reference,
        "kind": KIND,
        "schema_version": 1,
        "source_revision": args.source_revision,
    }


def validate_receipt(value: dict[str, object], args: argparse.Namespace) -> None:
    expected = expected_fields(args)
    if set(value) != set(expected) | {"media_type", "platform_digest", "resolved_digest"}:
        raise Refusal("receipt field set is not exact")
    for key, wanted in expected.items():
        if value.get(key) != wanted:
            raise Refusal(f"receipt {key} binding does not match")
    if value.get("media_type") not in MEDIA_TYPES or not isinstance(value.get("resolved_digest"), str) or not DIGEST_RE.fullmatch(value["resolved_digest"]):
        raise Refusal("receipt registry identity is malformed")
    platform = value.get("platform_digest")
    if platform is not None and (not isinstance(platform, str) or not DIGEST_RE.fullmatch(platform)):
        raise Refusal("receipt platform digest is malformed")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser()
    result.add_argument("--repo", type=Path, required=True)
    result.add_argument("--skopeo", type=Path, default=Path("/usr/bin/skopeo"))
    sub = result.add_subparsers(dest="action", required=True)
    for name in ("produce", "inspect"):
        cmd = sub.add_parser(name)
        cmd.add_argument("--image-reference", required=True)
        cmd.add_argument("--architecture", choices=("amd64", "arm64"), required=True)
        cmd.add_argument("--source-revision", required=True)
        cmd.add_argument("--commit-epoch", required=True)
        cmd.add_argument("--receipt" if name == "inspect" else "--output", type=Path, required=True)
    return result


def main() -> int:
    try:
        args = parser().parse_args()
        validate_identity(args.repo.resolve(), args.source_revision, args.commit_epoch)
        resolved, media_type, platform = inspect_registry(args.skopeo, args.image_reference, args.architecture)
        if args.action == "produce":
            value = expected_fields(args) | {"media_type": media_type, "platform_digest": platform, "resolved_digest": resolved}
            publish(args.output, receipt_bytes(value))
            print(args.output)
        else:
            value = read_receipt(args.receipt)
            validate_receipt(value, args)
            if (value["resolved_digest"], value["media_type"], value["platform_digest"]) != (resolved, media_type, platform):
                raise Refusal("registry identity changed since receipt publication")
            print(json.dumps(value, sort_keys=True, separators=(",", ":")))
        return 0
    except (Refusal, OSError, subprocess.SubprocessError, ValueError) as exc:
        print(f"app-vm-base-image-receipt: REFUSED: {exc}", file=sys.stderr)
        return EXIT_REFUSED


if __name__ == "__main__":
    raise SystemExit(main())
