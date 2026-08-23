#!/usr/bin/env python3
"""Hostile self-test for bind-unpublished-signed-candidate.py.

No network. No live RPM sign. Tests use temp dirs outside the helper git
worktree and must never write /root/mcnf-private/. Fixture RPM bytes are
not a production candidate.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
HELPER = HERE / "bind-unpublished-signed-candidate.py"
ADMIT = HERE / "admit-unpublished-signed-candidate.py"
PRODUCTION = Path("/root/mcnf-private")
NEVRA = {
    "workstation": "magic-mesh-13.0.0-1.fc44.x86_64",
    "server": "magic-mesh-server-13.0.0-1.fc44.x86_64",
    "lighthouse": "magic-mesh-lighthouse-13.0.0-1.fc44.x86_64",
}


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


def write_rpm(path: Path, body: bytes) -> None:
    path.write_bytes(body)
    os.chmod(path, 0o400)


def command(*args: str, refused: bool = False) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        [sys.executable, str(HELPER), *args],
        check=False,
        capture_output=True,
        text=True,
    )
    if refused:
        assert result.returncode == 2, result.stderr or result.stdout
        assert result.stdout == "", result.stdout
        assert "REFUSED:" in result.stderr
        assert "{{JOIN_TOKEN}}" not in result.stderr
    else:
        assert result.returncode == 0, result.stderr
        assert "REFUSED:" not in result.stderr, result.stderr
    return result


def production_snapshot() -> dict[str, tuple[int, int]] | None:
    try:
        if not PRODUCTION.is_dir():
            return {}
        snapshot: dict[str, tuple[int, int]] = {}
        for child in PRODUCTION.iterdir():
            meta = child.lstat()
            snapshot[child.name] = (meta.st_mtime_ns, meta.st_size)
        return snapshot
    except PermissionError:
        return None


def main() -> None:
    before = production_snapshot()
    with tempfile.TemporaryDirectory(prefix="mcnf-bind-candidate-") as temporary:
        root = Path(temporary)
        os.chmod(root, 0o700)
        rpms = {}
        for name, body in (
            ("workstation", b"ws-rpm-bytes"),
            ("server", b"server-rpm-bytes"),
            ("lighthouse", b"lh-rpm-bytes"),
        ):
            path = root / f"{name}.rpm"
            write_rpm(path, body)
            rpms[name] = path

        dest = root / "unpublished-signed-candidate.json"
        result = command(
            "--workstation",
            str(rpms["workstation"]),
            "--server",
            str(rpms["server"]),
            "--lighthouse",
            str(rpms["lighthouse"]),
            "--dest",
            str(dest),
            "--workstation-nevra",
            NEVRA["workstation"],
            "--server-nevra",
            NEVRA["server"],
            "--lighthouse-nevra",
            NEVRA["lighthouse"],
        )
        assert "production_admitted=false" in result.stdout
        assert dest.is_file() and not dest.is_symlink()
        assert stat.S_IMODE(dest.stat().st_mode) == 0o400
        record = json.loads(dest.read_text(encoding="ascii"))
        assert record["kind"] == "mcnf-unpublished-signed-candidate"
        assert record["published"] is False
        assert record["production_admitted"] is False
        assert record["signer_fingerprint"] == "06B1C27EA0E08A225155EB3314018AA1497DDC7C"
        assert record["roles"]["workstation"]["sha256"] == hashlib.sha256(b"ws-rpm-bytes").hexdigest()
        admitted = subprocess.run(
            [sys.executable, str(ADMIT), "--dest", str(dest)],
            check=False,
            capture_output=True,
            text=True,
        )
        assert admitted.returncode == 0, admitted.stderr

        command(
            "--workstation",
            str(rpms["workstation"]),
            "--server",
            str(rpms["server"]),
            "--lighthouse",
            str(rpms["lighthouse"]),
            "--dest",
            str(dest),
            "--workstation-nevra",
            NEVRA["workstation"],
            "--server-nevra",
            NEVRA["server"],
            "--lighthouse-nevra",
            NEVRA["lighthouse"],
            refused=True,
        )
        previous = dest.read_bytes()

        old_dest = root / "old.json"
        old_result = command(
            "--workstation",
            str(rpms["workstation"]),
            "--server",
            str(rpms["server"]),
            "--lighthouse",
            str(rpms["lighthouse"]),
            "--dest",
            str(old_dest),
            "--workstation-nevra",
            "magic-mesh-12.1.6-35.x86_64",
            "--server-nevra",
            NEVRA["server"],
            "--lighthouse-nevra",
            NEVRA["lighthouse"],
            refused=True,
        )
        assert "13.0.0" in old_result.stderr
        assert not old_dest.exists()
        assert dest.read_bytes() == previous

        production_dest = PRODUCTION / "unpublished-signed-candidate.json"
        production_result = command(
            "--workstation",
            str(rpms["workstation"]),
            "--server",
            str(rpms["server"]),
            "--lighthouse",
            str(rpms["lighthouse"]),
            "--dest",
            str(production_dest),
            "--workstation-nevra",
            NEVRA["workstation"],
            "--server-nevra",
            NEVRA["server"],
            "--lighthouse-nevra",
            NEVRA["lighthouse"],
            refused=True,
        )
        assert "fixture bytes" in production_result.stderr or "rpm query" in production_result.stderr
        assert not production_dest.exists()

        inside = REPO / "install-helpers" / f".qu0026bd-cand-{os.getpid()}.json"
        try:
            command(
                "--workstation",
                str(rpms["workstation"]),
                "--server",
                str(rpms["server"]),
                "--lighthouse",
                str(rpms["lighthouse"]),
                "--dest",
                str(inside),
                "--workstation-nevra",
                NEVRA["workstation"],
                "--server-nevra",
                NEVRA["server"],
                "--lighthouse-nevra",
                NEVRA["lighthouse"],
                refused=True,
            )
            assert not inside.exists()
        finally:
            if inside.exists():
                inside.unlink()

        spec = importlib.util.spec_from_file_location("bind_candidate", HELPER)
        module = importlib.util.module_from_spec(spec)
        assert spec.loader is not None
        spec.loader.exec_module(module)
        os.environ["MACKESD_BOOTSTRAP_SSH_KEY"] = "/tmp/must-not-leak"
        os.environ["MACKESD_BOOTSTRAP_KNOWN_HOSTS"] = "/tmp/must-not-leak-hosts"
        os.environ["JOIN_TOKEN"] = "must-not-leak-token"
        seen: list[dict[str, str] | None] = []
        original = module.subprocess.run

        def capture(*args: object, **kwargs: object) -> object:
            env = kwargs.get("env")
            seen.append(env if isinstance(env, dict) else None)
            return original(*args, **kwargs)

        module.subprocess.run = capture  # type: ignore[method-assign]
        try:
            try:
                module.query_nevra(rpms["workstation"])
            except module.admit.Refusal:
                pass
        finally:
            module.subprocess.run = original  # type: ignore[method-assign]
            os.environ.pop("MACKESD_BOOTSTRAP_SSH_KEY", None)
            os.environ.pop("MACKESD_BOOTSTRAP_KNOWN_HOSTS", None)
            os.environ.pop("JOIN_TOKEN", None)
        assert seen and all(env is not None for env in seen)
        for env in seen:
            assert env is not None
            for name in (
                "MACKESD_BOOTSTRAP_SSH_KEY",
                "MACKESD_BOOTSTRAP_KNOWN_HOSTS",
                "JOIN_TOKEN",
            ):
                assert name not in env

    after = production_snapshot()
    if before is not None and after is not None:
        assert after == before, "self-test must never touch /root/mcnf-private"
    print("bind unpublished signed candidate hostile suite passed")


if __name__ == "__main__":
    main()
