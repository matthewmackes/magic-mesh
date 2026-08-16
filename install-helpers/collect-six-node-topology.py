#!/usr/bin/env python3
"""Collect bounded, read-only live evidence for the six-node topology verifier.

The collector never runs a drill and never turns current reachability into proof
that a historical loss, failover, or corrected-forward recovery succeeded.  It
does two things:

* probes exactly six explicit SSH targets using a fixed read-only command set;
* binds each target to a local candidate manifest, the installed RPM payload
  digest, and SHA-256 digests of its key installed binaries;
* reads each target's revision-scoped v2 drill ledger and converts only
  complete, typed semantic observations into the existing verifier schema.

The drill runner (which is intentionally outside this collector) must place one
JSON ledger on each node at::

    /var/lib/mde/six-node-observations/<revision>/<node-id>.json

The ledger is bound to the node id, hostname, SHA-256 of ``/etc/machine-id``,
source revision, candidate digests, timestamps, and typed before/action/after
facts for every required scenario and recovery observation.  Free-form pass
text and legacy v1 ledgers are diagnostic-only and cannot pass.  The collector
reads the ledger without ``sudo``.
It also requires active mackesd, Nebula, and Syncthing services and a fresh
``/run/mde/mesh-status.json`` view containing all six explicit nodes online.

Targets use this deliberately credential-free form::

    --target ID,ROLE,SSH_HOST,EXPECTED_HOSTNAME,MACHINE_ID_SHA256

Authentication is public-key/agent only.  Password and keyboard-interactive
SSH are disabled, host-key checking is strict, credential-bearing target URIs
are rejected, and credential-like command text is rejected.  Sensitive values
found in captured stdout/stderr are redacted before any artifact is written;
private-key material is rejected outright.

Examples::

    collect-six-node-topology.py --revision <40-hex> --output topology.json \
      --candidate-manifest candidate-digests.json \
      --target lh-1,lighthouse,10.42.0.1,lh-1,<machine-id-sha256> ...
    collect-six-node-topology.py --revision <40-hex> --dry-run \
      --candidate-manifest candidate-digests.json \
      --target ... --target ...
    collect-six-node-topology.py --self-test

On success, ``--output`` and its adjacent ``<stem>.artifacts`` directory are
published without overwriting existing evidence.  On failure no verifier input
is published; a bounded ``<output>.failed.json`` diagnostic preserves the safe
per-command outcomes when possible.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import re
import selectors
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from types import ModuleType
from typing import Any, Callable, NoReturn, Protocol, Sequence


KIND_LEDGER = "mcnf-six-node-observation-v2"
KIND_COLLECTION = "mcnf-six-node-live-collection-v1"
KIND_SCENARIO = "mcnf-six-node-scenario-observation-v2"
KIND_RECOVERY = "mcnf-six-node-recovery-observation-v2"
KIND_DRILL_EVENT = "mcnf-six-node-drill-event-v2"
KIND_CANDIDATE_MANIFEST = "mcnf-candidate-digest-manifest-v1"
KIND_FAILURE = "mcnf-six-node-collection-failure-v1"
KIND_PLAN = "mcnf-six-node-collection-plan-v1"

REVISION_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")
NODE_ID_RE = re.compile(r"^[a-z][a-z0-9-]{0,62}$")
HOST_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]{0,252}$")
HOSTNAME_RE = re.compile(
    r"^(?=.{1,253}$)[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?$"
)
SSH_USER_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_.-]{0,31}$")
MACHINE_ID_RE = re.compile(r"^[0-9a-f]{32}$")
PACKAGE_RE = re.compile(
    r"^(?:magic-mesh|magic-mesh-lighthouse) "
    r"[A-Za-z0-9.+~_-]+\.[A-Za-z0-9_]+$"
)
DRILL_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{7,127}$")
EVENT_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{7,191}$")
RPM_PAYLOAD_DIGEST_RE = re.compile(
    r"^(?:8|sha256|SHA256) (?P<digest>[0-9a-f]{64})$"
)
SHA256SUM_RE = re.compile(
    r"^(?P<digest>[0-9a-f]{64})  (?P<path>/usr/bin/(?:mackesd|mde-shell-egui))$"
)

MAX_COMMAND_TEXT_BYTES = 2 * 1024
MAX_EVENT_STREAM_BYTES = 64 * 1024
MAX_PROBE_OUTPUT_BYTES = 1024 * 1024
MAX_FAILURE_REPORT_BYTES = 4 * 1024 * 1024
MAX_LEDGER_BYTES = 1024 * 1024
MAX_TOPOLOGY_BYTES = 1024 * 1024
MAX_CANDIDATE_MANIFEST_BYTES = 64 * 1024
MAX_TIMEOUT_SECONDS = 60
MAX_AGE_SECONDS = 7 * 24 * 60 * 60
DEFAULT_OBSERVATION_ROOT = "/var/lib/mde/six-node-observations"

PRIVATE_MATERIAL_RE = re.compile(
    r"-----BEGIN (?:OPENSSH |RSA |EC |DSA |PGP )?PRIVATE KEY-----",
    re.IGNORECASE,
)
SSHPASS_RE = re.compile(r"(?:^|\s)sshpass(?:\s|$)", re.IGNORECASE)
URL_CREDENTIAL_RE = re.compile(
    r"(?P<scheme>[a-z][a-z0-9+.-]*://)(?P<userinfo>[^/@\s:]+:[^/@\s]+)@",
    re.IGNORECASE,
)
AUTHORIZATION_RE = re.compile(
    r"(?im)^(?P<prefix>\s*authorization\s*:\s*(?:basic|bearer)\s+)[^\s]+"
)
SECRET_ASSIGNMENT_RE = re.compile(
    r"(?i)(?P<prefix>\b(?:password|passwd|passphrase|token|secret|api[_-]?key|private[_-]?key)\b\s*[:=]\s*)(?P<value>[^\s,;]+)"
)
SECRET_OPTION_EQUALS_RE = re.compile(
    r"(?i)(?P<prefix>--(?:password|passwd|passphrase|token|secret|api[_-]?key)=)(?P<value>[^\s]+)"
)
SECRET_OPTION_VALUE_RE = re.compile(
    r"(?i)(?P<prefix>--(?:password|passwd|passphrase|token|secret|api[_-]?key)\s+)(?P<value>[^\s]+)"
)
MESH_ENROLLMENT_TOKEN_RE = re.compile(
    r"(?i)(?<![A-Za-z0-9])mesh:[A-Za-z0-9._-]{1,128}"
    r"@[0-9]{1,3}(?:\.[0-9]{1,3}){3}:[0-9]{1,5}"
    r"#[A-Za-z0-9_-]+={0,2}(?:\?fp=[0-9a-f]{64})?"
)

READ_BOUNDED_FILE = r"""
import os
import stat
import sys

path = sys.argv[1]
limit = int(sys.argv[2])
flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
fd = os.open(path, flags)
try:
    metadata = os.fstat(fd)
    if not stat.S_ISREG(metadata.st_mode):
        raise SystemExit(66)
    if metadata.st_size > limit:
        raise SystemExit(67)
    chunks = []
    remaining = limit + 1
    while remaining:
        chunk = os.read(fd, min(65536, remaining))
        if not chunk:
            break
        chunks.append(chunk)
        remaining -= len(chunk)
    data = b"".join(chunks)
    if len(data) > limit:
        raise SystemExit(67)
    sys.stdout.buffer.write(data)
finally:
    os.close(fd)
""".strip()


class CollectionError(ValueError):
    """A required live observation is absent, unsafe, or contradictory."""


def fail(message: str) -> NoReturn:
    raise CollectionError(message)


def load_verifier() -> ModuleType:
    path = Path(__file__).with_name("verify-six-node-topology.py")
    spec = importlib.util.spec_from_file_location("mcnf_six_node_topology_verifier", path)
    if spec is None or spec.loader is None:
        fail(f"cannot load adjacent topology verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


VERIFIER = load_verifier()
SCENARIOS: tuple[str, ...] = tuple(VERIFIER.SCENARIOS)
RECOVERY_STATES: tuple[str, ...] = tuple(VERIFIER.RECOVERY_STATES)
ROLES: dict[str, int] = dict(VERIFIER.ROLES)


def printable_text(value: Any, field_name: str, maximum_bytes: int) -> str:
    if not isinstance(value, str):
        fail(f"{field_name} must be a string")
    encoded = value.encode("utf-8")
    if len(encoded) > maximum_bytes:
        fail(f"{field_name} exceeds {maximum_bytes} bytes")
    if any(ord(character) < 32 and character not in "\n\r\t" for character in value):
        fail(f"{field_name} contains control characters")
    if "\x7f" in value:
        fail(f"{field_name} contains control characters")
    return value


def redact_stream(value: Any, field_name: str) -> tuple[str, int]:
    text = printable_text(value, field_name, MAX_EVENT_STREAM_BYTES)
    if PRIVATE_MATERIAL_RE.search(text):
        fail(f"{field_name} contains private-key material")
    redactions = 0

    def replace(match: re.Match[str]) -> str:
        nonlocal redactions
        redactions += 1
        prefix = match.groupdict().get("prefix")
        if prefix is not None:
            return prefix + "[REDACTED]"
        scheme = match.groupdict().get("scheme")
        if scheme is not None:
            return scheme + "[REDACTED]@"
        return "[REDACTED]"

    for pattern in (
        MESH_ENROLLMENT_TOKEN_RE,
        URL_CREDENTIAL_RE,
        AUTHORIZATION_RE,
        SECRET_ASSIGNMENT_RE,
        SECRET_OPTION_EQUALS_RE,
        SECRET_OPTION_VALUE_RE,
    ):
        text = pattern.sub(replace, text)
    return text, redactions


def safe_command(value: Any, field_name: str) -> str:
    command = printable_text(value, field_name, MAX_COMMAND_TEXT_BYTES).strip()
    if not command:
        fail(f"{field_name} must not be empty")
    if PRIVATE_MATERIAL_RE.search(command) or SSHPASS_RE.search(command):
        fail(f"{field_name} contains credential material")
    redacted, count = redact_stream(command, field_name)
    if count or redacted != command:
        fail(f"{field_name} contains a credential-bearing argument")
    return command


def positive_integer(value: Any, field_name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        fail(f"{field_name} must be a positive integer")
    return value


def require_revision(value: str, field_name: str = "revision") -> str:
    if not isinstance(value, str) or not REVISION_RE.fullmatch(value) or value == "0" * 40:
        fail(f"{field_name} must be a non-null 40-character lowercase Git revision")
    return value


def require_digest(value: Any, field_name: str) -> str:
    if not isinstance(value, str) or DIGEST_RE.fullmatch(value) is None:
        fail(f"{field_name} must be a 64-character lowercase SHA-256 digest")
    return value


def require_identifier(value: Any, field_name: str, pattern: re.Pattern[str]) -> str:
    if not isinstance(value, str) or pattern.fullmatch(value) is None:
        fail(f"{field_name} is malformed")
    return value


def decode_json_object(raw: bytes | str, field_name: str) -> dict[str, Any]:
    def reject_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                fail(f"{field_name} contains duplicate JSON field {key!r}")
            result[key] = value
        return result

    def reject_constant(value: str) -> NoReturn:
        fail(f"{field_name} contains non-finite JSON number {value}")

    try:
        text = raw.decode("utf-8", errors="strict") if isinstance(raw, bytes) else raw
        value = json.loads(
            text,
            object_pairs_hook=reject_duplicates,
            parse_constant=reject_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(f"{field_name} is not valid UTF-8 JSON: {exc}")
    if not isinstance(value, dict):
        fail(f"{field_name} must be a JSON object")
    return value


def validate_observation_root(value: str) -> str:
    if len(value.encode("utf-8")) > 256 or any(ord(char) < 32 for char in value):
        fail("observation root is invalid")
    path = PurePosixPath(value)
    if not path.is_absolute() or path == PurePosixPath("/") or ".." in path.parts:
        fail("observation root must be a bounded absolute path below /")
    if not re.fullmatch(r"/[A-Za-z0-9._/-]+", value):
        fail("observation root contains unsupported characters")
    return str(path)


@dataclass(frozen=True)
class Target:
    node_id: str
    role: str
    host: str
    expected_hostname: str
    machine_id_sha256: str


@dataclass(frozen=True)
class CandidateExpectation:
    package: str
    package_payload_sha256: str
    binaries: dict[str, str]


@dataclass(frozen=True)
class CandidateManifest:
    revision: str
    roles: dict[str, CandidateExpectation]
    sha256: str


def expected_binary_names(role: str) -> set[str]:
    names = {"mackesd"}
    if role == "workstation":
        names.add("mde-shell-egui")
    return names


def package_name_for_role(role: str) -> str:
    return "magic-mesh-lighthouse" if role == "lighthouse" else "magic-mesh"


def validate_candidate_record(value: Any, role: str, field_name: str) -> CandidateExpectation:
    if not isinstance(value, dict) or set(value) != {
        "binaries",
        "package",
        "package_payload_sha256",
    }:
        fail(f"{field_name} must contain exactly package and binary digest fields")
    package = value["package"]
    if not isinstance(package, str) or PACKAGE_RE.fullmatch(package) is None:
        fail(f"{field_name}.package is malformed")
    if package.split(" ", 1)[0] != package_name_for_role(role):
        fail(f"{field_name}.package does not match the {role} package contract")
    package_digest = require_digest(
        value["package_payload_sha256"], f"{field_name}.package_payload_sha256"
    )
    binaries = value["binaries"]
    required_names = expected_binary_names(role)
    if not isinstance(binaries, dict) or set(binaries) != required_names:
        fail(f"{field_name}.binaries must contain exactly {sorted(required_names)}")
    normalized_binaries = {
        name: require_digest(digest, f"{field_name}.binaries.{name}")
        for name, digest in binaries.items()
    }
    return CandidateExpectation(package, package_digest, normalized_binaries)


def read_candidate_manifest(path: Path, revision: str) -> CandidateManifest:
    try:
        metadata = path.lstat()
    except OSError as exc:
        fail(f"candidate manifest is not readable: {exc}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail("candidate manifest must be a regular non-symlink file")
    if metadata.st_size > MAX_CANDIDATE_MANIFEST_BYTES:
        fail(f"candidate manifest exceeds {MAX_CANDIDATE_MANIFEST_BYTES} bytes")
    try:
        raw = path.read_bytes()
    except OSError as exc:
        fail(f"candidate manifest is not readable: {exc}")
    value = decode_json_object(raw, "candidate manifest")
    if set(value) != {"kind", "revision", "roles", "schema_version"}:
        fail("candidate manifest has unsupported fields")
    if value["schema_version"] != 1 or isinstance(value["schema_version"], bool):
        fail("candidate manifest schema_version must be integer 1")
    if value["kind"] != KIND_CANDIDATE_MANIFEST:
        fail("candidate manifest kind is unsupported")
    manifest_revision = require_revision(value["revision"], "candidate manifest revision")
    if manifest_revision != revision:
        fail("candidate manifest revision does not match --revision")
    roles = value["roles"]
    if not isinstance(roles, dict) or set(roles) != set(ROLES):
        fail("candidate manifest must contain lighthouse and workstation roles")
    return CandidateManifest(
        revision=revision,
        roles={
            role: validate_candidate_record(
                record, role, f"candidate manifest.roles.{role}"
            )
            for role, record in roles.items()
        },
        sha256=hashlib.sha256(raw).hexdigest(),
    )


def parse_target(raw: str) -> Target:
    printable_text(raw, "target", 1024)
    if "://" in raw or "@" in raw:
        fail("target must not contain a URI, userinfo, or credentials")
    fields = raw.split(",")
    if len(fields) != 5:
        fail(
            "target must be ID,ROLE,SSH_HOST,EXPECTED_HOSTNAME,MACHINE_ID_SHA256"
        )
    node_id, role, host, hostname, machine_digest = (field.strip() for field in fields)
    if not NODE_ID_RE.fullmatch(node_id):
        fail(f"target node id is invalid: {node_id!r}")
    if role not in ROLES:
        fail(f"{node_id}: role must be lighthouse or workstation")
    if not HOST_RE.fullmatch(host) or host.startswith("-"):
        fail(f"{node_id}: SSH host is invalid")
    if not HOSTNAME_RE.fullmatch(hostname):
        fail(f"{node_id}: expected hostname is invalid")
    if not DIGEST_RE.fullmatch(machine_digest):
        fail(f"{node_id}: machine-id SHA-256 must be 64 lowercase hex characters")
    return Target(node_id, role, host, hostname, machine_digest)


def validate_targets(targets: Sequence[Target]) -> list[Target]:
    if len(targets) != 6:
        fail("exactly six explicit --target records are required")
    for attribute, label in (
        ("node_id", "node id"),
        ("host", "SSH host"),
        ("expected_hostname", "expected hostname"),
        ("machine_id_sha256", "machine-id digest"),
    ):
        values = [getattr(target, attribute).lower() for target in targets]
        if len(set(values)) != len(values):
            fail(f"six-node targets contain a duplicate {label}")
    counts = {role: sum(target.role == role for target in targets) for role in ROLES}
    if counts != ROLES:
        fail(
            "targets must contain exactly three lighthouses and three workstations"
        )
    return sorted(targets, key=lambda target: target.node_id)


@dataclass(frozen=True)
class Probe:
    name: str
    remote_argv: tuple[str, ...]
    logical_command: str
    output_limit: int = 16 * 1024


@dataclass
class RawOutcome:
    returncode: int | None
    stdout: bytes
    stderr: bytes
    started_at_ms: int
    finished_at_ms: int
    timed_out: bool = False
    output_limited: bool = False


class Runner(Protocol):
    def run(self, target: Target, probe: Probe, timeout_seconds: int) -> RawOutcome:
        """Run one fixed read-only probe."""


def bounded_process(
    argv: Sequence[str],
    *,
    timeout_seconds: int,
    output_limit: int,
    environment: dict[str, str],
) -> RawOutcome:
    started_at_ms = time.time_ns() // 1_000_000
    process = subprocess.Popen(
        list(argv),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
        close_fds=True,
    )
    assert process.stdout is not None and process.stderr is not None
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ, "stdout")
    selector.register(process.stderr, selectors.EVENT_READ, "stderr")
    streams = {"stdout": bytearray(), "stderr": bytearray()}
    deadline = time.monotonic() + timeout_seconds
    timed_out = False
    output_limited = False
    try:
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                timed_out = True
                process.kill()
                break
            events = selector.select(min(remaining, 0.25))
            if not events and process.poll() is not None:
                events = [(key, selectors.EVENT_READ) for key in selector.get_map().values()]
            for key, _ in events:
                try:
                    chunk = os.read(key.fileobj.fileno(), 16384)
                except BlockingIOError:
                    continue
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                streams[key.data].extend(chunk)
                if len(streams["stdout"]) + len(streams["stderr"]) > output_limit:
                    output_limited = True
                    process.kill()
                    break
            if output_limited:
                break
        if timed_out or output_limited:
            process.wait(timeout=5)
        else:
            process.wait(timeout=max(1, timeout_seconds))
    finally:
        selector.close()
        if process.poll() is None:
            process.kill()
            process.wait()
    finished_at_ms = time.time_ns() // 1_000_000
    return RawOutcome(
        returncode=process.returncode,
        stdout=bytes(streams["stdout"][:output_limit]),
        stderr=bytes(streams["stderr"][:output_limit]),
        started_at_ms=started_at_ms,
        finished_at_ms=finished_at_ms,
        timed_out=timed_out,
        output_limited=output_limited,
    )


class SshRunner:
    def __init__(
        self,
        *,
        ssh_user: str | None,
        ssh_port: int,
        connect_timeout_seconds: int,
        known_hosts: Path | None,
    ) -> None:
        self.ssh_user = ssh_user
        self.ssh_port = ssh_port
        self.connect_timeout_seconds = connect_timeout_seconds
        self.known_hosts = known_hosts

    def run(self, target: Target, probe: Probe, timeout_seconds: int) -> RawOutcome:
        destination = target.host
        if self.ssh_user is not None:
            destination = f"{self.ssh_user}@{destination}"
        command = [
            "ssh",
            "-p",
            str(self.ssh_port),
            "-o",
            "BatchMode=yes",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "KbdInteractiveAuthentication=no",
            "-o",
            "NumberOfPasswordPrompts=0",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "UpdateHostKeys=no",
            "-o",
            "ConnectionAttempts=1",
            "-o",
            f"ConnectTimeout={self.connect_timeout_seconds}",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=1",
            "-o",
            "LogLevel=ERROR",
        ]
        if self.known_hosts is not None:
            command.extend(("-o", f"UserKnownHostsFile={self.known_hosts}"))
        command.extend(("--", destination, shlex.join(probe.remote_argv)))
        environment = {
            "HOME": os.environ.get("HOME", "/nonexistent"),
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        }
        if "SSH_AUTH_SOCK" in os.environ:
            environment["SSH_AUTH_SOCK"] = os.environ["SSH_AUTH_SOCK"]
        return bounded_process(
            command,
            timeout_seconds=timeout_seconds,
            output_limit=probe.output_limit,
            environment=environment,
        )


def read_file_probe(name: str, path: str, limit: int, target: Target) -> Probe:
    return Probe(
        name=name,
        remote_argv=("python3", "-c", READ_BOUNDED_FILE, path, str(limit)),
        logical_command=f"ssh <target:{target.node_id}> read-bounded {path}",
        output_limit=limit + 4096,
    )


def probes_for(target: Target, observation_root: str, revision: str) -> list[Probe]:
    ledger_path = f"{observation_root}/{revision}/{target.node_id}.json"
    package_name = package_name_for_role(target.role)
    candidate_probes = [
        Probe(
            "revision.package_payload_sha256",
            (
                "rpm",
                "-q",
                "--qf",
                "%{PAYLOADDIGESTALGO} %{PAYLOADDIGEST}\\n",
                package_name,
            ),
            f"ssh <target:{target.node_id}> rpm -q {package_name} payload-digest",
            4096,
        ),
        Probe(
            "revision.binary.mackesd_sha256",
            ("sha256sum", "--", "/usr/bin/mackesd"),
            f"ssh <target:{target.node_id}> sha256sum /usr/bin/mackesd",
            4096,
        ),
    ]
    if target.role == "workstation":
        candidate_probes.append(
            Probe(
                "revision.binary.mde_shell_egui_sha256",
                ("sha256sum", "--", "/usr/bin/mde-shell-egui"),
                f"ssh <target:{target.node_id}> sha256sum /usr/bin/mde-shell-egui",
                4096,
            )
        )
    return [
        Probe(
            "identity.hostname",
            ("hostname", "--fqdn"),
            f"ssh <target:{target.node_id}> hostname --fqdn",
            4096,
        ),
        read_file_probe("identity.machine_id", "/etc/machine-id", 4096, target),
        Probe(
            "identity.clock",
            ("date", "+%s%3N"),
            f"ssh <target:{target.node_id}> date +%s%3N",
            4096,
        ),
        Probe(
            "revision.package",
            (
                "rpm",
                "-q",
                "--qf",
                "%{NAME} %{VERSION}-%{RELEASE}.%{ARCH}\\n",
                package_name,
            ),
            f"ssh <target:{target.node_id}> rpm -q {package_name}",
            4096,
        ),
        *candidate_probes,
        *[
            Probe(
                f"service.{service}",
                ("systemctl", "is-active", f"{service}.service"),
                f"ssh <target:{target.node_id}> systemctl is-active {service}.service",
                4096,
            )
            for service in ("mackesd", "nebula", "syncthing")
        ],
        read_file_probe("topology.snapshot", "/run/mde/mesh-status.json", MAX_TOPOLOGY_BYTES, target),
        read_file_probe("recovery.ledger", ledger_path, MAX_LEDGER_BYTES, target),
    ]


def normalize_outcome(probe: Probe, raw: RawOutcome) -> dict[str, Any]:
    try:
        stdout_raw = raw.stdout.decode("utf-8", errors="strict")
        stderr_raw = raw.stderr.decode("utf-8", errors="strict")
    except UnicodeDecodeError as exc:
        fail(f"{probe.name} returned non-UTF-8 output: {exc}")
    stdout, stdout_redactions = redact_stream(stdout_raw, f"{probe.name}.stdout")
    stderr, stderr_redactions = redact_stream(stderr_raw, f"{probe.name}.stderr")
    return {
        "probe": probe.name,
        "command": probe.logical_command,
        "started_at_ms": raw.started_at_ms,
        "finished_at_ms": raw.finished_at_ms,
        "returncode": raw.returncode,
        "timed_out": raw.timed_out,
        "output_limited": raw.output_limited,
        "stdout": stdout,
        "stderr": stderr,
        "redactions": stdout_redactions + stderr_redactions,
    }


def outcome_passed(outcome: dict[str, Any]) -> bool:
    return (
        outcome["returncode"] == 0
        and outcome["timed_out"] is False
        and outcome["output_limited"] is False
    )


def require_outcome(outcome: dict[str, Any], node_id: str) -> str:
    if not outcome_passed(outcome):
        fail(
            f"{node_id}/{outcome['probe']} failed "
            f"(returncode={outcome['returncode']}, timeout={outcome['timed_out']}, "
            f"output_limited={outcome['output_limited']})"
        )
    return outcome["stdout"].strip()


def parse_json_output(outcome: dict[str, Any], node_id: str) -> dict[str, Any]:
    text = require_outcome(outcome, node_id)
    return decode_json_object(text, f"{node_id}/{outcome['probe']}")


def parse_package_payload_digest(outcome: dict[str, Any], node_id: str) -> str:
    text = require_outcome(outcome, node_id)
    match = RPM_PAYLOAD_DIGEST_RE.fullmatch(text)
    if match is None:
        fail(
            f"{node_id}/{outcome['probe']} did not return a SHA-256 RPM payload digest"
        )
    return match.group("digest")


def parse_binary_digest(
    outcome: dict[str, Any], node_id: str, expected_path: str
) -> str:
    text = require_outcome(outcome, node_id)
    match = SHA256SUM_RE.fullmatch(text)
    if match is None or match.group("path") != expected_path:
        fail(f"{node_id}/{outcome['probe']} returned malformed binary digest evidence")
    return match.group("digest")


def semantic_object(
    value: Any, field_name: str, expected_keys: set[str]
) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != expected_keys:
        fail(f"{field_name} must contain exactly its typed semantic fields")
    return dict(value)


def require_exact(value: Any, expected: Any, field_name: str) -> None:
    if value != expected or (isinstance(expected, bool) and not isinstance(value, bool)):
        fail(f"{field_name} must be {expected!r}")


def validate_scenario_semantic(
    value: Any,
    *,
    scenario: str,
    node_id: str,
    field_name: str,
    revision: str,
    package_digest: str,
    all_node_ids: set[str],
    lighthouse_ids: set[str],
) -> dict[str, Any]:
    common = {"action", "scenario", "subject_node_id"}
    if scenario == "join":
        semantic = semantic_object(
            value,
            field_name,
            common
            | {
                "membership_after",
                "membership_before",
                "services_active_after",
                "topology_nodes_online_after",
            },
        )
        require_exact(semantic["action"], "enroll_node", f"{field_name}.action")
        require_exact(semantic["membership_before"], "absent", f"{field_name}.membership_before")
        require_exact(semantic["membership_after"], "present", f"{field_name}.membership_after")
        require_exact(semantic["services_active_after"], True, f"{field_name}.services_active_after")
        require_exact(
            semantic["topology_nodes_online_after"],
            len(all_node_ids),
            f"{field_name}.topology_nodes_online_after",
        )
    elif scenario == "steady_state":
        semantic = semantic_object(
            value,
            field_name,
            common
            | {
                "duration_ms",
                "leader_id_end",
                "leader_id_start",
                "services_active_end",
                "topology_nodes_online_end",
                "topology_nodes_online_start",
            },
        )
        require_exact(semantic["action"], "observe_steady_state", f"{field_name}.action")
        duration = positive_integer(semantic["duration_ms"], f"{field_name}.duration_ms")
        if duration < 5 * 60 * 1000:
            fail(f"{field_name}.duration_ms must cover at least five minutes")
        start_leader = semantic["leader_id_start"]
        end_leader = semantic["leader_id_end"]
        if start_leader not in lighthouse_ids or end_leader != start_leader:
            fail(f"{field_name} must prove one stable lighthouse leader")
        require_exact(semantic["services_active_end"], True, f"{field_name}.services_active_end")
        for key in ("topology_nodes_online_start", "topology_nodes_online_end"):
            require_exact(semantic[key], len(all_node_ids), f"{field_name}.{key}")
    elif scenario == "loss":
        semantic = semantic_object(
            value,
            field_name,
            common
            | {
                "fault_target_node_id",
                "loss_detected",
                "observer_count",
                "presence_before",
                "presence_during",
            },
        )
        require_exact(semantic["action"], "inject_overlay_loss", f"{field_name}.action")
        require_exact(semantic["fault_target_node_id"], node_id, f"{field_name}.fault_target_node_id")
        require_exact(semantic["presence_before"], "online", f"{field_name}.presence_before")
        require_exact(semantic["presence_during"], "offline", f"{field_name}.presence_during")
        require_exact(semantic["loss_detected"], True, f"{field_name}.loss_detected")
        require_exact(
            semantic["observer_count"],
            len(all_node_ids) - 1,
            f"{field_name}.observer_count",
        )
    elif scenario == "failover":
        semantic = semantic_object(
            value,
            field_name,
            common
            | {
                "automatic",
                "failed_lighthouse_id",
                "leader_after",
                "leader_before",
                "quorum_preserved",
            },
        )
        require_exact(semantic["action"], "inject_lighthouse_loss", f"{field_name}.action")
        failed = semantic["failed_lighthouse_id"]
        before = semantic["leader_before"]
        after = semantic["leader_after"]
        if failed not in lighthouse_ids or before != failed or after not in lighthouse_ids or after == failed:
            fail(f"{field_name} does not prove a lighthouse leader change")
        require_exact(semantic["automatic"], True, f"{field_name}.automatic")
        require_exact(semantic["quorum_preserved"], True, f"{field_name}.quorum_preserved")
    elif scenario == "re_enrollment":
        semantic = semantic_object(
            value,
            field_name,
            common
            | {
                "identity_rotated",
                "membership_after",
                "membership_before",
                "token_reuse_rejected",
                "topology_nodes_online_after",
            },
        )
        require_exact(semantic["action"], "re_enroll_node", f"{field_name}.action")
        require_exact(semantic["membership_before"], "absent", f"{field_name}.membership_before")
        require_exact(semantic["membership_after"], "present", f"{field_name}.membership_after")
        require_exact(semantic["identity_rotated"], True, f"{field_name}.identity_rotated")
        require_exact(semantic["token_reuse_rejected"], True, f"{field_name}.token_reuse_rejected")
        require_exact(
            semantic["topology_nodes_online_after"],
            len(all_node_ids),
            f"{field_name}.topology_nodes_online_after",
        )
    elif scenario == "corrected_forward_recovery":
        semantic = semantic_object(
            value,
            field_name,
            common
            | {
                "forward_revision",
                "installed_package_payload_sha256_after",
                "previous_revision",
                "re_enrolled",
                "rollback",
                "services_active_after",
            },
        )
        require_exact(semantic["action"], "correct_forward", f"{field_name}.action")
        previous = require_revision(semantic["previous_revision"], f"{field_name}.previous_revision")
        forward = require_revision(semantic["forward_revision"], f"{field_name}.forward_revision")
        if previous == forward or forward != revision:
            fail(f"{field_name} did not advance to the requested revision")
        require_exact(semantic["re_enrolled"], True, f"{field_name}.re_enrolled")
        require_exact(semantic["rollback"], False, f"{field_name}.rollback")
        require_exact(semantic["services_active_after"], True, f"{field_name}.services_active_after")
        require_exact(
            require_digest(
                semantic["installed_package_payload_sha256_after"],
                f"{field_name}.installed_package_payload_sha256_after",
            ),
            package_digest,
            f"{field_name}.installed_package_payload_sha256_after",
        )
    else:
        fail(f"{field_name} has unsupported scenario {scenario!r}")
    require_exact(semantic["scenario"], scenario, f"{field_name}.scenario")
    require_exact(semantic["subject_node_id"], node_id, f"{field_name}.subject_node_id")
    return semantic


def validate_recovery_state_semantic(
    value: Any,
    *,
    state: str,
    previous_state: str,
    field_name: str,
    revision: str,
    all_node_ids: set[str],
) -> dict[str, Any]:
    semantic = semantic_object(
        value,
        field_name,
        {
            "candidate_revision",
            "cause",
            "previous_state",
            "service_state",
            "state",
            "topology_nodes_online",
        },
    )
    expected_cause = {
        ("none", "healthy"): "baseline",
        ("healthy", "degraded"): "fault_observed",
        ("degraded", "recovering"): "repair_started",
        ("recovering", "healthy"): "health_restored",
    }.get((previous_state, state))
    if expected_cause is None:
        fail(f"{field_name} has an unsupported recovery transition")
    require_exact(semantic["state"], state, f"{field_name}.state")
    require_exact(semantic["previous_state"], previous_state, f"{field_name}.previous_state")
    require_exact(semantic["cause"], expected_cause, f"{field_name}.cause")
    require_exact(semantic["candidate_revision"], revision, f"{field_name}.candidate_revision")
    expected_service_state = {
        "baseline": "active",
        "fault_observed": "degraded",
        "repair_started": "recovering",
        "health_restored": "active",
    }[expected_cause]
    require_exact(semantic["service_state"], expected_service_state, f"{field_name}.service_state")
    online = positive_integer(semantic["topology_nodes_online"], f"{field_name}.topology_nodes_online")
    if state == "healthy" and online != len(all_node_ids):
        fail(f"{field_name} healthy state must include the complete topology")
    if state == "degraded" and online >= len(all_node_ids):
        fail(f"{field_name} degraded state must show a lost node")
    if state == "recovering" and online > len(all_node_ids):
        fail(f"{field_name} recovering state has an impossible node count")
    return semantic


def validate_recovery_failover_semantic(
    value: Any,
    *,
    failed_lighthouse: str,
    active_lighthouse: str,
    field_name: str,
    revision: str,
) -> dict[str, Any]:
    semantic = semantic_object(
        value,
        field_name,
        {
            "candidate_revision",
            "fault",
            "leader_after",
            "leader_before",
            "membership_quorum_preserved",
        },
    )
    require_exact(semantic["candidate_revision"], revision, f"{field_name}.candidate_revision")
    require_exact(semantic["fault"], "lighthouse_loss", f"{field_name}.fault")
    require_exact(semantic["leader_before"], failed_lighthouse, f"{field_name}.leader_before")
    require_exact(semantic["leader_after"], active_lighthouse, f"{field_name}.leader_after")
    require_exact(
        semantic["membership_quorum_preserved"],
        True,
        f"{field_name}.membership_quorum_preserved",
    )
    return semantic


def validate_corrected_forward_semantic(
    value: Any,
    *,
    previous_revision: str,
    revision: str,
    package_digest: str,
    field_name: str,
    all_node_ids: set[str],
) -> dict[str, Any]:
    semantic = semantic_object(
        value,
        field_name,
        {
            "installed_package_payload_sha256_after",
            "repair",
            "revision_after",
            "revision_before",
            "services_active_after",
            "topology_nodes_online_after",
        },
    )
    require_exact(semantic["repair"], "re_enroll_and_correct_forward", f"{field_name}.repair")
    require_exact(semantic["revision_before"], previous_revision, f"{field_name}.revision_before")
    require_exact(semantic["revision_after"], revision, f"{field_name}.revision_after")
    require_exact(semantic["services_active_after"], True, f"{field_name}.services_active_after")
    require_exact(
        semantic["topology_nodes_online_after"],
        len(all_node_ids),
        f"{field_name}.topology_nodes_online_after",
    )
    require_exact(
        require_digest(
            semantic["installed_package_payload_sha256_after"],
            f"{field_name}.installed_package_payload_sha256_after",
        ),
        package_digest,
        f"{field_name}.installed_package_payload_sha256_after",
    )
    return semantic


def validate_event(
    value: Any,
    *,
    node_id: str,
    field_name: str,
    now_ms: int,
    max_age_ms: int,
    drill_id: str,
    event_type: str,
    package_digest: str,
    semantic_validator: Callable[[Any], dict[str, Any]],
    extra_keys: set[str],
) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{node_id}/{field_name} must be an object")
    expected = {
        "candidate_package_payload_sha256",
        "command",
        "drill_id",
        "event_id",
        "event_type",
        "finished_at_ms",
        "kind",
        "node_id",
        "observed_at_ms",
        "returncode",
        "schema_version",
        "semantic",
        "started_at_ms",
        "status",
        "stderr",
        "stdout",
    } | extra_keys
    if set(value) != expected:
        fail(f"{node_id}/{field_name} must contain exactly its command outcome fields")
    if value["node_id"] != node_id:
        fail(f"{node_id}/{field_name}.node_id does not match")
    if value["schema_version"] != 2 or isinstance(value["schema_version"], bool):
        fail(f"{node_id}/{field_name} is not a v2 typed drill event")
    if value["kind"] != KIND_DRILL_EVENT or value["event_type"] != event_type:
        fail(f"{node_id}/{field_name} has the wrong typed event kind")
    if value["drill_id"] != drill_id:
        fail(f"{node_id}/{field_name}.drill_id does not match the ledger")
    require_identifier(value["event_id"], f"{node_id}/{field_name}.event_id", EVENT_ID_RE)
    if value["candidate_package_payload_sha256"] != package_digest:
        fail(f"{node_id}/{field_name} is not bound to the installed candidate package")
    returncode = value["returncode"]
    if (
        value["status"] != "pass"
        or isinstance(returncode, bool)
        or not isinstance(returncode, int)
        or returncode != 0
    ):
        fail(f"{node_id}/{field_name} is not a successful command outcome")
    started = positive_integer(value["started_at_ms"], f"{node_id}/{field_name}.started_at_ms")
    observed = positive_integer(value["observed_at_ms"], f"{node_id}/{field_name}.observed_at_ms")
    finished = positive_integer(value["finished_at_ms"], f"{node_id}/{field_name}.finished_at_ms")
    if not started <= observed <= finished or finished - started > 24 * 60 * 60 * 1000:
        fail(f"{node_id}/{field_name} has invalid event timing")
    age = now_ms - observed
    if age < 0 or age > max_age_ms:
        fail(f"{node_id}/{field_name} is stale or from the future (age_ms={age})")
    finished_age = now_ms - finished
    if finished_age < 0 or finished_age > max_age_ms:
        fail(f"{node_id}/{field_name} finished outside the admitted evidence window")
    command = safe_command(value["command"], f"{node_id}/{field_name}.command")
    stdout, stdout_redactions = redact_stream(
        value["stdout"], f"{node_id}/{field_name}.stdout"
    )
    stderr, stderr_redactions = redact_stream(
        value["stderr"], f"{node_id}/{field_name}.stderr"
    )
    normalized = dict(value)
    semantic = semantic_validator(value["semantic"])
    normalized.update(
        {
            "command": command,
            "stdout": stdout,
            "stderr": stderr,
            "semantic": semantic,
            "redactions": stdout_redactions + stderr_redactions,
        }
    )
    return normalized


def validate_ledger(
    value: dict[str, Any],
    *,
    target: Target,
    revision: str,
    now_ms: int,
    max_age_ms: int,
    lighthouse_ids: set[str],
    all_node_ids: set[str],
    candidate: CandidateExpectation,
) -> dict[str, Any]:
    expected_keys = {
        "candidate",
        "drill_id",
        "hostname",
        "kind",
        "machine_id_sha256",
        "node_id",
        "recorded_at_ms",
        "recovery",
        "revision",
        "scenarios",
        "schema_version",
    }
    if set(value) != expected_keys or value.get("schema_version") != 2:
        fail(f"{target.node_id}/recovery.ledger has an unsupported schema")
    if value["kind"] != KIND_LEDGER:
        fail(f"{target.node_id}/recovery.ledger has an unsupported kind")
    if value["node_id"] != target.node_id:
        fail(f"{target.node_id}/recovery.ledger node id does not match")
    if (
        not isinstance(value["hostname"], str)
        or value["hostname"].lower() != target.expected_hostname.lower()
    ):
        fail(f"{target.node_id}/recovery.ledger hostname does not match")
    if value["machine_id_sha256"] != target.machine_id_sha256:
        fail(f"{target.node_id}/recovery.ledger machine identity does not match")
    if value["revision"] != revision:
        fail(f"{target.node_id}/recovery.ledger revision does not match")
    drill_id = require_identifier(
        value["drill_id"],
        f"{target.node_id}/recovery.ledger.drill_id",
        DRILL_ID_RE,
    )
    ledger_candidate = validate_candidate_record(
        value["candidate"], target.role, f"{target.node_id}/recovery.ledger.candidate"
    )
    if ledger_candidate != candidate:
        fail(f"{target.node_id}/recovery.ledger candidate digests do not match live probes")
    recorded = positive_integer(
        value["recorded_at_ms"], f"{target.node_id}/recovery.ledger.recorded_at_ms"
    )
    recorded_age = now_ms - recorded
    if recorded_age < 0 or recorded_age > max_age_ms:
        fail(f"{target.node_id}/recovery.ledger is stale or from the future")

    scenarios = value["scenarios"]
    if not isinstance(scenarios, dict) or set(scenarios) != set(SCENARIOS):
        fail(f"{target.node_id}/recovery.ledger lacks a required scenario")
    normalized_scenarios: dict[str, dict[str, Any]] = {}
    for name in SCENARIOS:
        field_name = f"scenarios.{name}"
        normalized_scenarios[name] = validate_event(
            scenarios[name],
            node_id=target.node_id,
            field_name=field_name,
            now_ms=now_ms,
            max_age_ms=max_age_ms,
            drill_id=drill_id,
            event_type="scenario",
            package_digest=candidate.package_payload_sha256,
            semantic_validator=lambda semantic, name=name, field_name=field_name: validate_scenario_semantic(
                semantic,
                scenario=name,
                node_id=target.node_id,
                field_name=f"{target.node_id}/{field_name}.semantic",
                revision=revision,
                package_digest=candidate.package_payload_sha256,
                all_node_ids=all_node_ids,
                lighthouse_ids=lighthouse_ids,
            ),
            extra_keys=set(),
        )

    recovery = value["recovery"]
    if not isinstance(recovery, dict) or set(recovery) != {
        "corrected_forward",
        "failover",
        "states",
    }:
        fail(f"{target.node_id}/recovery.ledger lacks the recovery path")
    states = recovery["states"]
    if not isinstance(states, list) or len(states) != len(RECOVERY_STATES):
        fail(f"{target.node_id}/recovery.states has the wrong length")
    normalized_states = []
    previous_time: int | None = None
    previous_state = "none"
    for index, expected_state in enumerate(RECOVERY_STATES):
        state_field = f"recovery.states[{index}]"
        normalized = validate_event(
            states[index],
            node_id=target.node_id,
            field_name=state_field,
            now_ms=now_ms,
            max_age_ms=max_age_ms,
            drill_id=drill_id,
            event_type="recovery_state",
            package_digest=candidate.package_payload_sha256,
            semantic_validator=lambda semantic, expected_state=expected_state, previous_state=previous_state, state_field=state_field: validate_recovery_state_semantic(
                semantic,
                state=expected_state,
                previous_state=previous_state,
                field_name=f"{target.node_id}/{state_field}.semantic",
                revision=revision,
                all_node_ids=all_node_ids,
            ),
            extra_keys={"state"},
        )
        if normalized["state"] != expected_state:
            fail(
                f"{target.node_id}/recovery.states must be "
                "healthy -> degraded -> recovering -> healthy"
            )
        if previous_time is not None and normalized["observed_at_ms"] <= previous_time:
            fail(f"{target.node_id}/recovery.states is not strictly time ordered")
        previous_time = normalized["observed_at_ms"]
        previous_state = expected_state
        normalized_states.append(normalized)

    failover = validate_event(
        recovery["failover"],
        node_id=target.node_id,
        field_name="recovery.failover",
        now_ms=now_ms,
        max_age_ms=max_age_ms,
        drill_id=drill_id,
        event_type="recovery_failover",
        package_digest=candidate.package_payload_sha256,
        semantic_validator=lambda semantic: semantic_object(
            semantic,
            f"{target.node_id}/recovery.failover.semantic",
            {
                "candidate_revision",
                "fault",
                "leader_after",
                "leader_before",
                "membership_quorum_preserved",
            },
        ),
        extra_keys={"active_lighthouse_id", "automatic", "failed_lighthouse_id"},
    )
    if failover["automatic"] is not True:
        fail(f"{target.node_id}/recovery.failover was not automatic")
    failed_lighthouse = failover["failed_lighthouse_id"]
    active_lighthouse = failover["active_lighthouse_id"]
    if (
        not isinstance(failed_lighthouse, str)
        or not isinstance(active_lighthouse, str)
        or failed_lighthouse not in lighthouse_ids
        or active_lighthouse not in lighthouse_ids
        or failed_lighthouse == active_lighthouse
    ):
        fail(f"{target.node_id}/recovery.failover has invalid lighthouse identities")
    failover["semantic"] = validate_recovery_failover_semantic(
        failover["semantic"],
        failed_lighthouse=failed_lighthouse,
        active_lighthouse=active_lighthouse,
        field_name=f"{target.node_id}/recovery.failover.semantic",
        revision=revision,
    )

    corrected = validate_event(
        recovery["corrected_forward"],
        node_id=target.node_id,
        field_name="recovery.corrected_forward",
        now_ms=now_ms,
        max_age_ms=max_age_ms,
        drill_id=drill_id,
        event_type="corrected_forward",
        package_digest=candidate.package_payload_sha256,
        semantic_validator=lambda semantic: semantic_object(
            semantic,
            f"{target.node_id}/recovery.corrected_forward.semantic",
            {
                "installed_package_payload_sha256_after",
                "repair",
                "revision_after",
                "revision_before",
                "services_active_after",
                "topology_nodes_online_after",
            },
        ),
        extra_keys={"forward_revision", "previous_revision", "re_enrolled", "rollback"},
    )
    previous_revision = require_revision(
        corrected["previous_revision"],
        f"{target.node_id}/recovery.corrected_forward.previous_revision",
    )
    forward_revision = require_revision(
        corrected["forward_revision"],
        f"{target.node_id}/recovery.corrected_forward.forward_revision",
    )
    if previous_revision == forward_revision or forward_revision != revision:
        fail(f"{target.node_id}/recovery.corrected_forward did not advance to revision")
    if corrected["re_enrolled"] is not True or corrected["rollback"] is not False:
        fail(
            f"{target.node_id}/recovery.corrected_forward must prove re-enrollment "
            "with rollback disabled"
        )
    corrected["semantic"] = validate_corrected_forward_semantic(
        corrected["semantic"],
        previous_revision=previous_revision,
        revision=revision,
        package_digest=candidate.package_payload_sha256,
        field_name=f"{target.node_id}/recovery.corrected_forward.semantic",
        all_node_ids=all_node_ids,
    )
    all_events = list(normalized_scenarios.values()) + normalized_states + [
        failover,
        corrected,
    ]
    event_ids = [event["event_id"] for event in all_events]
    if len(set(event_ids)) != len(event_ids):
        fail(f"{target.node_id}/recovery.ledger reuses a typed event id")
    if any(event["finished_at_ms"] > recorded for event in all_events):
        fail(f"{target.node_id}/recovery.ledger was recorded before an event finished")
    scenario_failover = normalized_scenarios["failover"]["semantic"]
    if (
        scenario_failover["failed_lighthouse_id"] != failed_lighthouse
        or scenario_failover["leader_after"] != active_lighthouse
    ):
        fail(f"{target.node_id}/recovery.ledger failover observations disagree")
    scenario_corrected = normalized_scenarios["corrected_forward_recovery"]["semantic"]
    if (
        scenario_corrected["previous_revision"] != previous_revision
        or scenario_corrected["forward_revision"] != forward_revision
    ):
        fail(f"{target.node_id}/recovery.ledger corrected-forward observations disagree")
    return {
        "schema_version": 2,
        "kind": KIND_LEDGER,
        "drill_id": drill_id,
        "node_id": target.node_id,
        "hostname": target.expected_hostname,
        "machine_id_sha256": target.machine_id_sha256,
        "revision": revision,
        "candidate": {
            "package": candidate.package,
            "package_payload_sha256": candidate.package_payload_sha256,
            "binaries": candidate.binaries,
        },
        "recorded_at_ms": recorded,
        "scenarios": normalized_scenarios,
        "recovery": {
            "states": normalized_states,
            "failover": failover,
            "corrected_forward": corrected,
        },
    }


def validate_topology_snapshot(
    value: dict[str, Any],
    *,
    target: Target,
    targets: Sequence[Target],
    remote_now_ms: int,
    snapshot_max_age_ms: int,
) -> str:
    if str(value.get("self", "")).lower() != target.expected_hostname.lower():
        fail(f"{target.node_id}/topology.snapshot self identity does not match")
    generated = positive_integer(
        value.get("generated_ms"), f"{target.node_id}/topology.snapshot.generated_ms"
    )
    age = remote_now_ms - generated
    if age < 0 or age > snapshot_max_age_ms:
        fail(f"{target.node_id}/topology.snapshot is stale or from the future")
    nodes = value.get("nodes")
    if not isinstance(nodes, list):
        fail(f"{target.node_id}/topology.snapshot nodes must be a list")
    by_hostname: dict[str, dict[str, Any]] = {}
    for node in nodes:
        if not isinstance(node, dict) or not isinstance(node.get("hostname"), str):
            continue
        key = node["hostname"].lower()
        if key in by_hostname:
            fail(f"{target.node_id}/topology.snapshot contains duplicate hostnames")
        by_hostname[key] = node
    for expected in targets:
        node = by_hostname.get(expected.expected_hostname.lower())
        if node is None:
            fail(
                f"{target.node_id}/topology.snapshot is missing "
                f"{expected.expected_hostname}"
            )
        if node.get("role") != expected.role or node.get("presence") != "online":
            fail(
                f"{target.node_id}/topology.snapshot does not show "
                f"{expected.node_id} online as {expected.role}"
            )
        last_seen = positive_integer(
            node.get("last_seen_ms"),
            f"{target.node_id}/topology.snapshot.{expected.node_id}.last_seen_ms",
        )
        last_seen_age = remote_now_ms - last_seen
        if last_seen_age < 0 or last_seen_age > snapshot_max_age_ms:
            fail(
                f"{target.node_id}/topology.snapshot has stale presence for "
                f"{expected.node_id}"
            )
    network = value.get("network")
    if not isinstance(network, dict):
        fail(f"{target.node_id}/topology.snapshot network must be an object")
    leader = str(network.get("leader", "")).lower()
    lighthouse_names = {
        candidate.expected_hostname.lower(): candidate.node_id
        for candidate in targets
        if candidate.role == "lighthouse"
    }
    lighthouse_ids = {
        candidate.node_id.lower(): candidate.node_id
        for candidate in targets
        if candidate.role == "lighthouse"
    }
    canonical_leader = lighthouse_names.get(leader) or lighthouse_ids.get(leader)
    if canonical_leader is None:
        fail(f"{target.node_id}/topology.snapshot has no valid lighthouse leader")
    return canonical_leader


@dataclass
class NodeCapture:
    target: Target
    outcomes: list[dict[str, Any]] = field(default_factory=list)
    hostname: str | None = None
    remote_now_ms: int | None = None
    package: str | None = None
    package_payload_sha256: str | None = None
    binary_sha256: dict[str, str] | None = None
    leader_id: str | None = None
    topology: dict[str, Any] | None = None
    ledger: dict[str, Any] | None = None
    errors: list[str] = field(default_factory=list)


def collect_node(
    target: Target,
    *,
    targets: Sequence[Target],
    runner: Runner,
    revision: str,
    observation_root: str,
    command_timeout_seconds: int,
    local_now_ms: int,
    max_age_ms: int,
    max_clock_skew_ms: int,
    snapshot_max_age_ms: int,
    expected_candidate: CandidateExpectation,
) -> NodeCapture:
    capture = NodeCapture(target=target)
    probes = probes_for(target, observation_root, revision)
    outcomes: dict[str, dict[str, Any]] = {}

    # Bind the endpoint before reading any topology or drill evidence from it.
    for probe in probes[:3]:
        try:
            outcome = normalize_outcome(
                probe, runner.run(target, probe, command_timeout_seconds)
            )
        except (CollectionError, OSError, subprocess.SubprocessError) as exc:
            capture.errors.append(f"{target.node_id}/{probe.name}: {exc}")
            return capture
        capture.outcomes.append(outcome)
        outcomes[probe.name] = outcome
    try:
        hostname = require_outcome(outcomes["identity.hostname"], target.node_id)
        if hostname.lower() != target.expected_hostname.lower():
            fail(
                f"{target.node_id}/identity.hostname returned {hostname!r}, "
                f"expected {target.expected_hostname!r}"
            )
        machine_id = require_outcome(outcomes["identity.machine_id"], target.node_id)
        if not MACHINE_ID_RE.fullmatch(machine_id):
            fail(f"{target.node_id}/identity.machine_id is malformed")
        machine_digest = hashlib.sha256(machine_id.encode("ascii")).hexdigest()
        if machine_digest != target.machine_id_sha256:
            fail(f"{target.node_id}/identity.machine_id digest does not match target")
        # Retain the identity binding without publishing the raw machine id.
        outcomes["identity.machine_id"]["stdout"] = f"sha256:{machine_digest}\n"
        outcomes["identity.machine_id"]["redactions"] += 1
        remote_now_text = require_outcome(outcomes["identity.clock"], target.node_id)
        if not re.fullmatch(r"[0-9]{13}", remote_now_text):
            fail(f"{target.node_id}/identity.clock did not return epoch milliseconds")
        remote_now_ms = int(remote_now_text)
        if abs(remote_now_ms - local_now_ms) > max_clock_skew_ms:
            fail(f"{target.node_id}/identity.clock exceeds the allowed clock skew")
        capture.hostname = hostname
        capture.remote_now_ms = remote_now_ms
    except CollectionError as exc:
        capture.errors.append(str(exc))
        return capture

    for probe in probes[3:]:
        try:
            outcome = normalize_outcome(
                probe, runner.run(target, probe, command_timeout_seconds)
            )
        except (CollectionError, OSError, subprocess.SubprocessError) as exc:
            capture.errors.append(f"{target.node_id}/{probe.name}: {exc}")
            continue
        capture.outcomes.append(outcome)
        outcomes[probe.name] = outcome

    try:
        required_probes = [
            "revision.package",
            "revision.package_payload_sha256",
            "revision.binary.mackesd_sha256",
            "service.mackesd",
            "service.nebula",
            "service.syncthing",
            "topology.snapshot",
            "recovery.ledger",
        ]
        if target.role == "workstation":
            required_probes.append("revision.binary.mde_shell_egui_sha256")
        for required_probe in required_probes:
            if required_probe not in outcomes:
                fail(f"{target.node_id}/{required_probe} was not observed")
            require_outcome(outcomes[required_probe], target.node_id)
        package = outcomes["revision.package"]["stdout"].strip()
        if not PACKAGE_RE.fullmatch(package):
            fail(f"{target.node_id}/revision.package returned malformed package identity")
        package_payload_sha256 = parse_package_payload_digest(
            outcomes["revision.package_payload_sha256"], target.node_id
        )
        binary_sha256 = {
            "mackesd": parse_binary_digest(
                outcomes["revision.binary.mackesd_sha256"],
                target.node_id,
                "/usr/bin/mackesd",
            )
        }
        if target.role == "workstation":
            binary_sha256["mde-shell-egui"] = parse_binary_digest(
                outcomes["revision.binary.mde_shell_egui_sha256"],
                target.node_id,
                "/usr/bin/mde-shell-egui",
            )
        live_candidate = CandidateExpectation(
            package=package,
            package_payload_sha256=package_payload_sha256,
            binaries=binary_sha256,
        )
        if live_candidate != expected_candidate:
            fail(f"{target.node_id}/revision candidate digests do not match the local manifest")
        for service in ("mackesd", "nebula", "syncthing"):
            if outcomes[f"service.{service}"]["stdout"].strip() != "active":
                fail(f"{target.node_id}/service.{service} is not active")
        topology = parse_json_output(outcomes["topology.snapshot"], target.node_id)
        leader_id = validate_topology_snapshot(
            topology,
            target=target,
            targets=targets,
            remote_now_ms=capture.remote_now_ms,
            snapshot_max_age_ms=snapshot_max_age_ms,
        )
        ledger_raw = parse_json_output(outcomes["recovery.ledger"], target.node_id)
        ledger = validate_ledger(
            ledger_raw,
            target=target,
            revision=revision,
            now_ms=local_now_ms,
            max_age_ms=max_age_ms,
            lighthouse_ids={
                candidate.node_id for candidate in targets if candidate.role == "lighthouse"
            },
            all_node_ids={candidate.node_id for candidate in targets},
            candidate=live_candidate,
        )
        # Store the validated/redacted ledger, not the raw remote bytes.
        outcomes["recovery.ledger"]["stdout"] = json.dumps(
            ledger, sort_keys=True, separators=(",", ":")
        )
        capture.package = package
        capture.package_payload_sha256 = package_payload_sha256
        capture.binary_sha256 = binary_sha256
        capture.leader_id = leader_id
        capture.topology = topology
        capture.ledger = ledger
    except CollectionError as exc:
        capture.errors.append(str(exc))
    return capture


def json_bytes(value: Any, *, compact: bool = False) -> bytes:
    separators = (",", ":") if compact else None
    return (
        json.dumps(value, indent=None if compact else 2, sort_keys=True, separators=separators)
        + ("" if compact else "\n")
    ).encode("utf-8")


def exclusive_write(path: Path, data: bytes, mode: int = 0o600) -> None:
    descriptor = os.open(
        path,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
        mode,
    )
    try:
        with os.fdopen(descriptor, "wb") as stream:
            descriptor = -1
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
    finally:
        if descriptor >= 0:
            os.close(descriptor)


def write_artifact(root: Path, relative: Path, value: Any, *, compact: bool = False) -> tuple[str, str]:
    if relative.is_absolute() or ".." in relative.parts:
        fail("internal artifact path escaped the staging root")
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    data = json_bytes(value, compact=compact)
    if len(data) > VERIFIER.MAX_ARTIFACT_BYTES:
        fail(f"artifact {relative} exceeds the verifier artifact bound")
    exclusive_write(path, data)
    return relative.as_posix(), hashlib.sha256(data).hexdigest()


def verifier_record(
    event: dict[str, Any], artifact: tuple[str, str], *, extras: Sequence[str] = ()
) -> dict[str, Any]:
    record = {
        "status": "pass",
        "observed_at_ms": event["observed_at_ms"],
        "command": event["command"],
        "artifact": artifact[0],
        "sha256": artifact[1],
    }
    for key in extras:
        record[key] = event[key]
    return record


def candidate_binding(
    capture: NodeCapture, revision: str, manifest_sha256: str
) -> dict[str, Any]:
    assert capture.package is not None
    assert capture.package_payload_sha256 is not None
    assert capture.binary_sha256 is not None
    return {
        "revision": revision,
        "manifest_sha256": manifest_sha256,
        "package": capture.package,
        "package_payload_sha256": capture.package_payload_sha256,
        "binaries": capture.binary_sha256,
    }


def materialize_bundle(
    captures: Sequence[NodeCapture],
    *,
    stage_root: Path,
    artifact_dir_name: str,
    revision: str,
    generated_at_ms: int,
    candidate_manifest_sha256: str,
) -> dict[str, Any]:
    bundle_nodes = []
    for capture in sorted(captures, key=lambda item: item.target.node_id):
        target = capture.target
        assert capture.ledger is not None
        assert capture.hostname is not None
        assert capture.remote_now_ms is not None
        candidate = candidate_binding(capture, revision, candidate_manifest_sha256)
        node_root = Path(artifact_dir_name) / target.node_id
        collection = {
            "schema_version": 1,
            "kind": KIND_COLLECTION,
            "node_id": target.node_id,
            "role": target.role,
            "ssh_host": target.host,
            "hostname": capture.hostname,
            "machine_id_sha256": target.machine_id_sha256,
            "revision": revision,
            "collected_at_ms": generated_at_ms,
            "remote_clock_ms": capture.remote_now_ms,
            "package": capture.package,
            "candidate": candidate,
            "leader_id": capture.leader_id,
            "commands": capture.outcomes,
        }
        write_artifact(stage_root, node_root / "collection.json", collection)

        scenarios: dict[str, dict[str, Any]] = {}
        for scenario in SCENARIOS:
            event = capture.ledger["scenarios"][scenario]
            artifact = write_artifact(
                stage_root,
                node_root / "scenarios" / f"{scenario}.json",
                {
                    "schema_version": 1,
                    "kind": KIND_SCENARIO,
                    "node_id": target.node_id,
                    "hostname": capture.hostname,
                    "machine_id_sha256": target.machine_id_sha256,
                    "revision": revision,
                    "scenario": scenario,
                    "candidate": candidate,
                    "outcome": event,
                    "collected_at_ms": generated_at_ms,
                },
            )
            scenarios[scenario] = verifier_record(event, artifact)

        recovery_states = []
        for index, event in enumerate(capture.ledger["recovery"]["states"]):
            artifact = write_artifact(
                stage_root,
                node_root / "recovery" / f"state-{index}.json",
                {
                    "schema_version": 1,
                    "kind": KIND_RECOVERY,
                    "node_id": target.node_id,
                    "revision": revision,
                    "recovery_kind": "state",
                    "candidate": candidate,
                    "outcome": event,
                    "collected_at_ms": generated_at_ms,
                },
            )
            recovery_states.append(
                verifier_record(event, artifact, extras=("node_id", "state"))
            )

        failover_event = capture.ledger["recovery"]["failover"]
        failover_artifact = write_artifact(
            stage_root,
            node_root / "recovery" / "failover.json",
            {
                "schema_version": 1,
                "kind": KIND_RECOVERY,
                "node_id": target.node_id,
                "revision": revision,
                "recovery_kind": "failover",
                "candidate": candidate,
                "outcome": failover_event,
                "collected_at_ms": generated_at_ms,
            },
        )
        failover = verifier_record(
            failover_event,
            failover_artifact,
            extras=(
                "node_id",
                "active_lighthouse_id",
                "automatic",
                "failed_lighthouse_id",
            ),
        )

        corrected_event = capture.ledger["recovery"]["corrected_forward"]
        corrected_artifact = write_artifact(
            stage_root,
            node_root / "recovery" / "corrected-forward.json",
            {
                "schema_version": 1,
                "kind": KIND_RECOVERY,
                "node_id": target.node_id,
                "revision": revision,
                "recovery_kind": "corrected_forward",
                "candidate": candidate,
                "outcome": corrected_event,
                "collected_at_ms": generated_at_ms,
            },
        )
        corrected = verifier_record(
            corrected_event,
            corrected_artifact,
            extras=(
                "node_id",
                "forward_revision",
                "previous_revision",
                "re_enrolled",
                "rollback",
            ),
        )

        attestation_marker = {
            "kind": VERIFIER.LIVE_ATTESTATION_KIND,
            "node_id": target.node_id,
            "observed_at_ms": generated_at_ms,
            "revision": revision,
            "source": "live",
            "transport": "ssh",
        }
        attestation_artifact = write_artifact(
            stage_root,
            node_root / "live-attestation.json",
            attestation_marker,
            compact=True,
        )
        bundle_nodes.append(
            {
                "id": target.node_id,
                "role": target.role,
                "source": "live",
                "candidate": candidate,
                "scenarios": scenarios,
                "live_attestation": {
                    "status": "pass",
                    "observed_at_ms": generated_at_ms,
                    "command": (
                        "collect-six-node-topology.py fixed read-only live probes "
                        f"--target {target.node_id}"
                    ),
                    "artifact": attestation_artifact[0],
                    "sha256": attestation_artifact[1],
                    "node_id": target.node_id,
                    "transport": "ssh",
                },
                "recovery": {
                    "node_id": target.node_id,
                    "states": recovery_states,
                    "failover": failover,
                    "corrected_forward": corrected,
                },
            }
        )
    return {
        "schema": VERIFIER.SCHEMA,
        "revision": revision,
        "candidate_manifest_sha256": candidate_manifest_sha256,
        "generated_at_ms": generated_at_ms,
        "nodes": bundle_nodes,
    }


def safe_output_path(output: Path) -> tuple[Path, str]:
    if not output.name or output.name in {".", ".."}:
        fail("output must name a JSON file")
    if output.suffix.lower() != ".json":
        fail("output must end in .json")
    parent = output.parent.resolve()
    if not parent.is_dir():
        fail(f"output parent does not exist: {parent}")
    if output.is_symlink() or output.exists():
        fail(f"refusing to overwrite output: {output}")
    artifact_name = f"{output.stem}.artifacts"
    artifact_path = parent / artifact_name
    if artifact_path.is_symlink() or artifact_path.exists():
        fail(f"refusing to overwrite artifact directory: {artifact_path}")
    return parent / output.name, artifact_name


def publish_bundle(
    captures: Sequence[NodeCapture],
    *,
    output: Path,
    revision: str,
    generated_at_ms: int,
    max_age_ms: int,
    candidate_manifest_sha256: str,
) -> dict[str, Any]:
    final_output, artifact_name = safe_output_path(output)
    with tempfile.TemporaryDirectory(
        prefix=".six-node-collector-", dir=final_output.parent
    ) as temporary:
        stage = Path(temporary)
        bundle = materialize_bundle(
            captures,
            stage_root=stage,
            artifact_dir_name=artifact_name,
            revision=revision,
            generated_at_ms=generated_at_ms,
            candidate_manifest_sha256=candidate_manifest_sha256,
        )
        VERIFIER.validate(
            bundle,
            now_ms=generated_at_ms,
            max_age_ms=max_age_ms,
            require_live=True,
            artifact_root=stage,
            expected_revision=revision,
        )
        staged_bundle = stage / final_output.name
        exclusive_write(staged_bundle, json_bytes(bundle))
        staged_artifacts = stage / artifact_name
        final_artifacts = final_output.parent / artifact_name
        os.rename(staged_artifacts, final_artifacts)
        try:
            os.link(staged_bundle, final_output)
        except Exception:
            # This directory was created by this invocation and is not shared.
            shutil.rmtree(final_artifacts)
            raise
    return bundle


def failure_report(captures: Sequence[NodeCapture], revision: str, now_ms: int) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "kind": KIND_FAILURE,
        "revision": revision,
        "recorded_at_ms": now_ms,
        "status": "blocked",
        "nodes": [
            {
                "id": capture.target.node_id,
                "role": capture.target.role,
                "host": capture.target.host,
                "expected_hostname": capture.target.expected_hostname,
                "machine_id_sha256": capture.target.machine_id_sha256,
                "errors": capture.errors,
                "commands": capture.outcomes,
            }
            for capture in sorted(captures, key=lambda item: item.target.node_id)
        ],
    }


def write_failure_report(output: Path, report: dict[str, Any]) -> Path | None:
    path = output.with_suffix(output.suffix + ".failed.json")
    data = json_bytes(report)
    if len(data) > MAX_FAILURE_REPORT_BYTES or path.exists() or path.is_symlink():
        return None
    try:
        exclusive_write(path, data)
    except OSError:
        return None
    return path


def collect(
    *,
    targets: Sequence[Target],
    revision: str,
    output: Path,
    runner: Runner,
    observation_root: str,
    jobs: int,
    command_timeout_seconds: int,
    max_age_seconds: int,
    max_clock_skew_seconds: int,
    snapshot_max_age_seconds: int,
    candidate_manifest: CandidateManifest,
    clock: Callable[[], int] = lambda: time.time_ns() // 1_000_000,
) -> dict[str, Any]:
    if candidate_manifest.revision != revision:
        fail("candidate manifest revision does not match the requested revision")
    require_digest(candidate_manifest.sha256, "candidate manifest SHA-256")
    if set(candidate_manifest.roles) != set(ROLES):
        fail("candidate manifest does not cover both node roles")
    local_now_ms = clock()
    max_age_ms = max_age_seconds * 1000
    with ThreadPoolExecutor(max_workers=jobs, thread_name_prefix="six-node") as executor:
        futures = {
            executor.submit(
                collect_node,
                target,
                targets=targets,
                runner=runner,
                revision=revision,
                observation_root=observation_root,
                command_timeout_seconds=command_timeout_seconds,
                local_now_ms=local_now_ms,
                max_age_ms=max_age_ms,
                max_clock_skew_ms=max_clock_skew_seconds * 1000,
                snapshot_max_age_ms=snapshot_max_age_seconds * 1000,
                expected_candidate=candidate_manifest.roles[target.role],
            ): target
            for target in targets
        }
        captures = [future.result() for future in as_completed(futures)]
    leaders = {capture.leader_id for capture in captures if capture.leader_id is not None}
    if len(leaders) > 1:
        for capture in captures:
            capture.errors.append("six live topology views disagree on the active lighthouse")
    errors = [error for capture in captures for error in capture.errors]
    generated_at_ms = clock()
    if errors or any(capture.ledger is None for capture in captures):
        report_path = write_failure_report(
            output, failure_report(captures, revision, generated_at_ms)
        )
        suffix = f"; diagnostics={report_path}" if report_path is not None else ""
        fail(f"live collection blocked by {len(errors) or 1} required observation(s){suffix}")
    return publish_bundle(
        captures,
        output=output,
        revision=revision,
        generated_at_ms=generated_at_ms,
        max_age_ms=max_age_ms,
        candidate_manifest_sha256=candidate_manifest.sha256,
    )


def dry_run_plan(
    targets: Sequence[Target],
    *,
    revision: str,
    output: Path,
    observation_root: str,
    ssh_port: int,
    candidate_manifest: CandidateManifest,
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "kind": KIND_PLAN,
        "dry_run": True,
        "writes": False,
        "revision": revision,
        "candidate_manifest_sha256": candidate_manifest.sha256,
        "output": str(output),
        "ssh_policy": {
            "port": ssh_port,
            "batch_mode": True,
            "password_authentication": False,
            "keyboard_interactive_authentication": False,
            "strict_host_key_checking": True,
        },
        "targets": [
            {
                "id": target.node_id,
                "role": target.role,
                "host": target.host,
                "expected_hostname": target.expected_hostname,
                "machine_id_sha256": target.machine_id_sha256,
                "candidate": {
                    "package": candidate_manifest.roles[target.role].package,
                    "package_payload_sha256": candidate_manifest.roles[
                        target.role
                    ].package_payload_sha256,
                    "binaries": candidate_manifest.roles[target.role].binaries,
                },
                "probes": [probe.logical_command for probe in probes_for(target, observation_root, revision)],
            }
            for target in targets
        ],
    }


class FakeRunner:
    def __init__(self, outcomes: dict[tuple[str, str], RawOutcome]) -> None:
        self.outcomes = outcomes
        self.calls: list[tuple[str, str]] = []

    def run(self, target: Target, probe: Probe, timeout_seconds: int) -> RawOutcome:
        del timeout_seconds
        self.calls.append((target.node_id, probe.name))
        try:
            return self.outcomes[(target.node_id, probe.name)]
        except KeyError as exc:
            raise OSError(f"fixture has no outcome for {target.node_id}/{probe.name}") from exc


def fixture_event(
    node_id: str,
    *,
    drill_id: str,
    event_type: str,
    event_name: str,
    package_digest: str,
    observed_at_ms: int,
    semantic: dict[str, Any],
    **extra: Any,
) -> dict[str, Any]:
    return {
        "schema_version": 2,
        "kind": KIND_DRILL_EVENT,
        "drill_id": drill_id,
        "event_id": f"{drill_id}:{node_id}:{event_name}",
        "event_type": event_type,
        "node_id": node_id,
        "status": "pass",
        "started_at_ms": observed_at_ms - 100,
        "observed_at_ms": observed_at_ms,
        "finished_at_ms": observed_at_ms,
        "command": f"mcnf-six-node-drill --event {event_name} --node {node_id}",
        "returncode": 0,
        "stdout": "typed semantic observation recorded\n",
        "stderr": "",
        "candidate_package_payload_sha256": package_digest,
        "semantic": semantic,
        **extra,
    }


def fixture_candidate_manifest(
    revision: str,
) -> tuple[CandidateManifest, dict[str, Any]]:
    roles: dict[str, CandidateExpectation] = {}
    raw_roles: dict[str, dict[str, Any]] = {}
    for role in ROLES:
        binaries = {
            "mackesd": hashlib.sha256(f"{role}-mackesd".encode()).hexdigest()
        }
        if role == "workstation":
            binaries["mde-shell-egui"] = hashlib.sha256(
                b"workstation-mde-shell-egui"
            ).hexdigest()
        record = {
        "package": f"{package_name_for_role(role)} 13.0.0-1.x86_64",
            "package_payload_sha256": hashlib.sha256(
                f"{role}-rpm-payload".encode()
            ).hexdigest(),
            "binaries": binaries,
        }
        raw_roles[role] = record
        roles[role] = validate_candidate_record(record, role, f"fixture.{role}")
    value = {
        "schema_version": 1,
        "kind": KIND_CANDIDATE_MANIFEST,
        "revision": revision,
        "roles": raw_roles,
    }
    raw = json_bytes(value, compact=True)
    return CandidateManifest(revision, roles, hashlib.sha256(raw).hexdigest()), value


def fixture_inputs(
    now_ms: int,
) -> tuple[
    str,
    list[Target],
    CandidateManifest,
    dict[tuple[str, str], RawOutcome],
]:
    revision = "a" * 40
    previous_revision = "b" * 40
    manifest, _ = fixture_candidate_manifest(revision)
    definitions = [
        ("lh-1", "lighthouse"),
        ("lh-2", "lighthouse"),
        ("lh-3", "lighthouse"),
        ("ws-1", "workstation"),
        ("ws-2", "workstation"),
        ("ws-3", "workstation"),
    ]
    machine_ids = {
        node_id: f"{index + 1:032x}" for index, (node_id, _) in enumerate(definitions)
    }
    targets = [
        Target(
            node_id=node_id,
            role=role,
            host=f"192.0.2.{index + 1}",
            expected_hostname=node_id,
            machine_id_sha256=hashlib.sha256(machine_ids[node_id].encode()).hexdigest(),
        )
        for index, (node_id, role) in enumerate(definitions)
    ]
    topology_nodes = [
        {
            "hostname": target.expected_hostname,
            "role": target.role,
            "presence": "online",
            "last_seen_ms": now_ms - 1_000,
        }
        for target in targets
    ]
    outcomes: dict[tuple[str, str], RawOutcome] = {}

    def outcome(text: str, *, returncode: int = 0) -> RawOutcome:
        return RawOutcome(
            returncode=returncode,
            stdout=text.encode(),
            stderr=b"",
            started_at_ms=now_ms - 500,
            finished_at_ms=now_ms - 400,
        )

    for target in targets:
        candidate = manifest.roles[target.role]
        drill_id = f"drill-20260803-{target.node_id}"

        def scenario_semantic(name: str) -> dict[str, Any]:
            common: dict[str, Any] = {
                "scenario": name,
                "subject_node_id": target.node_id,
            }
            if name == "join":
                return {
                    **common,
                    "action": "enroll_node",
                    "membership_before": "absent",
                    "membership_after": "present",
                    "services_active_after": True,
                    "topology_nodes_online_after": len(targets),
                }
            if name == "steady_state":
                return {
                    **common,
                    "action": "observe_steady_state",
                    "duration_ms": 5 * 60 * 1000,
                    "leader_id_start": "lh-2",
                    "leader_id_end": "lh-2",
                    "services_active_end": True,
                    "topology_nodes_online_start": len(targets),
                    "topology_nodes_online_end": len(targets),
                }
            if name == "loss":
                return {
                    **common,
                    "action": "inject_overlay_loss",
                    "fault_target_node_id": target.node_id,
                    "presence_before": "online",
                    "presence_during": "offline",
                    "loss_detected": True,
                    "observer_count": len(targets) - 1,
                }
            if name == "failover":
                return {
                    **common,
                    "action": "inject_lighthouse_loss",
                    "failed_lighthouse_id": "lh-1",
                    "leader_before": "lh-1",
                    "leader_after": "lh-2",
                    "automatic": True,
                    "quorum_preserved": True,
                }
            if name == "re_enrollment":
                return {
                    **common,
                    "action": "re_enroll_node",
                    "membership_before": "absent",
                    "membership_after": "present",
                    "identity_rotated": True,
                    "token_reuse_rejected": True,
                    "topology_nodes_online_after": len(targets),
                }
            if name == "corrected_forward_recovery":
                return {
                    **common,
                    "action": "correct_forward",
                    "previous_revision": previous_revision,
                    "forward_revision": revision,
                    "re_enrolled": True,
                    "rollback": False,
                    "services_active_after": True,
                    "installed_package_payload_sha256_after": candidate.package_payload_sha256,
                }
            raise AssertionError(name)

        scenarios = {
            name: fixture_event(
                target.node_id,
                drill_id=drill_id,
                event_type="scenario",
                event_name=f"scenario-{name}",
                package_digest=candidate.package_payload_sha256,
                observed_at_ms=now_ms - 30_000,
                semantic=scenario_semantic(name),
            )
            for name in SCENARIOS
        }
        state_semantics = [
            {
                "state": "healthy",
                "previous_state": "none",
                "cause": "baseline",
                "service_state": "active",
                "topology_nodes_online": len(targets),
                "candidate_revision": revision,
            },
            {
                "state": "degraded",
                "previous_state": "healthy",
                "cause": "fault_observed",
                "service_state": "degraded",
                "topology_nodes_online": len(targets) - 1,
                "candidate_revision": revision,
            },
            {
                "state": "recovering",
                "previous_state": "degraded",
                "cause": "repair_started",
                "service_state": "recovering",
                "topology_nodes_online": len(targets) - 1,
                "candidate_revision": revision,
            },
            {
                "state": "healthy",
                "previous_state": "recovering",
                "cause": "health_restored",
                "service_state": "active",
                "topology_nodes_online": len(targets),
                "candidate_revision": revision,
            },
        ]
        states = [
            fixture_event(
                target.node_id,
                drill_id=drill_id,
                event_type="recovery_state",
                event_name=f"recovery-state-{index}-{state_name}",
                package_digest=candidate.package_payload_sha256,
                observed_at_ms=now_ms - (40_000 - index * 10_000),
                semantic=state_semantics[index],
                state=state_name,
            )
            for index, state_name in enumerate(RECOVERY_STATES)
        ]
        ledger = {
            "schema_version": 2,
            "kind": KIND_LEDGER,
            "drill_id": drill_id,
            "node_id": target.node_id,
            "hostname": target.expected_hostname,
            "machine_id_sha256": target.machine_id_sha256,
            "revision": revision,
            "candidate": {
                "package": candidate.package,
                "package_payload_sha256": candidate.package_payload_sha256,
                "binaries": candidate.binaries,
            },
            "recorded_at_ms": now_ms - 1_000,
            "scenarios": scenarios,
            "recovery": {
                "states": states,
                "failover": fixture_event(
                    target.node_id,
                    drill_id=drill_id,
                    event_type="recovery_failover",
                    event_name="recovery-failover",
                    package_digest=candidate.package_payload_sha256,
                    observed_at_ms=now_ms - 15_000,
                    semantic={
                        "candidate_revision": revision,
                        "fault": "lighthouse_loss",
                        "leader_before": "lh-1",
                        "leader_after": "lh-2",
                        "membership_quorum_preserved": True,
                    },
                    failed_lighthouse_id="lh-1",
                    active_lighthouse_id="lh-2",
                    automatic=True,
                ),
                "corrected_forward": fixture_event(
                    target.node_id,
                    drill_id=drill_id,
                    event_type="corrected_forward",
                    event_name="recovery-corrected-forward",
                    package_digest=candidate.package_payload_sha256,
                    observed_at_ms=now_ms - 5_000,
                    semantic={
                        "repair": "re_enroll_and_correct_forward",
                        "revision_before": previous_revision,
                        "revision_after": revision,
                        "services_active_after": True,
                        "topology_nodes_online_after": len(targets),
                        "installed_package_payload_sha256_after": candidate.package_payload_sha256,
                    },
                    previous_revision=previous_revision,
                    forward_revision=revision,
                    re_enrolled=True,
                    rollback=False,
                ),
            },
        }
        topology = {
            "generated_ms": now_ms - 1_000,
            "self": target.expected_hostname,
            "nodes": topology_nodes,
            "network": {"leader": "lh-2"},
        }
        values = {
            "identity.hostname": target.expected_hostname + "\n",
            "identity.machine_id": machine_ids[target.node_id] + "\n",
            "identity.clock": str(now_ms) + "\n",
            "revision.package": candidate.package + "\n",
            "revision.package_payload_sha256": (
                f"8 {candidate.package_payload_sha256}\n"
            ),
            "revision.binary.mackesd_sha256": (
                f"{candidate.binaries['mackesd']}  /usr/bin/mackesd\n"
            ),
            "service.mackesd": "active\n",
            "service.nebula": "active\n",
            "service.syncthing": "active\n",
            "topology.snapshot": json.dumps(topology),
            "recovery.ledger": json.dumps(ledger),
        }
        if target.role == "workstation":
            values["revision.binary.mde_shell_egui_sha256"] = (
                f"{candidate.binaries['mde-shell-egui']}  /usr/bin/mde-shell-egui\n"
            )
        for probe_name, text in values.items():
            outcomes[(target.node_id, probe_name)] = outcome(text)
    return revision, targets, manifest, outcomes


def assert_collection_rejected(
    *,
    temporary: Path,
    name: str,
    revision: str,
    targets: Sequence[Target],
    outcomes: dict[tuple[str, str], RawOutcome],
    now_ms: int,
    candidate_manifest: CandidateManifest,
) -> None:
    output = temporary / f"{name}.json"
    try:
        collect(
            targets=targets,
            revision=revision,
            output=output,
            runner=FakeRunner(outcomes),
            observation_root=DEFAULT_OBSERVATION_ROOT,
            jobs=6,
            command_timeout_seconds=5,
            max_age_seconds=3600,
            max_clock_skew_seconds=10,
            snapshot_max_age_seconds=60,
            candidate_manifest=candidate_manifest,
            clock=lambda: now_ms,
        )
    except CollectionError:
        if output.exists():
            raise AssertionError(f"{name}: blocked collection published verifier input")
    else:
        raise AssertionError(f"{name}: invalid live observations were accepted")


def self_test() -> None:
    now_ms = 1_760_000_000_000
    revision, targets, candidate_manifest, baseline = fixture_inputs(now_ms)
    with tempfile.TemporaryDirectory(prefix="six-node-collector-self-test-") as work:
        root = Path(work)
        output = root / "topology.json"
        runner = FakeRunner(dict(baseline))
        bundle = collect(
            targets=targets,
            revision=revision,
            output=output,
            runner=runner,
            observation_root=DEFAULT_OBSERVATION_ROOT,
            jobs=6,
            command_timeout_seconds=5,
            max_age_seconds=3600,
            max_clock_skew_seconds=10,
            snapshot_max_age_seconds=60,
            candidate_manifest=candidate_manifest,
            clock=lambda: now_ms,
        )
        result = VERIFIER.validate(
            bundle,
            now_ms=now_ms,
            max_age_ms=3_600_000,
            require_live=True,
            artifact_root=root,
            expected_revision=revision,
        )
        assert result["node_count"] == 6
        assert bundle["candidate_manifest_sha256"] == candidate_manifest.sha256
        assert all(
            node["candidate"]["revision"] == revision
            and node["candidate"]["package_payload_sha256"]
            == candidate_manifest.roles[node["role"]].package_payload_sha256
            for node in bundle["nodes"]
        )
        assert len(runner.calls) == sum(
            len(probes_for(target, DEFAULT_OBSERVATION_ROOT, revision))
            for target in targets
        )

        failed_service = dict(baseline)
        failed_service[("ws-1", "service.mackesd")] = RawOutcome(
            returncode=3,
            stdout=b"inactive\n",
            stderr=b"",
            started_at_ms=now_ms - 500,
            finished_at_ms=now_ms - 400,
        )
        assert_collection_rejected(
            temporary=root,
            name="failed-service",
            revision=revision,
            targets=targets,
            outcomes=failed_service,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        missing_scenario = dict(baseline)
        ledger_key = ("lh-1", "recovery.ledger")
        ledger = json.loads(missing_scenario[ledger_key].stdout)
        del ledger["scenarios"]["loss"]
        missing_scenario[ledger_key] = RawOutcome(
            returncode=0,
            stdout=json.dumps(ledger).encode(),
            stderr=b"",
            started_at_ms=now_ms - 500,
            finished_at_ms=now_ms - 400,
        )
        assert_collection_rejected(
            temporary=root,
            name="missing-scenario",
            revision=revision,
            targets=targets,
            outcomes=missing_scenario,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        credential_command = dict(baseline)
        ledger = json.loads(credential_command[ledger_key].stdout)
        ledger["scenarios"]["join"]["command"] = "probe --token hunter2"
        credential_command[ledger_key] = RawOutcome(
            returncode=0,
            stdout=json.dumps(ledger).encode(),
            stderr=b"",
            started_at_ms=now_ms - 500,
            finished_at_ms=now_ms - 400,
        )
        assert_collection_rejected(
            temporary=root,
            name="credential-command",
            revision=revision,
            targets=targets,
            outcomes=credential_command,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        mismatched_identity = dict(baseline)
        mismatched_identity[("ws-2", "identity.hostname")] = RawOutcome(
            returncode=0,
            stdout=b"wrong-host\n",
            stderr=b"",
            started_at_ms=now_ms - 500,
            finished_at_ms=now_ms - 400,
        )
        assert_collection_rejected(
            temporary=root,
            name="identity-mismatch",
            revision=revision,
            targets=targets,
            outcomes=mismatched_identity,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        def ledger_outcome(value: dict[str, Any]) -> RawOutcome:
            return RawOutcome(
                returncode=0,
                stdout=json.dumps(value).encode(),
                stderr=b"",
                started_at_ms=now_ms - 500,
                finished_at_ms=now_ms - 400,
            )

        legacy_ledger = dict(baseline)
        ledger = json.loads(legacy_ledger[ledger_key].stdout)
        ledger["schema_version"] = 1
        ledger["kind"] = "mcnf-six-node-observation-v1"
        legacy_ledger[ledger_key] = ledger_outcome(ledger)
        assert_collection_rejected(
            temporary=root,
            name="legacy-free-form-ledger",
            revision=revision,
            targets=targets,
            outcomes=legacy_ledger,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        free_form_semantic = dict(baseline)
        ledger = json.loads(free_form_semantic[ledger_key].stdout)
        ledger["scenarios"]["join"]["semantic"] = {"status": "pass"}
        free_form_semantic[ledger_key] = ledger_outcome(ledger)
        assert_collection_rejected(
            temporary=root,
            name="free-form-semantic-pass",
            revision=revision,
            targets=targets,
            outcomes=free_form_semantic,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        semantic_lie = dict(baseline)
        ledger = json.loads(semantic_lie[ledger_key].stdout)
        ledger["scenarios"]["loss"]["semantic"]["presence_during"] = "online"
        semantic_lie[ledger_key] = ledger_outcome(ledger)
        assert_collection_rejected(
            temporary=root,
            name="semantic-loss-lie",
            revision=revision,
            targets=targets,
            outcomes=semantic_lie,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        package_digest_mismatch = dict(baseline)
        package_digest_mismatch[("ws-1", "revision.package_payload_sha256")] = RawOutcome(
            returncode=0,
            stdout=("8 " + "c" * 64 + "\n").encode(),
            stderr=b"",
            started_at_ms=now_ms - 500,
            finished_at_ms=now_ms - 400,
        )
        assert_collection_rejected(
            temporary=root,
            name="package-payload-mismatch",
            revision=revision,
            targets=targets,
            outcomes=package_digest_mismatch,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        binary_digest_mismatch = dict(baseline)
        binary_digest_mismatch[("lh-2", "revision.binary.mackesd_sha256")] = RawOutcome(
            returncode=0,
            stdout=(("d" * 64) + "  /usr/bin/mackesd\n").encode(),
            stderr=b"",
            started_at_ms=now_ms - 500,
            finished_at_ms=now_ms - 400,
        )
        assert_collection_rejected(
            temporary=root,
            name="binary-digest-mismatch",
            revision=revision,
            targets=targets,
            outcomes=binary_digest_mismatch,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        ledger_candidate_mismatch = dict(baseline)
        ledger = json.loads(ledger_candidate_mismatch[ledger_key].stdout)
        ledger["candidate"]["package_payload_sha256"] = "e" * 64
        ledger_candidate_mismatch[ledger_key] = ledger_outcome(ledger)
        assert_collection_rejected(
            temporary=root,
            name="ledger-candidate-mismatch",
            revision=revision,
            targets=targets,
            outcomes=ledger_candidate_mismatch,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        event_drill_mismatch = dict(baseline)
        ledger = json.loads(event_drill_mismatch[ledger_key].stdout)
        ledger["scenarios"]["join"]["drill_id"] = "different-drill-id"
        event_drill_mismatch[ledger_key] = ledger_outcome(ledger)
        assert_collection_rejected(
            temporary=root,
            name="event-drill-mismatch",
            revision=revision,
            targets=targets,
            outcomes=event_drill_mismatch,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        duplicate_event = dict(baseline)
        ledger = json.loads(duplicate_event[ledger_key].stdout)
        ledger["scenarios"]["loss"]["event_id"] = ledger["scenarios"]["join"]["event_id"]
        duplicate_event[ledger_key] = ledger_outcome(ledger)
        assert_collection_rejected(
            temporary=root,
            name="duplicate-event-id",
            revision=revision,
            targets=targets,
            outcomes=duplicate_event,
            now_ms=now_ms,
            candidate_manifest=candidate_manifest,
        )

        bare_token = (
            "mesh:magic-mesh@192.0.2.10:4243#super-secret"
            + "?fp="
            + "a" * 64
        )
        redacted_token, token_count = redact_stream(
            f"join-token {bare_token}\n", "fixture.bare-token"
        )
        assert token_count == 1
        assert "super-secret" not in redacted_token and "mesh:magic-mesh" not in redacted_token

        redacted_collection = dict(baseline)
        ledger = json.loads(redacted_collection[ledger_key].stdout)
        ledger["scenarios"]["join"]["stdout"] = f"issued {bare_token}\n"
        redacted_collection[ledger_key] = ledger_outcome(ledger)
        redacted_output = root / "bare-token-redacted.json"
        collect(
            targets=targets,
            revision=revision,
            output=redacted_output,
            runner=FakeRunner(redacted_collection),
            observation_root=DEFAULT_OBSERVATION_ROOT,
            jobs=6,
            command_timeout_seconds=5,
            max_age_seconds=3600,
            max_clock_skew_seconds=10,
            snapshot_max_age_seconds=60,
            candidate_manifest=candidate_manifest,
            clock=lambda: now_ms,
        )
        published = b"".join(
            path.read_bytes() for path in root.rglob("*") if path.is_file()
        )
        assert b"super-secret" not in published
        assert b"mesh:magic-mesh@192.0.2.10" not in published

        redacted, count = redact_stream("token=hunter2\n", "fixture")
        assert count == 1 and "hunter2" not in redacted
        try:
            redact_stream("-----BEGIN OPENSSH PRIVATE KEY-----\n", "fixture")
        except CollectionError:
            pass
        else:
            raise AssertionError("private-key material was accepted")
        try:
            parse_target("lh-1,lighthouse,user:password@host,lh-1," + "a" * 64)
        except CollectionError:
            pass
        else:
            raise AssertionError("credential-bearing target was accepted")
        try:
            decode_json_object('{"field":1,"field":2}', "fixture.duplicate")
        except CollectionError:
            pass
        else:
            raise AssertionError("duplicate JSON fields were accepted")
        try:
            decode_json_object('{"field":NaN}', "fixture.nonfinite")
        except CollectionError:
            pass
        else:
            raise AssertionError("non-finite JSON number was accepted")

        _, manifest_value = fixture_candidate_manifest(revision)
        manifest_path = root / "candidate-manifest.json"
        manifest_path.write_bytes(json_bytes(manifest_value, compact=True))
        loaded_manifest = read_candidate_manifest(manifest_path, revision)
        assert loaded_manifest == candidate_manifest
        stale_manifest_value = dict(manifest_value)
        stale_manifest_value["revision"] = "f" * 40
        stale_manifest_path = root / "stale-candidate-manifest.json"
        stale_manifest_path.write_bytes(json_bytes(stale_manifest_value, compact=True))
        try:
            read_candidate_manifest(stale_manifest_path, revision)
        except CollectionError:
            pass
        else:
            raise AssertionError("candidate manifest for another revision was accepted")

        plan = dry_run_plan(
            targets,
            revision=revision,
            output=root / "dry-run.json",
            observation_root=DEFAULT_OBSERVATION_ROOT,
            ssh_port=22,
            candidate_manifest=candidate_manifest,
        )
        assert plan["writes"] is False and plan["dry_run"] is True
        assert not (root / "dry-run.json").exists()
    print(
        "collect-six-node-topology.py: self-test passed "
        "(2 positive/redaction, 17 negative/security cases)"
    )


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--validate-candidate-manifest",
        action="store_true",
        help="validate only the exact role candidate schema; do not connect or write",
    )
    parser.add_argument("--revision", help="candidate 40-character source revision")
    parser.add_argument(
        "--candidate-manifest",
        type=Path,
        help="local role-specific RPM payload and installed-binary digest manifest",
    )
    parser.add_argument(
        "--target",
        action="append",
        default=[],
        help="ID,ROLE,SSH_HOST,EXPECTED_HOSTNAME,MACHINE_ID_SHA256 (repeat exactly six times)",
    )
    parser.add_argument("--output", "--out", type=Path, help="output topology JSON")
    parser.add_argument("--dry-run", action="store_true", help="print the fixed probe plan; do not connect or write")
    parser.add_argument("--ssh-user", help="public-key SSH user (password auth is always disabled)")
    parser.add_argument("--ssh-port", type=int, default=22)
    parser.add_argument("--known-hosts", type=Path, help="explicit regular known_hosts file")
    parser.add_argument("--connect-timeout-seconds", type=int, default=8)
    parser.add_argument("--command-timeout-seconds", type=int, default=15)
    parser.add_argument("--max-age-seconds", type=int, default=86_400)
    parser.add_argument("--max-clock-skew-seconds", type=int, default=300)
    parser.add_argument("--snapshot-max-age-seconds", type=int, default=180)
    parser.add_argument("--jobs", type=int, default=6)
    parser.add_argument("--observation-root", default=DEFAULT_OBSERVATION_ROOT)
    return parser.parse_args(argv)


def validate_cli(
    args: argparse.Namespace,
) -> tuple[str, list[Target], Path, str, CandidateManifest]:
    if args.revision is None:
        fail("--revision is required")
    revision = require_revision(args.revision)
    targets = validate_targets([parse_target(raw) for raw in args.target])
    if args.output is None:
        fail("--output is required")
    if args.candidate_manifest is None:
        fail("--candidate-manifest is required")
    candidate_manifest = read_candidate_manifest(args.candidate_manifest, revision)
    if args.ssh_user is not None and not SSH_USER_RE.fullmatch(args.ssh_user):
        fail("--ssh-user is invalid")
    if not 1 <= args.ssh_port <= 65535:
        fail("--ssh-port must be from 1 to 65535")
    for field_name in (
        "connect_timeout_seconds",
        "command_timeout_seconds",
        "max_clock_skew_seconds",
        "snapshot_max_age_seconds",
    ):
        value = getattr(args, field_name)
        if not 1 <= value <= MAX_TIMEOUT_SECONDS * 10:
            fail(f"--{field_name.replace('_', '-')} is outside the safe bound")
    if not 1 <= args.max_age_seconds <= MAX_AGE_SECONDS:
        fail(f"--max-age-seconds must be from 1 to {MAX_AGE_SECONDS}")
    if not 1 <= args.jobs <= 6:
        fail("--jobs must be from 1 to 6")
    observation_root = validate_observation_root(args.observation_root)
    if args.known_hosts is not None:
        known_hosts = args.known_hosts
        if known_hosts.is_symlink() or not known_hosts.is_file():
            fail("--known-hosts must name a regular non-symlink file")
        if known_hosts.stat().st_size > 8 * 1024 * 1024:
            fail("--known-hosts exceeds the 8 MiB bound")
        args.known_hosts = known_hosts.resolve(strict=True)
    return revision, targets, args.output, observation_root, candidate_manifest


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        if args.self_test:
            self_test()
            return 0
        if args.validate_candidate_manifest:
            if args.revision is None or args.candidate_manifest is None:
                fail("candidate-manifest validation requires --revision and --candidate-manifest")
            if args.target or args.output is not None or args.dry_run:
                fail("candidate-manifest validation does not accept collection arguments")
            revision = require_revision(args.revision)
            manifest = read_candidate_manifest(args.candidate_manifest, revision)
            print(
                "collect-six-node-topology.py: PASS — exact candidate manifest "
                f"covers {', '.join(sorted(manifest.roles))} at {revision}"
            )
            return 0
        revision, targets, output, observation_root, candidate_manifest = validate_cli(args)
        if args.dry_run:
            print(
                json.dumps(
                    dry_run_plan(
                        targets,
                        revision=revision,
                        output=output,
                        observation_root=observation_root,
                        ssh_port=args.ssh_port,
                        candidate_manifest=candidate_manifest,
                    ),
                    indent=2,
                    sort_keys=True,
                )
            )
            return 0
        runner = SshRunner(
            ssh_user=args.ssh_user,
            ssh_port=args.ssh_port,
            connect_timeout_seconds=args.connect_timeout_seconds,
            known_hosts=args.known_hosts,
        )
        bundle = collect(
            targets=targets,
            revision=revision,
            output=output,
            runner=runner,
            observation_root=observation_root,
            jobs=args.jobs,
            command_timeout_seconds=args.command_timeout_seconds,
            max_age_seconds=args.max_age_seconds,
            max_clock_skew_seconds=args.max_clock_skew_seconds,
            snapshot_max_age_seconds=args.snapshot_max_age_seconds,
            candidate_manifest=candidate_manifest,
        )
        print(
            "collect-six-node-topology.py: PASS — published verifier input "
            f"{output} for {len(bundle['nodes'])} live nodes"
        )
        return 0
    except (CollectionError, OSError, json.JSONDecodeError, VERIFIER.EvidenceError) as exc:
        print(f"collect-six-node-topology.py: BLOCKED: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
