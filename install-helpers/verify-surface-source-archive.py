#!/usr/bin/env python3
"""Fail-closed path/link validation for pinned Surface source archives."""

from __future__ import annotations

import io
import posixpath
import sys
import tarfile
import tempfile
from pathlib import Path, PurePosixPath


def validate_members(members: list[tarfile.TarInfo]) -> None:
    if not members or len(members) > 250_000:
        raise ValueError("source archive member count is outside bounds")
    paths = [PurePosixPath(member.name) for member in members]
    if any(path.is_absolute() or ".." in path.parts or not path.parts for path in paths):
        raise ValueError("source archive contains an unsafe path")
    roots = {path.parts[0] for path in paths}
    if len(roots) != 1:
        raise ValueError("source archive must have exactly one top-level directory")
    root = next(iter(roots))
    for member, path in zip(members, paths):
        if not (member.issym() or member.islnk()):
            continue
        target = PurePosixPath(member.linkname)
        # Symlinks resolve from their containing directory. Tar hard links name
        # an archive-root-relative member. Parent traversal is valid only when
        # the normalized destination remains inside the one admitted root.
        candidate = path.parent / target if member.issym() else target
        resolved = PurePosixPath(posixpath.normpath(str(candidate)))
        if (
            target.is_absolute()
            or resolved.is_absolute()
            or not resolved.parts
            or ".." in resolved.parts
            or resolved.parts[0] != root
        ):
            raise ValueError("source archive contains an unsafe link")


def validate_archive(path: Path) -> None:
    with tarfile.open(path, "r:*") as stream:
        validate_members(stream.getmembers())


def self_test() -> None:
    def archive(link_name: str) -> bytes:
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w") as stream:
            directory = tarfile.TarInfo("root/a/b/")
            directory.type = tarfile.DIRTYPE
            stream.addfile(directory)
            target = tarfile.TarInfo("root/target")
            target.size = 1
            stream.addfile(target, io.BytesIO(b"x"))
            link = tarfile.TarInfo("root/a/b/link")
            link.type = tarfile.SYMTYPE
            link.linkname = link_name
            stream.addfile(link)
        return output.getvalue()

    with tempfile.TemporaryDirectory(prefix="surface-archive-test-") as raw:
        root = Path(raw)
        safe = root / "safe.tar"
        escape = root / "escape.tar"
        absolute = root / "absolute.tar"
        safe.write_bytes(archive("../../target"))
        escape.write_bytes(archive("../../../outside"))
        absolute.write_bytes(archive("/outside"))
        validate_archive(safe)
        for hostile in (escape, absolute):
            try:
                validate_archive(hostile)
            except ValueError:
                continue
            raise SystemExit(f"self-test admitted unsafe archive: {hostile.name}")
    print("Surface source archive verifier self-test passed")


def main() -> None:
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        return
    if len(sys.argv) < 2:
        raise SystemExit("usage: verify-surface-source-archive.py ARCHIVE... | --self-test")
    try:
        for raw in sys.argv[1:]:
            validate_archive(Path(raw))
    except (OSError, tarfile.TarError, ValueError) as exc:
        raise SystemExit(str(exc)) from exc


if __name__ == "__main__":
    main()
