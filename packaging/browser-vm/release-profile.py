#!/usr/bin/env python3
"""Freeze the governed Browser profile template for one exact release commit."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import stat
import subprocess

SOURCE_PATH = "packaging/browser-vm/profile.env"
MARKER = b"BROWSER_VM_SOURCE_COMMIT=@RELEASE_REVISION@"
REVISION = re.compile(r"[0-9a-f]{40}")
MAX_TEMPLATE_BYTES = 16 * 1024


class FreezeError(ValueError):
    pass


def git(repo: Path, *args: str) -> bytes:
    try:
        return subprocess.run(
            ["git", "-C", str(repo), *args], check=True, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, timeout=30,
        ).stdout
    except (OSError, subprocess.SubprocessError) as exc:
        raise FreezeError(f"Git source lookup failed: {exc}") from exc


def frozen_bytes(repo: Path, revision: str) -> bytes:
    if REVISION.fullmatch(revision) is None or revision == "0" * 40:
        raise FreezeError("source revision must be one non-null lowercase Git commit ID")
    if git(repo, "cat-file", "-t", revision) != b"commit\n":
        raise FreezeError("source revision is not a Git commit")
    template = git(repo, "show", f"{revision}:{SOURCE_PATH}")
    if not template or len(template) > MAX_TEMPLATE_BYTES:
        raise FreezeError("profile template size is outside the bounded contract")
    if template.count(MARKER) != 1:
        raise FreezeError("profile template must contain exactly one release marker")
    return template.replace(MARKER, f"BROWSER_VM_SOURCE_COMMIT={revision}".encode(), 1)


def write_exclusive(output: Path, body: bytes) -> None:
    try:
        metadata = output.parent.lstat()
    except OSError as exc:
        raise FreezeError(f"output parent metadata unavailable: {exc}") from exc
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        raise FreezeError("output parent must be a real directory")
    if metadata.st_mode & 0o022:
        raise FreezeError("output parent must not be group/other writable")
    temporary = output.with_name(f".{output.name}.freeze-{os.getpid()}")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    old_umask = os.umask(0o377)
    descriptor = None
    try:
        descriptor = os.open(temporary, flags, 0o400)
        with os.fdopen(descriptor, "wb", closefd=True) as handle:
            descriptor = None
            handle.write(body)
            handle.flush()
            os.fsync(handle.fileno())
        os.link(temporary, output, follow_symlinks=False)
        os.unlink(temporary)
        directory = os.open(output.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    except OSError as exc:
        raise FreezeError(f"refused profile output: {exc}") from exc
    finally:
        os.umask(old_umask)
        if descriptor is not None:
            os.close(descriptor)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        body = frozen_bytes(args.repo.resolve(), args.source_revision)
        write_exclusive(args.output, body)
    except FreezeError as exc:
        print(f"freeze-browser-vm-profile: {exc}", file=os.sys.stderr)
        return 1
    print(f"Frozen Browser VM release profile: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
