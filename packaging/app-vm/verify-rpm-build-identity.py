#!/usr/bin/env python3
"""Verify one streamed ELF carries the exact governed Magic Mesh build identity."""

from __future__ import annotations

import argparse
import os
import re
import sys


REVISION = re.compile(r"[0-9a-f]{40}\Z")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9._-]+)?\Z")
DEFAULT_MAX_BYTES = 536_870_912


def fail(message: str) -> None:
    print(f"App VM RPM build identity refused: {message}", file=sys.stderr)
    raise SystemExit(2)


def positive_limit() -> int:
    raw = os.environ.get("MCNF_APP_VM_MAX_IDENTITY_BINARY_BYTES", str(DEFAULT_MAX_BYTES))
    try:
        value = int(raw, 10)
    except ValueError:
        fail("MCNF_APP_VM_MAX_IDENTITY_BINARY_BYTES must be a positive integer")
    if value <= 0:
        fail("MCNF_APP_VM_MAX_IDENTITY_BINARY_BYTES must be a positive integer")
    return value


def verify_stream(source_commit: str, package_version: str, member: str) -> None:
    if REVISION.fullmatch(source_commit) is None or source_commit == "0" * 40:
        fail("source commit must be a non-null lowercase 40-character Git revision")
    if VERSION.fullmatch(package_version) is None:
        fail("package version is malformed")
    if member not in {"./usr/bin/mackesd", "./usr/bin/mde-shell-egui"}:
        fail("RPM member is not an approved build-identity carrier")

    # mde-theme's compile-time BuildInfo constants are emitted contiguously in
    # both production binaries as VERSION + codename + GIT_HASH. Requiring that
    # exact sequence avoids confusing Rust/compiler/library object hashes with
    # the governed source receipt. We parse bytes only; package code is never
    # executed on the build driver.
    needle = (package_version + "Construct" + source_commit).encode("ascii")
    overlap = len(needle) - 1
    retained = b""
    total = 0
    matches = 0
    saw_elf = False
    maximum = positive_limit()

    while True:
        chunk = sys.stdin.buffer.read(1024 * 1024)
        if not chunk:
            break
        if not saw_elf:
            saw_elf = True
            if not chunk.startswith(b"\x7fELF"):
                fail(f"{member} is not an ELF payload")
        total += len(chunk)
        if total > maximum:
            fail(f"{member} exceeds the build-identity inspection bound")
        window = retained + chunk
        start = 0
        while True:
            found = window.find(needle, start)
            if found < 0:
                break
            matches += 1
            start = found + 1
        retained = window[-overlap:] if overlap else b""

    if total == 0:
        fail(f"{member} is empty or absent from the RPM")
    if matches != 1:
        fail(
            f"{member} carries {matches} exact source-revision build identities; expected one"
        )


def self_test() -> None:
    revision = "a" * 40
    version = "13.0.0"
    needle = (version + "Construct" + revision).encode()
    if needle != b"13.0.0Construct" + b"a" * 40:
        fail("identity construction self-test failed")
    if REVISION.fullmatch(revision) is None or REVISION.fullmatch("a" * 39) is not None:
        fail("revision-width self-test failed")
    if VERSION.fullmatch(version) is None or VERSION.fullmatch("latest") is not None:
        fail("version-shape self-test failed")
    print("App VM RPM build-identity parser self-test passed")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--source-commit")
    parser.add_argument("--package-version")
    parser.add_argument("--member")
    args = parser.parse_args()
    if args.self_test:
        if any((args.source_commit, args.package_version, args.member)):
            fail("--self-test takes no build-identity inputs")
        self_test()
        return
    if not all((args.source_commit, args.package_version, args.member)):
        fail("--source-commit, --package-version, and --member are required")
    verify_stream(args.source_commit, args.package_version, args.member)


if __name__ == "__main__":
    main()
