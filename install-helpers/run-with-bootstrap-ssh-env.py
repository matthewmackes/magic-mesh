#!/usr/bin/env python3
"""Source dest identity env for a child enroll worker only.

Reads a dest env file whose body is exactly the two dest-path
assignments and runs a command as a child with a copied environment
plus those two vars. It never sets those vars on this process, never
prints key or env-file bytes, and never claims enroll succeeded.
Lifecycle mutation argv (`enroll-token`, `add-peer`, `invite-issue`,
`enroll`, `reenroll`, `recovery`, `offboard`, `join`, `found`,
`mesh-init`, `leave`, `decommission`, `onboard`, `mde-enroll`,
`magic-setup`, `meshctl provision`/`init`, ssh/bash wrappers embedding
those verbs, or the mint helper) refuses while the unpublished signed
candidate dest is absent. After dest admit, mutation argv runs
`seat-update-warning.sh`.
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
import importlib.util
from pathlib import Path

HERE = Path(__file__).resolve().parent


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


candidate = _load(
    "admit_unpublished_signed_candidate",
    HERE / "admit-unpublished-signed-candidate.py",
)
warning = _load(
    "require_seat_mutation_warning",
    HERE / "require-seat-mutation-warning.py",
)
DEFAULT_ENV_FILE = Path("/root/mcnf-private/bootstrap-ssh.env")
SIDECAR_KIND = "mcnf-bootstrap-ssh-env-run"
ENV_MODE = 0o400
KEY_MODE = 0o600
KNOWN_HOSTS_MODE = 0o400
SIDECAR_MODE = 0o400
MAX_ENV_BYTES = 8192
MAX_DEST_BYTES = 1024 * 1024
EXIT_REFUSED = 2
SAFE_PATH = re.compile(r"^/[A-Za-z0-9._/-]+$")
JOIN_TOKEN_PLACEHOLDER = "{{JOIN_TOKEN}}"
ENV_KEYS = ("MACKESD_BOOTSTRAP_SSH_KEY", "MACKESD_BOOTSTRAP_KNOWN_HOSTS")
# Live mesh mutation verbs. Operator lock: seats mutate only when an
# unpublished signed candidate dest admits. Missing dest still refuses.
LIFECYCLE_MUTATION_NAMES = frozenset(
    {
        "enroll-token",
        "add-peer",
        "invite-issue",
        "enroll",
        "reenroll",
        "recovery",
        "offboard",
        "join",
        "found",
        "mesh-create",
        "mesh-init",
        "leave",
        "mint-enroll-bearer",
        "mint-enroll-bearer.py",
        "provision",
        "decommission",
        "remove-peer",
        "adopt-xcp",
        "role-provision",
        "spawn-lighthouse",
        "onboard",
        "mde-enroll",
        "magic-setup",
    }
)
# meshctl mesh init does not put "mesh-init" on argv.
MESHCTL_MUTATION_NAMES = frozenset(
    {"provision", "join", "leave", "decommission", "init"}
)
SHELL_WRAPPERS = frozenset(
    {"ssh", "scp", "sshpass", "bash", "sh", "dash", "python", "python3"}
)
LIFECYCLE_BINARIES = frozenset(
    {
        "mackesd",
        "meshctl",
        "mde-enroll",
        "magic-setup",
        "mint-enroll-bearer",
        "mint-enroll-bearer.py",
    }
)
_MUTATION_WORD = re.compile(
    r"(?<![A-Za-z0-9_-])(?:"
    + "|".join(
        re.escape(name)
        for name in sorted(LIFECYCLE_MUTATION_NAMES | MESHCTL_MUTATION_NAMES, key=len, reverse=True)
    )
    + r")(?![A-Za-z0-9_-])"
)
_LIFECYCLE_BINARY_WORD = re.compile(
    r"(?<![A-Za-z0-9_-])(?:"
    + "|".join(re.escape(name) for name in sorted(LIFECYCLE_BINARIES, key=len, reverse=True))
    + r")(?![A-Za-z0-9_-])"
)


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def command_basename(token: str) -> str:
    return Path(token).name


def wrapper_embeds_lifecycle_mutation(token: str) -> bool:
    if "enroll-token" in token:
        return True
    if _LIFECYCLE_BINARY_WORD.search(token) and (
        _MUTATION_WORD.search(token)
        or command_basename(token) in LIFECYCLE_BINARIES
    ):
        return True
    return False


def command_is_lifecycle_mutation(command: list[str]) -> bool:
    names = [command_basename(token) for token in command]
    if any(
        name in LIFECYCLE_MUTATION_NAMES or "enroll-token" in token
        for token, name in zip(command, names)
    ):
        return True
    if "meshctl" in names and MESHCTL_MUTATION_NAMES.intersection(names):
        return True
    if SHELL_WRAPPERS.intersection(names):
        return any(wrapper_embeds_lifecycle_mutation(token) for token in command)
    return False


def admit_not_lifecycle_mutation(command: list[str]) -> None:
    if not command_is_lifecycle_mutation(command):
        return
    try:
        candidate.admit_unpublished_signed_candidate(for_production_mutation=True)
    except candidate.Refusal as error:
        refuse(f"lifecycle mutation argv refuses; {error}")
    try:
        warning.require_seat_mutation_warning(for_production_mutation=True)
    except warning.Refusal as error:
        refuse(str(error))


def helper_process_env() -> dict[str, str]:
    env = os.environ.copy()
    for name in (*ENV_KEYS, "JOIN_TOKEN", candidate.DEST_ENV, warning.HELPER_ENV):
        env.pop(name, None)
    return env


def helper_worktree_root() -> Path:
    result = subprocess.run(
        ["git", "-C", str(HERE), "rev-parse", "--show-toplevel"],
        check=False,
        capture_output=True,
        text=True,
        env=helper_process_env(),
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


def contain_join_token(*values: object) -> bool:
    return any(JOIN_TOKEN_PLACEHOLDER in str(value) for value in values)


def admit_regular_file(path: Path, label: str, required_mode: int) -> os.stat_result:
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
    mode = stat.S_IMODE(meta.st_mode)
    if mode != required_mode:
        if mode & (stat.S_IWGRP | stat.S_IWOTH):
            refuse(f"{label} is group/other writable")
        refuse(f"{label} mode is not {required_mode:04o}")
    return meta


def admit_env_file(path: Path, worktree: Path) -> Path:
    admit_regular_file(path, "env file", ENV_MODE)
    resolved = dest_resolved(path)
    if is_inside(resolved, worktree):
        refuse("env file is inside the git worktree")
    if contain_join_token(resolved):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    return resolved


def admit_dest_identity(path: Path, label: str, required_mode: int) -> Path:
    admit_regular_file(path, label, required_mode)
    resolved = dest_resolved(path)
    if not SAFE_PATH.match(str(resolved)):
        refuse(f"{label} path is not a bound assignment value")
    if contain_join_token(resolved):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    return resolved


def read_regular(path: Path, label: str, limit: int) -> bytes:
    meta = path.lstat()
    if meta.st_size > limit:
        refuse(f"{label} exceeds its bound")
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
        body = os.read(fd, limit + 1)
        after = os.fstat(fd)
    finally:
        os.close(fd)
    if not body:
        refuse(f"{label} is empty")
    if len(body) > limit:
        refuse(f"{label} exceeds its bound")
    if len(body) != meta.st_size or after.st_size != meta.st_size:
        refuse(f"{label} changed while being read")
    return body


def parse_env_body(body: bytes) -> tuple[Path, Path]:
    if contain_join_token(body):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    try:
        text = body.decode("ascii")
    except UnicodeDecodeError:
        refuse("env file is not ASCII")
        raise AssertionError
    if "\r" in text:
        refuse("env file must be exactly two assignments")
    if not text.endswith("\n"):
        refuse("env file must be exactly two assignments")
    lines = text.split("\n")[:-1]
    if len(lines) != 2:
        refuse("env file must be exactly two assignments")
    parsed: list[tuple[str, str]] = []
    for line in lines:
        if not line or line.startswith("#") or line.startswith(" ") or line.count("=") != 1:
            refuse("env file keys are extra or missing")
        key, value = line.split("=", 1)
        if key not in ENV_KEYS or not value:
            refuse("env file keys are extra or missing")
        parsed.append((key, value))
    if [key for key, _value in parsed] != list(ENV_KEYS):
        refuse("env file keys are extra or missing")
    dest_key = Path(parsed[0][1])
    dest_known_hosts = Path(parsed[1][1])
    if contain_join_token(dest_key, dest_known_hosts):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    if not SAFE_PATH.match(str(dest_key)) or not SAFE_PATH.match(str(dest_known_hosts)):
        refuse("dest path is not a bound assignment value")
    return dest_key, dest_known_hosts


def write_exclusive(path: Path, data: bytes, mode: int, label: str) -> os.stat_result:
    if not data:
        refuse(f"{label} is empty")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0)
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        fd = os.open(path, flags, mode)
    except FileExistsError as error:
        refuse(f"{label} already exists; run is no-replace")
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


def admit_sidecar_path(path: Path, worktree: Path, reserved: set[Path]) -> Path:
    if path.exists() or path.is_symlink():
        try:
            if path.is_symlink() or stat.S_ISLNK(path.lstat().st_mode):
                refuse("sidecar is a symlink")
        except OSError:
            refuse("sidecar is a symlink")
        refuse("sidecar already exists; run is no-replace")
    parent = path.parent
    try:
        meta = parent.lstat()
    except OSError as error:
        refuse("sidecar parent is missing")
        raise AssertionError from error
    if stat.S_ISLNK(meta.st_mode):
        refuse("sidecar parent is a symlink")
    if not stat.S_ISDIR(meta.st_mode):
        refuse("sidecar parent is not a directory")
    resolved = dest_resolved(path)
    if is_inside(resolved, worktree):
        refuse("sidecar is inside the git worktree")
    if contain_join_token(resolved):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    if resolved in reserved:
        refuse("sidecar must be distinct from dest identity files and dest env file")
    return resolved


def bind_sidecar(
    dest_key: Path,
    key_meta: os.stat_result,
    key_digest: str,
    dest_known_hosts: Path,
    hosts_meta: os.stat_result,
    hosts_digest: str,
    command: list[str],
    sidecar_path: Path,
) -> dict[str, object]:
    record = {
        "command_argv": list(command),
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
    if "MACKESD_BOOTSTRAP_SSH_KEY" in record["command_argv"]:
        refuse("command argv must not carry env values")
    if "MACKESD_BOOTSTRAP_KNOWN_HOSTS" in record["command_argv"]:
        refuse("command argv must not carry env values")
    return record


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"


def write_run_sidecar(
    dest_key: Path,
    dest_known_hosts: Path,
    command: list[str],
    sidecar: Path,
    worktree: Path,
    reserved: set[Path],
) -> None:
    sidecar = admit_sidecar_path(sidecar, worktree, reserved)
    key_body = read_regular(dest_key, "dest key", MAX_DEST_BYTES)
    hosts_body = read_regular(dest_known_hosts, "dest known-hosts", MAX_DEST_BYTES)
    key_meta = dest_key.lstat()
    hosts_meta = dest_known_hosts.lstat()
    record = bind_sidecar(
        dest_key,
        key_meta,
        hashlib.sha256(key_body).hexdigest(),
        dest_known_hosts,
        hosts_meta,
        hashlib.sha256(hosts_body).hexdigest(),
        command,
        sidecar,
    )
    write_exclusive(sidecar, canonical(record), SIDECAR_MODE, "sidecar")


def child_environment(dest_key: Path, dest_known_hosts: Path) -> dict[str, str]:
    child_env = os.environ.copy()
    for name in ("JOIN_TOKEN", candidate.DEST_ENV, warning.HELPER_ENV):
        child_env.pop(name, None)
    child_env[ENV_KEYS[0]] = str(dest_key)
    child_env[ENV_KEYS[1]] = str(dest_known_hosts)
    return child_env


def run_with_env(env_file: Path, command: list[str], sidecar: Path | None = None) -> int:
    if not command:
        refuse("command argv is empty")
    admit_not_lifecycle_mutation(command)
    worktree = helper_worktree_root()
    env_file = admit_env_file(env_file, worktree)
    body = read_regular(env_file, "env file", MAX_ENV_BYTES)
    dest_key, dest_known_hosts = parse_env_body(body)
    dest_key = admit_dest_identity(dest_key, "dest key", KEY_MODE)
    dest_known_hosts = admit_dest_identity(dest_known_hosts, "dest known-hosts", KNOWN_HOSTS_MODE)
    if dest_key == dest_known_hosts:
        refuse("dest key and dest known-hosts must be distinct paths")
    if sidecar is not None:
        write_run_sidecar(
            dest_key,
            dest_known_hosts,
            command,
            sidecar,
            worktree,
            {env_file, dest_key, dest_known_hosts},
        )
    try:
        completed = subprocess.run(command, env=child_environment(dest_key, dest_known_hosts))
    except OSError as error:
        refuse(f"command cannot be started: {error}")
        raise AssertionError from error
    for name in ENV_KEYS:
        if name in os.environ:
            refuse("helper process must not carry dest identity env")
    return completed.returncode


def main() -> int:
    if "--" not in sys.argv:
        refuse("command argv must follow --")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env-file", type=Path, default=DEFAULT_ENV_FILE)
    parser.add_argument("--print-sidecar", type=Path, default=None)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = list(args.command)
    if command[:1] == ["--"]:
        command = command[1:]
    return run_with_env(args.env_file, command, args.print_sidecar)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, Refusal) as error:
        print(f"REFUSED: {error}", file=sys.stderr)
        raise SystemExit(EXIT_REFUSED)
