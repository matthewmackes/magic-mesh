#!/usr/bin/env python3
"""Bind dest identity files as a sourceable env file (no login mutation).

SshBootstrap resolves MACKESD_BOOTSTRAP_SSH_KEY and
MACKESD_BOOTSTRAP_KNOWN_HOSTS to regular files (symlink refused). This
helper writes a no-replace env file whose body is exactly those two path
assignments. It never exports those vars, never prints key bytes, and
never claims enroll succeeded.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_DEST_PARENT = Path("/root/mcnf-private")
DEST_KEY_NAME = "bootstrap-ssh-key"
DEST_KNOWN_HOSTS_NAME = "bootstrap-known-hosts"
ENV_FILE_NAME = "bootstrap-ssh.env"
SIDECAR_NAME = "bootstrap-ssh-env.json"
SIDECAR_KIND = "mcnf-bootstrap-ssh-env"
ENV_MODE = 0o400
SIDECAR_MODE = 0o400
EXIT_REFUSED = 2
SAFE_PATH = re.compile(r"^/[A-Za-z0-9._/-]+$")


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


def admit_existing_identity(path: Path, label: str) -> Path:
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
    resolved = dest_resolved(path)
    if not SAFE_PATH.match(str(resolved)):
        refuse(f"{label} path is not a bound assignment value")
    return resolved


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
        refuse(f"{label} already exists; bind is no-replace")
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
        refuse(f"{label} already exists; bind is no-replace")
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


def env_file_body(dest_key: Path, dest_known_hosts: Path) -> bytes:
    return (
        f"MACKESD_BOOTSTRAP_SSH_KEY={dest_key}\n"
        f"MACKESD_BOOTSTRAP_KNOWN_HOSTS={dest_known_hosts}\n"
    ).encode("ascii")


def bind_sidecar(env_path: Path, env_meta: os.stat_result, env_digest: str, sidecar_path: Path) -> dict[str, object]:
    record = {
        "enroll_succeeded": False,
        "env_file": file_record(env_path, env_meta, env_digest),
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


def bind(
    dest_key: Path,
    dest_known_hosts: Path,
    env_file: Path,
    sidecar: Path,
) -> dict[str, object]:
    worktree = helper_worktree_root()
    dest_key = admit_existing_identity(dest_key, "dest key")
    dest_known_hosts = admit_existing_identity(dest_known_hosts, "dest known-hosts")
    env_file = admit_dest_path(env_file, "dest env file", worktree)
    sidecar = admit_dest_path(sidecar, "sidecar", worktree)
    if dest_key == dest_known_hosts:
        refuse("dest key and dest known-hosts must be distinct paths")
    if len({dest_key, dest_known_hosts, env_file, sidecar}) != 4:
        refuse("dest identity files, dest env file, and sidecar must be distinct paths")
    body = env_file_body(dest_key, dest_known_hosts)
    written: list[Path] = []
    try:
        env_meta = write_exclusive(env_file, body, ENV_MODE, "dest env file")
        written.append(env_file)
        record = bind_sidecar(env_file, env_meta, hashlib.sha256(body).hexdigest(), sidecar)
        write_exclusive(sidecar, canonical(record), SIDECAR_MODE, "sidecar")
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
    parser.add_argument("--dest-parent", type=Path, default=DEFAULT_DEST_PARENT)
    parser.add_argument("--dest-key", type=Path, default=None)
    parser.add_argument("--dest-known-hosts", type=Path, default=None)
    parser.add_argument("--env-file", type=Path, default=None)
    parser.add_argument("--sidecar", type=Path, default=None)
    args = parser.parse_args()
    dest_parent = args.dest_parent
    dest_key = args.dest_key if args.dest_key is not None else dest_parent / DEST_KEY_NAME
    dest_known_hosts = (
        args.dest_known_hosts if args.dest_known_hosts is not None else dest_parent / DEST_KNOWN_HOSTS_NAME
    )
    env_file = args.env_file if args.env_file is not None else dest_parent / ENV_FILE_NAME
    sidecar = args.sidecar if args.sidecar is not None else dest_parent / SIDECAR_NAME
    record = bind(
        dest_key=dest_key,
        dest_known_hosts=dest_known_hosts,
        env_file=env_file,
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
