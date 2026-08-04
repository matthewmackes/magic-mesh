#!/usr/bin/env python3
"""Validate the immutable Browser VM production-control image contract."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path, PurePosixPath
import posixpath
import shlex
import stat
import sys
import tempfile
from typing import Any, Callable, NoReturn


CONTROLLER_BINARY = PurePosixPath(
    "/usr/libexec/mcnf/browser-vm-guest-audio-probe-controller"
)
CONTROLLER_SERVICE = PurePosixPath(
    "/usr/lib/systemd/system/browser-vm-guest-audio-probe-controller.service"
)
CONTROLLER_ENABLE_LINK = PurePosixPath(
    "/etc/systemd/system/multi-user.target.wants/"
    "browser-vm-guest-audio-probe-controller.service"
)
CONTROLLER_CONFIG = PurePosixPath(
    "/etc/mcnf/browser-vm-guest-audio-probe-controller.json"
)
CHROMIUM_POLICY = PurePosixPath(
    "/etc/chromium/policies/managed/mcnf-browser-vm.json"
)
CONTROLLER_SECRET = PurePosixPath(
    "/etc/mcnf/browser-vm-control/controller-secret"
)
CONTROLLER_SECRET_DIRECTORY = CONTROLLER_SECRET.parent
PROBE_ACCOUNT = "mcnf-browser-probe"
EXPECTED_HOST_ADDRESS = "192.168.122.1"
EXPECTED_HOST_NETWORK = "192.168.122.1/32"
EXPECTED_BROWSER_ORIGIN = "http://127.0.0.1:38443/*"
MAX_TEXT_BYTES = 64 * 1024
EXPECTED_CONFIG_FIELDS = frozenset(
    {
        "schema_version",
        "listen_address",
        "listen_port",
        "allowed_host_address",
        "controller_secret_file",
        "max_jobs",
    }
)


class ContractError(ValueError):
    """The image does not implement the bounded production-control contract."""


def fail(message: str) -> NoReturn:
    raise ContractError(message)


def image_path(root: Path, virtual_path: PurePosixPath) -> Path:
    if not virtual_path.is_absolute() or ".." in virtual_path.parts:
        fail(f"contract path is not a normalized absolute path: {virtual_path}")
    return root.joinpath(*virtual_path.parts[1:])


def require_regular_file(
    root: Path,
    virtual_path: PurePosixPath,
    label: str,
    *,
    executable: bool = False,
) -> Path:
    path = image_path(root, virtual_path)
    try:
        metadata = path.lstat()
    except OSError as exc:
        fail(f"{label} is missing: {virtual_path}: {exc}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a regular non-symlink file: {virtual_path}")
    if metadata.st_mode & 0o022:
        fail(f"{label} must not be writable by group or other: {virtual_path}")
    if executable and not metadata.st_mode & 0o111:
        fail(f"{label} is not executable: {virtual_path}")
    return path


def read_text(path: Path, label: str) -> str:
    try:
        size = path.stat().st_size
        if size > MAX_TEXT_BYTES:
            fail(f"{label} exceeds {MAX_TEXT_BYTES} bytes")
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        fail(f"{label} is not readable UTF-8: {exc}")


def reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def read_json(path: Path, label: str) -> dict[str, Any]:
    try:
        value = json.loads(
            read_text(path, label), object_pairs_hook=reject_duplicate_keys
        )
    except json.JSONDecodeError as exc:
        fail(f"{label} is malformed JSON: {exc}")
    if not isinstance(value, dict):
        fail(f"{label} root must be one JSON object")
    return value


def parse_unit(path: Path) -> dict[tuple[str, str], list[str]]:
    logical_lines: list[str] = []
    continued = ""
    for raw_line in read_text(path, "controller service unit").splitlines():
        line = raw_line.rstrip()
        if line.endswith("\\"):
            continued += line[:-1] + " "
            continue
        logical_lines.append(continued + line)
        continued = ""
    if continued:
        fail("controller service unit ends with an unterminated continuation")

    section = ""
    directives: dict[tuple[str, str], list[str]] = {}
    for raw_line in logical_lines:
        line = raw_line.strip()
        if not line or line.startswith(("#", ";")):
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip()
            continue
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        directives.setdefault((section, key.strip()), []).append(value.strip())
    return directives


def require_single_directive(
    directives: dict[tuple[str, str], list[str]], section: str, key: str
) -> str:
    values = directives.get((section, key), [])
    if len(values) != 1:
        fail(f"controller service must set [{section}] {key} exactly once")
    return values[0]


def validate_service_unit(path: Path) -> None:
    directives = parse_unit(path)
    if require_single_directive(directives, "Service", "User") != PROBE_ACCOUNT:
        fail(f"controller service User must be {PROBE_ACCOUNT}")
    if require_single_directive(directives, "Service", "Group") != PROBE_ACCOUNT:
        fail(f"controller service Group must be {PROBE_ACCOUNT}")

    command = require_single_directive(directives, "Service", "ExecStart")
    try:
        command_parts = shlex.split(command, posix=True)
    except ValueError as exc:
        fail(f"controller service ExecStart is malformed: {exc}")
    if command_parts != [str(CONTROLLER_BINARY)]:
        fail(f"controller service ExecStart must be exactly {CONTROLLER_BINARY}")

    deny_tokens = {
        token
        for value in directives.get(("Service", "IPAddressDeny"), [])
        for token in value.split()
    }
    if deny_tokens != {"any"}:
        fail("controller service must set IPAddressDeny=any")

    allow_tokens = {
        token
        for value in directives.get(("Service", "IPAddressAllow"), [])
        for token in value.split()
    }
    expected_allow_sets = (
        {"localhost", EXPECTED_HOST_NETWORK},
        {"127.0.0.0/8", "::1/128", EXPECTED_HOST_NETWORK},
    )
    if allow_tokens not in expected_allow_sets:
        fail(
            "controller service IPAddressAllow must contain only localhost "
            f"(literal or loopback CIDRs) and {EXPECTED_HOST_NETWORK}"
        )


def validate_controller_config(path: Path) -> None:
    config = read_json(path, "controller config")
    fields = frozenset(config)
    if fields != EXPECTED_CONFIG_FIELDS:
        missing = EXPECTED_CONFIG_FIELDS - fields
        extra = fields - EXPECTED_CONFIG_FIELDS
        if missing:
            fail(f"controller config is missing fields: {', '.join(sorted(missing))}")
        fail(f"controller config has unexpected fields: {', '.join(sorted(extra))}")
    expected_values: dict[str, Any] = {
        "schema_version": 1,
        "listen_address": "0.0.0.0",
        "listen_port": 38443,
        "allowed_host_address": EXPECTED_HOST_ADDRESS,
        "controller_secret_file": str(CONTROLLER_SECRET),
        "max_jobs": 4,
    }
    for field, expected in expected_values.items():
        if config[field] != expected or (
            isinstance(expected, int) and isinstance(config[field], bool)
        ):
            fail(f"controller config {field} does not match the image contract")


def validate_chromium_policy(path: Path) -> None:
    policy = read_json(path, "Chromium managed policy")
    if policy.get("AudioCaptureAllowed") is not False:
        fail(
            "Chromium managed policy must set AudioCaptureAllowed=false so "
            "non-allowlisted origins cannot prompt for microphone access"
        )
    if policy.get("AudioCaptureAllowedUrls") != [EXPECTED_BROWSER_ORIGIN]:
        fail(
            "Chromium AudioCaptureAllowedUrls must contain only "
            f"{EXPECTED_BROWSER_ORIGIN}"
        )


def validate_policy_directory(policy_path: Path) -> None:
    validate_chromium_policy(policy_path)
    for sibling in sorted(policy_path.parent.glob("*.json")):
        if sibling == policy_path:
            continue
        sibling_policy = read_json(sibling, f"Chromium managed policy {sibling.name}")
        conflicting = {
            "AudioCaptureAllowed", "AudioCaptureAllowedUrls"
        }.intersection(sibling_policy)
        if conflicting:
            fail(
                f"Chromium managed policy {sibling.name} also defines microphone "
                f"access: {', '.join(sorted(conflicting))}"
            )


def parse_colon_records(path: Path, label: str, field_count: int) -> list[list[str]]:
    records: list[list[str]] = []
    for number, line in enumerate(read_text(path, label).splitlines(), start=1):
        if not line:
            continue
        fields = line.split(":")
        if len(fields) != field_count:
            fail(f"{label} line {number} is malformed")
        records.append(fields)
    return records


def validate_probe_account(root: Path) -> tuple[int, int]:
    passwd_path = require_regular_file(
        root, PurePosixPath("/etc/passwd"), "passwd database"
    )
    group_path = require_regular_file(
        root, PurePosixPath("/etc/group"), "group database"
    )
    passwd = parse_colon_records(passwd_path, "passwd database", 7)
    groups = parse_colon_records(group_path, "group database", 4)
    matches = [entry for entry in passwd if entry[0] == PROBE_ACCOUNT]
    if len(matches) != 1:
        fail(f"image must contain exactly one {PROBE_ACCOUNT} account")
    account = matches[0]
    try:
        uid = int(account[2])
        gid = int(account[3])
    except ValueError:
        fail(f"{PROBE_ACCOUNT} account has a non-numeric UID or GID")
    if uid == 0 or gid == 0:
        fail(f"{PROBE_ACCOUNT} must be unprivileged")
    if account[6] not in {"/usr/sbin/nologin", "/sbin/nologin", "/bin/false"}:
        fail(f"{PROBE_ACCOUNT} must use a non-login shell")

    group_matches = [
        entry for entry in groups if entry[0] == PROBE_ACCOUNT and entry[2] == str(gid)
    ]
    if len(group_matches) != 1:
        fail(f"{PROBE_ACCOUNT} must have a dedicated primary group")

    browser_matches = [entry for entry in passwd if entry[0] == "mcnf-browser"]
    if browser_matches and browser_matches[0][2] == str(uid):
        fail(f"{PROBE_ACCOUNT} must not share the mcnf-browser UID")
    return uid, gid


def require_owner(path: Path, label: str, uid: int, gid: int) -> None:
    metadata = path.lstat()
    if metadata.st_uid != uid or metadata.st_gid != gid:
        fail(f"{label} has the wrong immutable owner")


def validate_secret_directory(
    root: Path, probe_gid: int, *, enforce_ownership: bool
) -> None:
    directory = image_path(root, CONTROLLER_SECRET_DIRECTORY)
    try:
        metadata = directory.lstat()
    except OSError as exc:
        fail(f"controller secret directory is missing: {exc}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        fail("controller secret directory must be a real directory")
    if enforce_ownership and (metadata.st_uid != 0 or metadata.st_gid != probe_gid):
        fail("controller secret directory must be owned by root and the probe group")
    if stat.S_IMODE(metadata.st_mode) != 0o750:
        fail("controller secret directory must have mode 0750")


def validate_enable_link(root: Path) -> None:
    link_path = image_path(root, CONTROLLER_ENABLE_LINK)
    if not link_path.is_symlink():
        fail(f"controller service is not enabled: {CONTROLLER_ENABLE_LINK}")
    try:
        target = os.readlink(link_path)
    except OSError as exc:
        fail(f"controller service enable link is unreadable: {exc}")
    if target.startswith("/"):
        virtual_target = posixpath.normpath(target)
    else:
        virtual_target = posixpath.normpath(
            posixpath.join(str(CONTROLLER_ENABLE_LINK.parent), target)
        )
    if virtual_target != str(CONTROLLER_SERVICE):
        fail(
            "controller service enable link does not target the immutable unit: "
            f"{target}"
        )


def validate_image_root(root: Path, *, enforce_ownership: bool) -> list[str]:
    if root.is_symlink() or not root.is_dir():
        fail("image root must be a real directory")
    root = root.resolve()

    binary_path = require_regular_file(
        root, CONTROLLER_BINARY, "guest audio probe controller", executable=True
    )
    service_path = require_regular_file(
        root, CONTROLLER_SERVICE, "guest audio probe controller service"
    )
    config_path = require_regular_file(root, CONTROLLER_CONFIG, "controller config")
    policy_path = require_regular_file(root, CHROMIUM_POLICY, "Chromium managed policy")
    validate_service_unit(service_path)
    validate_controller_config(config_path)
    validate_policy_directory(policy_path)
    probe_uid, probe_gid = validate_probe_account(root)
    validate_enable_link(root)
    validate_secret_directory(
        root, probe_gid, enforce_ownership=enforce_ownership
    )

    if enforce_ownership:
        require_owner(binary_path, "guest audio probe controller", 0, 0)
        require_owner(service_path, "guest audio probe controller service", 0, 0)
        require_owner(policy_path, "Chromium managed policy", 0, 0)
        require_owner(config_path, "controller config", probe_uid, probe_gid)

    secret_path = image_path(root, CONTROLLER_SECRET)
    if os.path.lexists(secret_path):
        fail(
            "controller shared secret is embedded in the immutable image; it must "
            f"be provisioned at runtime: {CONTROLLER_SECRET}"
        )

    return [
        f"controller executable present: {CONTROLLER_BINARY}",
        f"controller service present and enabled: {CONTROLLER_SERVICE}",
        f"dedicated service account present: {PROBE_ACCOUNT}",
        f"controller config matches bounded endpoint: {CONTROLLER_CONFIG}",
        f"Chromium microphone policy is loopback-only: {CHROMIUM_POLICY}",
        "controller service has kernel IP ingress restrictions",
        f"controller secret is absent from immutable image: {CONTROLLER_SECRET}",
    ]


def require_source_file(path: Path, label: str) -> Path:
    try:
        metadata = path.lstat()
    except OSError as exc:
        fail(f"{label} is missing: {path}: {exc}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a regular non-symlink file: {path}")
    if metadata.st_mode & 0o022:
        fail(f"{label} must not be writable by group or other: {path}")
    return path


def validate_source_assets(
    service_unit: Path, controller_config: Path, chromium_policy: Path
) -> list[str]:
    service_unit = require_source_file(service_unit, "controller service source")
    controller_config = require_source_file(
        controller_config, "controller config source"
    )
    chromium_policy = require_source_file(
        chromium_policy, "Chromium managed-policy source"
    )
    validate_service_unit(service_unit)
    validate_controller_config(controller_config)
    validate_policy_directory(chromium_policy)
    return [
        f"controller service source is bounded: {service_unit}",
        f"controller config source is bounded: {controller_config}",
        f"Chromium microphone policy source is loopback-only: {chromium_policy}",
    ]


VALID_UNIT = f"""[Unit]
Description=test controller

[Service]
User={PROBE_ACCOUNT}
Group={PROBE_ACCOUNT}
ExecStart={CONTROLLER_BINARY}
IPAddressDeny=any
IPAddressAllow=localhost
IPAddressAllow={EXPECTED_HOST_NETWORK}

[Install]
WantedBy=multi-user.target
"""


def write_fixture(root: Path) -> None:
    def write(virtual_path: PurePosixPath, value: str, mode: int = 0o644) -> Path:
        path = image_path(root, virtual_path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value, encoding="utf-8")
        path.chmod(mode)
        return path

    write(CONTROLLER_BINARY, "test binary\n", 0o755)
    write(CONTROLLER_SERVICE, VALID_UNIT)
    write(
        CONTROLLER_CONFIG,
        json.dumps(
            {
                "schema_version": 1,
                "listen_address": "0.0.0.0",
                "listen_port": 38443,
                "allowed_host_address": EXPECTED_HOST_ADDRESS,
                "controller_secret_file": str(CONTROLLER_SECRET),
                "max_jobs": 4,
            }
        ),
    )
    write(
        CHROMIUM_POLICY,
        json.dumps(
            {
                "AudioCaptureAllowed": False,
                "AudioCaptureAllowedUrls": [EXPECTED_BROWSER_ORIGIN],
            }
        ),
    )
    write(
        PurePosixPath("/etc/passwd"),
        "root:x:0:0:root:/root:/bin/bash\n"
        "mcnf-browser:x:1000:1000::/var/lib/mcnf-browser:/bin/bash\n"
        f"{PROBE_ACCOUNT}:x:991:991::/nonexistent:/usr/sbin/nologin\n",
    )
    write(
        PurePosixPath("/etc/group"),
        "root:x:0:\n"
        "mcnf-browser:x:1000:\n"
        f"{PROBE_ACCOUNT}:x:991:\n",
    )
    secret_directory = image_path(root, CONTROLLER_SECRET_DIRECTORY)
    secret_directory.mkdir(parents=True, exist_ok=True)
    secret_directory.chmod(0o750)
    enable_link = image_path(root, CONTROLLER_ENABLE_LINK)
    enable_link.parent.mkdir(parents=True, exist_ok=True)
    enable_link.symlink_to(str(CONTROLLER_SERVICE))


def rewrite(path: Path, old: str, new: str) -> None:
    value = path.read_text(encoding="utf-8")
    if old not in value:
        raise AssertionError(f"self-test fixture does not contain {old!r}")
    path.write_text(value.replace(old, new), encoding="utf-8")


def expect_rejected(
    parent: Path, label: str, mutate: Callable[[Path], None]
) -> None:
    root = parent / label
    write_fixture(root)
    mutate(root)
    try:
        validate_image_root(root, enforce_ownership=False)
    except ContractError:
        return
    raise AssertionError(f"self-test accepted invalid fixture: {label}")


def run_self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="mcnf-browser-control-contract-") as temp:
        parent = Path(temp)
        valid = parent / "valid"
        write_fixture(valid)
        validate_image_root(valid, enforce_ownership=False)

        explicit_loopback = parent / "valid-explicit-loopback"
        write_fixture(explicit_loopback)
        rewrite(
            image_path(explicit_loopback, CONTROLLER_SERVICE),
            "IPAddressAllow=localhost",
            "IPAddressAllow=127.0.0.0/8\nIPAddressAllow=::1/128",
        )
        validate_image_root(explicit_loopback, enforce_ownership=False)

        expect_rejected(
            parent,
            "controller-not-executable",
            lambda root: image_path(root, CONTROLLER_BINARY).chmod(0o644),
        )
        expect_rejected(
            parent,
            "wrong-service-user",
            lambda root: rewrite(
                image_path(root, CONTROLLER_SERVICE),
                f"User={PROBE_ACCOUNT}",
                "User=mcnf-browser",
            ),
        )
        expect_rejected(
            parent,
            "broad-service-network",
            lambda root: rewrite(
                image_path(root, CONTROLLER_SERVICE),
                f"IPAddressAllow={EXPECTED_HOST_NETWORK}",
                "IPAddressAllow=0.0.0.0/0",
            ),
        )
        expect_rejected(
            parent,
            "global-microphone-access",
            lambda root: rewrite(
                image_path(root, CHROMIUM_POLICY), "false", "true"
            ),
        )
        expect_rejected(
            parent,
            "extra-microphone-origin",
            lambda root: rewrite(
                image_path(root, CHROMIUM_POLICY),
                f'"{EXPECTED_BROWSER_ORIGIN}"',
                f'"{EXPECTED_BROWSER_ORIGIN}", "https://example.invalid/*"',
            ),
        )

        def add_conflicting_policy(root: Path) -> None:
            conflict = image_path(root, CHROMIUM_POLICY).with_name("conflict.json")
            conflict.write_text(
                json.dumps({"AudioCaptureAllowed": True}), encoding="utf-8"
            )

        expect_rejected(
            parent, "conflicting-managed-policy", add_conflicting_policy
        )

        def add_secret(root: Path) -> None:
            secret = image_path(root, CONTROLLER_SECRET)
            secret.parent.mkdir(parents=True, exist_ok=True)
            secret.write_text("not-a-real-secret\n", encoding="utf-8")

        expect_rejected(parent, "embedded-secret", add_secret)
        expect_rejected(
            parent,
            "writable-secret-directory",
            lambda root: image_path(root, CONTROLLER_SECRET_DIRECTORY).chmod(0o770),
        )
        expect_rejected(
            parent,
            "missing-service-enable-link",
            lambda root: image_path(root, CONTROLLER_ENABLE_LINK).unlink(),
        )
        expect_rejected(
            parent,
            "missing-dedicated-account",
            lambda root: rewrite(
                image_path(root, PurePosixPath("/etc/passwd")),
                f"{PROBE_ACCOUNT}:x:991:991::/nonexistent:/usr/sbin/nologin\n",
                "",
            ),
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--image-root", type=Path)
    mode.add_argument("--self-test", action="store_true")
    mode.add_argument("--source-assets", action="store_true")
    parser.add_argument("--service-unit", type=Path)
    parser.add_argument("--controller-config", type=Path)
    parser.add_argument("--chromium-policy", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.self_test:
            run_self_test()
            print("Browser VM production-control image self-tests passed")
            return 0
        if args.source_assets:
            if not all(
                (args.service_unit, args.controller_config, args.chromium_policy)
            ):
                fail(
                    "--source-assets requires --service-unit, "
                    "--controller-config, and --chromium-policy"
                )
            messages = validate_source_assets(
                args.service_unit, args.controller_config, args.chromium_policy
            )
            success = "Browser VM production-control source contract passed"
        else:
            if any((args.service_unit, args.controller_config, args.chromium_policy)):
                fail("source-asset paths require --source-assets")
            messages = validate_image_root(args.image_root, enforce_ownership=True)
            success = "Browser VM production-control image contract passed"
        for message in messages:
            print(f"  OK   {message}")
        print(success)
        return 0
    except (ContractError, AssertionError) as exc:
        print(f"verify-browser-vm-production-control: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
