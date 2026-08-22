#!/usr/bin/env python3
"""Hostile self-test for provision-bootstrap-ssh-identity.py.

No network. No live SSH. Tests must use temp dirs. Sidecar must land
outside the helper git worktree. Does not set MACKESD_BOOTSTRAP_SSH_KEY
or MACKESD_BOOTSTRAP_KNOWN_HOSTS in this process.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
HELPER = HERE / "provision-bootstrap-ssh-identity.py"


def resolve_repo() -> Path:
    result = subprocess.run(
        ["git", "-C", str(HERE), "rev-parse", "--show-toplevel"],
        check=False,
        capture_output=True,
        text=True,
    )
    root = result.stdout.strip()
    if result.returncode == 0 and root:
        return Path(root).resolve()
    return HERE.parent.resolve()


REPO = resolve_repo()
FORBIDDEN_ENV = ("MACKESD_BOOTSTRAP_SSH_KEY", "MACKESD_BOOTSTRAP_KNOWN_HOSTS")
KEY_MARKERS = (b"BEGIN OPENSSH PRIVATE KEY", b"BEGIN RSA PRIVATE KEY", b"BEGIN EC PRIVATE KEY")


def inside_repo(path: Path) -> bool:
    try:
        path.resolve().relative_to(REPO)
    except ValueError:
        return False
    return True


def command(*args: str, ok: bool = True) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    for name in FORBIDDEN_ENV:
        env.pop(name, None)
    result = subprocess.run(
        [sys.executable, str(HELPER), *args],
        text=True,
        capture_output=True,
        env=env,
    )
    if ok and result.returncode != 0:
        raise AssertionError(result.stderr or result.stdout)
    if not ok and result.returncode == 0:
        raise AssertionError(f"unexpected admission: {' '.join(args)}")
    if not ok:
        assert result.returncode == 2, result.stderr
        assert result.stdout == "", result.stdout
        assert "REFUSED:" in result.stderr
        for marker in (b"BEGIN OPENSSH", b"BEGIN RSA", b"BEGIN EC"):
            assert marker not in result.stderr.encode("utf-8", "replace")
    return result


def write_source_pair(root: Path) -> tuple[Path, Path]:
    key = root / "id_ed25519"
    hosts = root / "known_hosts"
    generated = subprocess.run(
        ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)],
        check=False,
        capture_output=True,
        text=True,
    )
    if generated.returncode != 0:
        raise AssertionError(generated.stderr or "ssh-keygen failed")
    os.chmod(key, 0o600)
    hosts.write_text("172.20.0.99 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixtureHostKeyDoNotUse\n")
    os.chmod(hosts, 0o600)
    return key, hosts


def dest_args(parent: Path) -> list[str]:
    return [
        "--dest-parent",
        str(parent),
        "--dest-key",
        str(parent / "bootstrap-ssh-key"),
        "--dest-known-hosts",
        str(parent / "bootstrap-known-hosts"),
        "--sidecar",
        str(parent / "bootstrap-ssh-identity.json"),
    ]


def main() -> None:
    for name in FORBIDDEN_ENV:
        assert name not in os.environ, f"{name} must stay unset in the test process"

    with tempfile.TemporaryDirectory(prefix="mcnf-bootstrap-ssh-test-") as temporary:
        root = Path(temporary)
        assert not inside_repo(root)
        source_root = root / "src"
        dest_root = root / "dest"
        source_root.mkdir()
        dest_root.mkdir()
        source_key, source_hosts = write_source_pair(source_root)
        base = ["--source-key", str(source_key), "--source-known-hosts", str(source_hosts)]

        linked_key = root / "linked-key"
        linked_key.symlink_to(source_key)
        command(
            "--source-key",
            str(linked_key),
            "--source-known-hosts",
            str(source_hosts),
            *dest_args(dest_root),
            ok=False,
        )

        linked_hosts = root / "linked-hosts"
        linked_hosts.symlink_to(source_hosts)
        command(
            "--source-key",
            str(source_key),
            "--source-known-hosts",
            str(linked_hosts),
            *dest_args(dest_root),
            ok=False,
        )

        dest_symlink_parent = root / "dest-symlink"
        dest_symlink_parent.mkdir()
        dest_key_link = dest_symlink_parent / "bootstrap-ssh-key"
        dest_key_link.symlink_to(source_key)
        command(
            *base,
            "--dest-parent",
            str(dest_symlink_parent),
            "--dest-key",
            str(dest_key_link),
            "--dest-known-hosts",
            str(dest_symlink_parent / "bootstrap-known-hosts"),
            "--sidecar",
            str(dest_symlink_parent / "bootstrap-ssh-identity.json"),
            ok=False,
        )

        dest_exists_parent = root / "dest-exists"
        dest_exists_parent.mkdir()
        existing = dest_exists_parent / "bootstrap-ssh-key"
        existing.write_bytes(b"already-here")
        os.chmod(existing, 0o600)
        command(*base, *dest_args(dest_exists_parent), ok=False)
        assert existing.read_bytes() == b"already-here"

        empty_parent = root / "empty-src"
        empty_dest = root / "empty-dest"
        empty_parent.mkdir()
        empty_dest.mkdir()
        empty_key = empty_parent / "empty-key"
        empty_key.write_bytes(b"")
        os.chmod(empty_key, 0o600)
        command(
            "--source-key",
            str(empty_key),
            "--source-known-hosts",
            str(source_hosts),
            *dest_args(empty_dest),
            ok=False,
        )

        inside_parent = REPO / "install-helpers" / f".qu0014bs-dest-{os.getpid()}"
        try:
            inside_parent.mkdir()
            command(*base, *dest_args(inside_parent), ok=False)
            assert not (inside_parent / "bootstrap-ssh-key").exists()
            assert not (inside_parent / "bootstrap-ssh-identity.json").exists()
        finally:
            shutil.rmtree(inside_parent, ignore_errors=True)

        happy = dest_root
        result = command(*base, *dest_args(happy))
        dest_key = happy / "bootstrap-ssh-key"
        dest_hosts = happy / "bootstrap-known-hosts"
        sidecar = happy / "bootstrap-ssh-identity.json"
        assert dest_key.is_file() and not dest_key.is_symlink()
        assert dest_hosts.is_file() and not dest_hosts.is_symlink()
        assert sidecar.is_file() and not sidecar.is_symlink()
        assert stat.S_IMODE(dest_key.stat().st_mode) == 0o600
        assert stat.S_IMODE(dest_hosts.stat().st_mode) == 0o400
        assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
        assert dest_key.stat().st_nlink == 1
        assert dest_hosts.stat().st_nlink == 1
        assert not inside_repo(dest_key)
        assert not inside_repo(dest_hosts)
        assert not inside_repo(sidecar)
        record = json.loads(result.stdout)
        stored = json.loads(sidecar.read_text(encoding="ascii"))
        assert record == stored
        assert record["kind"] == "mcnf-bootstrap-ssh-identity"
        assert record["schema_version"] == 1
        assert record["enroll_succeeded"] is False
        assert record["production_admitted"] is False
        assert record["dest_key"]["path"] == str(dest_key)
        assert record["dest_key"]["mode"] == "0600"
        assert record["dest_known_hosts"]["path"] == str(dest_hosts)
        assert record["dest_known_hosts"]["mode"] == "0400"
        assert record["sidecar_path"] == str(sidecar)
        key_bytes = dest_key.read_bytes()
        hosts_bytes = dest_hosts.read_bytes()
        assert key_bytes == source_key.read_bytes()
        assert hosts_bytes == source_hosts.read_bytes()
        assert record["dest_key"]["sha256"] == hashlib.sha256(key_bytes).hexdigest()
        assert record["dest_known_hosts"]["sha256"] == hashlib.sha256(hosts_bytes).hexdigest()
        stdout_bytes = result.stdout.encode("ascii")
        for marker in KEY_MARKERS:
            assert marker not in stdout_bytes
            assert marker not in sidecar.read_bytes()
        accepted = subprocess.run(
            ["ssh-keygen", "-y", "-f", str(dest_key)],
            check=False,
            capture_output=True,
            text=True,
        )
        assert accepted.returncode == 0, accepted.stderr
        assert accepted.stdout.startswith("ssh-ed25519 ")
        command(*base, *dest_args(happy), ok=False)

    for name in FORBIDDEN_ENV:
        assert name not in os.environ, f"{name} leaked into the test process"


if __name__ == "__main__":
    main()
    print("PASS")
