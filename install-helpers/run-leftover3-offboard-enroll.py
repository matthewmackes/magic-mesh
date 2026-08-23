#!/usr/bin/env python3
"""Leftover (3): dest-backed leave then enroll --token-stdin.

Operator 2026-08-23: create the confirmation dest and run live-seat
offboard+reenroll. Confirmation seed never leaves the control host.
Bearer and join token never print. production_admitted stays false.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
SIGN = HERE / "sign-lifecycle-confirmation.py"
WARNING = HERE / "seat-update-warning.sh"
EXIT_REFUSED = 2
BEARER_LEN = 43


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def dest_resolved(path: Path) -> Path:
    return path.parent.resolve() / path.name


def read_regular(path: Path, mode_mask: int, label: str) -> bytes:
    dest = dest_resolved(path)
    try:
        meta = dest.lstat()
    except OSError as error:
        refuse(f"{label} dest is missing")
        raise AssertionError from error
    if stat.S_ISLNK(meta.st_mode) or not stat.S_ISREG(meta.st_mode):
        refuse(f"{label} dest must be a regular non-symlink file")
    if stat.S_IMODE(meta.st_mode) & mode_mask:
        refuse(f"{label} dest mode is too open")
    return dest.read_bytes()


def ssh_argv(user_host: str, identity: Path, remote: str) -> list[str]:
    return [
        "ssh",
        "-i",
        str(identity),
        "-o",
        "BatchMode=yes",
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "ConnectTimeout=15",
        user_host,
        remote,
    ]


def run_ssh(user_host: str, identity: Path, remote: str, stdin: bytes | None = None) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        ssh_argv(user_host, identity, remote),
        input=stdin,
        capture_output=True,
        check=False,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seat", required=True, help="user@underlay")
    parser.add_argument("--identity", type=Path, required=True)
    parser.add_argument("--seed", type=Path, required=True)
    parser.add_argument("--bearer", type=Path, required=True)
    parser.add_argument("--enroll-fp", type=Path, required=True)
    parser.add_argument("--mesh-id", required=True)
    parser.add_argument("--lighthouse", required=True)
    parser.add_argument("--confirmation-out", type=Path, required=True)
    parser.add_argument("--lead-seconds", type=int, default=12)
    args = parser.parse_args()
    try:
        bearer = read_regular(args.bearer, 0o177, "enroll bearer").decode("ascii").strip()
        if len(bearer) != BEARER_LEN:
            refuse("enroll bearer dest is not 43 characters")
        fingerprint = read_regular(args.enroll_fp, 0o377, "enroll fingerprint").decode("ascii").strip()
        if len(fingerprint) != 64 or any(c not in "0123456789abcdef" for c in fingerprint):
            refuse("enroll fingerprint dest is not 64 lowercase hex")
        probe = run_ssh(args.seat, args.identity, "hostname; date +%s")
        if probe.returncode != 0:
            refuse("seat probe failed")
        lines = probe.stdout.decode("ascii", errors="strict").strip().splitlines()
        if len(lines) != 2:
            refuse("seat probe did not return hostname and unix time")
        hostname, seat_now = lines[0].strip(), int(lines[1].strip())
        if not hostname or hostname == "unknown":
            refuse("seat hostname is missing")
        generation = seat_now + max(args.lead_seconds, 8)
        # leave.rs: node_id=peer:<hostname>, request_id=offboard-{node_id}-{generation}
        session_id = f"offboard-peer:{hostname}-{generation}"
        scope = hashlib.sha256(f"peer:{hostname}".encode("ascii")).hexdigest()
        if dest_resolved(args.confirmation_out).exists():
            refuse("confirmation dest already exists")
        signed = subprocess.run(
            [
                sys.executable,
                str(SIGN),
                "--seed",
                str(args.seed),
                "--session-id",
                session_id,
                "--generation",
                str(generation),
                "--scope-digest-hex",
                scope,
                "--output",
                str(args.confirmation_out),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        if signed.returncode != 0:
            refuse(signed.stderr.strip() or "confirmation sign failed")
        confirmation = dest_resolved(args.confirmation_out).read_text(encoding="ascii")
        seed = read_regular(args.seed, 0o077, "confirmation seed")
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

        verifying = (
            Ed25519PrivateKey.from_private_bytes(seed)
            .public_key()
            .public_bytes(Encoding.Raw, PublicFormat.Raw)
            .hex()
        )
        copied = subprocess.run(
            [
                "scp",
                "-i",
                str(args.identity),
                "-o",
                "BatchMode=yes",
                "-o",
                "IdentitiesOnly=yes",
                str(WARNING),
                f"{args.seat}:/tmp/seat-update-warning.sh",
            ],
            capture_output=True,
            check=False,
        )
        if copied.returncode != 0:
            refuse("could not copy seat-update-warning.sh")
        warn = run_ssh(args.seat, args.identity, "bash /tmp/seat-update-warning.sh")
        if warn.returncode != 0:
            refuse("seat mutation warning failed")
        remote_leave = (
            "python3 - <<'PY'\n"
            "import json,os,subprocess,sys,time\n"
            f"need={generation}\n"
            "now=int(time.time())\n"
            "if now>need:\n"
            "    sys.exit(75)\n"
            "while int(time.time())<need:\n"
            "    time.sleep(0.05)\n"
            "if int(time.time())!=need:\n"
            "    sys.exit(75)\n"
            "conf=sys.stdin.read()\n"
            f"key={verifying!r}\n"
            "argv=['sudo','-n','/usr/bin/mackesd','leave','--yes',"
            "'--confirmation-json',conf,'--verifying-key-hex',key]\n"
            "raise SystemExit(subprocess.run(argv).returncode)\n"
            "PY\n"
        )
        leave = run_ssh(args.seat, args.identity, remote_leave, stdin=confirmation.encode("ascii"))
        if leave.returncode == 75:
            refuse("leave missed the confirmation generation window")
        if leave.returncode != 0:
            refuse("leave failed on the seat")
        token = f"mesh:{args.mesh_id}@{args.lighthouse}:4243#{bearer}?fp={fingerprint}"
        enroll = run_ssh(
            args.seat,
            args.identity,
            "sudo -n /usr/bin/mackesd enroll --token-stdin --role workstation",
            stdin=f"{token}\n".encode("ascii"),
        )
        if enroll.returncode != 0:
            refuse("enroll --token-stdin failed on the seat")
        print(
            "run-leftover3-offboard-enroll: leave+enroll completed; "
            f"seat_host={hostname}; production_admitted=false; enroll_succeeded=true"
        )
        return 0
    except (OSError, Refusal, ValueError, UnicodeError) as error:
        print(f"run-leftover3-offboard-enroll: REFUSED: {error}", file=sys.stderr)
        return EXIT_REFUSED


if __name__ == "__main__":
    raise SystemExit(main())
