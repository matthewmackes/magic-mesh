#!/usr/bin/env python3
"""Run and record the narrow, real VDI framebuffer proof.

This helper is intentionally fail-closed.  It only invokes the two existing
ignored shell tests (live VNC or live SPICE), or the existing ignored RDP
integration test, through ``xcp-build.sh`` and only records ``observed`` when
the test prints its exact non-empty ``FRAME OK`` marker.  A reachable TCP
port, a requested session, a fixture, or a failed decoder never becomes
framebuffer evidence.

The output contains no endpoint ticket and no raw probe log; it records the
protocol, bounded frame dimensions/hash, input observation, command result,
and a digest of the captured log for later correlation.

Usage:
  verify-vdi-live-proof.py discover --seat NAME=HOST [--seat NAME=HOST ...]
  verify-vdi-live-proof.py run --protocol rdp --target HOST:PORT[,user,pass] \
    --source-commit <git-sha> --host 172.20.0.90 --slot vdi-proof-1 --out evidence.json
  verify-vdi-live-proof.py validate evidence.json
  verify-vdi-live-proof.py --self-test
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from typing import Any


SCHEMA_VERSION = 1
MAX_LOG_BYTES = 4 * 1024 * 1024
FRAME_RE = re.compile(r"live-shell-(?:vnc|spice): FRAME OK ([1-9][0-9]{0,4})x([1-9][0-9]{0,4}) fnv1a64=(0x[0-9a-fA-F]{16})")
ECHO_RE = re.compile(r"live-shell-(?:vnc|spice): INPUT ECHOED before=(0x[0-9a-fA-F]{16}) after=(0x[0-9a-fA-F]{16})")
UNCHANGED_RE = re.compile(r"live-shell-(?:vnc|spice): INPUT sent; framebuffer unchanged fnv1a64=(0x[0-9a-fA-F]{16})")
RDP_FRAME_RE = re.compile(r"^live: FRAME OK ([1-9][0-9]{0,4})x([1-9][0-9]{0,4})\b.*?fnv1a64=(0x[0-9a-fA-F]{16})\b.*$", re.MULTILINE)
RDP_ECHO_RE = re.compile(r"^live: INPUT ECHOED .*?before=(0x[0-9a-fA-F]{16}) after=(0x[0-9a-fA-F]{16})$", re.MULTILINE)
RDP_UNCHANGED_RE = re.compile(r"^live: INPUT sent OK; framebuffer UNCHANGED .*?fnv1a64=(0x[0-9a-fA-F]{16}).*$", re.MULTILINE)
TARGET_RE = re.compile(r"^(?P<host>[A-Za-z0-9_.:-]+):(?P<port>[0-9]{1,5})(?P<credentials>(?:,[^,]*){0,2})?$")


class ProofError(Exception):
    pass


def die(message: str) -> "NoReturn":
    raise ProofError(message)


def parse_target(raw: str) -> tuple[str, int]:
    match = TARGET_RE.fullmatch(raw)
    if not match:
        die("target must be HOST:PORT[,ticket] with no whitespace")
    host = match.group("host")
    port = int(match.group("port"))
    if not 1 <= port <= 65535:
        die("target port must be from 1 to 65535")
    if any(ch.isspace() or ord(ch) < 32 for ch in raw):
        die("target contains whitespace or control characters")
    return host, port


def source_for_protocol(protocol: str) -> str:
    if protocol == "rdp":
        return "mde-vdi-rdp ignored live integration test"
    return "mde-shell-egui ignored live worker test"


def bounded_log(stdout: bytes, stderr: bytes) -> str:
    combined = stdout + (b"\n" if stdout and stderr else b"") + stderr
    if len(combined) > MAX_LOG_BYTES:
        die(f"probe log exceeds {MAX_LOG_BYTES} bytes")
    return combined.decode("utf-8", errors="replace")


def parse_probe(log: str, protocol: str, returncode: int) -> dict[str, Any]:
    if protocol == "rdp":
        frame_matches = list(RDP_FRAME_RE.finditer(log))
        echo_matches = list(RDP_ECHO_RE.finditer(log))
        unchanged_matches = list(RDP_UNCHANGED_RE.finditer(log))
    else:
        frame_matches = [match for match in FRAME_RE.finditer(log) if match.group(0).startswith(f"live-shell-{protocol}:")]
        echo_matches = [match for match in ECHO_RE.finditer(log) if match.group(0).startswith(f"live-shell-{protocol}:")]
        unchanged_matches = [match for match in UNCHANGED_RE.finditer(log) if match.group(0).startswith(f"live-shell-{protocol}:")]
    if returncode != 0:
        return {"status": "failed", "reason": f"probe exited {returncode}"}
    if len(frame_matches) != 1:
        return {"status": "unavailable", "reason": "exactly one non-empty guest FRAME OK marker was not observed"}

    frame = frame_matches[0]
    input_observation = "not-reported"
    echo = echo_matches[0] if echo_matches else None
    unchanged = unchanged_matches[0] if unchanged_matches else None
    if echo:
        input_observation = "echoed"
    elif unchanged:
        input_observation = "unchanged"
    return {
        "status": "observed",
        "frame": {
            "width": int(frame.group(1)),
            "height": int(frame.group(2)),
            "fnv1a64": frame.group(3).lower(),
        },
        "input_observation": input_observation,
    }


def make_evidence(
    protocol: str,
    target: str,
    returncode: int,
    log: str,
    source_commit: str | None = None,
) -> dict[str, Any]:
    host, port = parse_target(target)
    parsed = parse_probe(log, protocol, returncode)
    evidence: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "source_commit": source_commit,
        "status": parsed["status"],
        "protocol": protocol,
        "target": {"host": host, "port": port},
        "probe": {
            "returncode": returncode,
            "log_sha256": hashlib.sha256(log.encode()).hexdigest(),
            "source": source_for_protocol(protocol),
        },
        "recorded_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
    }
    if parsed["status"] == "observed":
        evidence["frame"] = parsed["frame"]
        evidence["input_observation"] = parsed["input_observation"]
    else:
        evidence["reason"] = parsed["reason"]
    return evidence


def validate_evidence(data: Any) -> None:
    if not isinstance(data, dict) or data.get("schema_version") != SCHEMA_VERSION:
        die("unsupported or missing evidence schema")
    if data.get("status") not in {"observed", "unavailable", "failed"}:
        die("invalid evidence status")
    source_commit = data.get("source_commit")
    if source_commit is not None and (
        not isinstance(source_commit, str)
        or not re.fullmatch(r"[0-9a-fA-F]{7,64}", source_commit)
    ):
        die("source_commit is missing or malformed")
    if data.get("protocol") not in {"rdp", "vnc", "spice"}:
        die("invalid evidence protocol")
    target = data.get("target")
    if not isinstance(target, dict) or not isinstance(target.get("host"), str):
        die("bounded target evidence is missing")
    parse_target(f"{target['host']}:{target.get('port')}")
    probe = data.get("probe")
    if not isinstance(probe, dict) or not re.fullmatch(r"[0-9a-f]{64}", str(probe.get("log_sha256", ""))):
        die("probe log digest is missing or malformed")
    if probe.get("source") != source_for_protocol(data["protocol"]):
        die("evidence source is not the approved real-worker test")
    if not isinstance(data.get("recorded_at"), str) or not data["recorded_at"].endswith("Z"):
        die("recorded_at must be a UTC timestamp")
    if data["status"] == "observed":
        frame = data.get("frame")
        if not isinstance(frame, dict) or not isinstance(frame.get("width"), int) or not isinstance(frame.get("height"), int):
            die("observed evidence has no framebuffer dimensions")
        if not (1 <= frame["width"] <= 65535 and 1 <= frame["height"] <= 65535):
            die("framebuffer dimensions are outside bounds")
        if not re.fullmatch(r"0x[0-9a-f]{16}", str(frame.get("fnv1a64", ""))):
            die("observed evidence has no bounded framebuffer digest")
        if data.get("input_observation") not in {"echoed", "unchanged", "not-reported"}:
            die("invalid input observation")
    elif not isinstance(data.get("reason"), str) or not data["reason"]:
        die("non-observed evidence must explain why proof is unavailable")


def write_evidence(path: Path, evidence: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def run_probe(args: argparse.Namespace) -> int:
    parse_target(args.target)
    if not args.host or not args.slot or not args.source_commit:
        die("run requires explicit --source-commit, --host, and --slot; local execution is not proof")
    test_name = {
        "vnc": "live_vnc_worker_renders_real_console_and_accepts_input",
        "spice": "live_spice_worker_renders_real_console_and_accepts_input",
        "rdp": "live_rdp_renders_accepts_input_and_applies_tier_on_reconnect",
    }[args.protocol]
    env = os.environ.copy()
    env["MCNF_BUILD_HOST"] = args.host
    env["MCNF_BUILD_SLOT"] = args.slot
    env[f"MDE_{args.protocol.upper()}_LIVE_TARGET"] = args.target
    if args.protocol == "rdp":
        command = [
            str(Path(__file__).with_name("xcp-build.sh")),
            "cargo", "test", "-p", "mde-vdi-rdp", "--features", "live-connect",
            "--test", "live_rdp", "--", "--ignored", "--nocapture",
        ]
    else:
        command = [
            str(Path(__file__).with_name("xcp-build.sh")),
            "cargo", "test", "-p", "mde-shell-egui", "--features", "live-vdi",
            test_name, "--", "--ignored", "--nocapture",
        ]
    completed = subprocess.run(command, cwd=Path(__file__).parent.parent, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    log = bounded_log(completed.stdout, completed.stderr)
    evidence = make_evidence(
        args.protocol, args.target, completed.returncode, log, args.source_commit
    )
    validate_evidence(evidence)
    write_evidence(Path(args.out), evidence)
    print(json.dumps(evidence, indent=2, sort_keys=True))
    return 0 if evidence["status"] == "observed" else 1


def discover_targets(args: argparse.Namespace) -> int:
    """Run the bounded approved-seat inventory without retaining probe output."""
    helper = Path(__file__).with_name("discover-vdi-live-targets.sh")
    command = [str(helper)]
    for seat in args.seat:
        command.extend(("--seat", seat))
    completed = subprocess.run(
        command,
        cwd=helper.parent.parent,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if len(completed.stdout) > 64 * 1024:
        die("VDI target inventory exceeds the bounded output limit")
    try:
        inventory = json.loads(completed.stdout.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        die(f"VDI target inventory is not valid JSON: {exc}")
    if (
        not isinstance(inventory, dict)
        or inventory.get("schema_version") != 1
        or inventory.get("kind") != "vdi_live_target_inventory"
        or not isinstance(inventory.get("seats"), list)
    ):
        die("VDI target inventory has an invalid schema")
    print(json.dumps(inventory, indent=2, sort_keys=True))
    return 0 if completed.returncode == 0 else 1


def self_test() -> None:
    valid_log = "live-shell-vnc: FRAME OK 1024x768 fnv1a64=0x0123456789abcdef\nlive-shell-vnc: INPUT ECHOED before=0x0123456789abcdef after=0xfedcba9876543210\n"
    source_commit = "0123456789abcdef0123456789abcdef01234567"
    evidence = make_evidence("vnc", "127.0.0.1:15903", 0, valid_log, source_commit)
    validate_evidence(evidence)
    assert evidence["status"] == "observed"
    assert evidence["frame"]["width"] == 1024
    rdp_log = (
        "live: CONNECTED tier=Full desktop=1024x768\n"
        "live: FRAME OK 1024x768 rects=1 fnv1a64=0x0123456789abcdef distinct_colors=42\n"
        "live: INPUT sent OK; framebuffer UNCHANGED after keystroke (fnv1a64=0x0123456789abcdef)\n"
        "live: RECONNECTED tier=Compressed desktop=1024x768\n"
        "live: TIER FRAME OK 1024x768 rects=1 fnv1a64=0xfedcba9876543210 distinct_colors=43\n"
    )
    rdp_evidence = make_evidence("rdp", "127.0.0.1:13389,mde,mde-live-proof", 0, rdp_log, source_commit)
    validate_evidence(rdp_evidence)
    assert rdp_evidence["status"] == "observed"
    assert rdp_evidence["protocol"] == "rdp"
    assert rdp_evidence["target"] == {"host": "127.0.0.1", "port": 13389}
    assert "mde-live-proof" not in json.dumps(rdp_evidence)
    assert rdp_evidence["input_observation"] == "unchanged"
    for log, code in (("", 0), ("live-shell-vnc: FRAME OK 0x0 fnv1a64=0x0123456789abcdef", 0), (valid_log, 1)):
        rejected = make_evidence("vnc", "127.0.0.1:15903", code, log, source_commit)
        assert rejected["status"] != "observed"
    broken = dict(evidence)
    broken["frame"] = dict(evidence["frame"])
    broken["frame"]["fnv1a64"] = "0xnot-a-digest"
    try:
        validate_evidence(broken)
    except ProofError:
        pass
    else:
        die("self-test accepted malformed framebuffer evidence")
    print("verify-vdi-live-proof.py: self-test passed")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    sub = parser.add_subparsers(dest="command")
    discover = sub.add_parser("discover")
    discover.add_argument("--seat", action="append", required=True)
    run = sub.add_parser("run")
    run.add_argument("--protocol", choices=("rdp", "vnc", "spice"), required=True)
    run.add_argument("--target", required=True, help="HOST:PORT[,ticket] or RDP HOST:PORT[,user,pass]; credentials are never written to evidence")
    run.add_argument("--source-commit", required=True, help="candidate source commit bound into evidence")
    run.add_argument("--host", required=True, help="explicit farm build host")
    run.add_argument("--slot", required=True, help="isolated farm build slot")
    run.add_argument("--out", required=True, type=Path)
    validate = sub.add_parser("validate")
    validate.add_argument("evidence", type=Path)
    args = parser.parse_args()
    try:
        if args.self_test:
            self_test()
            return 0
        if args.command == "run":
            return run_probe(args)
        if args.command == "discover":
            return discover_targets(args)
        if args.command == "validate":
            validate_evidence(json.loads(args.evidence.read_text(encoding="utf-8")))
            print(f"verify-vdi-live-proof.py: valid {args.evidence}")
            return 0
        parser.error("choose --self-test, discover, run, or validate")
    except (OSError, ProofError, json.JSONDecodeError) as exc:
        print(f"verify-vdi-live-proof.py: FAIL: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
