#!/usr/bin/python3
"""Materialize a bounded authenticated cache of live overlay claim leases.

This producer is deliberately post-overlay: it performs one linearizable range
over the strict runtime claim namespace, validates the exact claimant key/value
contract and etcd lease metadata, then writes an HMAC-authenticated snapshot and
an authenticated current-boot commitment.  It never reads a private key, a raw
machine id, or certificate/key material.  The dedicated 32-byte HMAC credential
authenticates only this local cache.

The cache is a prerequisite, not cold-boot authority.  Its commitment lives in
``/run`` and its provenance is bound to the producer boot, so persisted evidence
cannot authorize a later boot.  A future authenticated pre-Nebula transport must
refresh both files during the current boot before the guard can be activated.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import hmac
import ipaddress
import json
import os
from pathlib import Path
import re
import resource
import secrets
import signal
import stat
import subprocess
import sys
import tempfile
import time
from typing import Any, NoReturn
from urllib.parse import urlsplit


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

DEFAULT_ENDPOINTS = Path("/etc/mackesd/etcd-endpoints")
DEFAULT_OUTPUT = Path(
    "/var/lib/mackesd/overlay-identity-claims/active-claims.json"
)
DEFAULT_COMMITMENT = Path("/run/mackesd/overlay-identity-active-claims.commit.json")
DEFAULT_AUTH_KEY = Path("/etc/mackesd/overlay-identity-snapshot-hmac")
DEFAULT_BOOT_ID = Path("/proc/sys/kernel/random/boot_id")
DEFAULT_ETCDCTL = Path("/usr/bin/etcdctl")

MAX_ENDPOINTS_BYTES = 4 * 1024
MAX_ETCD_RESPONSE_BYTES = 64 * 1024
MAX_CLAIM_VALUE_BYTES = 1_024
MAX_SNAPSHOT_BYTES = 64 * 1024
MAX_COMMITMENT_BYTES = 2 * 1024
MAX_CLAIMS = 12
MAX_VALIDITY_SECONDS = 30
MAX_COMMAND_TIMEOUT_SECONDS = 5

EXIT_MALFORMED = 21
EXIT_UNTRUSTED = 23
EXIT_DEPENDENCY = 24
EXIT_WRITE = 26
EXIT_PRIVILEGE = 77

NODE_ID_RE = re.compile(r"peer:[A-Za-z0-9][A-Za-z0-9._-]{0,127}\Z")
DIGEST_RE = re.compile(r"[0-9a-f]{64}\Z")
BOOT_ID_RE = re.compile(
    rb"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\Z"
)
DECIMAL_RE = re.compile(r"[1-9][0-9]{0,19}\Z")
OVERLAY_NETWORK = ipaddress.ip_network("10.42.0.0/17")


class MaterializerError(Exception):
    """An expected, credential-free producer failure."""

    def __init__(self, exit_code: int, reason: str) -> None:
        super().__init__(reason)
        self.exit_code = exit_code
        self.reason = reason


def fail(exit_code: int, reason: str) -> NoReturn:
    raise MaterializerError(exit_code, reason)


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


def strict_uint(value: Any, *, maximum: int, reason: str) -> int:
    if isinstance(value, bool):
        fail(EXIT_MALFORMED, reason)
    if isinstance(value, int):
        number = value
    elif isinstance(value, str) and DECIMAL_RE.fullmatch(value) is not None:
        number = int(value, 10)
    else:
        fail(EXIT_MALFORMED, reason)
    if number < 1 or number > maximum:
        fail(EXIT_MALFORMED, reason)
    return number


def strict_count(value: Any) -> int:
    if isinstance(value, bool):
        fail(EXIT_MALFORMED, "etcd-count")
    if isinstance(value, int):
        number = value
    elif isinstance(value, str) and re.fullmatch(r"(?:0|[1-9][0-9]*)", value) is not None:
        number = int(value, 10)
    else:
        fail(EXIT_MALFORMED, "etcd-count")
    if number < 0 or number > MAX_CLAIMS:
        fail(EXIT_MALFORMED, "etcd-count")
    return number


def read_regular(
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
        fd = os.open(path, flags)
    except OSError:
        fail(EXIT_UNTRUSTED, f"{kind}-unsafe")
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode):
            fail(EXIT_UNTRUSTED, f"{kind}-not-regular")
        if info.st_uid != os.geteuid() or info.st_mode & 0o022 or info.st_nlink != 1:
            fail(EXIT_UNTRUSTED, f"{kind}-untrusted")
        if not allow_zero_stat_size and (info.st_size <= 0 or info.st_size > maximum):
            fail(EXIT_MALFORMED, f"{kind}-size")
        chunks: list[bytes] = []
        remaining = maximum + 1
        while remaining > 0:
            chunk = os.read(fd, min(16 * 1024, remaining))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        data = b"".join(chunks)
        if not data or len(data) > maximum or (exact_size is not None and len(data) != exact_size):
            fail(EXIT_MALFORMED, f"{kind}-size")
        return data
    finally:
        os.close(fd)


def read_boot_id(path: Path) -> bytes:
    raw = read_regular(path, 128, kind="boot-id", allow_zero_stat_size=True).rstrip(b"\n")
    if BOOT_ID_RE.fullmatch(raw) is None or raw.replace(b"-", b"") == b"0" * 32:
        fail(EXIT_MALFORMED, "boot-id-malformed")
    return raw


def boot_attestation(key: bytes, boot_id: bytes) -> str:
    return hmac.new(key, BOOT_ATTESTATION_DOMAIN + b"\0" + boot_id, hashlib.sha256).hexdigest()


def parse_endpoints(raw: bytes) -> list[str]:
    try:
        text = raw.decode("ascii", errors="strict").strip()
    except UnicodeDecodeError:
        fail(EXIT_MALFORMED, "endpoints-malformed")
    if not text or any(character.isspace() for character in text):
        fail(EXIT_MALFORMED, "endpoints-malformed")
    endpoints = text.split(",")
    if len(endpoints) > MAX_CLAIMS or len(set(endpoints)) != len(endpoints):
        fail(EXIT_MALFORMED, "endpoints-count")
    for endpoint in endpoints:
        parsed = urlsplit(endpoint)
        if (
            parsed.scheme != "http"
            or parsed.username is not None
            or parsed.password is not None
            or parsed.path not in {"", "/"}
            or parsed.query
            or parsed.fragment
            or parsed.port != 2379
        ):
            fail(EXIT_MALFORMED, "endpoint-shape")
        try:
            address = ipaddress.ip_address(parsed.hostname or "")
        except ValueError:
            fail(EXIT_MALFORMED, "endpoint-address")
        if address.version != 4 or address not in OVERLAY_NETWORK:
            fail(EXIT_MALFORMED, "endpoint-scope")
    return endpoints


def validate_executable(path: Path) -> None:
    if not path.is_absolute():
        fail(EXIT_DEPENDENCY, "etcdctl-unavailable")
    try:
        info = os.lstat(path)
    except OSError:
        fail(EXIT_DEPENDENCY, "etcdctl-unavailable")
    if (
        not stat.S_ISREG(info.st_mode)
        or info.st_uid != os.geteuid()
        or info.st_mode & 0o022
        or info.st_nlink != 1
        or not os.access(path, os.X_OK)
    ):
        fail(EXIT_DEPENDENCY, "etcdctl-untrusted")


def range_claims(
    binary: Path,
    endpoints: list[str],
    *,
    timeout_seconds: int,
) -> bytes:
    validate_executable(binary)

    def limit_output() -> None:
        resource.setrlimit(
            resource.RLIMIT_FSIZE,
            (MAX_ETCD_RESPONSE_BYTES + 1, MAX_ETCD_RESPONSE_BYTES + 1),
        )

    command = [
        str(binary),
        f"--endpoints={','.join(endpoints)}",
        f"--command-timeout={timeout_seconds}s",
        "--write-out=json",
        "get",
        CLAIM_PREFIX,
        "--prefix",
        "--consistency=l",
    ]
    with tempfile.TemporaryFile() as output:
        try:
            process = subprocess.Popen(
                command,
                stdin=subprocess.DEVNULL,
                stdout=output,
                stderr=subprocess.DEVNULL,
                env={"PATH": "/usr/bin:/bin", "ETCDCTL_API": "3"},
                start_new_session=True,
                preexec_fn=limit_output,
            )
        except OSError:
            fail(EXIT_DEPENDENCY, "etcdctl-execution-failed")
        try:
            return_code = process.wait(timeout=timeout_seconds)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
            fail(EXIT_DEPENDENCY, "etcdctl-timed-out")
        if return_code != 0:
            fail(EXIT_DEPENDENCY, "etcdctl-range-failed")
        output.seek(0, os.SEEK_END)
        size = output.tell()
        if size <= 0 or size > MAX_ETCD_RESPONSE_BYTES:
            fail(EXIT_MALFORMED, "etcd-response-size")
        output.seek(0)
        return output.read(MAX_ETCD_RESPONSE_BYTES + 1)


def parse_json(raw: bytes, reason: str) -> Any:
    try:
        return json.loads(raw.decode("utf-8", errors="strict"), object_pairs_hook=strict_object)
    except MaterializerError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError):
        fail(EXIT_MALFORMED, reason)


def decode_base64(value: Any, maximum: int, reason: str) -> bytes:
    if not isinstance(value, str) or len(value) > ((maximum + 2) // 3) * 4 + 4:
        fail(EXIT_MALFORMED, reason)
    try:
        decoded = base64.b64decode(value, validate=True)
    except (binascii.Error, ValueError):
        fail(EXIT_MALFORMED, reason)
    if not decoded or len(decoded) > maximum:
        fail(EXIT_MALFORMED, reason)
    return decoded


def valid_digest(value: Any) -> bool:
    return (
        isinstance(value, str)
        and DIGEST_RE.fullmatch(value) is not None
        and value != "0" * 64
    )


def parse_claim(key_bytes: bytes, value_bytes: bytes) -> tuple[str, dict[str, Any]]:
    try:
        key = key_bytes.decode("ascii", errors="strict")
    except UnicodeDecodeError:
        fail(EXIT_MALFORMED, "claim-key-encoding")
    if len(key_bytes) != CLAIM_KEY_BYTES or not key.startswith(CLAIM_PREFIX):
        fail(EXIT_MALFORMED, "claim-key-shape")
    components = key[len(CLAIM_PREFIX) :].split("/")
    if len(components) != 3 or not all(valid_digest(component) for component in components):
        fail(EXIT_MALFORMED, "claim-key-components")

    claim = parse_json(value_bytes, "claim-value-malformed")
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
    if machine == boot:
        fail(EXIT_MALFORMED, "claim-digest-domain")
    if components != [fingerprint, machine, boot]:
        fail(EXIT_MALFORMED, "claim-key-value-mismatch")
    return key, claim


def parse_range(raw: bytes) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    response = parse_json(raw, "etcd-response-malformed")
    if not isinstance(response, dict):
        fail(EXIT_MALFORMED, "etcd-response-shape")
    exact_keys(response, {"header", "kvs", "count"}, "etcd-response-fields")
    header = response["header"]
    if not isinstance(header, dict):
        fail(EXIT_MALFORMED, "etcd-header-shape")
    exact_keys(
        header,
        {"cluster_id", "member_id", "revision", "raft_term"},
        "etcd-header-fields",
    )
    cluster_id = strict_uint(header["cluster_id"], maximum=2**64 - 1, reason="cluster-id")
    member_id = strict_uint(header["member_id"], maximum=2**64 - 1, reason="member-id")
    revision = strict_uint(header["revision"], maximum=2**63 - 1, reason="etcd-revision")
    raft_term = strict_uint(header["raft_term"], maximum=2**63 - 1, reason="raft-term")

    kvs = response["kvs"]
    if not isinstance(kvs, list) or len(kvs) > MAX_CLAIMS:
        fail(EXIT_MALFORMED, "etcd-claim-count")
    count = strict_count(response["count"])
    if count != len(kvs):
        fail(EXIT_MALFORMED, "etcd-count-mismatch")

    claims: list[dict[str, Any]] = []
    seen_keys: set[str] = set()
    seen_leases: set[str] = set()
    for kv in kvs:
        if not isinstance(kv, dict):
            fail(EXIT_MALFORMED, "etcd-kv-shape")
        exact_keys(
            kv,
            {"key", "create_revision", "mod_revision", "version", "value", "lease"},
            "etcd-kv-fields",
        )
        create_revision = strict_uint(
            kv["create_revision"], maximum=revision, reason="claim-create-revision"
        )
        mod_revision = strict_uint(kv["mod_revision"], maximum=revision, reason="claim-mod-revision")
        version = strict_uint(kv["version"], maximum=2**63 - 1, reason="claim-version")
        if create_revision > mod_revision or version < 1:
            fail(EXIT_MALFORMED, "claim-revision-order")
        lease = strict_uint(kv["lease"], maximum=2**63 - 1, reason="claim-lease")
        lease_text = str(lease)
        key, claim = parse_claim(
            decode_base64(kv["key"], CLAIM_KEY_BYTES, "claim-key-base64"),
            decode_base64(kv["value"], MAX_CLAIM_VALUE_BYTES, "claim-value-base64"),
        )
        if key in seen_keys or lease_text in seen_leases:
            fail(EXIT_MALFORMED, "duplicate-live-claim")
        seen_keys.add(key)
        seen_leases.add(lease_text)
        claims.append(
            {
                "key": key,
                "lease_id": lease_text,
                "mod_revision": mod_revision,
                "claim": claim,
            }
        )
    claims.sort(key=lambda item: item["key"])
    source = {
        "kind": SOURCE_KIND,
        "namespace": CLAIM_PREFIX,
        "cluster_id": str(cluster_id),
        "member_id": str(member_id),
        "etcd_revision": revision,
        "raft_term": raft_term,
    }
    return source, claims


def authenticate(domain: bytes, key: bytes, payload: dict[str, Any]) -> str:
    return hmac.new(key, domain + b"\0" + canonical_json(payload), hashlib.sha256).hexdigest()


def envelope(schema: str, domain: bytes, key: bytes, payload: dict[str, Any]) -> dict[str, Any]:
    return {
        "schema": schema,
        "payload": payload,
        "authentication": {
            "algorithm": AUTH_ALGORITHM,
            "key_id": AUTH_KEY_ID,
            "tag": authenticate(domain, key, payload),
        },
    }


def open_trusted_directory(path: Path) -> int:
    flags = os.O_RDONLY | os.O_CLOEXEC | getattr(os, "O_DIRECTORY", 0)
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
    except OSError:
        fail(EXIT_WRITE, "output-directory-unavailable")
    info = os.fstat(descriptor)
    if (
        not stat.S_ISDIR(info.st_mode)
        or info.st_uid != os.geteuid()
        or stat.S_IMODE(info.st_mode) != 0o700
    ):
        os.close(descriptor)
        fail(EXIT_WRITE, "output-directory-untrusted")
    return descriptor


def atomic_write(path: Path, data: bytes, maximum: int) -> None:
    if not path.is_absolute() or not data or len(data) > maximum:
        fail(EXIT_WRITE, "output-invalid")
    directory = open_trusted_directory(path.parent)
    temporary = f".{path.name}.{secrets.token_hex(16)}.tmp"
    descriptor: int | None = None
    try:
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        descriptor = os.open(temporary, flags, 0o600, dir_fd=directory)
        written = 0
        while written < len(data):
            written += os.write(descriptor, data[written:])
        os.fchmod(descriptor, 0o600)
        os.fsync(descriptor)
        os.close(descriptor)
        descriptor = None
        os.rename(temporary, path.name, src_dir_fd=directory, dst_dir_fd=directory)
        os.fsync(directory)
    except OSError:
        fail(EXIT_WRITE, "output-write-failed")
    finally:
        if descriptor is not None:
            os.close(descriptor)
        try:
            os.unlink(temporary, dir_fd=directory)
        except FileNotFoundError:
            pass
        except OSError:
            pass
        os.close(directory)


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
        description="Create a current-boot authenticated cache from strict live overlay claim leases."
    )
    parser.add_argument("--endpoints-file", type=Path, default=DEFAULT_ENDPOINTS)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--commitment", type=Path, default=DEFAULT_COMMITMENT)
    parser.add_argument("--auth-key", type=Path, default=DEFAULT_AUTH_KEY)
    parser.add_argument("--boot-id", type=Path, default=DEFAULT_BOOT_ID)
    parser.add_argument("--etcdctl-bin", type=Path, default=DEFAULT_ETCDCTL)
    parser.add_argument(
        "--validity-seconds",
        type=lambda value: bounded_positive(value, MAX_VALIDITY_SECONDS, "validity"),
        default=15,
    )
    parser.add_argument(
        "--command-timeout-seconds",
        type=lambda value: bounded_positive(
            value, MAX_COMMAND_TIMEOUT_SECONDS, "command timeout"
        ),
        default=MAX_COMMAND_TIMEOUT_SECONDS,
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    if os.geteuid() != 0:
        fail(EXIT_PRIVILEGE, "root-required")
    args = arguments(argv)
    auth_key = read_regular(args.auth_key, 32, kind="auth-key", exact_size=32)
    producer_boot = boot_attestation(auth_key, read_boot_id(args.boot_id))
    endpoints = parse_endpoints(
        read_regular(args.endpoints_file, MAX_ENDPOINTS_BYTES, kind="endpoints")
    )
    source, claims = parse_range(
        range_claims(
            args.etcdctl_bin,
            endpoints,
            timeout_seconds=args.command_timeout_seconds,
        )
    )
    generated = time.time_ns() // 1_000_000
    payload = {
        "schema": SNAPSHOT_SCHEMA,
        "generated_at_unix_ms": generated,
        "valid_until_unix_ms": generated + args.validity_seconds * 1_000,
        "producer_boot_digest": producer_boot,
        "source": source,
        "claims": claims,
    }
    snapshot = envelope(SNAPSHOT_SCHEMA, SNAPSHOT_AUTH_DOMAIN, auth_key, payload)
    snapshot_tag = snapshot["authentication"]["tag"]
    commitment_payload = {
        "schema": COMMITMENT_SCHEMA,
        "snapshot_tag": snapshot_tag,
        "producer_boot_digest": producer_boot,
        "etcd_revision": source["etcd_revision"],
        "generated_at_unix_ms": generated,
    }
    commitment = envelope(
        COMMITMENT_SCHEMA,
        COMMITMENT_AUTH_DOMAIN,
        auth_key,
        commitment_payload,
    )
    snapshot_bytes = canonical_json(snapshot) + b"\n"
    commitment_bytes = canonical_json(commitment) + b"\n"
    if len(snapshot_bytes) > MAX_SNAPSHOT_BYTES or len(commitment_bytes) > MAX_COMMITMENT_BYTES:
        fail(EXIT_WRITE, "authenticated-output-size")

    # Snapshot first, commitment last.  A crash between writes yields a
    # mismatch and the guard fails closed; the reverse order could authorize an
    # old snapshot under a new commitment.
    atomic_write(args.output, snapshot_bytes, MAX_SNAPSHOT_BYTES)
    atomic_write(args.commitment, commitment_bytes, MAX_COMMITMENT_BYTES)
    print(
        "overlay-identity-materializer: wrote authenticated-current-boot-snapshot "
        f"revision={source['etcd_revision']} active_claims={len(claims)}"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except MaterializerError as error:
        print(f"overlay-identity-materializer: blocked code={error.reason}", file=sys.stderr)
        raise SystemExit(error.exit_code) from None
