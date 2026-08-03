#!/usr/bin/env python3
"""Five-seat Mesh Teams/clipboard/file acceptance and recovery matrix.

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


SEATS = (
    SeatSpec("t480", "172.20.146.138", "mm", "password", "sudo-password"),
    SeatSpec("eagle", "172.20.146.145", "mm", "password", "sudo-password"),
    SeatSpec("basement", "172.20.0.15", "root", "key", "root"),
    SeatSpec("dell", "172.20.146.225", "mm", "key", "sudo-n"),
    SeatSpec("surface", "172.20.146.79", "root", "password", "root"),
)

SSH_COMMON = (
    "-o", "StrictHostKeyChecking=accept-new",
    "-o", "ConnectTimeout=7",
    "-o", "ServerAliveInterval=5",
    "-o", "ServerAliveCountMax=2",
)
ACCEPTANCE_BASE = (
    "systemd-run --quiet --wait --pipe --collect "
    "-p LoadCredentialEncrypted=cloud-arm-key:/etc/credstore.encrypted/cloud-arm-key "
    "-p Environment=MDE_BUS_ROOT=/run/mde-bus /usr/bin/mde-shell-egui"
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
        if self.spec.ssh_auth == "password":
            return ["sshpass", "-f", str(args.password_file), "ssh", *SSH_COMMON,
                    f"{self.spec.user}@{self.spec.address}"]
        return ["ssh", "-i", str(self.key), *SSH_COMMON,
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
        command = f"{ACCEPTANCE_BASE} {shlex.quote(verb)}"
        if argument is not None:
            command += f" {shlex.quote(argument)}"
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
    return text.replace(password.decode("utf-8", "ignore"), "[REDACTED]")


def wait_for(description: str, predicate, timeout: int = 75, interval: float = 1.0):
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except (Failure, json.JSONDecodeError, OSError) as error:
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
    remote_path = f"/tmp/mde-five-seat-{run_id}-{source.spec.label}-{target.spec.label}.txt"
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
    units = [active_unit(seat, "nebula*.service"), active_unit(seat, "syncthing*.service"), "mackesd.service"]
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
    parser = argparse.ArgumentParser()
    parser.add_argument("--password-file", type=Path, required=True)
    parser.add_argument("--key", type=Path, default=Path("/root/.ssh/mackes_mesh_ed25519"))
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--skip-recovery", action="store_true")
    return parser.parse_args()


def write_evidence(state: dict) -> None:
    args.evidence.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.evidence.with_name(args.evidence.name + f".{os.getpid()}.tmp")
    temporary.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, args.evidence)


args = parse_args()
password = args.password_file.read_bytes().splitlines()[0]
if not password:
    raise SystemExit("password file has an empty first line")
run_id = f"{int(time.time())}-{uuid.uuid4().hex[:8]}"
evidence: dict = {
    "schema_version": 1,
    "run_id": run_id,
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
    for spec in SEATS:
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

    if len(seats) < 2:
        evidence["result"] = "skipped-insufficient-reachable-seats"
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

    team_name = f"Acceptance · all seats · {run_id}"
    team = create_space(seats[0], "team", team_name)
    owned_spaces.append((seats[0], team))
    add_members(seats[0], team, seats, team_name)
    broadcast = f"broadcast:{run_id}"
    command(seats[0], {"send_message": {"space": team, "thread": None, "body": broadcast}})
    for seat in seats:
        wait_for(
            f"all-seat broadcast on {seat.spec.label}",
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

    evidence["result"] = "passed"
    return 0


try:
    exit_code = main()
except Exception as error:
    evidence["result"] = "failed"
    evidence["failure"] = redact(str(error))
    write_evidence(evidence)
    print(f"five-seat acceptance failed: {redact(str(error))}", file=sys.stderr)
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
