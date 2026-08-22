#!/usr/bin/env python3
"""Copy bootstrap SSH identity files onto dest regular files (no enroll).

SshBootstrap resolves systemd credentials or env paths
MACKESD_BOOTSTRAP_SSH_KEY and MACKESD_BOOTSTRAP_KNOWN_HOSTS; both must be
singly-used regular files (symlink refused). This helper only provisions
those dest files. It never sets those env vars, never prints key bytes,
and never claims enroll succeeded.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_DEST_PARENT = Path("/root/mcnf-private")
DEST_KEY_NAME = "bootstrap-ssh-key"
DEST_KNOWN_HOSTS_NAME = "bootstrap-known-hosts"
SIDECAR_NAME = "bootstrap-ssh-identity.json"
SIDECAR_KIND = "mcnf-bootstrap-ssh-identity"
KEY_MODE = 0o600
KNOWN_HOSTS_MODE = 0o400
SIDECAR_MODE = 0o400
MAX_SOURCE_BYTES = 1024 * 1024
EXIT_REFUSED = 2


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def helper_worktree_root() -> Path:
    result = subprocess.run(
        ["git", "-C", str(HERE), "rev-parse", "--show-toplevel"],
        check=False,
        capture_output=True,
        text=True,
    )
    root = result.stdout.strip()
    if result.returncode == 0 and root:
        return Path(root).resolve()
    # Farm slot trees rsync without .git; install-helpers always lives at repo root.
    return HERE.parent.resolve()


def is_inside(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root)
    except ValueError:
        return False
    return True


def dest_resolved(path: Path) -> Path:
    return path.parent.resolve() / path.name


def admit_regular_source(path: Path, label: str) -> os.stat_result:
    try:
        meta = path.lstat()
    except OSError as error:
        refuse(f"{label} is missing or inaccessible")
        raise AssertionError from error
    if stat.S_ISLNK(meta.st_mode):
        refuse(f"{label} is a symlink")
    if not stat.S_ISREG(meta.st_mode):
        refuse(f"{label} must be a regular file")
    if meta.st_nlink != 1:
        refuse(f"{label} must be a singly-used regular file")
    if meta.st_size <= 0:
        refuse(f"{label} is empty")
    if meta.st_size > MAX_SOURCE_BYTES:
        refuse(f"{label} exceeds its bound")
    return meta


def read_regular(path: Path, label: str) -> bytes:
    meta = admit_regular_source(path, label)
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except OSError as error:
        refuse(f"{label} is missing or inaccessible")
        raise AssertionError from error
    try:
        opened = os.fstat(fd)
        if (opened.st_dev, opened.st_ino) != (meta.st_dev, meta.st_ino):
            refuse(f"{label} changed while being read")
        if not stat.S_ISREG(opened.st_mode) or opened.st_nlink != 1:
            refuse(f"{label} must be a singly-used regular file")
        body = os.read(fd, MAX_SOURCE_BYTES + 1)
        after = os.fstat(fd)
    finally:
        os.close(fd)
    if not body:
        refuse(f"{label} is empty")
    if len(body) != meta.st_size or after.st_size != meta.st_size:
        refuse(f"{label} changed while being read")
    return body


def admit_dest_parent(path: Path, label: str) -> Path:
    parent = path.parent
    try:
        meta = parent.lstat()
    except OSError as error:
        refuse(f"{label} parent is missing")
        raise AssertionError from error
    if stat.S_ISLNK(meta.st_mode):
        refuse(f"{label} parent is a symlink")
    if not stat.S_ISDIR(meta.st_mode):
        refuse(f"{label} parent is not a directory")
    return parent.resolve()


def admit_dest_path(path: Path, label: str, worktree: Path) -> Path:
    if path.exists() or path.is_symlink():
        try:
            if path.is_symlink() or stat.S_ISLNK(path.lstat().st_mode):
                refuse(f"{label} is a symlink")
        except OSError:
            refuse(f"{label} is a symlink")
        refuse(f"{label} already exists; provision is no-replace")
    admit_dest_parent(path, label)
    resolved = dest_resolved(path)
    if is_inside(resolved, worktree):
        refuse(f"{label} is inside the git worktree")
    return resolved


def write_exclusive(path: Path, data: bytes, mode: int, label: str) -> os.stat_result:
    if not data:
        refuse(f"{label} is empty")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0)
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        fd = os.open(path, flags, mode)
    except FileExistsError as error:
        refuse(f"{label} already exists; provision is no-replace")
        raise AssertionError from error
    except OSError as error:
        refuse(f"{label} cannot be created: {error}")
        raise AssertionError from error
    try:
        os.fchmod(fd, mode)
        os.write(fd, data)
        os.fsync(fd)
        meta = os.fstat(fd)
    except Exception:
        os.close(fd)
        try:
            os.unlink(path)
        except OSError:
            pass
        raise
    os.close(fd)
    if meta.st_nlink != 1 or not stat.S_ISREG(meta.st_mode):
        try:
            os.unlink(path)
        except OSError:
            pass
        refuse(f"{label} must be a singly-used regular file")
    return meta


def file_record(path: Path, meta: os.stat_result, digest: str) -> dict[str, object]:
    return {
        "path": str(path),
        "mode": f"{stat.S_IMODE(meta.st_mode):04o}",
        "bytes": int(meta.st_size),
        "sha256": digest,
    }


def bind_sidecar(
    dest_key: Path,
    key_meta: os.stat_result,
    key_digest: str,
    dest_known_hosts: Path,
    hosts_meta: os.stat_result,
    hosts_digest: str,
    sidecar_path: Path,
) -> dict[str, object]:
    record = {
        "dest_key": file_record(dest_key, key_meta, key_digest),
        "dest_known_hosts": file_record(dest_known_hosts, hosts_meta, hosts_digest),
        "enroll_succeeded": False,
        "kind": SIDECAR_KIND,
        "production_admitted": False,
        "schema_version": 1,
        "sidecar_path": str(sidecar_path),
    }
    if record["kind"] != SIDECAR_KIND:
        refuse("sidecar kind is unsupported")
    if record["enroll_succeeded"] is not False:
        refuse("helper must never claim enroll succeeded")
    if record["production_admitted"] is not False:
        refuse("helper must never mark production_admitted")
    return record


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"


def provision(
    source_key: Path,
    source_known_hosts: Path,
    dest_key: Path,
    dest_known_hosts: Path,
    sidecar: Path,
) -> dict[str, object]:
    worktree = helper_worktree_root()
    key_body = read_regular(source_key, "source key")
    hosts_body = read_regular(source_known_hosts, "source known-hosts")
    dest_key = admit_dest_path(dest_key, "dest key", worktree)
    dest_known_hosts = admit_dest_path(dest_known_hosts, "dest known-hosts", worktree)
    sidecar = admit_dest_path(sidecar, "sidecar", worktree)
    if len({dest_key, dest_known_hosts, sidecar}) != 3:
        refuse("dest key, dest known-hosts, and sidecar must be distinct paths")
    written: list[Path] = []
    try:
        key_meta = write_exclusive(dest_key, key_body, KEY_MODE, "dest key")
        written.append(dest_key)
        hosts_meta = write_exclusive(dest_known_hosts, hosts_body, KNOWN_HOSTS_MODE, "dest known-hosts")
        written.append(dest_known_hosts)
        record = bind_sidecar(
            dest_key,
            key_meta,
            hashlib.sha256(key_body).hexdigest(),
            dest_known_hosts,
            hosts_meta,
            hashlib.sha256(hosts_body).hexdigest(),
            sidecar,
        )
        body = canonical(record)
        write_exclusive(sidecar, body, SIDECAR_MODE, "sidecar")
    except Exception:
        for path in written:
            try:
                os.unlink(path)
            except OSError:
                pass
        raise
    return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-key", required=True, type=Path)
    parser.add_argument("--source-known-hosts", required=True, type=Path)
    parser.add_argument("--dest-parent", type=Path, default=DEFAULT_DEST_PARENT)
    parser.add_argument("--dest-key", type=Path, default=None)
    parser.add_argument("--dest-known-hosts", type=Path, default=None)
    parser.add_argument("--sidecar", type=Path, default=None)
    args = parser.parse_args()
    dest_parent = args.dest_parent
    dest_key = args.dest_key if args.dest_key is not None else dest_parent / DEST_KEY_NAME
    dest_known_hosts = (
        args.dest_known_hosts if args.dest_known_hosts is not None else dest_parent / DEST_KNOWN_HOSTS_NAME
    )
    sidecar = args.sidecar if args.sidecar is not None else dest_parent / SIDECAR_NAME
    record = provision(
        source_key=args.source_key,
        source_known_hosts=args.source_known_hosts,
        dest_key=dest_key,
        dest_known_hosts=dest_known_hosts,
        sidecar=sidecar,
    )
    sys.stdout.buffer.write(canonical(record))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, Refusal) as error:
        print(f"REFUSED: {error}", file=sys.stderr)
        raise SystemExit(EXIT_REFUSED)
