#!/usr/bin/env python3
"""Hostile self-test for run-with-bootstrap-ssh-env.py.

No network. No live SSH. Tests must use temp dirs outside the helper
git worktree and must never touch /root/mcnf-private/. Does not set
MACKESD_BOOTSTRAP_SSH_KEY or MACKESD_BOOTSTRAP_KNOWN_HOSTS in this process.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
HELPER = HERE / "run-with-bootstrap-ssh-env.py"
PRODUCTION = Path("/root/mcnf-private")


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
FIXTURE_KEY = b"-----BEGIN OPENSSH PRIVATE KEY-----\nfixture-identity-not-for-login\n-----END OPENSSH PRIVATE KEY-----\n"
FIXTURE_HOSTS = b"172.20.0.99 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixtureHostKeyDoNotUse\n"


def inside_repo(path: Path) -> bool:
    try:
        path.resolve().relative_to(REPO)
    except ValueError:
        return False
    return True


def assert_away_from_production(*paths: Path) -> None:
    production = str(PRODUCTION)
    for path in paths:
        text = str(path)
        assert not text.startswith(production), path
        try:
            resolved = str(path.resolve())
        except OSError:
            continue
        assert not resolved.startswith(production), path


def load_helper():
    spec = importlib.util.spec_from_file_location("run_with_bootstrap_ssh_env", HELPER)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def env_body(key: Path, hosts: Path) -> str:
    return (
        f"MACKESD_BOOTSTRAP_SSH_KEY={key}\n"
        f"MACKESD_BOOTSTRAP_KNOWN_HOSTS={hosts}\n"
    )


def write_env_file(path: Path, key: Path, hosts: Path) -> Path:
    assert_away_from_production(path, key, hosts)
    path.write_text(env_body(key, hosts), encoding="ascii")
    os.chmod(path, 0o400)
    return path


def write_dest_pair(root: Path) -> tuple[Path, Path]:
    assert_away_from_production(root)
    key = root / "bootstrap-ssh-key"
    hosts = root / "bootstrap-known-hosts"
    key.write_bytes(FIXTURE_KEY)
    hosts.write_bytes(FIXTURE_HOSTS)
    os.chmod(key, 0o600)
    os.chmod(hosts, 0o400)
    return key.resolve(), hosts.resolve()


def child_check_script(key: Path, hosts: Path, marker: Path | None = None) -> str:
    lines = ["import os,sys"]
    if marker is not None:
        lines.append(f"open({str(marker)!r},'w').write('ok\\n')")
    lines.extend(
        [
            "k=os.environ.get('MACKESD_BOOTSTRAP_SSH_KEY')",
            "h=os.environ.get('MACKESD_BOOTSTRAP_KNOWN_HOSTS')",
            f"sys.exit(0 if k=={str(key)!r} and h=={str(hosts)!r} else 1)",
        ]
    )
    return "; ".join(lines)


def command(*args: str, refused: bool = False) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    for name in FORBIDDEN_ENV:
        env.pop(name, None)
    result = subprocess.run(
        [sys.executable, str(HELPER), *args],
        text=True,
        capture_output=True,
        env=env,
    )
    combined = (result.stdout + result.stderr).encode("utf-8", "replace")
    for marker in KEY_MARKERS:
        assert marker not in combined
    assert FIXTURE_KEY not in combined
    assert b"BEGIN OPENSSH" not in combined
    if refused:
        assert result.returncode == 2, result.stderr or result.stdout
        assert result.stdout == "", result.stdout
        assert "REFUSED:" in result.stderr
        assert "{{JOIN_TOKEN}}" not in result.stderr
    else:
        assert "REFUSED:" not in result.stderr, result.stderr
    return result


def main() -> None:
    for name in FORBIDDEN_ENV:
        assert name not in os.environ, f"{name} must stay unset in the test process"
    assert_away_from_production(Path(tempfile.gettempdir()), REPO)

    with tempfile.TemporaryDirectory(prefix="mcnf-bootstrap-ssh-run-test-") as temporary:
        root = Path(temporary)
        assert not inside_repo(root)
        assert_away_from_production(root)
        dest_root = root / "dest"
        dest_root.mkdir()
        dest_key, dest_hosts = write_dest_pair(dest_root)
        env_file = write_env_file(root / "bootstrap-ssh.env", dest_key, dest_hosts)
        base = ["--env-file", str(env_file)]

        linked_env = root / "linked.env"
        linked_env.symlink_to(env_file)
        command(*base[:1], str(linked_env), "--", "/usr/bin/true", refused=True)

        command(*base, "--", "/usr/bin/mackesd", "enroll-token", "--mesh-id", "x", refused=True)
        command(*base, "--", "/usr/bin/mackesd", "join", refused=True)
        command(*base, "--", "/usr/bin/mackesd", "offboard", refused=True)
        command(*base, "--", str(HERE / "mint-enroll-bearer.py"), refused=True)

        extra = root / "extra.env"
        extra.write_text(env_body(dest_key, dest_hosts) + "EXTRA=1\n", encoding="ascii")
        os.chmod(extra, 0o400)
        command("--env-file", str(extra), "--", "/usr/bin/true", refused=True)

        missing = root / "missing.env"
        missing.write_text(f"MACKESD_BOOTSTRAP_SSH_KEY={dest_key}\n", encoding="ascii")
        os.chmod(missing, 0o400)
        command("--env-file", str(missing), "--", "/usr/bin/true", refused=True)

        blank = root / "blank.env"
        blank.write_text(env_body(dest_key, dest_hosts).replace("\n", "\n\n", 1), encoding="ascii")
        os.chmod(blank, 0o400)
        command("--env-file", str(blank), "--", "/usr/bin/true", refused=True)

        missing_dest_parent = root / "missing-dest"
        missing_dest_parent.mkdir()
        missing_env = write_env_file(
            missing_dest_parent / "bootstrap-ssh.env",
            missing_dest_parent / "no-key",
            dest_hosts,
        )
        command("--env-file", str(missing_env), "--", "/usr/bin/true", refused=True)

        linked_key = root / "linked-key"
        linked_key.symlink_to(dest_key)
        linked_key_env = write_env_file(root / "linked-key.env", linked_key, dest_hosts)
        command("--env-file", str(linked_key_env), "--", "/usr/bin/true", refused=True)

        linked_hosts = root / "linked-hosts"
        linked_hosts.symlink_to(dest_hosts)
        linked_hosts_env = write_env_file(root / "linked-hosts.env", dest_key, linked_hosts)
        command("--env-file", str(linked_hosts_env), "--", "/usr/bin/true", refused=True)

        token_env = root / "token.env"
        token_env.write_text(
            "MACKESD_BOOTSTRAP_SSH_KEY=/tmp/{{JOIN_TOKEN}}/key\n"
            f"MACKESD_BOOTSTRAP_KNOWN_HOSTS={dest_hosts}\n",
            encoding="ascii",
        )
        os.chmod(token_env, 0o400)
        token_result = command("--env-file", str(token_env), "--", "/usr/bin/true", refused=True)
        assert "{{JOIN_TOKEN}}" not in token_result.stderr

        inside_parent = REPO / "install-helpers" / f".qu0024be-run-{os.getpid()}"
        try:
            inside_parent.mkdir()
            inside_env = inside_parent / "bootstrap-ssh.env"
            inside_env.write_text(env_body(dest_key, dest_hosts), encoding="ascii")
            os.chmod(inside_env, 0o400)
            command("--env-file", str(inside_env), "--", "/usr/bin/true", refused=True)
        finally:
            shutil.rmtree(inside_parent, ignore_errors=True)

        child_ok = [
            "/usr/bin/python3",
            "-c",
            child_check_script(dest_key, dest_hosts),
        ]
        result = command(*base, "--", *child_ok)
        assert result.returncode == 0
        assert result.stdout == ""
        for name in FORBIDDEN_ENV:
            assert name not in os.environ

        helper = load_helper()
        marker = root / "child-imported"
        imported_code = helper.run_with_env(
            env_file,
            ["/usr/bin/python3", "-c", child_check_script(dest_key, dest_hosts, marker)],
        )
        assert imported_code == 0
        assert marker.read_text(encoding="ascii") == "ok\n"
        for name in FORBIDDEN_ENV:
            assert name not in os.environ, f"{name} leaked into the helper process"

        sidecar = root / "bootstrap-ssh-env-run.json"
        sidecar_result = command(
            *base,
            "--print-sidecar",
            str(sidecar),
            "--",
            *child_ok,
        )
        assert sidecar_result.returncode == 0
        assert sidecar_result.stdout == ""
        assert sidecar.is_file() and not sidecar.is_symlink()
        assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
        assert sidecar.stat().st_nlink == 1
        assert not inside_repo(sidecar)
        record = json.loads(sidecar.read_text(encoding="ascii"))
        assert record["kind"] == "mcnf-bootstrap-ssh-env-run"
        assert record["schema_version"] == 1
        assert record["enroll_succeeded"] is False
        assert record["production_admitted"] is False
        assert record["command_argv"] == child_ok
        assert record["dest_key"]["path"] == str(dest_key)
        assert record["dest_key"]["mode"] == "0600"
        assert record["dest_key"]["sha256"] == hashlib.sha256(FIXTURE_KEY).hexdigest()
        assert record["dest_known_hosts"]["path"] == str(dest_hosts)
        assert record["dest_known_hosts"]["mode"] == "0400"
        assert record["dest_known_hosts"]["sha256"] == hashlib.sha256(FIXTURE_HOSTS).hexdigest()
        sidecar_bytes = sidecar.read_bytes()
        for marker_bytes in KEY_MARKERS:
            assert marker_bytes not in sidecar_bytes
        assert FIXTURE_KEY not in sidecar_bytes
        assert FIXTURE_HOSTS not in sidecar_bytes
        previous = sidecar.read_bytes()
        command(*base, "--print-sidecar", str(sidecar), "--", *child_ok, refused=True)
        assert sidecar.read_bytes() == previous

        exit_17 = command(*base, "--", "/usr/bin/python3", "-c", "import sys; sys.exit(17)")
        assert exit_17.returncode == 17
        assert "REFUSED:" not in exit_17.stderr
        for name in FORBIDDEN_ENV:
            assert name not in os.environ

        assert dest_key.read_bytes() == FIXTURE_KEY
        assert dest_hosts.read_bytes() == FIXTURE_HOSTS
        assert env_file.read_text(encoding="ascii") == env_body(dest_key, dest_hosts)

    for name in FORBIDDEN_ENV:
        assert name not in os.environ, f"{name} leaked into the test process"


if __name__ == "__main__":
    main()
    print("PASS")
