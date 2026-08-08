#!/usr/bin/env python3
"""Cryptographically verify a production resource-publisher attestation.

The payload mirrors mackes_mesh_types::ResourcePublisherAttestation exactly.
This consumer never mints a proof or discovers a key. The key comes only from
an explicit absolute path or the fixed systemd credential leaf under
CREDENTIALS_DIRECTORY, and key bytes are never included in diagnostics.

The HMAC authenticates the attestation fields and catalog content digest; its
payload does not contain a Git revision. The caller separately supplies the
trusted checkout revision, which must exactly equal release-evidence
source_commit before the surrounding signer binds both into its GPG bundle.
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import stat
import sys
import tempfile
import time
from typing import Any


ATTESTATION_KEY_ID = "resource-publisher-hmac-v1"
ATTESTATION_PREFIX = "publisher-attestation:v1:"
CATALOG_PREFIX = "catalog:v1:"
CREDENTIAL_LEAF = "resource-publisher-hmac"
MAX_KEY_BYTES = 4 * 1024
MAX_ATTESTATION_BYTES = 16 * 1024
MAX_EVIDENCE_BYTES = 8 * 1024 * 1024
MIN_TTL_MS = 1_000
MAX_TTL_MS = 7 * 24 * 60 * 60 * 1_000
IDENTIFIER = re.compile(r"^[A-Za-z0-9._:/@+\-]+$")
REVISION = re.compile(r"^(?:[0-9A-Fa-f]{40}|[0-9A-Fa-f]{64})$")
LOWER_HEX_64 = re.compile(r"^[0-9a-f]{64}$")
U64_MAX = (1 << 64) - 1
SECRET_SHAPE_MARKERS = (
    "authorization:",
    "proxy-authorization:",
    "bearer ",
    "password=",
    "password:",
    "passwd=",
    "token=",
    "access_token=",
    "refresh_token=",
    "api_key=",
    "apikey=",
    "client_secret=",
    "private_key=",
    "-----begin private key-----",
    "-----begin rsa private key-----",
    "-----begin openssh private key-----",
    '"password":',
    '"token":',
)


class VerificationError(Exception):
    """A bounded, credential-free verification failure."""


def fail(message: str) -> None:
    raise VerificationError(message)


def strict_json(raw: bytes, label: str) -> dict[str, Any]:
    def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        value: dict[str, Any] = {}
        for key, member in pairs:
            if key in value:
                fail(f"{label} contains a duplicate JSON field")
            value[key] = member
        return value

    try:
        value = json.loads(raw.decode("utf-8"), object_pairs_hook=unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise VerificationError(f"{label} is not valid UTF-8 JSON") from error
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def read_regular_bounded(path: Path, label: str, maximum: int) -> bytes:
    if not path.is_absolute():
        fail(f"{label} path must be absolute")
    try:
        before = path.lstat()
    except OSError as error:
        raise VerificationError(f"{label} is unavailable") from error
    if not stat.S_ISREG(before.st_mode) or stat.S_ISLNK(before.st_mode):
        fail(f"{label} must be a regular, non-symlink file")
    if before.st_size <= 0 or before.st_size > maximum:
        fail(f"{label} has an invalid bounded size")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise VerificationError(f"{label} could not be opened safely") from error
    try:
        opened = os.fstat(descriptor)
        if (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino):
            fail(f"{label} changed before it was opened")
        chunks: list[bytes] = []
        total = 0
        while True:
            chunk = os.read(descriptor, min(64 * 1024, maximum + 1 - total))
            if not chunk:
                break
            chunks.append(chunk)
            total += len(chunk)
            if total > maximum:
                fail(f"{label} exceeds its size bound")
        after = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    if (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns) != (
        after.st_dev,
        after.st_ino,
        after.st_size,
        after.st_mtime_ns,
    ):
        fail(f"{label} changed while it was read")
    raw = b"".join(chunks)
    if len(raw) != opened.st_size:
        fail(f"{label} changed length while it was read")
    return raw


def read_private_key(path: Path) -> bytes:
    if not path.is_absolute():
        fail("credential path must be absolute")
    try:
        metadata = path.lstat()
    except OSError as error:
        raise VerificationError("resource-publisher credential is unavailable") from error
    mode = stat.S_IMODE(metadata.st_mode)
    if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
        fail("resource-publisher credential must be a regular, non-symlink file")
    if metadata.st_nlink != 1:
        fail("resource-publisher credential must have exactly one filesystem link")
    if metadata.st_uid not in {0, os.geteuid()}:
        fail("resource-publisher credential has an untrusted owner")
    if mode not in {0o400, 0o600}:
        fail("resource-publisher credential permissions must be 0400 or 0600")
    raw = read_regular_bounded(path, "resource-publisher credential", MAX_KEY_BYTES)
    try:
        raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise VerificationError("resource-publisher credential is not UTF-8") from error
    if any(byte < 0x20 or byte == 0x7F for byte in raw):
        fail("resource-publisher credential contains control bytes")
    return raw


def credential_path(explicit: Path | None) -> Path:
    if explicit is not None:
        return explicit
    directory_text = os.environ.get("CREDENTIALS_DIRECTORY")
    if not directory_text:
        fail("production signing requires an explicit credential or CREDENTIALS_DIRECTORY")
    directory = Path(directory_text)
    if not directory.is_absolute():
        fail("CREDENTIALS_DIRECTORY must be absolute")
    try:
        metadata = directory.lstat()
    except OSError as error:
        raise VerificationError("CREDENTIALS_DIRECTORY is unavailable") from error
    if not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
        fail("CREDENTIALS_DIRECTORY must be a non-symlink directory")
    if metadata.st_uid not in {0, os.geteuid()}:
        fail("CREDENTIALS_DIRECTORY has an untrusted owner")
    if stat.S_IMODE(metadata.st_mode) & 0o022:
        fail("CREDENTIALS_DIRECTORY must not be group- or other-writable")
    return directory / CREDENTIAL_LEAF


def identifier_ok(value: Any) -> bool:
    if not isinstance(value, str) or not 0 < len(value.encode("utf-8")) <= 255:
        return False
    if not value.isascii() or not IDENTIFIER.fullmatch(value):
        return False
    lower = value.lower()
    scheme_end = lower.find("://")
    secret_url = False
    if scheme_end >= 0:
        authority = re.split(r"[/#?]", lower[scheme_end + 3 :], maxsplit=1)[0]
        secret_url = "@" in authority
    return not (
        value.startswith("/")
        or value.endswith("/")
        or "//" in value
        or any(marker in lower for marker in SECRET_SHAPE_MARKERS)
        or secret_url
        or any(segment in {".", ".."} for segment in value.split("/"))
    )


def unsigned_shape(attestation: dict[str, Any]) -> None:
    expected = {
        "catalog_content_digest",
        "expires_at_ms",
        "issued_at_ms",
        "key_id",
        "publisher",
        "schema_version",
        "signature",
    }
    if set(attestation) != expected:
        fail("resource-publisher attestation has unexpected fields")
    if type(attestation["schema_version"]) is not int or attestation["schema_version"] != 1:
        fail("resource-publisher attestation schema is unsupported")
    if not identifier_ok(attestation["publisher"]):
        fail("resource-publisher attestation publisher is invalid")
    if attestation["key_id"] != ATTESTATION_KEY_ID:
        fail("resource-publisher attestation key ID is unsupported")
    digest = attestation["catalog_content_digest"]
    if not isinstance(digest, str) or not digest.startswith(CATALOG_PREFIX):
        fail("resource-publisher catalog digest is invalid")
    if not LOWER_HEX_64.fullmatch(digest.removeprefix(CATALOG_PREFIX)):
        fail("resource-publisher catalog digest is invalid")
    issued = attestation["issued_at_ms"]
    expires = attestation["expires_at_ms"]
    if (
        type(issued) is not int
        or type(expires) is not int
        or issued <= 0
        or issued > U64_MAX
        or expires > U64_MAX
        or expires <= issued
    ):
        fail("resource-publisher attestation window is invalid")
    if not MIN_TTL_MS <= expires - issued <= MAX_TTL_MS:
        fail("resource-publisher attestation TTL is invalid")
    signature = attestation["signature"]
    if not isinstance(signature, str) or not signature.startswith(ATTESTATION_PREFIX):
        fail("resource-publisher attestation signature is invalid")
    if not LOWER_HEX_64.fullmatch(signature.removeprefix(ATTESTATION_PREFIX)):
        fail("resource-publisher attestation signature is invalid")


def push_canonical(parts: list[bytes], value: str) -> None:
    encoded = value.encode("utf-8")
    parts.extend((str(len(encoded)).encode("ascii"), b":", encoded))


def signing_payload(attestation: dict[str, Any]) -> bytes:
    parts: list[bytes] = []
    for value in (
        "resource-publisher-attestation",
        str(attestation["schema_version"]),
        attestation["publisher"],
        attestation["key_id"],
        attestation["catalog_content_digest"],
        str(attestation["issued_at_ms"]),
        str(attestation["expires_at_ms"]),
    ):
        push_canonical(parts, value)
    return b"".join(parts)


def descriptor_and_attestation(evidence: dict[str, Any]) -> tuple[dict[str, Any], dict[str, Any]]:
    provenance = evidence.get("provenance")
    if not isinstance(provenance, dict):
        fail("release evidence has no provenance object")
    descriptor = provenance.get("resource_publisher_attestation")
    if not isinstance(descriptor, dict):
        fail("production evidence has no resource-publisher attestation")
    envelope_keys = {
        "catalog_content_digest",
        "expires_at_ms",
        "issued_at_ms",
        "key_id",
        "publisher",
        "schema_version",
        "signature",
    }
    if set(descriptor) != envelope_keys | {"path", "sha256", "size_bytes"}:
        fail("resource-publisher attestation descriptor has unexpected fields")
    path = descriptor.get("path")
    size = descriptor.get("size_bytes")
    digest = descriptor.get("sha256")
    if (
        not isinstance(path, str)
        or not 0 < len(path.encode("utf-8")) <= 4096
        or any(ord(character) < 0x20 or ord(character) == 0x7F for character in path)
        or not Path(path).is_absolute()
    ):
        fail("resource-publisher attestation descriptor path must be absolute")
    if type(size) is not int or size <= 0 or size > MAX_ATTESTATION_BYTES:
        fail("resource-publisher attestation descriptor size is invalid")
    if not isinstance(digest, str) or not LOWER_HEX_64.fullmatch(digest):
        fail("resource-publisher attestation descriptor digest is invalid")
    raw = read_regular_bounded(Path(path), "resource-publisher attestation", MAX_ATTESTATION_BYTES)
    if len(raw) != size or not hmac.compare_digest(hashlib.sha256(raw).hexdigest(), digest):
        fail("resource-publisher attestation descriptor does not match its file")
    attestation = strict_json(raw, "resource-publisher attestation")
    if any(
        type(descriptor[key]) is not type(attestation.get(key))
        or descriptor[key] != attestation.get(key)
        for key in envelope_keys
    ):
        fail("embedded resource-publisher attestation does not match its file")
    return descriptor, attestation


def verify(
    evidence_path: Path,
    expected_revision: str,
    key_path: Path,
    *,
    now_ms: int | None = None,
) -> None:
    if not REVISION.fullmatch(expected_revision):
        fail("expected release revision must be an exact Git object ID")
    evidence_raw = read_regular_bounded(evidence_path, "release evidence", MAX_EVIDENCE_BYTES)
    evidence = strict_json(evidence_raw, "release evidence")
    if evidence.get("source_commit") != expected_revision:
        fail("release evidence source revision does not match the signing checkout")
    verdict = evidence.get("verdict")
    if not isinstance(verdict, dict) or verdict.get("production") != "pass":
        fail("cryptographic publisher verification is only valid for a production-pass envelope")
    _, attestation = descriptor_and_attestation(evidence)
    unsigned_shape(attestation)
    current = time.time_ns() // 1_000_000 if now_ms is None else now_ms
    if current < attestation["issued_at_ms"] or current >= attestation["expires_at_ms"]:
        fail("resource-publisher attestation is not fresh at signing time")
    key = read_private_key(key_path)
    expected = hmac.new(key, signing_payload(attestation), hashlib.sha256).digest()
    supplied = bytes.fromhex(attestation["signature"].removeprefix(ATTESTATION_PREFIX))
    if not hmac.compare_digest(expected, supplied):
        fail("resource-publisher attestation HMAC verification failed")


def mint_fixture(key: bytes, issued: int, expires: int) -> dict[str, Any]:
    attestation: dict[str, Any] = {
        "schema_version": 1,
        "publisher": "self-test-publisher",
        "key_id": ATTESTATION_KEY_ID,
        "catalog_content_digest": CATALOG_PREFIX + "a" * 64,
        "issued_at_ms": issued,
        "expires_at_ms": expires,
        "signature": "",
    }
    signature = hmac.new(key, signing_payload(attestation), hashlib.sha256).hexdigest()
    attestation["signature"] = ATTESTATION_PREFIX + signature
    return attestation


def write_fixture(root: Path, key: bytes, revision: str, now_ms: int) -> tuple[Path, Path, Path]:
    credential = root / "credential"
    credential.write_bytes(key)
    credential.chmod(0o600)
    attestation = mint_fixture(key, now_ms - 1_000, now_ms + 60_000)
    attestation_path = root / "attestation.json"
    raw = json.dumps(attestation, sort_keys=True, separators=(",", ":")).encode("utf-8") + b"\n"
    attestation_path.write_bytes(raw)
    descriptor = dict(attestation)
    descriptor.update(
        path=str(attestation_path),
        size_bytes=len(raw),
        sha256=hashlib.sha256(raw).hexdigest(),
    )
    evidence = {
        "source_commit": revision,
        "verdict": {"preview": "pass", "production": "pass"},
        "provenance": {"resource_publisher_attestation": descriptor},
    }
    evidence_path = root / "evidence.json"
    evidence_path.write_text(json.dumps(evidence, sort_keys=True), encoding="utf-8")
    return evidence_path, credential, attestation_path


def rewrite_descriptor(evidence_path: Path, attestation_path: Path) -> None:
    evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
    attestation = json.loads(attestation_path.read_text(encoding="utf-8"))
    raw = attestation_path.read_bytes()
    descriptor = dict(attestation)
    descriptor.update(
        path=str(attestation_path),
        size_bytes=len(raw),
        sha256=hashlib.sha256(raw).hexdigest(),
    )
    evidence["provenance"]["resource_publisher_attestation"] = descriptor
    evidence_path.write_text(json.dumps(evidence, sort_keys=True), encoding="utf-8")


def expect_reject(label: str, action: Any) -> None:
    try:
        action()
    except VerificationError as error:
        if "self-test-resource-publisher-key" in str(error):
            raise AssertionError(f"{label}: diagnostic exposed key bytes") from error
        return
    raise AssertionError(f"{label}: hostile fixture was accepted")


def self_test() -> None:
    revision = "0123456789abcdef0123456789abcdef01234567"
    other_revision = "f" * 40
    now_ms = 2_000_000_000_000
    key = b"self-test-resource-publisher-key"
    wrong_key = b"wrong-resource-publisher-test-key"
    with tempfile.TemporaryDirectory(prefix="resource-attestation-test-") as directory:
        root = Path(directory)
        evidence, credential, attestation_path = write_fixture(root, key, revision, now_ms)
        fixture_attestation = strict_json(attestation_path.read_bytes(), "fixture attestation")
        expected_payload = (
            b"30:resource-publisher-attestation"
            b"1:1"
            b"19:self-test-publisher"
            b"26:resource-publisher-hmac-v1"
            b"75:catalog:v1:"
            + b"a" * 64
            + b"13:1999999999000"
            + b"13:2000000060000"
        )
        if signing_payload(fixture_attestation) != expected_payload:
            raise AssertionError("canonical payload diverged from the Rust contract")
        verify(evidence, revision, credential, now_ms=now_ms)

        credentials_directory = root / "credentials"
        credentials_directory.mkdir(mode=0o700)
        systemd_credential = credentials_directory / CREDENTIAL_LEAF
        systemd_credential.write_bytes(key)
        systemd_credential.chmod(0o400)
        previous_directory = os.environ.get("CREDENTIALS_DIRECTORY")
        os.environ["CREDENTIALS_DIRECTORY"] = str(credentials_directory)
        try:
            verify(evidence, revision, credential_path(None), now_ms=now_ms)
            credentials_directory.chmod(0o770)
            expect_reject("writable credential directory", lambda: credential_path(None))
        finally:
            credentials_directory.chmod(0o700)
            if previous_directory is None:
                os.environ.pop("CREDENTIALS_DIRECTORY", None)
            else:
                os.environ["CREDENTIALS_DIRECTORY"] = previous_directory

        wrong = root / "wrong-key"
        wrong.write_bytes(wrong_key)
        wrong.chmod(0o600)
        expect_reject("wrong key", lambda: verify(evidence, revision, wrong, now_ms=now_ms))
        expect_reject(
            "evidence/checkout revision mismatch",
            lambda: verify(evidence, other_revision, credential, now_ms=now_ms),
        )
        expect_reject(
            "expired replay",
            lambda: verify(evidence, revision, credential, now_ms=now_ms + 60_000),
        )

        original_attestation = attestation_path.read_bytes()
        original_evidence = evidence.read_bytes()
        tampered = json.loads(original_attestation)
        tampered["publisher"] = "tampered-publisher"
        attestation_path.write_text(json.dumps(tampered), encoding="utf-8")
        expect_reject("tampered descriptor", lambda: verify(evidence, revision, credential, now_ms=now_ms))
        rewrite_descriptor(evidence, attestation_path)
        expect_reject(
            "cryptographic envelope tamper",
            lambda: verify(evidence, revision, credential, now_ms=now_ms),
        )
        attestation_path.write_bytes(original_attestation)
        evidence.write_bytes(original_evidence)

        credential.chmod(0o644)
        expect_reject("public credential mode", lambda: verify(evidence, revision, credential, now_ms=now_ms))
        credential.chmod(0o600)
        linked = root / "linked-key"
        linked.symlink_to(credential)
        expect_reject("symlink credential", lambda: verify(evidence, revision, linked, now_ms=now_ms))
        hardlinked = root / "hardlinked-key"
        os.link(credential, hardlinked)
        expect_reject(
            "hard-linked credential",
            lambda: verify(evidence, revision, hardlinked, now_ms=now_ms),
        )
        oversized = root / "oversized-key"
        oversized.write_bytes(b"k" * (MAX_KEY_BYTES + 1))
        oversized.chmod(0o600)
        expect_reject("oversized credential", lambda: verify(evidence, revision, oversized, now_ms=now_ms))

        original_evidence = evidence.read_bytes()
        evidence.write_text('{"source_commit":"first","source_commit":"second"}', encoding="utf-8")
        expect_reject("duplicate JSON field", lambda: verify(evidence, revision, credential, now_ms=now_ms))
        evidence.write_bytes(original_evidence)

    print(
        "verify-resource-publisher-attestation: self-test passed — "
        "canonical payload/systemd discovery accepted; wrong-key, revision mismatch, expiry replay, "
        "tamper, mode, symlink/hardlink, directory, duplicate-field, and size cases rejected"
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--evidence", type=Path)
    parser.add_argument("--expected-revision")
    parser.add_argument("--credential", type=Path)
    args = parser.parse_args()
    if args.self_test:
        if args.evidence is not None or args.expected_revision is not None or args.credential is not None:
            parser.error("--self-test accepts no verification inputs")
    elif args.evidence is None or args.expected_revision is None:
        parser.error("--evidence and --expected-revision are required")
    return args


def main() -> int:
    args = parse_args()
    try:
        if args.self_test:
            self_test()
        else:
            path = credential_path(args.credential)
            verify(args.evidence, args.expected_revision, path)
            print(
                "verify-resource-publisher-attestation: verified publisher HMAC; "
                f"evidence separately matches checkout revision {args.expected_revision}"
            )
    except (VerificationError, OSError) as error:
        print(f"verify-resource-publisher-attestation: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
