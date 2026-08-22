#!/usr/bin/env python3
"""Hostile self-test for bind-bootstrap-ssh-env.py.

No network. No live SSH. Tests must use temp dirs. Env file and sidecar
must land outside the helper git worktree. Does not set
MACKESD_BOOTSTRAP_SSH_KEY or MACKESD_BOOTSTRAP_KNOWN_HOSTS in this process.
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
HELPER = HERE / "bind-bootstrap-ssh-env.py"


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
FIXTURE_KEY = b"fixture-identity-not-for-login\n"
FIXTURE_HOSTS = b"172.20.0.99 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixtureHostKeyDoNotUse\n"


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
        assert FIXTURE_KEY not in result.stderr.encode("utf-8", "replace")
    return result


def write_dest_pair(root: Path) -> tuple[Path, Path]:
    key = root / "bootstrap-ssh-key"
    hosts = root / "bootstrap-known-hosts"
    key.write_bytes(FIXTURE_KEY)
    hosts.write_bytes(FIXTURE_HOSTS)
    os.chmod(key, 0o600)
    os.chmod(hosts, 0o400)
    return key, hosts


def dest_args(parent: Path, key: Path, hosts: Path) -> list[str]:
    return [
        "--dest-parent",
        str(parent),
        "--dest-key",
        str(key),
        "--dest-known-hosts",
        str(hosts),
        "--env-file",
        str(parent / "bootstrap-ssh.env"),
        "--sidecar",
        str(parent / "bootstrap-ssh-env.json"),
    ]


def expected_body(key: Path, hosts: Path) -> str:
    return (
        f"MACKESD_BOOTSTRAP_SSH_KEY={key}\n"
        f"MACKESD_BOOTSTRAP_KNOWN_HOSTS={hosts}\n"
    )


def main() -> None:
    for name in FORBIDDEN_ENV:
        assert name not in os.environ, f"{name} must stay unset in the test process"

    with tempfile.TemporaryDirectory(prefix="mcnf-bootstrap-ssh-env-test-") as temporary:
        root = Path(temporary)
        assert not inside_repo(root)
        dest_root = root / "dest"
        dest_root.mkdir()
        dest_key, dest_hosts = write_dest_pair(dest_root)
        base = dest_args(dest_root, dest_key, dest_hosts)

        linked_key = root / "linked-key"
        linked_key.symlink_to(dest_key)
        command(
            *dest_args(dest_root, linked_key, dest_hosts),
            ok=False,
        )

        linked_hosts = root / "linked-hosts"
        linked_hosts.symlink_to(dest_hosts)
        command(
            *dest_args(dest_root, dest_key, linked_hosts),
            ok=False,
        )

        dest_exists_parent = root / "dest-exists"
        dest_exists_parent.mkdir()
        exists_key, exists_hosts = write_dest_pair(dest_exists_parent)
        existing = dest_exists_parent / "bootstrap-ssh.env"
        existing.write_text("already-here\n", encoding="ascii")
        os.chmod(existing, 0o400)
        command(*dest_args(dest_exists_parent, exists_key, exists_hosts), ok=False)
        assert existing.read_text(encoding="ascii") == "already-here\n"
        assert not (dest_exists_parent / "bootstrap-ssh-env.json").exists()

        empty_parent = root / "empty-src"
        empty_parent.mkdir()
        empty_key = empty_parent / "empty-key"
        empty_key.write_bytes(b"")
        os.chmod(empty_key, 0o600)
        command(*dest_args(empty_parent, empty_key, dest_hosts), ok=False)

        missing_parent = root / "missing"
        missing_parent.mkdir()
        command(
            *dest_args(missing_parent, missing_parent / "no-key", dest_hosts),
            ok=False,
        )

        inside_parent = REPO / "install-helpers" / f".qu0018be-env-{os.getpid()}"
        try:
            inside_parent.mkdir()
            command(*dest_args(inside_parent, dest_key, dest_hosts), ok=False)
            assert not (inside_parent / "bootstrap-ssh.env").exists()
            assert not (inside_parent / "bootstrap-ssh-env.json").exists()
        finally:
            shutil.rmtree(inside_parent, ignore_errors=True)

        result = command(*base)
        env_file = dest_root / "bootstrap-ssh.env"
        sidecar = dest_root / "bootstrap-ssh-env.json"
        assert env_file.is_file() and not env_file.is_symlink()
        assert sidecar.is_file() and not sidecar.is_symlink()
        assert stat.S_IMODE(env_file.stat().st_mode) == 0o400
        assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
        assert env_file.stat().st_nlink == 1
        assert not inside_repo(env_file)
        assert not inside_repo(sidecar)
        body = env_file.read_text(encoding="ascii")
        assert body == expected_body(dest_key.resolve(), dest_hosts.resolve())
        assert body.count("=") == 2
        assert body.splitlines() == [
            f"MACKESD_BOOTSTRAP_SSH_KEY={dest_key.resolve()}",
            f"MACKESD_BOOTSTRAP_KNOWN_HOSTS={dest_hosts.resolve()}",
        ]
        record = json.loads(result.stdout)
        stored = json.loads(sidecar.read_text(encoding="ascii"))
        assert record == stored
        assert record["kind"] == "mcnf-bootstrap-ssh-env"
        assert record["schema_version"] == 1
        assert record["enroll_succeeded"] is False
        assert record["production_admitted"] is False
        assert record["env_file"]["path"] == str(env_file)
        assert record["env_file"]["mode"] == "0400"
        assert record["env_file"]["bytes"] == len(body.encode("ascii"))
        assert record["env_file"]["sha256"] == hashlib.sha256(body.encode("ascii")).hexdigest()
        assert record["sidecar_path"] == str(sidecar)
        assert "dest_key" not in record
        assert "dest_known_hosts" not in record
        stdout_bytes = result.stdout.encode("ascii")
        sidecar_bytes = sidecar.read_bytes()
        for marker in KEY_MARKERS:
            assert marker not in stdout_bytes
            assert marker not in sidecar_bytes
        assert FIXTURE_KEY not in stdout_bytes
        assert FIXTURE_KEY not in sidecar_bytes
        assert dest_key.read_bytes() == FIXTURE_KEY
        assert dest_hosts.read_bytes() == FIXTURE_HOSTS
        command(*base, ok=False)

    for name in FORBIDDEN_ENV:
        assert name not in os.environ, f"{name} leaked into the test process"


if __name__ == "__main__":
    main()
    print("PASS")
