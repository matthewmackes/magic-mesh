#!/usr/bin/python3
"""Fail-closed local-console preflight for Surface SSH access recovery."""

from __future__ import annotations

import argparse
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Callable


SCHEMA_VERSION = 1
MAX_EVIDENCE_BYTES = 8192
COMMAND_TIMEOUT_SECONDS = 5
APPROVED_KEY = Path("/usr/share/magic-mesh/recovery/surface-root-ed25519.pub")
APPROVED_FINGERPRINT = Path(
    "/usr/share/magic-mesh/recovery/surface-root-ed25519.sha256"
)
PRO5_SKUS = {"Surface_Pro_1796", "Surface_Pro_1807"}
FINGERPRINT = re.compile(r"^SHA256:[A-Za-z0-9+/]{43}$")
PHYSICAL_CONSOLE = re.compile(r"^/dev/tty[1-9][0-9]*$")


def read_bounded(path: Path, maximum: int) -> bytes:
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
        raise ValueError("not-regular")
    if metadata.st_size <= 0 or metadata.st_size > maximum:
        raise ValueError("invalid-size")
    with path.open("rb") as stream:
        value = stream.read(maximum + 1)
    if len(value) > maximum:
        raise ValueError("invalid-size")
    return value


def file_gate(path: Path, modes: set[int], maximum: int) -> tuple[bool, str]:
    try:
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
            return False, "not-regular"
        if metadata.st_uid != 0 or metadata.st_gid != 0:
            return False, "not-root-owned"
        if stat.S_IMODE(metadata.st_mode) not in modes:
            return False, "unsafe-mode"
        if metadata.st_nlink != 1:
            return False, "unexpected-link-count"
        if not 0 < metadata.st_size <= maximum:
            return False, "invalid-size"
        return True, "ok"
    except FileNotFoundError:
        return False, "missing"
    except OSError:
        return False, "unreadable"


def directory_gate(path: Path, modes: set[int]) -> tuple[bool, str]:
    try:
        metadata = path.lstat()
        if not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
            return False, "not-directory"
        if metadata.st_uid != 0 or metadata.st_gid != 0:
            return False, "not-root-owned"
        if stat.S_IMODE(metadata.st_mode) not in modes:
            return False, "unsafe-mode"
        return True, "ok"
    except FileNotFoundError:
        return False, "missing"
    except OSError:
        return False, "unreadable"


def parse_os_release(raw: bytes) -> dict[str, str]:
    result: dict[str, str] = {}
    for line in raw.decode("utf-8", "strict").splitlines():
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        if not re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
            continue
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
            value = value[1:-1]
        result[key] = value
    return result


def read_os_release(root: Path) -> bytes:
    path = root / "etc/os-release"
    if path.is_symlink():
        link = os.readlink(path)
        if link not in ("../usr/lib/os-release", "/usr/lib/os-release"):
            raise ValueError("unexpected-os-release-link")
        path = root / "usr/lib/os-release"
    return read_bounded(path, 4096)


def read_sysfs_bounded(path: Path, maximum: int) -> bytes:
    """Read a sysfs attribute whose reported stat size is conventionally 4096."""
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
        raise ValueError("not-regular")
    with path.open("rb", buffering=0) as stream:
        value = stream.read(maximum + 1)
    if not value or len(value) > maximum:
        raise ValueError("invalid-size")
    return value


def exact_identity(root: Path, generation: int) -> tuple[bool, str]:
    try:
        base = root / "sys/class/dmi/id"
        vendor = read_sysfs_bounded(base / "sys_vendor", 128).decode().strip()
        product = read_sysfs_bounded(base / "product_name", 128).decode().strip()
        sku = read_sysfs_bounded(base / "product_sku", 128).decode().strip()
    except (OSError, UnicodeError, ValueError):
        return False, "dmi-unavailable"
    if vendor != "Microsoft Corporation":
        return False, "manufacturer-mismatch"
    if generation == 6:
        return (product == "Surface Pro 6", "ok" if product == "Surface Pro 6" else "model-mismatch")
    valid = product == "Surface Pro" and sku in PRO5_SKUS
    return valid, "ok" if valid else "model-or-sku-mismatch"


def validate_key_material(root: Path, runner: Callable[..., subprocess.CompletedProcess[str]]) -> tuple[bool, str]:
    key = root / APPROVED_KEY.relative_to("/")
    fingerprint_path = root / APPROVED_FINGERPRINT.relative_to("/")
    key_ok, key_reason = file_gate(key, {0o600, 0o644}, 1024)
    if not key_ok:
        return False, f"approved-key-{key_reason}"
    fingerprint_ok, fingerprint_reason = file_gate(
        fingerprint_path, {0o600, 0o644}, 128
    )
    if not fingerprint_ok:
        return False, f"approved-fingerprint-{fingerprint_reason}"
    try:
        key_line = read_bounded(key, 1024).decode("ascii").strip()
        expected = read_bounded(fingerprint_path, 128).decode("ascii").strip()
    except (OSError, UnicodeError, ValueError):
        return False, "approved-material-unreadable"
    fields = key_line.split()
    if len(fields) not in (2, 3) or fields[0] != "ssh-ed25519":
        return False, "approved-key-format"
    if not FINGERPRINT.fullmatch(expected):
        return False, "approved-fingerprint-format"
    try:
        result = runner(
            ["/usr/bin/ssh-keygen", "-lf", str(key), "-E", "sha256"],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=COMMAND_TIMEOUT_SECONDS,
            env={"PATH": "/usr/bin:/usr/sbin", "LANG": "C", "LC_ALL": "C"},
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False, "fingerprint-command-failed"
    if result.returncode != 0 or len(result.stdout) > 512:
        return False, "fingerprint-command-failed"
    columns = result.stdout.split()
    if len(columns) < 2 or columns[1] != expected:
        return False, "fingerprint-mismatch"
    return True, "ok"


def effective_sshd(runner: Callable[..., subprocess.CompletedProcess[str]]) -> tuple[bool, str]:
    try:
        result = runner(
            ["/usr/sbin/sshd", "-T", "-C", "user=root,host=Surface,addr=127.0.0.1"],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=COMMAND_TIMEOUT_SECONDS,
            env={"PATH": "/usr/bin:/usr/sbin", "LANG": "C", "LC_ALL": "C"},
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False, "sshd-query-failed"
    if result.returncode != 0 or len(result.stdout) > 64 * 1024:
        return False, "sshd-query-failed"
    values: dict[str, list[str]] = {}
    for line in result.stdout.splitlines():
        key, separator, value = line.partition(" ")
        if separator:
            values.setdefault(key.lower(), []).append(value.strip())
    if values.get("pubkeyauthentication") != ["yes"]:
        return False, "pubkey-authentication-disabled"
    if values.get("permitrootlogin") not in (["yes"], ["prohibit-password"], ["without-password"]):
        return False, "root-public-key-login-disabled"
    authorized = " ".join(values.get("authorizedkeysfile", []))
    if ".ssh/authorized_keys" not in authorized.split():
        return False, "authorized-keys-path-mismatch"
    methods = values.get("authenticationmethods", ["any"])
    if methods != ["any"] and not any("publickey" == item for item in " ".join(methods).split()):
        return False, "public-key-alone-not-admitted"
    return True, "ok"


def inspect(
    root: Path,
    generation: int,
    tty_path: str,
    uid: int,
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> dict[str, object]:
    gates: dict[str, dict[str, object]] = {}

    def record(name: str, result: tuple[bool, str]) -> None:
        gates[name] = {"pass": result[0], "reason": result[1]}

    record("physical_console", (bool(PHYSICAL_CONSOLE.fullmatch(tty_path)), "ok" if PHYSICAL_CONSOLE.fullmatch(tty_path) else "not-linux-virtual-console"))
    record("root_operator", (uid == 0, "ok" if uid == 0 else "root-required"))
    record("hardware_identity", exact_identity(root, generation))
    try:
        release = parse_os_release(read_os_release(root))
        fedora = release.get("ID") == "fedora" and release.get("VERSION_ID") == "44"
        record("fedora_44", (fedora, "ok" if fedora else "os-release-mismatch"))
    except (OSError, UnicodeError, ValueError):
        record("fedora_44", (False, "os-release-unavailable"))
    try:
        hostname = read_bounded(root / "etc/hostname", 256).decode("utf-8", "strict").strip()
        canonical = generation != 6 or hostname == "Surface"
        record("canonical_hostname", (canonical, "ok" if canonical else "hostname-mismatch"))
    except (OSError, UnicodeError, ValueError):
        record("canonical_hostname", (False, "hostname-unavailable"))
    record("root_home", directory_gate(root / "root", {0o500, 0o550, 0o700, 0o750}))
    record("root_ssh_directory", directory_gate(root / "root/.ssh", {0o700}))
    record("root_authorized_keys", file_gate(root / "root/.ssh/authorized_keys", {0o600}, 256 * 1024))
    record("approved_key_authority", validate_key_material(root, runner))
    record("effective_sshd_policy", effective_sshd(runner))
    passed = all(bool(gate["pass"]) for gate in gates.values())
    # Even a fully passing preview cannot authorize a write: repository review found
    # no tracked approved key/fingerprint pair and intentionally ships no committer.
    return {
        "schema_version": SCHEMA_VERSION,
        "operation": "surface-ssh-console-recovery-preflight",
        "mode": "preview",
        "generation": generation,
        "status": "blocked",
        "all_environment_gates_pass": passed,
        "gates": gates,
        "blocker": "no-repository-approved-key-and-fingerprint; commit-path-not-implemented",
        "mutations": 0,
    }


def emit(value: dict[str, object]) -> None:
    encoded = (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()
    if len(encoded) > MAX_EVIDENCE_BYTES:
        raise RuntimeError("evidence-exceeds-bound")
    sys.stdout.buffer.write(encoded)


def self_test() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        for path, value in (
            ("sys/class/dmi/id/sys_vendor", "Microsoft Corporation\n"),
            ("sys/class/dmi/id/product_name", "Surface Pro 6\n"),
            ("sys/class/dmi/id/product_sku", "Surface_Pro_6\n"),
            ("etc/os-release", 'ID=fedora\nVERSION_ID="44"\n'),
            ("etc/hostname", "Surface\n"),
        ):
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(value)
        (root / "root").mkdir(mode=0o700)
        (root / "root/.ssh").mkdir(mode=0o700)
        (root / "root/.ssh/authorized_keys").write_text("placeholder\n")
        os.chmod(root / "root/.ssh/authorized_keys", 0o600)

        def fake_runner(argv: list[str], **_kwargs: object) -> subprocess.CompletedProcess[str]:
            if argv[0].endswith("sshd"):
                return subprocess.CompletedProcess(argv, 0, "pubkeyauthentication yes\npermitrootlogin prohibit-password\nauthorizedkeysfile .ssh/authorized_keys\nauthenticationmethods any\n", "")
            raise AssertionError("ssh-keygen must not run without the authority files")

        value = inspect(root, 6, "/dev/tty1", 0, fake_runner)
        assert value["status"] == "blocked" and value["mutations"] == 0
        gates = value["gates"]
        assert isinstance(gates, dict)
        assert gates["hardware_identity"]["pass"] is True
        assert gates["approved_key_authority"]["reason"] == "approved-key-missing"
        assert len(json.dumps(value).encode()) < MAX_EVIDENCE_BYTES
        assert exact_identity(root, 5) == (False, "model-or-sku-mismatch")
        (root / "sys/class/dmi/id/product_name").write_text("Surface Pro\n")
        (root / "sys/class/dmi/id/product_sku").write_text("Surface_Pro_1796\n")
        assert exact_identity(root, 5) == (True, "ok")
        os.chmod(root / "root/.ssh/authorized_keys", 0o644)
        hostile = inspect(root, 5, "/dev/pts/9", 1000, fake_runner)
        hostile_gates = hostile["gates"]
        assert isinstance(hostile_gates, dict)
        assert hostile_gates["physical_console"]["pass"] is False
        assert hostile_gates["root_operator"]["pass"] is False
        # Non-root farm fixtures also exercise the ownership rejection before
        # the deliberately loose mode can be reached.
        assert hostile_gates["root_authorized_keys"]["pass"] is False
        assert hostile_gates["root_authorized_keys"]["reason"] in {
            "not-root-owned",
            "unsafe-mode",
        }
    print("preflight-surface-ssh-console-recovery: self-test passed")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--preview", action="store_true", help="run the read-only preflight (default)")
    mode.add_argument("--commit", action="store_true", help="request commit; always fails closed until authority is packaged")
    parser.add_argument("--generation", type=int, choices=(5, 6), default=6)
    parser.add_argument("--self-test", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    if args.commit:
        emit({
            "schema_version": SCHEMA_VERSION,
            "operation": "surface-ssh-console-recovery-preflight",
            "mode": "commit",
            "generation": args.generation,
            "status": "blocked",
            "blocker": "no-repository-approved-key-and-fingerprint; commit-path-not-implemented",
            "mutations": 0,
        })
        return 3
    try:
        tty_path = os.ttyname(sys.stdin.fileno()) if sys.stdin.isatty() else "not-a-tty"
    except OSError:
        tty_path = "not-a-tty"
    emit(inspect(Path("/"), args.generation, tty_path, os.geteuid()))
    return 3


if __name__ == "__main__":
    raise SystemExit(main())
