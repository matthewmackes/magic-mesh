#!/usr/bin/env python3
"""Admit a dest-backed unpublished signed three-RPM candidate.

Leftover (1)/(3) must not hardcode "candidate is absent". This helper
reads a dest sidecar and the three role RPM files it names. Missing dest
refuses with that phrase. A present dest still refuses published,
production_admitted, digest mismatch, or the wrong signer fingerprint.
This leftover does not create a candidate and does not enroll.
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
DEFAULT_DEST = Path("/root/mcnf-private/unpublished-signed-candidate.json")
DEST_ENV = "MCNF_UNPUBLISHED_SIGNED_CANDIDATE"
KIND = "mcnf-unpublished-signed-candidate"
SIGNER_FINGERPRINT = "06B1C27EA0E08A225155EB3314018AA1497DDC7C"
ROLES = ("workstation", "server", "lighthouse")
DEST_MODE = 0o400
EXIT_REFUSED = 2
JOIN_TOKEN_PLACEHOLDER = "{{JOIN_TOKEN}}"
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
SAFE_PATH = re.compile(r"^/[A-Za-z0-9._/-]+$")
NEVRA_RE = re.compile(r"^[A-Za-z0-9._+-]+$")
ROLE_NEVRA_PREFIX = {
    "workstation": "magic-mesh-13.0.0-",
    "server": "magic-mesh-server-13.0.0-",
    "lighthouse": "magic-mesh-lighthouse-13.0.0-",
}
PRODUCTION_DEST_PARENT = Path("/root/mcnf-private")
SIGNER_KEY_ID = SIGNER_FINGERPRINT[-8:].lower()
# Leftover (2): dest identity and join-token env stay off rpm/git children.
DEST_CHILD_ENV_STRIP = (
    "MACKESD_BOOTSTRAP_SSH_KEY",
    "MACKESD_BOOTSTRAP_KNOWN_HOSTS",
    "JOIN_TOKEN",
)


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def child_process_env() -> dict[str, str]:
    env = os.environ.copy()
    for name in DEST_CHILD_ENV_STRIP:
        env.pop(name, None)
    return env


def helper_worktree_root() -> Path:
    result = subprocess.run(
        ["git", "-C", str(HERE), "rev-parse", "--show-toplevel"],
        check=False,
        capture_output=True,
        text=True,
        env=child_process_env(),
    )
    root = result.stdout.strip()
    if result.returncode == 0 and root:
        return Path(root).resolve()
    return HERE.parent.resolve()


def dest_resolved(path: Path) -> Path:
    return path.parent.resolve() / path.name


def contain_join_token(*values: object) -> bool:
    return any(JOIN_TOKEN_PLACEHOLDER in str(value) for value in values)


def default_dest() -> Path:
    raw = os.environ.get(DEST_ENV)
    if raw:
        return Path(raw)
    return DEFAULT_DEST


def under_production_dest(path: Path) -> bool:
    resolved = dest_resolved(path)
    try:
        resolved.relative_to(PRODUCTION_DEST_PARENT.resolve())
    except ValueError:
        return False
    return True


def verify_rpm_signature(rpm: Path) -> None:
    """Require a GPG signature from the governed fingerprint.

    `rpm --checksig` exits 0 on unsigned packages that only have payload
    digests. Production mutation must not treat that as a signed candidate.
    """
    resolved = dest_resolved(rpm)
    try:
        completed = subprocess.run(
            ["rpm", "--checksig", "-v", str(resolved)],
            check=False,
            capture_output=True,
            text=True,
            env=child_process_env(),
        )
    except OSError:
        refuse("rpm signature verify is required for a production candidate dest")
        raise AssertionError
    text = f"{completed.stdout}{completed.stderr}"
    if contain_join_token(text):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    if completed.returncode != 0:
        refuse("RPM signature did not verify")
    lowered = text.lower()
    if "not ok" in lowered or "nokey" in lowered:
        refuse("RPM signature did not verify")
    if "signature" not in lowered:
        refuse("RPM is not GPG-signed")
    if SIGNER_FINGERPRINT.lower() not in lowered and SIGNER_KEY_ID not in lowered:
        refuse("RPM is not signed by the governed fingerprint")


def admit_regular_file(path: Path, label: str, mode: int | None = None) -> os.stat_result:
    resolved = dest_resolved(path)
    if contain_join_token(resolved):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    if not SAFE_PATH.match(str(resolved)):
        refuse(f"{label} path is not a bound assignment value")
    try:
        meta = resolved.lstat()
    except OSError:
        refuse(f"{label} is missing or inaccessible")
        raise AssertionError
    if stat.S_ISLNK(meta.st_mode):
        refuse(f"{label} is a symlink")
    if not stat.S_ISREG(meta.st_mode):
        refuse(f"{label} must be a regular file")
    if mode is not None and stat.S_IMODE(meta.st_mode) != mode:
        refuse(f"{label} mode must be {mode:04o}")
    if stat.S_IMODE(meta.st_mode) & (stat.S_IWGRP | stat.S_IWOTH):
        refuse(f"{label} is group/other writable")
    return meta


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def admit_role(name: str, body: object, worktree: Path) -> dict[str, object]:
    if not isinstance(body, dict):
        refuse(f"{name} role is not an object")
    path_raw = body.get("path")
    sha_raw = body.get("sha256")
    nevra = body.get("nevra")
    if not isinstance(path_raw, str) or not isinstance(sha_raw, str) or not isinstance(nevra, str):
        refuse(f"{name} role fields are missing")
    if contain_join_token(path_raw, sha_raw, nevra):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    if not SHA256_RE.fullmatch(sha_raw):
        refuse(f"{name} sha256 is not 64 hex")
    if not NEVRA_RE.fullmatch(nevra):
        refuse(f"{name} nevra is not a bound RPM identity")
    prefix = ROLE_NEVRA_PREFIX[name]
    if not nevra.startswith(prefix):
        refuse(f"{name} nevra is not the 13.0.0 {name} RPM")
    rpm = dest_resolved(Path(path_raw))
    if not str(rpm).endswith(".rpm"):
        refuse(f"{name} path is not an RPM")
    try:
        rpm.relative_to(worktree)
    except ValueError:
        pass
    else:
        refuse(f"{name} RPM is inside the git worktree")
    admit_regular_file(rpm, f"{name} RPM")
    digest = file_sha256(rpm)
    if digest != sha_raw:
        refuse(f"{name} RPM digest does not match the dest sidecar")
    return {"path": str(rpm), "sha256": digest, "nevra": nevra}


def admit_unpublished_signed_candidate(
    dest: Path | None = None,
    *,
    for_production_mutation: bool = False,
) -> dict[str, object]:
    worktree = helper_worktree_root()
    if for_production_mutation:
        dest_path = dest_resolved(DEFAULT_DEST)
    else:
        dest_path = dest_resolved(dest if dest is not None else default_dest())
    if contain_join_token(dest_path):
        refuse("JOIN_TOKEN placeholder is not a bearer")
    if not dest_path.exists() and not dest_path.is_symlink():
        refuse("unpublished signed candidate is absent")
    try:
        dest_path.relative_to(worktree)
    except ValueError:
        pass
    else:
        refuse("candidate dest is inside the git worktree")
    admit_regular_file(dest_path, "candidate dest", DEST_MODE)
    try:
        record = json.loads(dest_path.read_text(encoding="ascii"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        refuse("candidate dest is not a bound sidecar")
        raise AssertionError
    if not isinstance(record, dict):
        refuse("candidate dest is not a bound sidecar")
    if record.get("kind") != KIND:
        refuse("candidate dest kind is unsupported")
    if record.get("schema_version") != 1:
        refuse("candidate dest schema is unsupported")
    if record.get("published") is not False:
        refuse("candidate dest is published; leftover requires unpublished")
    if record.get("production_admitted") is not False:
        refuse("helper must never mark production_admitted")
    if record.get("signer_fingerprint") != SIGNER_FINGERPRINT:
        refuse("candidate dest signer is not the governed RPM fingerprint")
    roles_raw = record.get("roles")
    if not isinstance(roles_raw, dict):
        refuse("candidate dest roles are missing")
    roles = {}
    for name in ROLES:
        roles[name] = admit_role(name, roles_raw.get(name), worktree)
    if set(roles_raw) != set(ROLES):
        refuse("candidate dest roles must be exactly workstation, server, lighthouse")
    if for_production_mutation or under_production_dest(dest_path):
        for name in ROLES:
            verify_rpm_signature(Path(str(roles[name]["path"])))
    return {
        "kind": KIND,
        "production_admitted": False,
        "published": False,
        "roles": roles,
        "schema_version": 1,
        "signer_fingerprint": SIGNER_FINGERPRINT,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dest", type=Path, default=None)
    args = parser.parse_args()
    admit_unpublished_signed_candidate(args.dest)
    print("admit-unpublished-signed-candidate: admitted dest; production_admitted=false")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, Refusal) as error:
        message = f"REFUSED: {error}"
        if JOIN_TOKEN_PLACEHOLDER in message or "mesh:" in message:
            message = "REFUSED: helper refused without printing token material"
        print(message, file=sys.stderr)
        raise SystemExit(EXIT_REFUSED)
