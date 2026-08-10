#!/usr/bin/env python3
"""Three-seat Mesh Teams/clipboard/file acceptance and recovery matrix.

The harness drives the root shell's hidden acceptance verbs, so commands use
the same signed action publisher and native clipboard provider as the visible
UI. Missing seats are recorded as skipped. Any failed assertion on a reachable
seat is fatal. Password material is read from a caller-supplied file, retained
only in process memory, and never emitted to logs or evidence.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time
import uuid


@dataclasses.dataclass(frozen=True)
class SeatSpec:
    label: str
    address: str
    user: str
    ssh_auth: str
    privilege: str
    proxy_jump: str | None = None


def seat_address(label: str, default: str) -> str:
    """Allow a DHCP-renumbered seat to be tested without changing the harness."""
    return os.environ.get(f"MCNF_THREE_SEAT_{label.upper()}_ADDRESS", default)


def seat_proxy_jump(label: str) -> str | None:
    """Route a seat through a reachable mesh peer when its LAN path is absent."""
    return os.environ.get(f"MCNF_THREE_SEAT_{label.upper()}_PROXY_JUMP")


BASELINE_SEATS = (
    SeatSpec("dell", seat_address("dell", "172.20.146.225"), "mm", "key", "sudo-n", seat_proxy_jump("dell")),
    SeatSpec("seat15", seat_address("seat15", "172.20.0.15"), "root", "key", "root", seat_proxy_jump("seat15")),
    SeatSpec("surface", seat_address("surface", "172.20.146.79"), "root", "key", "root", seat_proxy_jump("surface")),
)
OPTIONAL_INSPECTION_SEATS = {
    "eagle": SeatSpec("eagle", seat_address("eagle", "172.20.146.88"), "mm", "password", "sudo-password", seat_proxy_jump("eagle")),
    "t480": SeatSpec("t480", seat_address("t480", "172.20.146.68"), "mm", "password", "sudo-password", seat_proxy_jump("t480")),
}
MAX_ACTIVITY_SEATS = 3

SSH_COMMON = (
    "-o", "StrictHostKeyChecking=accept-new",
    "-o", "ConnectTimeout=7",
    "-o", "ServerAliveInterval=5",
    "-o", "ServerAliveCountMax=2",
)
ACCEPTANCE_BASE = (
    "systemd-run --quiet --wait --collect "
    "-p LoadCredentialEncrypted=cloud-arm-key:/etc/credstore.encrypted/cloud-arm-key "
    "-p Environment=MDE_BUS_ROOT=/run/mde-bus"
)


class Failure(RuntimeError):
    pass


class Seat:
    def __init__(self, spec: SeatSpec, key: Path, password: bytes):
        self.spec = spec
        self.key = key
        self.password = password
        self.hostname = ""

    def _ssh(self) -> list[str]:
        route = ["-J", self.spec.proxy_jump] if self.spec.proxy_jump else []
        if self.spec.ssh_auth == "password":
            return ["sshpass", "-f", str(args.password_file), "ssh", *SSH_COMMON, *route,
                    f"{self.spec.user}@{self.spec.address}"]
        return ["ssh", "-i", str(self.key), *SSH_COMMON, *route,
                f"{self.spec.user}@{self.spec.address}"]

    def run(
        self,
        command: str,
        *,
        payload: bytes = b"",
        root: bool = False,
        timeout: int = 45,
        check: bool = True,
    ) -> subprocess.CompletedProcess[bytes]:
        remote = f"bash -lc {shlex.quote(command)}"
        stdin = payload
        if root and self.spec.privilege == "sudo-password":
            remote = f"sudo -S -p '' -- {remote}"
            stdin = self.password + b"\n" + payload
        elif root and self.spec.privilege == "sudo-n":
            remote = f"sudo -n -- {remote}"
        result = subprocess.run(
            [*self._ssh(), remote],
            input=stdin,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            check=False,
        )
        if check and result.returncode != 0:
            stderr = result.stderr.decode("utf-8", "replace").strip()
            raise Failure(
                f"{self.spec.label}: command failed ({result.returncode}): {redact(stderr)}"
            )
        return result

    def acceptance(self, verb: str, payload: str = "", argument: str | None = None) -> dict:
        transport_id = uuid.uuid4().hex
        input_path = f"/run/mde-three-seat-accept-{transport_id}.in"
        output_path = f"/run/mde-three-seat-accept-{transport_id}.out"
        error_path = f"/run/mde-three-seat-accept-{transport_id}.err"
        service = (
            f"{ACCEPTANCE_BASE} "
            f"-p StandardInput=file:{input_path} "
            f"-p StandardOutput=file:{output_path} "
            f"-p StandardError=file:{error_path} "
            f"/usr/bin/mde-shell-egui {shlex.quote(verb)}"
        )
        if argument is not None:
            service += f" {shlex.quote(argument)}"
        command = (
            "umask 077; "
            f"dd of={shlex.quote(input_path)} status=none; "
            f"status=0; {service} || status=$?; "
            f"test ! -s {shlex.quote(output_path)} || cat {shlex.quote(output_path)}; "
            f"if test \"$status\" -ne 0 && test -s {shlex.quote(error_path)}; then "
            f"cat {shlex.quote(error_path)} >&2; fi; "
            f"rm -f -- {shlex.quote(input_path)} {shlex.quote(output_path)} {shlex.quote(error_path)}; "
            "exit \"$status\""
        )
        result = self.run(
            command,
            payload=payload.encode(),
            root=True,
            timeout=30,
        )
        lines = [line for line in result.stdout.decode().splitlines() if line.strip()]
        if not lines:
            raise Failure(f"{self.spec.label}: acceptance command returned no status")
        try:
            return json.loads(lines[-1])
        except json.JSONDecodeError as error:
            raise Failure(f"{self.spec.label}: malformed acceptance status") from error

    def latest_body(self, topic: str) -> dict | None:
        result = self.run(
            "mde-bus history " + shlex.quote(topic)
            + " --bus-root /run/mde-bus --count 1 --reverse --json",
            timeout=15,
        )
        lines = [line for line in result.stdout.decode().splitlines() if line.strip()]
        if not lines:
            return None
        stored = json.loads(lines[-1])
        body = stored.get("body")
        return json.loads(body) if isinstance(body, str) else None


def redact(text: str) -> str:
    if not text:
        return text
    secret = password.decode("utf-8", "ignore")
    return text.replace(secret, "[REDACTED]") if secret else text


def wait_for(description: str, predicate, timeout: int = 75, interval: float = 1.0):
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except (Failure, json.JSONDecodeError, OSError, subprocess.TimeoutExpired) as error:
            last_error = error
        time.sleep(interval)
    suffix = f"; last error: {last_error}" if last_error else ""
    raise Failure(f"timed out waiting for {description}{suffix}")


def command(seat: Seat, body: dict) -> None:
    status = seat.acceptance("--acceptance-collab-command", json.dumps(body))
    if not status.get("ok"):
        raise Failure(f"{seat.spec.label}: collaboration command was not accepted")


def directory_space(seat: Seat, name: str) -> dict | None:
    body = seat.latest_body("state/collab/directory") or {}
    return next((row for row in body.get("spaces", []) if row.get("name") == name), None)


def create_space(owner: Seat, kind: str, name: str) -> str:
    command(owner, {"create_space": {"kind": kind, "name": name}})
    row = wait_for(f"{name} directory row on {owner.spec.label}", lambda: directory_space(owner, name))
    return str(row["id"])


def add_members(owner: Seat, space: str, members: list[Seat], name: str) -> None:
    for member in members:
        if member is owner:
            continue
        command(owner, {"add_member": {"space": space, "actor": member.hostname, "role": "member"}})
    for member in members:
        wait_for(
            f"{name} membership on {member.spec.label}",
            lambda member=member: (
                (row := directory_space(member, name))
                and int(row.get("members", 0)) == len(members)
            ),
            timeout=120,
        )


def send_and_observe(source: Seat, target: Seat, space: str, marker: str) -> None:
    command(source, {"send_message": {"space": space, "thread": None, "body": marker}})
    topic = f"state/collab/conversation/{space}"
    wait_for(
        f"message {source.spec.label}->{target.spec.label}",
        lambda: any(
            message.get("body") == marker and message.get("author") == source.hostname
            for message in (target.latest_body(topic) or {}).get("messages", [])
        ),
        timeout=90,
    )


def call_and_observe(source: Seat, target: Seat, space: str) -> None:
    call = str(uuid.uuid4())
    topic = f"state/collab/call-state/{space}"
    command(
        source,
        {"start_call": {"space": space, "call": call, "kind": "audio"}},
    )

    def participant_connected(seat: Seat, actor: str) -> bool:
        state = seat.latest_body(topic) or {}
        return any(
            row.get("call") == call
            and any(
                participant.get("actor") == actor
                and participant.get("state") == "connected"
                for participant in row.get("participants", [])
            )
            for row in state.get("active", [])
        )

    wait_for(
        f"audio call ring {source.spec.label}->{target.spec.label}",
        lambda: participant_connected(target, source.hostname),
        timeout=90,
    )
    command(target, {"answer_call": {"call": call}})
    for seat in (source, target):
        wait_for(
            f"audio call answer on {seat.spec.label}",
            lambda seat=seat: participant_connected(seat, target.hostname),
            timeout=90,
        )

    command(source, {"hang_up_call": {"call": call}})
    command(target, {"hang_up_call": {"call": call}})
    for seat in (source, target):
        wait_for(
            f"audio call cleanup on {seat.spec.label}",
            lambda seat=seat: not any(
                row.get("call") == call
                for row in (seat.latest_body(topic) or {}).get("active", [])
            ),
            timeout=90,
        )


def clipboard_and_observe(source: Seat, target: Seat, marker: str) -> None:
    expected = hashlib.sha256(marker.encode()).hexdigest()
    status = source.acceptance("--acceptance-clipboard", marker)
    if not status.get("ok") or not status.get("published"):
        raise Failure(f"{source.spec.label}: clipboard value was not newly published")

    def materialized() -> bool:
        status = target.acceptance("--acceptance-read-clipboard")
        return status.get("sha256") == expected and status.get("len") == len(marker.encode())

    wait_for(
        f"native clipboard {source.spec.label}->{target.spec.label}",
        materialized,
        timeout=90,
    )


def file_and_observe(source: Seat, target: Seat, space: str, marker: str, cleanup: list[tuple[Seat, str]]) -> None:
    digest = hashlib.sha256(marker.encode()).hexdigest()
    remote_path = f"/tmp/mde-three-seat-{run_id}-{source.spec.label}-{target.spec.label}.txt"
    source.run(
        f"umask 077; dd of={shlex.quote(remote_path)} status=none",
        payload=marker.encode(),
    )
    cleanup.append((source, remote_path))
    status = source.acceptance("--acceptance-link-file", remote_path + "\n", space)
    file_id = status.get("file")
    if not status.get("ok") or not file_id:
        raise Failure(f"{source.spec.label}: file link/transfer was not accepted")

    projection_topic = f"state/collab/file-references/{space}"
    wait_for(
        f"file reference {source.spec.label}->{target.spec.label}",
        lambda: any(
            row.get("file") == file_id
            and row.get("reference", {}).get("sha256_hex") == digest
            for row in (target.latest_body(projection_topic) or {}).get("files", [])
        ),
        timeout=120,
    )
    content_path = f"/mnt/mesh-storage/collab/content/{digest[:2]}/{digest}"

    def content_arrived() -> bool:
        result = target.run(
            f"test -f {shlex.quote(content_path)} && sha256sum -- {shlex.quote(content_path)}",
            check=False,
            timeout=15,
        )
        return result.returncode == 0 and result.stdout.decode().split()[0] == digest

    wait_for(
        f"content bytes {source.spec.label}->{target.spec.label}",
        content_arrived,
        timeout=180,
        interval=2,
    )
    cleanup.append((target, content_path))


def active_unit(seat: Seat, pattern: str) -> str:
    script = (
        "systemctl list-units --type=service --state=active --no-legend "
        + shlex.quote(pattern)
        + " | awk 'NR==1 {print $1}'"
    )
    result = seat.run(script, root=True)
    unit = result.stdout.decode().strip()
    if not unit:
        raise Failure(f"{seat.spec.label}: no active service matched {pattern}")
    return unit


def recovery_cycle(seat: Seat) -> list[str]:
    units = [
        active_unit(seat, "nebula*.service"),
        active_unit(seat, "syncthing*.service"),
        "mackesd.target",
    ]
    for unit in units:
        seat.run(
            f"systemctl stop {shlex.quote(unit)}; systemctl start {shlex.quote(unit)}; "
            f"systemctl is-active --quiet {shlex.quote(unit)}",
            root=True,
            timeout=45,
        )
        wait_for(
            f"{unit} recovery on {seat.spec.label}",
            lambda unit=unit: seat.run(
                f"systemctl is-active --quiet {shlex.quote(unit)}",
                root=True,
                check=False,
            ).returncode == 0,
            timeout=45,
        )
    return units


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run the required three-seat release baseline or an explicitly optional non-baseline inspection.",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--required-baseline",
        action="store_true",
        help="require exactly Dell, seat15, and Surface (the default)",
    )
    mode.add_argument(
        "--inspect-seat",
        action="append",
        choices=sorted(OPTIONAL_INSPECTION_SEATS),
        help="inspect an explicitly named non-baseline seat; never release-gating",
    )
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--key", type=Path, default=Path("/root/.ssh/mackes_mesh_ed25519"))
    parser.add_argument("--evidence", type=Path)
    parser.add_argument("--skip-recovery", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def selected_specs(parsed: argparse.Namespace) -> tuple[tuple[SeatSpec, ...], str]:
    if not parsed.inspect_seat:
        return BASELINE_SEATS, "required-baseline"
    labels = list(dict.fromkeys(parsed.inspect_seat))
    if len(labels) != len(parsed.inspect_seat):
        raise Failure("optional inspection seat names must be unique")
    specs = tuple(OPTIONAL_INSPECTION_SEATS[label] for label in labels)
    if not specs or len(specs) > MAX_ACTIVITY_SEATS:
        raise Failure(f"one activity must select between 1 and {MAX_ACTIVITY_SEATS} seats")
    return specs, "optional-inspection"


def self_test() -> None:
    baseline, mode = selected_specs(argparse.Namespace(inspect_seat=None))
    if mode != "required-baseline" or [seat.label for seat in baseline] != ["dell", "seat15", "surface"]:
        raise Failure("required baseline is not exactly Dell, seat15, and Surface")
    optional, mode = selected_specs(argparse.Namespace(inspect_seat=["eagle", "t480"]))
    if mode != "optional-inspection" or [seat.label for seat in optional] != ["eagle", "t480"]:
        raise Failure("explicit non-baseline inspection selection changed")
    if len(baseline) > MAX_ACTIVITY_SEATS or len(optional) > MAX_ACTIVITY_SEATS:
        raise Failure("physical-seat activity cap exceeded")
    try:
        selected_specs(argparse.Namespace(inspect_seat=["eagle", "eagle"]))
    except Failure:
        pass
    else:
        raise Failure("duplicate optional inspection was accepted")
    print("three-seat acceptance collector self-test passed (3-seat cap and optional isolation enforced)")


def write_evidence(state: dict) -> None:
    args.evidence.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.evidence.with_name(args.evidence.name + f".{os.getpid()}.tmp")
    temporary.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, args.evidence)


args = parse_args()
if args.self_test:
    self_test()
    raise SystemExit(0)
if args.evidence is None:
    raise SystemExit("--evidence is required outside --self-test")
specs, activity_mode = selected_specs(args)
if any(spec.ssh_auth == "password" or spec.privilege == "sudo-password" for spec in specs):
    if args.password_file is None:
        raise SystemExit("--password-file is required for the selected optional inspection")
    password = args.password_file.read_bytes().splitlines()[0]
    if not password:
        raise SystemExit("password file has an empty first line")
else:
    password = b""
run_id = f"{int(time.time())}-{uuid.uuid4().hex[:8]}"
evidence: dict = {
    "schema_version": 1,
    "run_id": run_id,
    "activity_mode": activity_mode,
    "required_baseline": activity_mode == "required-baseline",
    "selected_seats": [spec.label for spec in specs],
    "started_at_unix": int(time.time()),
    "reachable": [],
    "skipped": [],
    "broadcast": None,
    "service_identity": None,
    "directed_pairs": [],
    "recovery": [],
    "result": "running",
}
cleanup: list[tuple[Seat, str]] = []
owned_spaces: list[tuple[Seat, str]] = []


def main() -> int:
    seats: list[Seat] = []
    for spec in specs:
        seat = Seat(spec, args.key, password)
        probe = seat.run("hostname", check=False, timeout=12)
        if probe.returncode != 0:
            evidence["skipped"].append({"seat": spec.label, "reason": "unreachable"})
            continue
        seat.hostname = probe.stdout.decode().strip()
        if not seat.hostname:
            raise Failure(f"{spec.label}: reachable seat returned no hostname")
        seats.append(seat)
        evidence["reachable"].append({"seat": spec.label, "hostname": seat.hostname})

    if activity_mode == "required-baseline" and len(seats) != len(BASELINE_SEATS):
        missing = sorted({spec.label for spec in BASELINE_SEATS} - {seat.spec.label for seat in seats})
        raise Failure(f"required three-seat baseline is incomplete; unreachable={missing}")

    if len(seats) < 2:
        evidence["result"] = "inspection-skipped-insufficient-reachable-seats"
        write_evidence(evidence)
        return 0

    fingerprints: dict[str, str] = {}
    for seat in seats:
        result = seat.run(
            "id -u MDE-MESH >/dev/null; "
            "ssh-keygen -lf /etc/ssh/mackes-mesh/mesh_authorized_keys -E sha256 "
            "| awk 'NR==1 {print $2}'",
            root=True,
        )
        fingerprint = result.stdout.decode().strip()
        if not fingerprint.startswith("SHA256:"):
            raise Failure(f"{seat.spec.label}: MDE-MESH public-key fingerprint is unavailable")
        fingerprints[seat.spec.label] = fingerprint
    if len(set(fingerprints.values())) != 1:
        raise Failure("reachable seats do not share one mesh-scoped MDE-MESH public key")
    evidence["service_identity"] = {
        "user": "MDE-MESH",
        "reachable_seats": len(seats),
        "shared_mesh_key": True,
        "fingerprint": next(iter(fingerprints.values())),
    }

    team_name = f"Acceptance · selected seats · {run_id}"
    team = create_space(seats[0], "team", team_name)
    owned_spaces.append((seats[0], team))
    add_members(seats[0], team, seats, team_name)
    broadcast = f"broadcast:{run_id}"
    command(seats[0], {"send_message": {"space": team, "thread": None, "body": broadcast}})
    for seat in seats:
        wait_for(
            f"selected-seat broadcast on {seat.spec.label}",
            lambda seat=seat: any(
                row.get("body") == broadcast
                for row in (seat.latest_body(f"state/collab/conversation/{team}") or {}).get("messages", [])
            ),
        )
    evidence["broadcast"] = {"members": len(seats), "passed": True}

    direct_spaces: dict[tuple[int, int], str] = {}
    for left in range(len(seats)):
        for right in range(left + 1, len(seats)):
            name = f"Acceptance · {seats[left].spec.label} · {seats[right].spec.label} · {run_id}"
            space = create_space(seats[left], "direct", name)
            owned_spaces.append((seats[left], space))
            add_members(seats[left], space, [seats[left], seats[right]], name)
            direct_spaces[(left, right)] = space

    for source_index, source in enumerate(seats):
        for target_index, target in enumerate(seats):
            if source_index == target_index:
                continue
            key = tuple(sorted((source_index, target_index)))
            space = direct_spaces[key]
            pair = {"source": source.spec.label, "target": target.spec.label}
            send_and_observe(source, target, space, f"message:{run_id}:{source.spec.label}:{target.spec.label}")
            pair["message"] = "passed"
            call_and_observe(source, target, space)
            pair["call_signaling"] = "passed"
            clipboard_and_observe(source, target, f"clipboard:{run_id}:{source.spec.label}:{target.spec.label}")
            pair["clipboard"] = "passed"
            file_and_observe(source, target, space, f"file:{run_id}:{source.spec.label}:{target.spec.label}\n", cleanup)
            pair["file"] = "passed"
            evidence["directed_pairs"].append(pair)
            write_evidence(evidence)

    if not args.skip_recovery:
        for seat in seats:
            units = recovery_cycle(seat)
            evidence["recovery"].append({"seat": seat.spec.label, "units": units, "passed": True})
            write_evidence(evidence)

    evidence["result"] = "passed" if activity_mode == "required-baseline" else "inspection-passed"
    return 0


try:
    exit_code = main()
except Exception as error:
    evidence["result"] = "failed"
    evidence["failure"] = redact(str(error))
    write_evidence(evidence)
    print(f"three-seat acceptance failed: {redact(str(error))}", file=sys.stderr)
    exit_code = 1
finally:
    # Delete only exact per-run files and spaces. Missing cleanup targets are
    # harmless; no broad glob or recursive delete is ever used.
    for seat, path in list(dict.fromkeys(cleanup)):
        seat.run(f"rm -f -- {shlex.quote(path)}", root=path.startswith("/mnt/"), check=False)
    for owner, space in reversed(owned_spaces):
        try:
            command(owner, {"delete_space": {"space": space}})
        except Exception:
            pass
    password = b""

raise SystemExit(exit_code)
