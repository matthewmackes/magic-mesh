#!/usr/bin/env python3
"""Produce and revalidate the governed Cuttlefish image identity receipt."""

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
MAX_RECEIPT = 16 * 1024
DIGEST_RE = re.compile(r"sha256:[0-9a-f]{64}\Z")
REVISION_RE = re.compile(r"[0-9a-f]{40}\Z")
IDENTITY_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+:-]{0,254}\Z")
KIND = "mcnf-cuttlefish-image-receipt"
MEDIA_TYPES = {
    "application/vnd.docker.distribution.manifest.v2+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.oci.image.index.v1+json",
}
FORMATS = ("android-cuttlefish-host-package", "android-cuttlefish-image-archive")


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


def validate_common(args: argparse.Namespace) -> None:
    for label in ("provider_identity", "android_release_id", "compatibility_id"):
        if not IDENTITY_RE.fullmatch(getattr(args, label)):
            raise Refusal(f"{label} is malformed")


def inspect_registry(skopeo: Path, source: str, architecture: str) -> tuple[str, str, str | None]:
    if not source or any(ch.isspace() for ch in source) or source.startswith("docker://"):
        raise Refusal("registry source must be one bounded transport-free reference")
    result = subprocess.run(
        [str(skopeo), "inspect", "--raw", f"docker://{source}"],
        capture_output=True, timeout=30, check=False,
    )
    if result.returncode or not result.stdout or len(result.stdout) > MAX_DOCUMENT:
        raise Refusal("registry manifest inspection failed or exceeded 16 MiB")
    document = exact_json(result.stdout, "registry manifest")
    media_type = document.get("mediaType")
    if media_type not in MEDIA_TYPES:
        raise Refusal("registry media type is absent or unsupported")
    platform_digest: str | None = None
    if str(media_type).endswith(("manifest.list.v2+json", "image.index.v1+json")):
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
        metadata = subprocess.run(
            [str(skopeo), "inspect", "--format", "{{.Architecture}}", f"docker://{source}"],
            text=True, capture_output=True, timeout=30, check=False,
        )
        if metadata.returncode or metadata.stdout.strip() != architecture:
            raise Refusal("registry manifest architecture does not match")
    digest = "sha256:" + hashlib.sha256(result.stdout).hexdigest()
    pinned = source.rsplit("@", 1)[1] if "@" in source else None
    if pinned is not None and pinned != digest:
        raise Refusal("source digest does not match registry manifest bytes")
    return digest, str(media_type), platform_digest


def inspect_artifact(path: Path) -> str:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as exc:
        raise Refusal(f"image artifact cannot be opened safely: {exc}") from exc
    digest = hashlib.sha256()
    try:
        before = os.fstat(descriptor)
        named = os.stat(path, follow_symlinks=False)
        identity = lambda value: (
            value.st_dev, value.st_ino, value.st_mode, value.st_nlink,
            value.st_uid, value.st_gid, value.st_size, value.st_mtime_ns,
            value.st_ctime_ns,
        )
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size <= 0:
            raise Refusal("image artifact must be a non-empty single-link regular file")
        if identity(before) != identity(named):
            raise Refusal("image artifact changed before hashing")
        while True:
            chunk = os.read(descriptor, 1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
        after = os.fstat(descriptor)
        closed = os.stat(path, follow_symlinks=False)
        if identity(after) != identity(before) or identity(closed) != identity(before):
            raise Refusal("image artifact changed while hashing")
    finally:
        os.close(descriptor)
    return "sha256:" + digest.hexdigest()


def expected(args: argparse.Namespace) -> dict[str, object]:
    return {
        "android_release_id": args.android_release_id,
        "architecture": args.architecture,
        "commit_epoch": int(args.commit_epoch),
        "compatibility_id": args.compatibility_id,
        "kind": KIND,
        "original_source": args.original_source,
        "provider_identity": args.provider_identity,
        "schema_version": 1,
        "source_kind": args.source_kind,
        "source_revision": args.source_revision,
    }


def observe(args: argparse.Namespace) -> dict[str, object]:
    if args.source_kind == "registry":
        digest, media_type, platform = inspect_registry(args.skopeo, args.original_source, args.architecture)
        return {"digest": digest, "media_type": media_type, "format": "registry-manifest", "platform_digest": platform}
    path = Path(args.original_source)
    if not path.is_absolute():
        raise Refusal("artifact original source must be an absolute path")
    return {"digest": inspect_artifact(path), "media_type": args.media_type, "format": args.artifact_format, "platform_digest": None}


def publish(path: Path, payload: bytes) -> None:
    if path.exists() or path.is_symlink():
        raise Refusal("receipt output already exists")
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".cuttlefish-image.", dir=path.parent)
    try:
        os.fchmod(fd, 0o400)
        with os.fdopen(fd, "wb", closefd=True) as stream:
            stream.write(payload); stream.flush(); os.fsync(stream.fileno())
        os.link(temporary, path); os.unlink(temporary)
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


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser()
    result.add_argument("--repo", type=Path, required=True)
    result.add_argument("--skopeo", type=Path, default=Path("/usr/bin/skopeo"))
    sub = result.add_subparsers(dest="action", required=True)
    for action in ("produce", "inspect"):
        command = sub.add_parser(action)
        command.add_argument("--source-kind", choices=("registry", "artifact"), required=True)
        command.add_argument("--original-source", required=True)
        command.add_argument("--architecture", choices=("amd64", "arm64"), required=True)
        command.add_argument("--provider-identity", required=True)
        command.add_argument("--android-release-id", required=True)
        command.add_argument("--compatibility-id", required=True)
        command.add_argument("--source-revision", required=True)
        command.add_argument("--commit-epoch", required=True)
        command.add_argument("--media-type", default="application/octet-stream")
        command.add_argument("--artifact-format", choices=FORMATS, default=FORMATS[0])
        command.add_argument("--receipt" if action == "inspect" else "--output", type=Path, required=True)
    return result


def main() -> int:
    try:
        args = parser().parse_args()
        validate_identity(args.repo.resolve(), args.source_revision, args.commit_epoch)
        validate_common(args)
        observed = observe(args)
        if args.action == "produce":
            payload = expected(args) | observed
            publish(args.output, (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode())
            print(args.output)
        else:
            receipt = read_receipt(args.receipt)
            wanted = expected(args) | observed
            if set(receipt) != set(wanted):
                raise Refusal("receipt field set is not exact")
            for key, value in wanted.items():
                if receipt.get(key) != value:
                    raise Refusal(f"receipt {key} binding does not match")
            print(json.dumps(receipt, sort_keys=True, separators=(",", ":")))
        return 0
    except (Refusal, OSError, subprocess.SubprocessError, ValueError) as exc:
        print(f"cuttlefish-image-receipt: REFUSED: {exc}", file=sys.stderr)
        return EXIT_REFUSED


if __name__ == "__main__":
    raise SystemExit(main())
