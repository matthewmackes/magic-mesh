#!/usr/bin/env python3
"""Bounded WL-CRIT-007/S4 corrected-forward recovery verifier.

The live modes are read-only.  `preflight` proves the exact enrolled target and
package-owned recovery path before an outer controller is allowed to reboot it.
`post-reboot` additionally proves a new boot, dependency order, one identity,
one process per grouped worker, strict coordination quorum, substrate health,
and one active seat session.  `verify-forward` binds those two captures to the
exact package transition authorized by the release controller, so a retained
pre-upgrade capture or rollback cannot satisfy corrected-forward evidence.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import stat
import subprocess
import sys
from pathlib import Path
from typing import Any

GROUPS = ("control", "observation", "actions", "data", "compute", "integrations")
XDG_DIRS = ("Documents", "Downloads", "Music", "Pictures", "Videos")
IDENTIFIER = re.compile(r"^[A-Za-z0-9._:-]{1,128}$")
PACKAGE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+~:-]{0,255}$")
OVERLAY = re.compile(r"^(?:[0-9]{1,3}\.){3}[0-9]{1,3}/(?:[0-9]|[12][0-9]|3[0-2])$")
MAX_OUTPUT = 16_384
MAX_EVIDENCE = 64 * 1024
TIMEOUT_SECONDS = 8
# The package boot gate performs three bounded etcd endpoint-health probes in
# addition to the local service and bind checks.  A healthy 2/3 live quorum took
# 11.09 seconds on the release seat, so the generic 8-second subprocess bound
# falsely rejected recovered nodes.  Keep ordinary probes tight while giving
# this explicit aggregate gate a still-bounded allowance.
BOOT_GATE_TIMEOUT_SECONDS = 30


class Refused(RuntimeError):
    pass


def refuse(message: str) -> None:
    raise Refused(message)


def run(
    *argv: str,
    allow_failure: bool = False,
    timeout_seconds: int = TIMEOUT_SECONDS,
) -> str:
    try:
        completed = subprocess.run(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout_seconds,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        refuse(f"command unavailable or timed out: {argv[0]}: {error}")
    output = completed.stdout[: MAX_OUTPUT + 1]
    if len(output) > MAX_OUTPUT:
        refuse(f"command output exceeded {MAX_OUTPUT} bytes: {argv[0]}")
    text = output.decode("utf-8", errors="replace").strip()
    if completed.returncode != 0 and not allow_failure:
        refuse(f"command failed ({completed.returncode}): {' '.join(argv)}: {text[:240]}")
    return text


def command_succeeds(*argv: str) -> bool:
    try:
        completed = subprocess.run(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return completed.returncode == 0


def strict_majority(total: int) -> int:
    if total <= 0:
        refuse("coordination endpoint set is empty")
    return total // 2 + 1


def bounded_evidence(path: Path) -> dict[str, Any]:
    """Read one immutable, single-link evidence inode and revalidate its path."""
    flags = os.O_RDONLY | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        refuse(f"evidence file is unavailable or unsafe: {path}: {error}")
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
            refuse(f"evidence must be a single-link regular file: {path}")
        if before.st_size <= 0 or before.st_size > MAX_EVIDENCE:
            refuse(f"evidence size is outside the supported bound: {path}")
        chunks: list[bytes] = []
        remaining = MAX_EVIDENCE + 1
        while remaining:
            chunk = os.read(descriptor, min(remaining, 8192))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        body = b"".join(chunks)
        if len(body) > MAX_EVIDENCE:
            refuse(f"evidence exceeded the supported bound while reading: {path}")
        after = os.fstat(descriptor)
        current = path.stat(follow_symlinks=False)
        identity = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
        if identity != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
            refuse(f"evidence inode changed while reading: {path}")
        if identity != (current.st_dev, current.st_ino, current.st_size, current.st_mtime_ns):
            refuse(f"evidence path was replaced while reading: {path}")
    except OSError as error:
        refuse(f"evidence could not be read safely: {path}: {error}")
    finally:
        os.close(descriptor)
    try:
        value = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        refuse(f"evidence is not bounded UTF-8 JSON: {path}: {error}")
    if not isinstance(value, dict):
        refuse(f"evidence root must be an object: {path}")
    return value


def validate_corrected_forward(
    before: dict[str, Any],
    after: dict[str, Any],
    *,
    expect_target: str,
    expect_previous_package: str,
    expect_forward_package: str,
) -> dict[str, str | bool]:
    if not IDENTIFIER.fullmatch(expect_target):
        refuse("expected target is malformed")
    for value, label in (
        (expect_previous_package, "expected previous package"),
        (expect_forward_package, "expected forward package"),
    ):
        if not PACKAGE_ID.fullmatch(value):
            refuse(f"{label} is malformed")
    if expect_previous_package == expect_forward_package:
        refuse("corrected-forward package must differ from the previous package")
    for field in ("target", "role", "overlay", "session_user"):
        if before.get(field) != after.get(field):
            refuse(f"corrected-forward capture changed target authority: {field}")
    if before.get("target") != expect_target:
        refuse("corrected-forward capture does not belong to the expected target")
    if before.get("package") != expect_previous_package:
        refuse("pre-upgrade capture does not match the authorized previous package")
    if after.get("package") != expect_forward_package:
        refuse("post-upgrade capture does not match the authorized forward package")
    before_boot_id = before.get("boot_id")
    if not isinstance(before_boot_id, str) or not IDENTIFIER.fullmatch(before_boot_id):
        refuse("pre-upgrade capture has an invalid boot identity")
    after_boot_id = after.get("boot_id")
    if not isinstance(after_boot_id, str) or not IDENTIFIER.fullmatch(after_boot_id):
        refuse("post-upgrade capture has an invalid boot identity")
    validate_post(after, before_boot_id)
    return {
        "corrected_forward": True,
        "rollback": False,
        "target": expect_target,
        "previous_package": expect_previous_package,
        "forward_package": expect_forward_package,
        "before_boot_id": before_boot_id,
        "after_boot_id": after_boot_id,
    }


def configured_etcd_endpoints(path: Path) -> list[str]:
    try:
        body = path.read_text(encoding="utf-8")
    except OSError as error:
        refuse(f"coordination endpoint file is unreadable: {error}")
    values: list[str] = []
    for value in re.split(r"[\n,]", body):
        endpoint = value.strip()
        if not endpoint or endpoint in values:
            continue
        if len(endpoint) > 256 or any(char.isspace() for char in endpoint):
            refuse("coordination endpoint is malformed")
        values.append(endpoint)
    if not values or len(values) > 9:
        refuse("coordination endpoint count is outside the supported 1..9 bound")
    return values


def etcd_quorum_health(endpoints: list[str]) -> dict[str, int]:
    healthy = sum(
        command_succeeds(
            "/usr/bin/etcdctl", f"--endpoints={endpoint}", "endpoint", "health"
        )
        for endpoint in endpoints
    )
    result = {
        "configured": len(endpoints),
        "healthy": healthy,
        "required": strict_majority(len(endpoints)),
    }
    if healthy < result["required"]:
        refuse(
            "coordination strict quorum unavailable: "
            f"healthy={healthy} configured={len(endpoints)} required={result['required']}"
        )
    return result


def trusted_package_file(path: Path) -> str:
    try:
        metadata = path.lstat()
    except OSError as error:
        refuse(f"required recovery path is unavailable: {path}: {error}")
    if not stat.S_ISREG(metadata.st_mode) or path.is_symlink():
        refuse(f"required recovery path is not a regular nofollow file: {path}")
    if metadata.st_uid != 0 or metadata.st_mode & 0o022:
        refuse(f"required recovery path is not root-owned and non-writable by group/world: {path}")
    owner = run("/usr/bin/rpm", "-qf", str(path))
    if not owner.startswith("magic-mesh-"):
        refuse(f"recovery path is not owned by a magic-mesh package: {path}: {owner}")
    return owner


def systemctl_value(unit: str, field: str) -> str:
    return run("/usr/bin/systemctl", "show", unit, f"-p{field}", "--value")


def active(unit: str) -> bool:
    return command_succeeds("/usr/bin/systemctl", "is-active", "--quiet", unit)


def unit_pid(unit: str) -> int:
    value = systemctl_value(unit, "MainPID")
    if not value.isdigit() or int(value) <= 1:
        refuse(f"{unit} has no live MainPID")
    return int(value)


def unit_started(unit: str) -> int:
    value = systemctl_value(unit, "ActiveEnterTimestampMonotonic")
    if not value.isdigit() or int(value) <= 0:
        refuse(f"{unit} has no active monotonic timestamp")
    return int(value)


def read_role() -> str:
    body = Path("/var/lib/mde/role.toml").read_text(encoding="utf-8")
    match = re.search(r'^\s*role\s*=\s*"?([^"\s]+)', body, re.MULTILINE)
    if not match:
        refuse("enrolled role file is malformed")
    return match.group(1)


def overlay_addresses() -> list[str]:
    body = run("/usr/sbin/ip", "-j", "-4", "address", "show", "dev", "nebula1")
    try:
        records = json.loads(body)
    except json.JSONDecodeError as error:
        refuse(f"overlay address output is malformed: {error}")
    addresses = [
        f"{info['local']}/{info['prefixlen']}"
        for record in records
        for info in record.get("addr_info", [])
        if info.get("family") == "inet" and info.get("scope") == "global"
    ]
    return addresses


def process_groups() -> dict[str, list[int]]:
    found = {group: [] for group in GROUPS}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            args = entry.joinpath("cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        decoded = [arg.decode("utf-8", errors="replace") for arg in args if arg]
        for group in GROUPS:
            if "serve" in decoded and "--group" in decoded:
                index = decoded.index("--group")
                if index + 1 < len(decoded) and decoded[index + 1] == group:
                    found[group].append(int(entry.name))
    return found


def exact_xdg_binds(session_user: str) -> dict[str, bool]:
    """Prove every communal directory is an exact, active bind mount."""
    source_root = Path("/mnt/mesh-storage/home")
    target_root = Path("/home") / session_user
    matches: dict[str, bool] = {}
    for name in XDG_DIRS:
        source = source_root / name
        target = target_root / name
        try:
            safe_paths = (
                source.is_dir()
                and target.is_dir()
                and not source.is_symlink()
                and not target.is_symlink()
            )
            matches[name] = (
                safe_paths
                and command_succeeds("/usr/bin/mountpoint", "--quiet", str(target))
                and source.samefile(target)
            )
        except OSError:
            matches[name] = False
    return matches


def collect(args: argparse.Namespace) -> dict[str, Any]:
    if os.geteuid() != 0:
        refuse("live verification must run as root")
    for value, label in (
        (args.expect_host, "expected host"),
        (args.expect_role, "expected role"),
        (args.session_user, "session user"),
    ):
        if not IDENTIFIER.fullmatch(value):
            refuse(f"{label} is malformed")
    if not OVERLAY.fullmatch(args.expect_overlay):
        refuse("expected overlay is malformed")

    hostname = run("/usr/bin/hostname")
    role = read_role()
    addresses = overlay_addresses()
    if hostname != args.expect_host:
        refuse(f"target identity mismatch: expected {args.expect_host}, observed {hostname}")
    if role != args.expect_role:
        refuse(f"target role mismatch: expected {args.expect_role}, observed {role}")
    if addresses != [args.expect_overlay]:
        refuse(f"expected one exact overlay {args.expect_overlay}, observed {addresses}")

    paths = (
        Path("/usr/libexec/mackesd/mesh-peer-recovery"),
        Path("/usr/libexec/mackesd/mesh-xdg-bind-recovery"),
        Path("/usr/libexec/mackesd/verify-boot-recovery"),
        Path("/usr/libexec/mackesd/seat-update-warning"),
        Path("/usr/lib/systemd/system/mcnf-peer-recovery.service"),
        Path("/usr/lib/systemd/system/mcnf-xdg-bind-recovery.service"),
        Path("/usr/lib/systemd/system-sleep/mcnf-peer-recovery"),
        Path("/etc/NetworkManager/dispatcher.d/90-mcnf-peer-recovery"),
    )
    owners = {trusted_package_file(path) for path in paths}
    if len(owners) != 1:
        refuse(f"recovery paths span multiple package identities: {sorted(owners)}")
    unit = run("/usr/bin/systemctl", "cat", "mcnf-peer-recovery.service")
    for token in (
        "ExecStart=/usr/libexec/mackesd/mesh-peer-recovery",
        "TimeoutStartSec=90",
        "RuntimeMaxSec=90",
        "RuntimeDirectory=mcnf-peer-recovery",
    ):
        if token not in unit:
            refuse(f"installed recovery unit is missing bounded contract: {token}")
    xdg_unit = run("/usr/bin/systemctl", "cat", "mcnf-xdg-bind-recovery.service")
    for token in (
        "Type=oneshot",
        "ExecStart=/usr/libexec/mackesd/mesh-xdg-bind-recovery",
        "TimeoutStartSec=60",
    ):
        if token not in xdg_unit:
            refuse(f"installed XDG recovery unit is missing host-namespace contract: {token}")
    identity = run("/usr/libexec/mackesd/verify-boot-recovery", "--identity-guard")
    if "PASS" not in identity:
        refuse("installed identity guard did not admit exactly one trusted identity")

    return {
        "schema_version": 1,
        "target": hostname,
        "role": role,
        "overlay": addresses[0],
        "package": next(iter(owners)),
        "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
        "session_user": args.session_user,
        "recovery_path": "package-owned",
    }


def validate_post(snapshot: dict[str, Any], before_boot_id: str) -> None:
    if snapshot.get("boot_id") == before_boot_id:
        refuse("boot id did not change; reboot was not proven")
    if snapshot.get("nebula_processes") != 1:
        refuse("expected exactly one Nebula process after recovery")
    group_pids = snapshot.get("group_pids")
    if not isinstance(group_pids, dict) or set(group_pids) != set(GROUPS):
        refuse("grouped worker PID evidence is incomplete")
    pids = list(group_pids.values())
    if any(not isinstance(pid, int) or pid <= 1 for pid in pids) or len(set(pids)) != len(GROUPS):
        refuse("grouped workers do not have six unique live processes")
    process_matches = snapshot.get("group_process_matches")
    if process_matches != {group: 1 for group in GROUPS}:
        refuse("duplicate or missing grouped worker process detected")
    starts = snapshot.get("starts")
    if not isinstance(starts, dict):
        refuse("recovery ordering timestamps are missing")
    nebula = starts.get("nebula")
    substrate = starts.get("substrate")
    workers = starts.get("workers")
    if not all(isinstance(value, int) and value > 0 for value in (nebula, substrate, workers)):
        refuse("recovery ordering timestamps are invalid")
    if not nebula <= substrate <= workers:
        refuse("recovery order violated: expected Nebula, substrate, grouped workers")
    if snapshot.get("substrate_healthy") is not True:
        refuse("coordination/file substrate is not healthy")
    quorum = snapshot.get("coordination_quorum")
    if quorum is not None:
        if not isinstance(quorum, dict):
            refuse("coordination strict-quorum evidence is invalid")
        configured = quorum.get("configured")
        healthy = quorum.get("healthy")
        required = quorum.get("required")
        # Evidence is untrusted JSON.  Validate the scalar types before doing
        # comparisons so hostile strings/bools cannot escape as verifier
        # exceptions or be treated as quorum counts.
        if any(
            not isinstance(value, int) or isinstance(value, bool) or value < 1
            for value in (configured, healthy, required)
        ):
            refuse("coordination strict-quorum evidence is invalid")
        if healthy < required or configured < required or healthy > configured:
            refuse("coordination strict-quorum evidence is invalid")
    session_state = snapshot.get("session_state")
    if snapshot.get("role") == "workstation":
        if session_state not in ("active", "online"):
            refuse("expected workstation session did not recover")
        session_pid = snapshot.get("session_pid")
        session_processes = snapshot.get("session_processes")
        if not isinstance(session_pid, int) or session_pid <= 1 or session_processes != 1:
            refuse("expected exactly one workstation shell session authority")
        if snapshot.get("xdg_binds") != {name: True for name in XDG_DIRS}:
            refuse("communal Workstation XDG binds are missing, inexact, or inactive")
    elif snapshot.get("role") == "lighthouse":
        if session_state != "headless":
            refuse("lighthouse recovery unexpectedly claimed a desktop session")
    else:
        refuse("recovered role is unsupported")
    if snapshot.get("boot_gate") is not True:
        refuse("installed boot recovery gate failed")


def collect_post(args: argparse.Namespace) -> dict[str, Any]:
    snapshot = collect(args)
    if not active("nebula.service"):
        refuse("Nebula is not active after recovery")
    nebula_pid = unit_pid("nebula.service")
    nebula_processes = run("/usr/bin/pgrep", "-x", "nebula", allow_failure=True).splitlines()
    if str(nebula_pid) not in nebula_processes:
        refuse("Nebula MainPID does not match the sole overlay process")

    syncthing_configured = Path(
        "/etc/systemd/system/syncthing.service.d/10-home.conf"
    ).is_file()
    if syncthing_configured and not active("syncthing.service"):
        refuse("configured Syncthing file substrate is inactive")
    substrate_times = [unit_started("nebula.service")]
    if syncthing_configured:
        substrate_times.append(unit_started("syncthing.service"))
    if Path("/etc/etcd/etcd.env").is_file():
        if not active("etcd.service"):
            refuse("configured local etcd member is inactive")
        substrate_times.append(unit_started("etcd.service"))
    endpoints = Path("/etc/mackesd/etcd-endpoints")
    coordination_quorum = None
    if endpoints.is_file() and endpoints.stat().st_size:
        coordination_quorum = etcd_quorum_health(configured_etcd_endpoints(endpoints))

    group_pids: dict[str, int] = {}
    worker_times: list[int] = []
    for group in GROUPS:
        unit = f"mackesd-{group}.service"
        if not active(unit):
            refuse(f"grouped worker is inactive: {unit}")
        group_pids[group] = unit_pid(unit)
        worker_times.append(unit_started(unit))
    matches = process_groups()
    for group, pid in group_pids.items():
        if matches[group] != [pid]:
            refuse(f"{group} worker process identity mismatch: unit={pid}, processes={matches[group]}")

    if snapshot["role"] == "workstation":
        if not active("mde-shell-egui.service"):
            refuse("workstation shell session is inactive")
        session_pid = unit_pid("mde-shell-egui.service")
        shell_processes = run(
            "/usr/bin/pgrep", "-x", "mde-shell-egui", allow_failure=True
        ).splitlines()
        if shell_processes != [str(session_pid)]:
            refuse(
                "workstation shell process identity mismatch: "
                f"unit={session_pid}, processes={shell_processes}"
            )
        session_state = run(
            "/usr/bin/loginctl", "show-user", args.session_user, "-pState", "--value"
        )
        xdg_binds = exact_xdg_binds(args.session_user)
    else:
        session_pid = None
        shell_processes = []
        session_state = "headless"
        xdg_binds = {}
    boot_gate = run(
        "/usr/libexec/mackesd/verify-boot-recovery",
        allow_failure=True,
        timeout_seconds=BOOT_GATE_TIMEOUT_SECONDS,
    )
    if "PASS" not in boot_gate:
        refuse(f"installed boot recovery gate failed: {boot_gate[-1024:]}")
    snapshot.update(
        {
            "nebula_processes": len(nebula_processes),
            "group_pids": group_pids,
            "group_process_matches": {group: len(matches[group]) for group in GROUPS},
            "starts": {
                "nebula": unit_started("nebula.service"),
                "substrate": max(substrate_times),
                "workers": min(worker_times),
            },
            "substrate_healthy": True,
            "coordination_quorum": coordination_quorum,
            "session_state": session_state,
            "session_pid": session_pid,
            "session_processes": len(shell_processes),
            "xdg_binds": xdg_binds,
            "boot_gate": True,
        }
    )
    validate_post(snapshot, args.before_boot_id)
    return snapshot


def self_test() -> None:
    valid = {
        "role": "workstation",
        "boot_id": "after",
        "nebula_processes": 1,
        "group_pids": {group: index + 10 for index, group in enumerate(GROUPS)},
        "group_process_matches": {group: 1 for group in GROUPS},
        "starts": {"nebula": 10, "substrate": 20, "workers": 30},
        "substrate_healthy": True,
        "coordination_quorum": {"configured": 3, "healthy": 2, "required": 2},
        "session_state": "active",
        "session_pid": 20,
        "session_processes": 1,
        "xdg_binds": {name: True for name in XDG_DIRS},
        "boot_gate": True,
    }
    validate_post(valid, "before")
    cases = (
        ({**valid, "boot_id": "before"}, "boot id"),
        ({**valid, "nebula_processes": 2}, "one Nebula"),
        ({**valid, "starts": {"nebula": 20, "substrate": 10, "workers": 30}}, "order"),
        (
            {
                **valid,
                "group_process_matches": {**valid["group_process_matches"], "compute": 2},
            },
            "Duplicate worker",
        ),
        ({**valid, "boot_gate": False}, "Boot gate"),
        (
            {
                **valid,
                "coordination_quorum": {"configured": 3, "healthy": 1, "required": 2},
            },
            "Lost quorum",
        ),
        (
            {
                **valid,
                "coordination_quorum": {
                    "configured": "3",
                    "healthy": 2,
                    "required": 2,
                },
            },
            "Malformed quorum count",
        ),
        ({**valid, "session_processes": 2}, "Duplicate shell session"),
        (
            {**valid, "xdg_binds": {**valid["xdg_binds"], "Music": False}},
            "Missing XDG bind",
        ),
    )
    for hostile, label in cases:
        try:
            validate_post(hostile, "before")
        except Refused:
            continue
        raise AssertionError(f"{label} fixture was accepted")
    lighthouse = {
        **valid,
        "role": "lighthouse",
        "session_state": "headless",
        "session_pid": None,
        "session_processes": 0,
        "xdg_binds": {},
    }
    validate_post(lighthouse, "before")
    before = {
        "target": "seat-15",
        "role": "workstation",
        "overlay": "172.20.0.15/24",
        "session_user": "mm",
        "package": "magic-mesh-32.0.0-1.x86_64",
        "boot_id": "before",
    }
    forward = {
        **valid,
        "target": "seat-15",
        "overlay": "172.20.0.15/24",
        "session_user": "mm",
        "package": "magic-mesh-33.0.0-1.x86_64",
    }
    validate_corrected_forward(
        before,
        forward,
        expect_target="seat-15",
        expect_previous_package="magic-mesh-32.0.0-1.x86_64",
        expect_forward_package="magic-mesh-33.0.0-1.x86_64",
    )
    try:
        validate_corrected_forward(
            before,
            {**forward, "package": before["package"]},
            expect_target="seat-15",
            expect_previous_package="magic-mesh-32.0.0-1.x86_64",
            expect_forward_package="magic-mesh-33.0.0-1.x86_64",
        )
    except Refused:
        pass
    else:
        raise AssertionError("retained pre-upgrade package authority was accepted")
    assert strict_majority(1) == 1
    assert strict_majority(3) == 2
    assert strict_majority(5) == 3
    print("verify-corrected-forward-recovery: self-test passed 15/15")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser()
    sub = result.add_subparsers(dest="command", required=True)
    sub.add_parser("self-test")
    forward = sub.add_parser("verify-forward")
    forward.add_argument("--before", type=Path, required=True)
    forward.add_argument("--after", type=Path, required=True)
    forward.add_argument("--expect-target", required=True)
    forward.add_argument("--expect-previous-package", required=True)
    forward.add_argument("--expect-forward-package", required=True)
    for name in ("preflight", "post-reboot"):
        command = sub.add_parser(name)
        command.add_argument("--expect-host", required=True)
        command.add_argument("--expect-role", required=True)
        command.add_argument("--expect-overlay", required=True)
        command.add_argument("--session-user", required=True)
        if name == "post-reboot":
            command.add_argument("--before-boot-id", required=True)
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        if args.command == "self-test":
            self_test()
            return 0
        if args.command == "verify-forward":
            result = validate_corrected_forward(
                bounded_evidence(args.before),
                bounded_evidence(args.after),
                expect_target=args.expect_target,
                expect_previous_package=args.expect_previous_package,
                expect_forward_package=args.expect_forward_package,
            )
            print(json.dumps(result, sort_keys=True, separators=(",", ":")))
            return 0
        snapshot = collect(args) if args.command == "preflight" else collect_post(args)
        print(json.dumps(snapshot, sort_keys=True, separators=(",", ":")))
        return 0
    except (OSError, ValueError, Refused) as error:
        print(f"verify-corrected-forward-recovery: REFUSED: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
