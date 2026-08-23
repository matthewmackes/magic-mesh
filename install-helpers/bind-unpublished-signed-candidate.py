#!/usr/bin/env python3
"""Write a dest-backed unpublished signed three-RPM candidate sidecar.

Admit leftover (1) can read dest; this helper is the no-replace producer.
It never marks production_admitted, never publishes, and never enrolls.
Historical 12.1.6 NEVRAs refuse. A dest under `/root/mcnf-private` also
requires `rpm -qp` identity so fixture bytes cannot unlock mint.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import stat
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


admit = _load(
    "admit_unpublished_signed_candidate",
    HERE / "admit-unpublished-signed-candidate.py",
)

PRODUCTION_DEST_PARENT = Path("/root/mcnf-private")
EXIT_REFUSED = admit.EXIT_REFUSED


def refuse(message: str) -> None:
    admit.refuse(message)


def under_production_dest(path: Path) -> bool:
    resolved = admit.dest_resolved(path)
    try:
        resolved.relative_to(PRODUCTION_DEST_PARENT.resolve())
    except ValueError:
        return False
    return True


def query_nevra(rpm: Path) -> str:
    try:
        completed = subprocess.run(
            ["rpm", "-qp", "--queryformat", "%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}", str(rpm)],
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError:
        refuse("rpm query is required for a production candidate dest")
        raise AssertionError
    if completed.returncode != 0:
        refuse("rpm query failed; fixture bytes are not a candidate")
    nevra = completed.stdout.strip()
    if not nevra or "\n" in nevra:
        refuse("rpm query did not return one NEVRA")
    if admit.contain_join_token(nevra):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    return nevra


def write_exclusive(path: Path, data: bytes, mode: int) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0)
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        fd = os.open(path, flags, mode)
    except FileExistsError:
        refuse("candidate dest already exists; bind is no-replace")
        raise AssertionError
    except OSError as error:
        refuse(f"candidate dest cannot be created: {error}")
        raise AssertionError
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
        refuse("candidate dest must be a singly-used regular file")


def bind_unpublished_signed_candidate(
    workstation: Path,
    server: Path,
    lighthouse: Path,
    dest: Path | None = None,
    workstation_nevra: str | None = None,
    server_nevra: str | None = None,
    lighthouse_nevra: str | None = None,
) -> dict[str, object]:
    worktree = admit.helper_worktree_root()
    dest_path = admit.dest_resolved(dest if dest is not None else admit.default_dest())
    if dest_path.exists() or dest_path.is_symlink():
        refuse("candidate dest already exists; bind is no-replace")
    try:
        dest_path.relative_to(worktree)
    except ValueError:
        pass
    else:
        refuse("candidate dest is inside the git worktree")
    parent = dest_path.parent
    try:
        meta = parent.lstat()
    except OSError:
        refuse("candidate dest parent is missing")
        raise AssertionError
    if stat.S_ISLNK(meta.st_mode):
        refuse("candidate dest parent is a symlink")
    if not stat.S_ISDIR(meta.st_mode):
        refuse("candidate dest parent is not a directory")
    if stat.S_IMODE(meta.st_mode) & (stat.S_IWGRP | stat.S_IWOTH):
        refuse("candidate dest parent is group/other writable")

    require_rpm = under_production_dest(dest_path)
    supplied = {
        "workstation": (workstation, workstation_nevra),
        "server": (server, server_nevra),
        "lighthouse": (lighthouse, lighthouse_nevra),
    }
    roles: dict[str, dict[str, str]] = {}
    for name, (raw, nevra) in supplied.items():
        rpm = admit.dest_resolved(raw)
        admit.admit_regular_file(rpm, f"{name} RPM")
        if not str(rpm).endswith(".rpm"):
            refuse(f"{name} path is not an RPM")
        try:
            rpm.relative_to(worktree)
        except ValueError:
            pass
        else:
            refuse(f"{name} RPM is inside the git worktree")
        if require_rpm:
            queried = query_nevra(rpm)
            if nevra is not None and nevra != queried:
                refuse(f"{name} NEVRA does not match rpm -qp")
            nevra = queried
            admit.verify_rpm_signature(rpm)
        if nevra is None:
            refuse(f"{name} NEVRA is required")
        prefix = admit.ROLE_NEVRA_PREFIX[name]
        if not nevra.startswith(prefix):
            refuse(f"{name} nevra is not the 13.0.0 {name} RPM")
        roles[name] = {
            "path": str(rpm),
            "sha256": admit.file_sha256(rpm),
            "nevra": nevra,
        }

    record = {
        "kind": admit.KIND,
        "production_admitted": False,
        "published": False,
        "roles": roles,
        "schema_version": 1,
        "signer_fingerprint": admit.SIGNER_FINGERPRINT,
    }
    body = json.dumps(record, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"
    if admit.contain_join_token(body.decode("ascii")):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    write_exclusive(dest_path, body, admit.DEST_MODE)
    admitted = admit.admit_unpublished_signed_candidate(dest_path)
    if admitted["production_admitted"] is not False:
        try:
            dest_path.unlink()
        except OSError:
            pass
        refuse("helper must never mark production_admitted")
    return admitted


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workstation", type=Path, required=True)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--lighthouse", type=Path, required=True)
    parser.add_argument("--dest", type=Path, default=None)
    parser.add_argument("--workstation-nevra", default=None)
    parser.add_argument("--server-nevra", default=None)
    parser.add_argument("--lighthouse-nevra", default=None)
    args = parser.parse_args()
    bind_unpublished_signed_candidate(
        args.workstation,
        args.server,
        args.lighthouse,
        dest=args.dest,
        workstation_nevra=args.workstation_nevra,
        server_nevra=args.server_nevra,
        lighthouse_nevra=args.lighthouse_nevra,
    )
    print("bind-unpublished-signed-candidate: wrote dest; production_admitted=false")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, admit.Refusal) as error:
        message = f"REFUSED: {error}"
        if admit.JOIN_TOKEN_PLACEHOLDER in message or "mesh:" in message:
            message = "REFUSED: helper refused without printing token material"
        print(message, file=sys.stderr)
        raise SystemExit(EXIT_REFUSED)
