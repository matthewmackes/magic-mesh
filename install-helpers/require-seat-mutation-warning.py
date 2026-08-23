#!/usr/bin/env python3
"""Run the governed seat mutation warning before leftover (3) mutation.

Leftover (3) is live enroll/offboard under red `AI-GENERATED-ALERT` + 5s.
Mint and dest-env mutation must not skip `seat-update-warning.sh` after a
candidate dest admits. This leftover does not enroll and does not publish
the live toast from tests.
"""

from __future__ import annotations

import argparse
import os
import stat
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_HELPER = HERE / "seat-update-warning.sh"
HELPER_ENV = "MCNF_SEAT_MUTATION_WARNING"
ALERT_FLAG = "AI-GENERATED-ALERT"
WAIT_MARK = "WAIT_SECONDS=5"
EXIT_REFUSED = 2
# Dest identity and join-token env must not leak into the warning child.
# Login leftover (2): only the dest-env runner sources those vars.
DEST_CHILD_ENV_STRIP = (
    "MACKESD_BOOTSTRAP_SSH_KEY",
    "MACKESD_BOOTSTRAP_KNOWN_HOSTS",
    "JOIN_TOKEN",
    "MCNF_UNPUBLISHED_SIGNED_CANDIDATE",
    HELPER_ENV,
)


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def dest_resolved(path: Path) -> Path:
    return path.parent.resolve() / path.name


def default_helper() -> Path:
    raw = os.environ.get(HELPER_ENV)
    if raw:
        return Path(raw)
    return DEFAULT_HELPER


def resolve_warning_helper(
    path: Path | None = None,
    *,
    for_production_mutation: bool = False,
) -> Path:
    if for_production_mutation:
        return dest_resolved(DEFAULT_HELPER)
    if path is not None:
        return dest_resolved(path)
    return dest_resolved(default_helper())


def admit_warning_helper(
    path: Path | None = None,
    *,
    for_production_mutation: bool = False,
) -> Path:
    helper = resolve_warning_helper(path, for_production_mutation=for_production_mutation)
    try:
        meta = helper.lstat()
    except OSError:
        refuse("seat mutation warning helper is missing")
        raise AssertionError
    if stat.S_ISLNK(meta.st_mode):
        refuse("seat mutation warning helper is a symlink")
    if not stat.S_ISREG(meta.st_mode):
        refuse("seat mutation warning helper must be a regular file")
    if not (meta.st_mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)):
        refuse("seat mutation warning helper must be executable")
    try:
        body = helper.read_text(encoding="utf-8")
    except OSError:
        refuse("seat mutation warning helper is unreadable")
        raise AssertionError
    if ALERT_FLAG not in body:
        refuse("seat mutation warning helper lacks AI-GENERATED-ALERT")
    if WAIT_MARK not in body:
        refuse("seat mutation warning helper does not pin a five-second interval")
    return helper


def require_seat_mutation_warning(
    path: Path | None = None,
    *,
    for_production_mutation: bool = False,
) -> None:
    helper = admit_warning_helper(path, for_production_mutation=for_production_mutation)
    child_env = os.environ.copy()
    for name in DEST_CHILD_ENV_STRIP:
        child_env.pop(name, None)
    try:
        completed = subprocess.run(
            [str(helper)],
            check=False,
            capture_output=True,
            text=True,
            env=child_env,
        )
    except OSError as error:
        refuse(f"seat mutation warning cannot be started: {error}")
        raise AssertionError from error
    if completed.returncode != 0:
        refuse("seat mutation warning failed; mutation was not started")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path, default=None)
    parser.add_argument("--admit-only", action="store_true")
    args = parser.parse_args()
    if args.admit_only:
        admit_warning_helper(args.helper)
        print("require-seat-mutation-warning: helper admitted; toast not published")
        return 0
    require_seat_mutation_warning(args.helper)
    print("require-seat-mutation-warning: warning completed; production_admitted=false")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, Refusal) as error:
        print(f"REFUSED: {error}", file=sys.stderr)
        raise SystemExit(EXIT_REFUSED)
