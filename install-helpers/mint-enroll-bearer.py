#!/usr/bin/env python3
"""Extract a 43-char enroll bearer from a caller-supplied mackesd binary.

Wraps `mackesd enroll-token` supplied by `--mackesd`. It does not mint into
a production workgroup, does not SSH, and does not enroll. The bearer is
written only to `--output`. Helper stdout never carries bearer or token
bytes. Dest or sidecar under `/root/mcnf-private` refuses while the
unpublished signed candidate dest is absent. After dest admit, production
mint runs `seat-update-warning.sh`. This leftover does not claim a
production mint.
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
DEFAULT_DEST_PARENT = Path("/root/mcnf-private")
PRODUCTION_DEST_PARENTS = (DEFAULT_DEST_PARENT,)
SIDECAR_KIND = "mcnf-enroll-bearer-mint"
DEST_MODE = 0o600
SIDECAR_MODE = 0o400
EXIT_REFUSED = 2
BEARER_LEN = 43
NOTE_MAX_BYTES = 256
JOIN_TOKEN_PLACEHOLDER = "{{JOIN_TOKEN}}"
BEARER_CHARS = re.compile(r"^[A-Za-z0-9_-]+$")
TOKEN_RE = re.compile(
    r"^mesh:([A-Za-z0-9._-]+)@([^@#/\s]+):([1-9][0-9]{0,4})#"
    r"([A-Za-z0-9_-]+)(?:\?fp=([0-9a-f]{64}))?$"
)
OPENSSH_MARK = b"BEGIN OPENSSH"
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
        env=child_environment(None),
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


def under_production_dest(path: Path) -> bool:
    resolved = dest_resolved(path)
    for parent in PRODUCTION_DEST_PARENTS:
        try:
            resolved.relative_to(parent.resolve())
        except ValueError:
            continue
        return True
    return False


def admit_not_production_dest(path: Path, label: str) -> None:
    if under_production_dest(path):
        try:
            candidate.admit_unpublished_signed_candidate(for_production_mutation=True)
        except candidate.Refusal as error:
            refuse(f"{label} is a production dest; {error}")
        try:
            warning.require_seat_mutation_warning(for_production_mutation=True)
        except warning.Refusal as error:
            refuse(str(error))


def contain_join_token(*values: object) -> bool:
    return any(JOIN_TOKEN_PLACEHOLDER in str(value) for value in values)


def contain_openssh(*values: object) -> bool:
    for value in values:
        raw = value if isinstance(value, (bytes, bytearray)) else str(value).encode("utf-8", "replace")
        if OPENSSH_MARK in raw:
            return True
    return False


def admit_mackesd(path: Path) -> Path:
    try:
        meta = path.lstat()
    except OSError as error:
        refuse("mackesd is missing or inaccessible")
        raise AssertionError from error
    if stat.S_ISLNK(meta.st_mode):
        refuse("mackesd is a symlink")
    if not stat.S_ISREG(meta.st_mode):
        refuse("mackesd must be a regular executable")
    if not (meta.st_mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)):
        refuse("mackesd must be a regular executable")
    resolved = dest_resolved(path)
    if contain_join_token(resolved):
        refuse("JOIN_TOKEN placeholder is not a bearer")
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
    if stat.S_IMODE(meta.st_mode) & (stat.S_IWGRP | stat.S_IWOTH):
        refuse(f"{label} parent is group/other writable")
    return parent.resolve()


def admit_no_replace(path: Path, label: str, worktree: Path) -> Path:
    if path.exists() or path.is_symlink():
        try:
            if path.is_symlink() or stat.S_ISLNK(path.lstat().st_mode):
                refuse(f"{label} is a symlink")
        except OSError:
            refuse(f"{label} is a symlink")
        refuse(f"{label} already exists; mint is no-replace")
    admit_dest_parent(path, label)
    resolved = dest_resolved(path)
    if not SAFE_PATH.match(str(resolved)):
        refuse(f"{label} path is not a bound assignment value")
    if is_inside(resolved, worktree):
        refuse(f"{label} is inside the git worktree")
    if contain_join_token(resolved):
        refuse("JOIN_TOKEN placeholder is not a bearer")
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
        refuse(f"{label} already exists; mint is no-replace")
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


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"


def admit_note(note: str | None) -> str:
    text = "" if note is None else note
    if len(text.encode("utf-8")) > NOTE_MAX_BYTES:
        refuse("note exceeds 256 bytes")
    if any(ord(ch) < 32 or ord(ch) == 127 for ch in text):
        refuse("note must not contain control characters")
    if contain_join_token(text):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    return text


def admit_mesh_id(mesh_id: str) -> str:
    if not mesh_id or not re.fullmatch(r"[A-Za-z0-9._-]+", mesh_id):
        refuse("mesh-id is not URL-safe")
    if contain_join_token(mesh_id):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    return mesh_id


def admit_lighthouse(lighthouse: str | None) -> str | None:
    if lighthouse is None:
        return None
    if not lighthouse or any(ord(ch) < 32 or ord(ch) == 127 or ch.isspace() for ch in lighthouse):
        refuse("lighthouse is not a bound host")
    if contain_join_token(lighthouse):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    return lighthouse


def admit_workgroup_root(path: Path | None) -> Path | None:
    if path is None:
        return None
    resolved = dest_resolved(path)
    if contain_join_token(resolved):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    try:
        meta = resolved.lstat()
    except OSError as error:
        refuse("workgroup-root is missing or inaccessible")
        raise AssertionError from error
    if stat.S_ISLNK(meta.st_mode):
        refuse("workgroup-root is a symlink")
    if not stat.S_ISDIR(meta.st_mode):
        refuse("workgroup-root must be a directory")
    return resolved


# Dest identity and join-token env must not leak into enroll-token.
# Login leftover (2): only the dest-env runner sources those vars.
DEST_CHILD_ENV_STRIP = (
    "MACKESD_BOOTSTRAP_SSH_KEY",
    "MACKESD_BOOTSTRAP_KNOWN_HOSTS",
    "JOIN_TOKEN",
    candidate.DEST_ENV,
    warning.HELPER_ENV,
)


def child_environment(workgroup_root: Path | None) -> dict[str, str]:
    child_env = os.environ.copy()
    for name in DEST_CHILD_ENV_STRIP:
        child_env.pop(name, None)
    if workgroup_root is None:
        return child_env
    # enroll-token has no --workgroup-root flag; mackesd honors MDE_WORKGROUP_ROOT.
    # Do not invent a second ledger or a CLI flag the binary does not accept.
    child_env["MDE_WORKGROUP_ROOT"] = str(workgroup_root)
    return child_env


def enroll_argv(
    mackesd: Path,
    mesh_id: str,
    lighthouse: str | None,
    note: str,
) -> list[str]:
    argv = [str(mackesd), "enroll-token", "--mesh-id", mesh_id]
    if lighthouse is not None:
        argv.extend(["--lighthouse", lighthouse])
    if note:
        argv.extend(["--note", note])
    return argv


def token_matches(line: str) -> re.Match[str] | None:
    return TOKEN_RE.fullmatch(line)


def extract_bearer(stdout: bytes) -> str:
    if contain_openssh(stdout):
        refuse("mackesd output is not a join token")
    try:
        text = stdout.decode("ascii")
    except UnicodeDecodeError:
        refuse("mackesd stdout is not a join token")
        raise AssertionError
    if text.count("\n") > 1 or (text.count("\n") == 1 and not text.endswith("\n")):
        refuse("mackesd stdout is not exactly one token line")
    line = text[:-1] if text.endswith("\n") else text
    if "\n" in line or "\r" in line:
        refuse("mackesd stdout is not exactly one token line")
    matches = list(TOKEN_RE.finditer(line))
    if len(matches) != 1:
        refuse("mackesd stdout is not exactly one token line")
    if line != matches[0].group(0):
        refuse("mackesd stdout is not exactly one token line")
    match = token_matches(line)
    if match is None:
        refuse("mackesd stdout is not a join token")
        raise AssertionError
    bearer = match.group(4)
    if contain_join_token(line, bearer):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    if len(bearer) != BEARER_LEN or BEARER_CHARS.fullmatch(bearer) is None:
        refuse("enrollment bearer must be 43 URL-safe characters")
    return bearer


def bind_sidecar(
    dest: Path,
    dest_meta: os.stat_result,
    dest_digest: str,
    mesh_id: str,
    note: str,
    sidecar_path: Path,
) -> dict[str, object]:
    record = {
        "bearer_sha256": dest_digest,
        "dest": {
            "bytes": int(dest_meta.st_size),
            "mode": f"{stat.S_IMODE(dest_meta.st_mode):04o}",
            "path": str(dest),
        },
        "enroll_succeeded": False,
        "kind": SIDECAR_KIND,
        "mesh_id": mesh_id,
        "note_len": len(note.encode("utf-8")),
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


def refuse_if_leaks(*values: object, bearer: str | None = None) -> None:
    for value in values:
        if contain_openssh(value):
            refuse("helper must not print OPENSSH material")
        if bearer and bearer in str(value):
            refuse("helper must not print the bearer")
        if contain_join_token(value):
            refuse("JOIN_TOKEN placeholder is not a bearer")
        text = value if isinstance(value, str) else (
            value.decode("utf-8", "replace") if isinstance(value, (bytes, bytearray)) else str(value)
        )
        if "mesh:" in text and "#" in text:
            refuse("helper must not print a join token")


def run_mint(
    mackesd: Path,
    mesh_id: str,
    output: Path,
    lighthouse: str | None = None,
    note: str | None = None,
    workgroup_root: Path | None = None,
    sidecar: Path | None = None,
) -> None:
    worktree = helper_worktree_root()
    mackesd = admit_mackesd(mackesd)
    mesh_id = admit_mesh_id(mesh_id)
    lighthouse = admit_lighthouse(lighthouse)
    note = admit_note(note)
    workgroup_root = admit_workgroup_root(workgroup_root)
    dest = admit_no_replace(output, "dest", worktree)
    admit_not_production_dest(dest, "dest")
    sidecar_path = None
    if sidecar is not None:
        sidecar_path = admit_no_replace(sidecar, "sidecar", worktree)
        admit_not_production_dest(sidecar_path, "sidecar")
        if sidecar_path == dest:
            refuse("sidecar must be distinct from dest")
    argv = enroll_argv(mackesd, mesh_id, lighthouse, note)
    try:
        completed = subprocess.run(
            argv,
            env=child_environment(workgroup_root),
            capture_output=True,
            check=False,
        )
    except OSError as error:
        refuse(f"mackesd cannot be started: {error}")
        raise AssertionError from error
    if contain_openssh(completed.stdout, completed.stderr):
        refuse("mackesd output is not a join token")
    if completed.returncode != 0:
        refuse("mackesd enroll-token failed")
    bearer = extract_bearer(completed.stdout)
    dest_bytes = bearer.encode("ascii")
    dest_meta = write_exclusive(dest, dest_bytes, DEST_MODE, "dest")
    dest_digest = hashlib.sha256(dest_bytes).hexdigest()
    if sidecar_path is not None:
        record = bind_sidecar(dest, dest_meta, dest_digest, mesh_id, note, sidecar_path)
        body = canonical(record)
        refuse_if_leaks(body, bearer=bearer)
        write_exclusive(sidecar_path, body, SIDECAR_MODE, "sidecar")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mackesd", type=Path, required=True)
    parser.add_argument("--mesh-id", required=True)
    parser.add_argument("--lighthouse", default=None)
    parser.add_argument("--note", default=None)
    parser.add_argument("--workgroup-root", type=Path, default=None)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--sidecar", type=Path, default=None)
    args = parser.parse_args()
    run_mint(
        args.mackesd,
        args.mesh_id,
        args.output,
        lighthouse=args.lighthouse,
        note=args.note,
        workgroup_root=args.workgroup_root,
        sidecar=args.sidecar,
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, Refusal) as error:
        message = f"REFUSED: {error}"
        if OPENSSH_MARK in message.encode("utf-8", "replace") or "mesh:" in message:
            message = "REFUSED: helper refused without printing token material"
        print(message, file=sys.stderr)
        raise SystemExit(EXIT_REFUSED)
