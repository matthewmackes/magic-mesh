#!/usr/bin/env python3
"""Fail closed on conflicting live claims in an authenticated local snapshot.

The guard accepts only the exact v2 snapshot and current-boot commitment emitted
by ``mcnf-overlay-identity-claims-materializer.py``.  Both documents carry
domain-separated HMAC-SHA256 authentication under a dedicated local credential.
The guard validates their source revision, current-boot provenance, freshness,
and strict claimant key/value contract before comparing the local public Nebula
identity and privacy-bounded claimant digests.

No private key is opened.  Raw machine-id and boot-id values are validated and
used only in memory to derive certificate-scoped digests; they never enter the
snapshot, diagnostics, or command line.
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import ipaddress
import json
import os
from pathlib import Path
import re
import resource
import signal
import stat
import subprocess
import sys
import tempfile
import time
from typing import Any, NoReturn


CLAIM_PREFIX = "/mesh/overlay-identity-claims/v1/"
CLAIM_KEY_BYTES = len(CLAIM_PREFIX) + (3 * 64) + 2
CLAIM_SCHEMA_VERSION = 1
SNAPSHOT_SCHEMA = "mcnf.overlay-identity-claim-snapshot.v2"
COMMITMENT_SCHEMA = "mcnf.overlay-identity-claim-snapshot-commitment.v1"
SOURCE_KIND = "etcd-linearizable-lease-range"
AUTH_ALGORITHM = "hmac-sha256"
AUTH_KEY_ID = "local-overlay-claim-snapshot-hmac-v1"
SNAPSHOT_AUTH_DOMAIN = b"mcnf-overlay-claim-snapshot-v2"
COMMITMENT_AUTH_DOMAIN = b"mcnf-overlay-claim-snapshot-commitment-v1"
BOOT_ATTESTATION_DOMAIN = b"mcnf-overlay-claim-snapshot-producer-boot-v1"
MACHINE_CLAIMANT_DOMAIN = b"mcnf-overlay-machine-claimant-v1"
BOOT_CLAIMANT_DOMAIN = b"mcnf-overlay-boot-claimant-v1"

DEFAULT_CERTIFICATE = Path("/etc/nebula/identity/current/host.crt")
DEFAULT_FALLBACK_CERTIFICATE = Path("/etc/nebula/host.crt")
DEFAULT_SNAPSHOT = Path(
    "/var/lib/mackesd/overlay-identity-claims/active-claims.json"
)
DEFAULT_COMMITMENT = Path("/run/mackesd/overlay-identity-active-claims.commit.json")
DEFAULT_AUTH_KEY = Path("/etc/mackesd/overlay-identity-snapshot-hmac")
DEFAULT_MACHINE_ID = Path("/etc/machine-id")
DEFAULT_BOOT_ID = Path("/proc/sys/kernel/random/boot_id")
DEFAULT_NEBULA_CERT = Path("/usr/bin/nebula-cert")
DEFAULT_RUNTIME_DIR = Path("/run/mackesd")

OVERLAY_NETWORK = ipaddress.ip_network("10.42.0.0/17")
MAX_SNAPSHOT_BYTES = 64 * 1024
MAX_COMMITMENT_BYTES = 2 * 1024
MAX_CERTIFICATE_BYTES = 128 * 1024
MAX_CERT_PRINT_BYTES = 64 * 1024
MAX_CLAIM_VALUE_BYTES = 1_024
MAX_CLAIMS = 12
MAX_SNAPSHOT_AGE_SECONDS = 30
MAX_VALIDITY_SECONDS = 30
MAX_CLOCK_SKEW_MS = 5_000
MAX_CERT_PRINT_TIMEOUT_SECONDS = 5

EXIT_COLLISION = 20
EXIT_MALFORMED = 21
EXIT_STALE = 22
EXIT_UNTRUSTED = 23
EXIT_DEPENDENCY = 24
EXIT_REPLAY = 25

NODE_ID_RE = re.compile(r"peer:[A-Za-z0-9][A-Za-z0-9._-]{0,127}\Z")
DIGEST_RE = re.compile(r"[0-9a-f]{64}\Z")
DECIMAL_RE = re.compile(r"[1-9][0-9]{0,19}\Z")
MACHINE_ID_RE = re.compile(rb"[0-9a-f]{32}\Z")
BOOT_ID_RE = re.compile(
    rb"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\Z"
)


class GuardError(Exception):
    """An expected, credential-free contract failure."""

    def __init__(self, exit_code: int, reason: str) -> None:
        super().__init__(reason)
        self.exit_code = exit_code
        self.reason = reason


def fail(exit_code: int, reason: str) -> NoReturn:
    raise GuardError(exit_code, reason)


def strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(EXIT_MALFORMED, "duplicate-json-key")
        result[key] = value
    return result


def exact_keys(value: dict[str, Any], expected: set[str], reason: str) -> None:
    if set(value) != expected:
        fail(EXIT_MALFORMED, reason)


def canonical_json(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode(
        "ascii"
    )


def strict_int(value: Any, *, minimum: int, maximum: int, reason: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        fail(EXIT_MALFORMED, reason)
    if value < minimum or value > maximum:
        fail(EXIT_MALFORMED, reason)
    return value


def valid_digest(value: Any) -> bool:
    return (
        isinstance(value, str)
        and DIGEST_RE.fullmatch(value) is not None
        and value != "0" * 64
    )


def read_descriptor(
    descriptor: int,
    maximum: int,
    *,
    kind: str,
    exact_size: int | None = None,
    allow_zero_stat_size: bool = False,
) -> bytes:
    info = os.fstat(descriptor)
    if not stat.S_ISREG(info.st_mode):
        fail(EXIT_UNTRUSTED, f"{kind}-not-regular")
    if info.st_uid != os.geteuid() or info.st_mode & 0o022 or info.st_nlink != 1:
        fail(EXIT_UNTRUSTED, f"{kind}-untrusted")
    if not allow_zero_stat_size and (info.st_size <= 0 or info.st_size > maximum):
        fail(EXIT_MALFORMED, f"{kind}-size")
    chunks: list[bytes] = []
    remaining = maximum + 1
    while remaining > 0:
        chunk = os.read(descriptor, min(16 * 1024, remaining))
        if not chunk:
            break
        chunks.append(chunk)
        remaining -= len(chunk)
    data = b"".join(chunks)
    if not data or len(data) > maximum or (exact_size is not None and len(data) != exact_size):
        fail(EXIT_MALFORMED, f"{kind}-size")
    return data


def safe_read(
    path: Path,
    maximum: int,
    *,
    kind: str,
    exact_size: int | None = None,
    allow_zero_stat_size: bool = False,
) -> bytes:
    flags = os.O_RDONLY | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
    except OSError:
        fail(EXIT_UNTRUSTED, f"{kind}-unsafe")
    try:
        return read_descriptor(
            descriptor,
            maximum,
            kind=kind,
            exact_size=exact_size,
            allow_zero_stat_size=allow_zero_stat_size,
        )
    finally:
        os.close(descriptor)


def read_active_certificate(primary: Path, fallback: Path) -> bytes:
    current = primary.parent
    identity_root = current.parent
    is_generation_layout = current.name == "current" and identity_root.name == "identity"
    if not is_generation_layout:
        try:
            os.lstat(primary)
        except FileNotFoundError:
            return safe_read(fallback, MAX_CERTIFICATE_BYTES, kind="certificate")
        except OSError:
            fail(EXIT_UNTRUSTED, "certificate-unsafe")
        return safe_read(primary, MAX_CERTIFICATE_BYTES, kind="certificate")

    try:
        switch_info = os.lstat(current)
    except FileNotFoundError:
        return safe_read(fallback, MAX_CERTIFICATE_BYTES, kind="certificate")
    except OSError:
        fail(EXIT_UNTRUSTED, "certificate-current-unsafe")
    try:
        root_info = os.lstat(identity_root)
        target = os.readlink(current)
    except OSError:
        fail(EXIT_UNTRUSTED, "certificate-current-unsafe")
    if (
        not stat.S_ISLNK(switch_info.st_mode)
        or not stat.S_ISDIR(root_info.st_mode)
        or root_info.st_uid != os.geteuid()
        or stat.S_IMODE(root_info.st_mode) != 0o700
    ):
        fail(EXIT_UNTRUSTED, "certificate-current-untrusted")
    target_path = Path(target)
    if (
        target_path.is_absolute()
        or len(target_path.parts) != 1
        or target_path.name in {"", ".", ".."}
    ):
        fail(EXIT_UNTRUSTED, "certificate-current-target")
    generation = identity_root / target_path.name
    try:
        generation_info = os.lstat(generation)
    except OSError:
        fail(EXIT_UNTRUSTED, "certificate-generation-unsafe")
    if (
        stat.S_ISLNK(generation_info.st_mode)
        or not stat.S_ISDIR(generation_info.st_mode)
        or generation_info.st_uid != os.geteuid()
        or stat.S_IMODE(generation_info.st_mode) != 0o700
    ):
        fail(EXIT_UNTRUSTED, "certificate-generation-untrusted")
    directory_flags = os.O_RDONLY | os.O_CLOEXEC | getattr(os, "O_DIRECTORY", 0)
    if hasattr(os, "O_NOFOLLOW"):
        directory_flags |= os.O_NOFOLLOW
    try:
        directory = os.open(generation, directory_flags)
        file_flags = os.O_RDONLY | os.O_CLOEXEC
        if hasattr(os, "O_NOFOLLOW"):
            file_flags |= os.O_NOFOLLOW
        descriptor = os.open(primary.name, file_flags, dir_fd=directory)
    except OSError:
        fail(EXIT_UNTRUSTED, "certificate-generation-open")
    try:
        return read_descriptor(descriptor, MAX_CERTIFICATE_BYTES, kind="certificate")
    finally:
        os.close(descriptor)
        os.close(directory)


def read_machine_id(path: Path) -> bytes:
    raw = safe_read(path, 128, kind="machine-id").rstrip(b"\n")
    if MACHINE_ID_RE.fullmatch(raw) is None or raw == b"0" * 32:
        fail(EXIT_MALFORMED, "machine-id-malformed")
    return raw


def read_boot_id(path: Path) -> bytes:
    raw = safe_read(
        path,
        128,
        kind="boot-id",
        allow_zero_stat_size=True,
    ).rstrip(b"\n")
    if BOOT_ID_RE.fullmatch(raw) is None or raw.replace(b"-", b"") == b"0" * 32:
        fail(EXIT_MALFORMED, "boot-id-malformed")
    return raw


def boot_attestation(key: bytes, boot_id: bytes) -> str:
    return hmac.new(key, BOOT_ATTESTATION_DOMAIN + b"\0" + boot_id, hashlib.sha256).hexdigest()


def claimant_digest(domain: bytes, certificate_fingerprint: str, raw_id: bytes) -> str:
    digest = hashlib.sha256()
    digest.update(domain)
    digest.update(b"\0")
    digest.update(certificate_fingerprint.encode("ascii"))
    digest.update(b"\0")
    digest.update(raw_id)
    return digest.hexdigest()


def open_trusted_runtime(path: Path) -> None:
    try:
        info = os.lstat(path)
    except OSError:
        fail(EXIT_UNTRUSTED, "parser-runtime-unavailable")
    if (
        not stat.S_ISDIR(info.st_mode)
        or info.st_uid != os.geteuid()
        or stat.S_IMODE(info.st_mode) != 0o700
    ):
        fail(EXIT_UNTRUSTED, "parser-runtime-untrusted")


def validate_executable(path: Path) -> None:
    if not path.is_absolute():
        fail(EXIT_DEPENDENCY, "nebula-cert-unavailable")
    try:
        info = os.lstat(path)
    except OSError:
        fail(EXIT_DEPENDENCY, "nebula-cert-unavailable")
    if (
        not stat.S_ISREG(info.st_mode)
        or info.st_uid != os.geteuid()
        or info.st_mode & 0o022
        or info.st_nlink != 1
        or not os.access(path, os.X_OK)
    ):
        fail(EXIT_DEPENDENCY, "nebula-cert-untrusted")


def bounded_nebula_print(
    binary: Path,
    certificate: bytes,
    runtime_dir: Path,
    *,
    timeout_seconds: int,
) -> bytes:
    validate_executable(binary)
    open_trusted_runtime(runtime_dir)

    def limit_output() -> None:
        resource.setrlimit(
            resource.RLIMIT_FSIZE,
            (MAX_CERT_PRINT_BYTES + 1, MAX_CERT_PRINT_BYTES + 1),
        )

    staged_path: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            prefix=".mcnf-nebula-cert-",
            suffix=".crt",
            dir=runtime_dir,
            mode="w+b",
            delete=False,
        ) as staged:
            staged_path = staged.name
            os.fchmod(staged.fileno(), 0o600)
            staged.write(certificate)
            staged.flush()
            os.fsync(staged.fileno())
            staged_info = os.fstat(staged.fileno())
            if (
                staged_info.st_uid != os.geteuid()
                or stat.S_IMODE(staged_info.st_mode) != 0o600
                or staged_info.st_nlink != 1
                or staged_info.st_size != len(certificate)
            ):
                fail(EXIT_UNTRUSTED, "certificate-stage-untrusted")
        with tempfile.TemporaryFile() as output:
            try:
                process = subprocess.Popen(
                    [str(binary), "print", "-json", "-path", staged_path],
                    stdin=subprocess.DEVNULL,
                    stdout=output,
                    stderr=subprocess.DEVNULL,
                    env={"PATH": "/usr/bin:/bin"},
                    start_new_session=True,
                    preexec_fn=limit_output,
                )
            except OSError:
                fail(EXIT_DEPENDENCY, "nebula-cert-execution-failed")
            try:
                return_code = process.wait(timeout=timeout_seconds)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
                fail(EXIT_DEPENDENCY, "nebula-cert-timed-out")
            if return_code != 0:
                fail(EXIT_MALFORMED, "certificate-unreadable")
            output.seek(0, os.SEEK_END)
            size = output.tell()
            if size <= 0 or size > MAX_CERT_PRINT_BYTES:
                fail(EXIT_MALFORMED, "certificate-print-size")
            output.seek(0)
            return output.read(MAX_CERT_PRINT_BYTES + 1)
    finally:
        if staged_path is not None:
            try:
                os.unlink(staged_path)
            except FileNotFoundError:
                pass


def parse_json(raw: bytes, reason: str) -> Any:
    try:
        return json.loads(raw.decode("utf-8", errors="strict"), object_pairs_hook=strict_object)
    except GuardError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError):
        fail(EXIT_MALFORMED, reason)


def parse_local_certificate(raw: bytes) -> tuple[str, str, str, str]:
    value = parse_json(raw, "certificate-print-malformed")
    if isinstance(value, list):
        if len(value) != 1:
            fail(EXIT_MALFORMED, "certificate-count")
        value = value[0]
    if not isinstance(value, dict):
        fail(EXIT_MALFORMED, "certificate-print-shape")
    details = value.get("details")
    if not isinstance(details, dict):
        fail(EXIT_MALFORMED, "certificate-details")
    node_id = details.get("name")
    fingerprint = value.get("fingerprint")
    issuer = details.get("issuer")
    if not isinstance(node_id, str) or NODE_ID_RE.fullmatch(node_id) is None:
        fail(EXIT_MALFORMED, "certificate-node-id")
    if not valid_digest(fingerprint) or not valid_digest(issuer):
        fail(EXIT_MALFORMED, "certificate-fingerprint")
    networks = details.get("networks", details.get("ips"))
    if not isinstance(networks, list) or len(networks) != 1 or not isinstance(networks[0], str):
        fail(EXIT_MALFORMED, "certificate-overlay-count")
    try:
        interface = ipaddress.ip_interface(networks[0])
    except ValueError:
        fail(EXIT_MALFORMED, "certificate-overlay-address")
    if (
        interface.version != 4
        or interface.network.prefixlen != OVERLAY_NETWORK.prefixlen
        or interface.ip not in OVERLAY_NETWORK
        or networks[0] != f"{interface.ip}/{interface.network.prefixlen}"
    ):
        fail(EXIT_MALFORMED, "certificate-overlay-scope")
    return node_id, str(interface.ip), fingerprint, issuer


def authenticate(domain: bytes, key: bytes, payload: dict[str, Any]) -> str:
    return hmac.new(key, domain + b"\0" + canonical_json(payload), hashlib.sha256).hexdigest()


def verify_envelope(
    raw: bytes,
    *,
    expected_schema: str,
    domain: bytes,
    key: bytes,
    kind: str,
) -> tuple[dict[str, Any], str]:
    value = parse_json(raw, f"{kind}-malformed")
    if not isinstance(value, dict):
        fail(EXIT_MALFORMED, f"{kind}-shape")
    exact_keys(value, {"schema", "payload", "authentication"}, f"{kind}-fields")
    if value["schema"] != expected_schema or not isinstance(value["payload"], dict):
        fail(EXIT_MALFORMED, f"{kind}-schema")
    authentication = value["authentication"]
    if not isinstance(authentication, dict):
        fail(EXIT_MALFORMED, f"{kind}-authentication-shape")
    exact_keys(
        authentication,
        {"algorithm", "key_id", "tag"},
        f"{kind}-authentication-fields",
    )
    tag = authentication["tag"]
    if (
        authentication["algorithm"] != AUTH_ALGORITHM
        or authentication["key_id"] != AUTH_KEY_ID
        or not valid_digest(tag)
    ):
        fail(EXIT_UNTRUSTED, f"{kind}-authentication")
    expected = authenticate(domain, key, value["payload"])
    if not hmac.compare_digest(tag, expected):
        fail(EXIT_UNTRUSTED, f"{kind}-authentication")
    return value["payload"], tag


def validate_claim_entry(entry: Any, revision: int) -> dict[str, Any]:
    if not isinstance(entry, dict):
        fail(EXIT_MALFORMED, "claim-entry-shape")
    exact_keys(entry, {"key", "lease_id", "mod_revision", "claim"}, "claim-entry-fields")
    key = entry["key"]
    lease = entry["lease_id"]
    if not isinstance(key, str) or len(key.encode("utf-8")) != CLAIM_KEY_BYTES:
        fail(EXIT_MALFORMED, "claim-key-shape")
    if not key.startswith(CLAIM_PREFIX):
        fail(EXIT_MALFORMED, "claim-key-prefix")
    components = key[len(CLAIM_PREFIX) :].split("/")
    if len(components) != 3 or not all(valid_digest(component) for component in components):
        fail(EXIT_MALFORMED, "claim-key-components")
    if not isinstance(lease, str) or DECIMAL_RE.fullmatch(lease) is None:
        fail(EXIT_MALFORMED, "claim-lease")
    strict_int(entry["mod_revision"], minimum=1, maximum=revision, reason="claim-revision")
    claim = entry["claim"]
    if not isinstance(claim, dict):
        fail(EXIT_MALFORMED, "claim-value-shape")
    exact_keys(
        claim,
        {
            "schema_version",
            "nebula_node_id",
            "nebula_name",
            "nebula_address",
            "certificate_fingerprint",
            "machine_claimant_digest",
            "boot_claimant_digest",
        },
        "claim-value-fields",
    )
    if claim["schema_version"] != CLAIM_SCHEMA_VERSION:
        fail(EXIT_MALFORMED, "claim-schema")
    node = claim["nebula_node_id"]
    if (
        not isinstance(node, str)
        or NODE_ID_RE.fullmatch(node) is None
        or claim["nebula_name"] != node
    ):
        fail(EXIT_MALFORMED, "claim-node")
    address_text = claim["nebula_address"]
    try:
        address = ipaddress.ip_address(address_text)
    except ValueError:
        fail(EXIT_MALFORMED, "claim-address")
    if (
        not isinstance(address_text, str)
        or address.version != 4
        or address not in OVERLAY_NETWORK
        or str(address) != address_text
    ):
        fail(EXIT_MALFORMED, "claim-address")
    fingerprint = claim["certificate_fingerprint"]
    machine = claim["machine_claimant_digest"]
    boot = claim["boot_claimant_digest"]
    if not valid_digest(fingerprint) or not valid_digest(machine) or not valid_digest(boot):
        fail(EXIT_MALFORMED, "claim-digest")
    if machine == boot or components != [fingerprint, machine, boot]:
        fail(EXIT_MALFORMED, "claim-key-value-mismatch")
    if len(canonical_json(claim)) > MAX_CLAIM_VALUE_BYTES:
        fail(EXIT_MALFORMED, "claim-value-size")
    return claim


def parse_snapshot(
    payload: dict[str, Any],
    *,
    now_ms: int,
    current_boot_digest: str,
    max_snapshot_age_ms: int,
) -> tuple[int, list[dict[str, Any]], int]:
    exact_keys(
        payload,
        {
            "schema",
            "generated_at_unix_ms",
            "valid_until_unix_ms",
            "producer_boot_digest",
            "source",
            "claims",
        },
        "snapshot-payload-fields",
    )
    if payload["schema"] != SNAPSHOT_SCHEMA:
        fail(EXIT_MALFORMED, "snapshot-payload-schema")
    generated = strict_int(
        payload["generated_at_unix_ms"], minimum=1, maximum=2**63 - 1, reason="snapshot-time"
    )
    valid_until = strict_int(
        payload["valid_until_unix_ms"], minimum=1, maximum=2**63 - 1, reason="snapshot-validity"
    )
    if generated > now_ms + MAX_CLOCK_SKEW_MS:
        fail(EXIT_STALE, "snapshot-from-future")
    if now_ms - generated > max_snapshot_age_ms or now_ms > valid_until:
        fail(EXIT_STALE, "snapshot-stale")
    if valid_until <= generated or valid_until - generated > MAX_VALIDITY_SECONDS * 1_000:
        fail(EXIT_MALFORMED, "snapshot-validity-window")
    producer_boot = payload["producer_boot_digest"]
    if not valid_digest(producer_boot):
        fail(EXIT_MALFORMED, "snapshot-boot-digest")
    if not hmac.compare_digest(producer_boot, current_boot_digest):
        fail(EXIT_STALE, "snapshot-previous-boot")

    source = payload["source"]
    if not isinstance(source, dict):
        fail(EXIT_MALFORMED, "snapshot-source-shape")
    exact_keys(
        source,
        {"kind", "namespace", "cluster_id", "member_id", "etcd_revision", "raft_term"},
        "snapshot-source-fields",
    )
    if source["kind"] != SOURCE_KIND or source["namespace"] != CLAIM_PREFIX:
        fail(EXIT_UNTRUSTED, "snapshot-source-untrusted")
    if (
        not isinstance(source["cluster_id"], str)
        or DECIMAL_RE.fullmatch(source["cluster_id"]) is None
        or not isinstance(source["member_id"], str)
        or DECIMAL_RE.fullmatch(source["member_id"]) is None
    ):
        fail(EXIT_MALFORMED, "snapshot-source-identity")
    revision = strict_int(
        source["etcd_revision"], minimum=1, maximum=2**63 - 1, reason="snapshot-revision"
    )
    strict_int(source["raft_term"], minimum=1, maximum=2**63 - 1, reason="snapshot-raft-term")
    entries = payload["claims"]
    if not isinstance(entries, list) or len(entries) > MAX_CLAIMS:
        fail(EXIT_MALFORMED, "snapshot-claim-count")
    claims: list[dict[str, Any]] = []
    keys: set[str] = set()
    leases: set[str] = set()
    previous_key: str | None = None
    for entry in entries:
        claim = validate_claim_entry(entry, revision)
        key = entry["key"]
        lease = entry["lease_id"]
        if key in keys or lease in leases or (previous_key is not None and key <= previous_key):
            fail(EXIT_MALFORMED, "duplicate-or-unsorted-claim")
        keys.add(key)
        leases.add(lease)
        previous_key = key
        claims.append(claim)
    return revision, claims, generated


def verify_commitment(
    payload: dict[str, Any],
    *,
    snapshot_tag: str,
    boot_digest: str,
    revision: int,
    generated: int,
) -> None:
    exact_keys(
        payload,
        {
            "schema",
            "snapshot_tag",
            "producer_boot_digest",
            "etcd_revision",
            "generated_at_unix_ms",
        },
        "commitment-payload-fields",
    )
    if payload["schema"] != COMMITMENT_SCHEMA:
        fail(EXIT_MALFORMED, "commitment-payload-schema")
    if (
        payload["snapshot_tag"] != snapshot_tag
        or payload["producer_boot_digest"] != boot_digest
        or payload["etcd_revision"] != revision
        or payload["generated_at_unix_ms"] != generated
    ):
        fail(EXIT_REPLAY, "snapshot-replayed")


def opaque_identity(node_id: str) -> str:
    return hashlib.sha256(node_id.encode("utf-8")).hexdigest()[:12]


def evaluate(
    local_node: str,
    local_address: str,
    local_fingerprint: str,
    local_machine_digest: str,
    local_boot_digest: str,
    claims: list[dict[str, Any]],
) -> None:
    for claim in claims:
        same_public_identity = (
            claim["nebula_node_id"] == local_node
            and claim["nebula_address"] == local_address
            and claim["certificate_fingerprint"] == local_fingerprint
        )
        same_claimant = (
            same_public_identity
            and claim["machine_claimant_digest"] == local_machine_digest
            and claim["boot_claimant_digest"] == local_boot_digest
        )
        if same_claimant:
            continue
        if (
            claim["nebula_node_id"] == local_node
            or claim["nebula_address"] == local_address
            or claim["certificate_fingerprint"] == local_fingerprint
        ):
            print(
                "overlay-identity-guard: blocked code=active-identity-collision "
                f"local_address={local_address} claimant_token={opaque_identity(claim['nebula_node_id'])}",
                file=sys.stderr,
            )
            raise SystemExit(EXIT_COLLISION)


def bounded_positive(raw: str, maximum: int, option: str) -> int:
    try:
        value = int(raw, 10)
    except ValueError:
        raise argparse.ArgumentTypeError(f"{option} must be an integer") from None
    if value < 1 or value > maximum:
        raise argparse.ArgumentTypeError(f"{option} must be between 1 and {maximum}")
    return value


def arguments(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Refuse a conflicting identity using an authenticated current-boot claim cache."
    )
    parser.add_argument("--certificate", type=Path, default=DEFAULT_CERTIFICATE)
    parser.add_argument("--fallback-certificate", type=Path, default=DEFAULT_FALLBACK_CERTIFICATE)
    parser.add_argument("--snapshot", type=Path, default=DEFAULT_SNAPSHOT)
    parser.add_argument("--commitment", type=Path, default=DEFAULT_COMMITMENT)
    parser.add_argument("--auth-key", type=Path, default=DEFAULT_AUTH_KEY)
    parser.add_argument("--machine-id", type=Path, default=DEFAULT_MACHINE_ID)
    parser.add_argument("--boot-id", type=Path, default=DEFAULT_BOOT_ID)
    parser.add_argument("--nebula-cert-bin", type=Path, default=DEFAULT_NEBULA_CERT)
    parser.add_argument("--runtime-dir", type=Path, default=DEFAULT_RUNTIME_DIR)
    parser.add_argument(
        "--max-snapshot-age-seconds",
        type=lambda value: bounded_positive(
            value, MAX_SNAPSHOT_AGE_SECONDS, "snapshot age"
        ),
        default=15,
    )
    parser.add_argument(
        "--certificate-parser-timeout-seconds",
        type=lambda value: bounded_positive(
            value, MAX_CERT_PRINT_TIMEOUT_SECONDS, "certificate parser timeout"
        ),
        default=2,
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = arguments(argv)
    auth_key = safe_read(args.auth_key, 32, kind="auth-key", exact_size=32)
    machine_id = read_machine_id(args.machine_id)
    boot_id = read_boot_id(args.boot_id)
    current_boot_digest = boot_attestation(auth_key, boot_id)

    certificate = read_active_certificate(args.certificate, args.fallback_certificate)
    printed = bounded_nebula_print(
        args.nebula_cert_bin,
        certificate,
        args.runtime_dir,
        timeout_seconds=args.certificate_parser_timeout_seconds,
    )
    local_node, local_address, local_fingerprint, _local_issuer = parse_local_certificate(printed)

    snapshot_payload, snapshot_tag = verify_envelope(
        safe_read(args.snapshot, MAX_SNAPSHOT_BYTES, kind="snapshot"),
        expected_schema=SNAPSHOT_SCHEMA,
        domain=SNAPSHOT_AUTH_DOMAIN,
        key=auth_key,
        kind="snapshot",
    )
    commitment_payload, _commitment_tag = verify_envelope(
        safe_read(args.commitment, MAX_COMMITMENT_BYTES, kind="commitment"),
        expected_schema=COMMITMENT_SCHEMA,
        domain=COMMITMENT_AUTH_DOMAIN,
        key=auth_key,
        kind="commitment",
    )
    now_ms = time.time_ns() // 1_000_000
    revision, claims, generated = parse_snapshot(
        snapshot_payload,
        now_ms=now_ms,
        current_boot_digest=current_boot_digest,
        max_snapshot_age_ms=args.max_snapshot_age_seconds * 1_000,
    )
    verify_commitment(
        commitment_payload,
        snapshot_tag=snapshot_tag,
        boot_digest=current_boot_digest,
        revision=revision,
        generated=generated,
    )

    local_machine_digest = claimant_digest(
        MACHINE_CLAIMANT_DOMAIN,
        local_fingerprint,
        machine_id,
    )
    local_boot_claimant = claimant_digest(
        BOOT_CLAIMANT_DOMAIN,
        local_fingerprint,
        boot_id,
    )
    evaluate(
        local_node,
        local_address,
        local_fingerprint,
        local_machine_digest,
        local_boot_claimant,
        claims,
    )
    print(
        "overlay-identity-guard: safe "
        f"local_address={local_address} evidence_revision={revision} active_claimants={len(claims)}"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except GuardError as error:
        print(f"overlay-identity-guard: blocked code={error.reason}", file=sys.stderr)
        raise SystemExit(error.exit_code) from None
