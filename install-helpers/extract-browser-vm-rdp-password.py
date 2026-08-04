#!/usr/bin/env python3
"""Extract one canonical Browser VM RDP password without printing it."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import stat
import tempfile


MAX_CREDENTIAL_BYTES = 4096
PASSWORD_RE = re.compile(r"[0-9a-f]{64}")
USERNAME_RE = re.compile(r"[a-z_][a-z0-9_-]{0,127}")
EXPECTED_FIELDS = {"schema_version", "username", "password"}


class ExtractionError(RuntimeError):
    """The credential envelope or private destination is not trustworthy."""


def read_private_file(path: Path) -> bytes:
    if not path.is_absolute():
        raise ExtractionError("credential path must be absolute")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as exc:
        raise ExtractionError("credential is unavailable") from exc
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise ExtractionError("credential must be a regular file")
        if metadata.st_uid != os.geteuid():
            raise ExtractionError("credential has the wrong owner")
        if metadata.st_mode & 0o077:
            raise ExtractionError("credential grants group or other access")
        if not 0 < metadata.st_size <= MAX_CREDENTIAL_BYTES:
            raise ExtractionError("credential has an invalid bounded size")
        chunks: list[bytes] = []
        remaining = MAX_CREDENTIAL_BYTES + 1
        while remaining:
            chunk = os.read(descriptor, remaining)
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        payload = b"".join(chunks)
        if len(payload) != metadata.st_size:
            raise ExtractionError("credential changed while it was read")
        return payload
    finally:
        os.close(descriptor)


def parse_envelope(payload: bytes, expected_username: str) -> str:
    try:
        value = json.loads(payload)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ExtractionError("credential is not canonical JSON") from exc
    if not isinstance(value, dict) or set(value) != EXPECTED_FIELDS:
        raise ExtractionError("credential fields do not match the canonical schema")
    if type(value["schema_version"]) is not int or value["schema_version"] != 1:
        raise ExtractionError("credential schema version is unsupported")
    if value["username"] != expected_username:
        raise ExtractionError("credential username does not match the requested account")
    password = value["password"]
    if not isinstance(password, str) or PASSWORD_RE.fullmatch(password) is None:
        raise ExtractionError("credential password is not a 256-bit lowercase token")
    return password


def validate_parent(output: Path) -> Path:
    if not output.is_absolute():
        raise ExtractionError("output path must be absolute")
    parent = output.parent
    try:
        canonical = parent.resolve(strict=True)
        metadata = parent.stat()
    except OSError as exc:
        raise ExtractionError("output parent is unavailable") from exc
    if canonical != parent:
        raise ExtractionError("output parent must be canonical and contain no symlink")
    if not stat.S_ISDIR(metadata.st_mode):
        raise ExtractionError("output parent is not a directory")
    if metadata.st_uid != os.geteuid():
        raise ExtractionError("output parent has the wrong owner")
    if metadata.st_mode & 0o022:
        raise ExtractionError("output parent grants untrusted write access")
    return parent


def write_private_password(output: Path, password: str) -> None:
    parent = validate_parent(output)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    descriptor: int | None = None
    created = False
    complete = False
    try:
        descriptor = os.open(output, flags, 0o600)
        created = True
        payload = password.encode("ascii")
        if os.write(descriptor, payload) != len(payload):
            raise ExtractionError("password output was incomplete")
        os.fsync(descriptor)
        metadata = os.fstat(descriptor)
        if metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) != 0o600:
            raise ExtractionError("password output ownership or mode changed")
        os.close(descriptor)
        descriptor = None
        parent_descriptor = os.open(parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(parent_descriptor)
        finally:
            os.close(parent_descriptor)
        complete = True
    except FileExistsError as exc:
        raise ExtractionError("password output already exists") from exc
    except OSError as exc:
        raise ExtractionError("password output could not be published") from exc
    finally:
        if descriptor is not None:
            os.close(descriptor)
        if created and not complete:
            try:
                output.unlink()
            except (FileNotFoundError, IsADirectoryError):
                pass


def extract(credential: Path, output: Path, username: str) -> None:
    if USERNAME_RE.fullmatch(username) is None:
        raise ExtractionError("requested username is malformed")
    password = parse_envelope(read_private_file(credential), username)
    try:
        write_private_password(output, password)
    finally:
        password = ""


def self_test() -> None:
    fixture = Path(tempfile.mkdtemp(prefix="mcnf-rdp-envelope-"))
    try:
        fixture.chmod(0o700)
        credential = fixture / "credential.json"
        output = fixture / "password"
        password = "a" * 64
        credential.write_text(
            json.dumps(
                {"schema_version": 1, "username": "mcnf-browser", "password": password},
                separators=(",", ":"),
            )
            + "\n",
            encoding="utf-8",
        )
        credential.chmod(0o600)
        extract(credential, output, "mcnf-browser")
        assert output.read_text(encoding="ascii") == password
        assert stat.S_IMODE(output.stat().st_mode) == 0o600

        existing = fixture / "existing"
        existing.write_text("retain-me", encoding="ascii")
        existing.chmod(0o600)
        try:
            extract(credential, existing, "mcnf-browser")
        except ExtractionError:
            pass
        else:
            raise AssertionError("existing password output was overwritten")
        assert existing.read_text(encoding="ascii") == "retain-me"

        rejected = fixture / "rejected"
        for value in (
            {"schema_version": 1, "username": "other", "password": password},
            {"schema_version": 1, "username": "mcnf-browser", "password": "short"},
            {
                "schema_version": 1,
                "username": "mcnf-browser",
                "password": password,
                "extra": True,
            },
        ):
            credential.write_text(json.dumps(value), encoding="utf-8")
            credential.chmod(0o600)
            try:
                extract(credential, rejected, "mcnf-browser")
            except ExtractionError:
                pass
            else:
                raise AssertionError("invalid credential envelope was accepted")
            assert not rejected.exists()
    finally:
        shutil.rmtree(fixture)
    print("extract-browser-vm-rdp-password: self-test passed")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--credential", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--username", default="mcnf-browser")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        if args.credential is not None or args.output is not None:
            parser.error("--self-test does not accept credential or output paths")
        self_test()
        return 0
    if args.credential is None or args.output is None:
        parser.error("--credential and --output are required")
    try:
        extract(args.credential, args.output, args.username)
    except ExtractionError as exc:
        parser.exit(1, f"extract-browser-vm-rdp-password: {exc}\n")
    print("extract-browser-vm-rdp-password: private password materialized")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
