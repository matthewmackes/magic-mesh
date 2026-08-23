#!/usr/bin/env python3
"""Hostile self-test for mint-enroll-bearer.py.

No network. No live enroll-token against a real workgroup. Tests must use
temp dirs outside the helper git worktree and must never touch
/root/mcnf-private/. A fake mackesd script prints a well-formed token.
This leftover does not claim a production bearer was minted.
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
HELPER = HERE / "mint-enroll-bearer.py"
PRODUCTION = Path("/root/mcnf-private")
FIXTURE_BEARER = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-abcde"
assert len(FIXTURE_BEARER) == 43
MESH_ID = "test-mesh"
LIGHTHOUSE = "203.0.113.7"
PORT = "4243"


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


def join_token(bearer: str, fingerprint: str | None = None) -> str:
    token = f"mesh:{MESH_ID}@{LIGHTHOUSE}:{PORT}#{bearer}"
    if fingerprint is not None:
        token = f"{token}?fp={fingerprint}"
    return token


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


def production_snapshot() -> dict[str, tuple[int, int]] | None:
    try:
        if not PRODUCTION.is_dir():
            return {}
        snapshot: dict[str, tuple[int, int]] = {}
        for child in PRODUCTION.iterdir():
            try:
                meta = child.lstat()
            except OSError:
                continue
            snapshot[child.name] = (meta.st_mtime_ns, meta.st_size)
        return snapshot
    except PermissionError:
        # Farm slot user cannot read /root/mcnf-private; that is still "never touch".
        return None


def write_fake_mackesd(path: Path, body: str) -> Path:
    assert_away_from_production(path)
    path.write_text(body, encoding="ascii")
    os.chmod(path, 0o700)
    assert not path.is_symlink()
    assert stat.S_ISREG(path.stat().st_mode)
    return path


def fake_script(stdout: str, exit_code: int = 0) -> str:
    return (
        "#!/usr/bin/env python3\n"
        "import sys\n"
        f"sys.stdout.write({stdout!r})\n"
        "sys.stdout.flush()\n"
        f"raise SystemExit({exit_code})\n"
    )


def command(*args: str, refused: bool = False) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        [sys.executable, str(HELPER), *args],
        text=True,
        capture_output=True,
    )
    combined = result.stdout + result.stderr
    assert FIXTURE_BEARER not in result.stdout
    assert FIXTURE_BEARER not in result.stderr
    assert "BEGIN OPENSSH" not in combined
    assert "{{JOIN_TOKEN}}" not in combined
    assert f"mesh:{MESH_ID}@" not in result.stdout
    if refused:
        assert result.returncode == 2, result.stderr or result.stdout
        assert result.stdout == "", result.stdout
        assert "REFUSED:" in result.stderr
    else:
        assert result.returncode == 0, result.stderr or result.stdout
        assert "REFUSED:" not in result.stderr, result.stderr
        assert result.stdout == "", result.stdout
    return result


def dest_parent(root: Path) -> Path:
    parent = root / "dest-parent"
    parent.mkdir(mode=0o700)
    os.chmod(parent, 0o700)
    assert_away_from_production(parent)
    assert not inside_repo(parent)
    return parent


def main() -> None:
    assert_away_from_production(Path(tempfile.gettempdir()), REPO)
    before = production_snapshot()

    with tempfile.TemporaryDirectory(prefix="mcnf-mint-enroll-bearer-test-") as temporary:
        root = Path(temporary)
        assert not inside_repo(root)
        assert_away_from_production(root)
        parent = dest_parent(root)
        dest = parent / "enroll-bearer"
        sidecar = parent / "enroll-bearer.json"
        fake = write_fake_mackesd(
            root / "mackesd",
            fake_script(join_token(FIXTURE_BEARER) + "\n"),
        )
        base = [
            "--mackesd",
            str(fake),
            "--mesh-id",
            MESH_ID,
            "--lighthouse",
            LIGHTHOUSE,
            "--output",
            str(dest),
        ]

        result = command(*base, "--sidecar", str(sidecar), "--note", "fixture")
        assert dest.is_file() and not dest.is_symlink()
        assert stat.S_IMODE(dest.stat().st_mode) == 0o600
        assert dest.stat().st_nlink == 1
        assert dest.read_bytes() == FIXTURE_BEARER.encode("ascii")
        assert not inside_repo(dest)
        assert sidecar.is_file() and not sidecar.is_symlink()
        assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
        record = json.loads(sidecar.read_text(encoding="ascii"))
        assert record["kind"] == "mcnf-enroll-bearer-mint"
        assert record["schema_version"] == 1
        assert record["production_admitted"] is False
        assert record["enroll_succeeded"] is False
        assert record["mesh_id"] == MESH_ID
        assert record["note_len"] == len("fixture")
        assert "note" not in record
        assert FIXTURE_BEARER not in sidecar.read_text(encoding="ascii")
        assert "mesh:" not in sidecar.read_text(encoding="ascii")
        assert record["bearer_sha256"] == hashlib.sha256(dest.read_bytes()).hexdigest()
        assert record["dest"]["path"] == str(dest.resolve())
        assert record["dest"]["mode"] == "0600"
        assert record["dest"]["bytes"] == 43
        assert result.stdout == ""

        placeholder = write_fake_mackesd(
            root / "mackesd-placeholder",
            fake_script(join_token("{{JOIN_TOKEN}}") + "\n"),
        )
        command(
            "--mackesd",
            str(placeholder),
            "--mesh-id",
            MESH_ID,
            "--output",
            str(parent / "placeholder-dest"),
            refused=True,
        )
        assert not (parent / "placeholder-dest").exists()

        for length in (42, 44):
            short_long = write_fake_mackesd(
                root / f"mackesd-{length}",
                fake_script(join_token(("A" * length)) + "\n"),
            )
            dest_len = parent / f"dest-{length}"
            command(
                "--mackesd",
                str(short_long),
                "--mesh-id",
                MESH_ID,
                "--output",
                str(dest_len),
                refused=True,
            )
            assert not dest_len.exists()

        two_tokens = write_fake_mackesd(
            root / "mackesd-two",
            fake_script(
                join_token(FIXTURE_BEARER) + "\n" + join_token(FIXTURE_BEARER) + "\n"
            ),
        )
        command(
            "--mackesd",
            str(two_tokens),
            "--mesh-id",
            MESH_ID,
            "--output",
            str(parent / "two-dest"),
            refused=True,
        )
        assert not (parent / "two-dest").exists()

        garbage = write_fake_mackesd(
            root / "mackesd-garbage",
            fake_script("noise\n" + join_token(FIXTURE_BEARER) + "\n"),
        )
        command(
            "--mackesd",
            str(garbage),
            "--mesh-id",
            MESH_ID,
            "--output",
            str(parent / "garbage-dest"),
            refused=True,
        )
        assert not (parent / "garbage-dest").exists()

        trailing = write_fake_mackesd(
            root / "mackesd-trailing",
            fake_script(join_token(FIXTURE_BEARER) + "\nextra\n"),
        )
        command(
            "--mackesd",
            str(trailing),
            "--mesh-id",
            MESH_ID,
            "--output",
            str(parent / "trailing-dest"),
            refused=True,
        )
        assert not (parent / "trailing-dest").exists()

        prefix = write_fake_mackesd(
            root / "mackesd-prefix",
            fake_script("prefix " + join_token(FIXTURE_BEARER) + "\n"),
        )
        command(
            "--mackesd",
            str(prefix),
            "--mesh-id",
            MESH_ID,
            "--output",
            str(parent / "prefix-dest"),
            refused=True,
        )
        assert not (parent / "prefix-dest").exists()

        existing = parent / "already-exists"
        existing.write_bytes(b"stale")
        os.chmod(existing, 0o600)
        command(*base[:-1], str(existing), refused=True)
        assert existing.read_bytes() == b"stale"

        inside_parent = REPO / "install-helpers" / f".qu0026mb-mint-{os.getpid()}"
        try:
            inside_parent.mkdir()
            os.chmod(inside_parent, 0o700)
            command(
                "--mackesd",
                str(fake),
                "--mesh-id",
                MESH_ID,
                "--output",
                str(inside_parent / "enroll-bearer"),
                refused=True,
            )
            assert not (inside_parent / "enroll-bearer").exists()
        finally:
            shutil.rmtree(inside_parent, ignore_errors=True)

        before_production = production_snapshot()
        production_dest = PRODUCTION / "enroll-bearer-probe"
        production_result = command(
            "--mackesd",
            str(fake),
            "--mesh-id",
            MESH_ID,
            "--output",
            str(production_dest),
            refused=True,
        )
        assert "unpublished signed candidate is absent" in production_result.stderr
        assert not production_dest.exists()
        after_production = production_snapshot()
        if before_production is not None and after_production is not None:
            assert after_production == before_production

        failing = write_fake_mackesd(
            root / "mackesd-fail",
            fake_script(join_token(FIXTURE_BEARER) + "\n", exit_code=1),
        )
        fail_dest = parent / "fail-dest"
        command(
            "--mackesd",
            str(failing),
            "--mesh-id",
            MESH_ID,
            "--output",
            str(fail_dest),
            refused=True,
        )
        assert not fail_dest.exists()

        linked = root / "mackesd-link"
        linked.symlink_to(fake)
        command(
            "--mackesd",
            str(linked),
            "--mesh-id",
            MESH_ID,
            "--output",
            str(parent / "link-dest"),
            refused=True,
        )

        fp = "a" * 64
        fp_fake = write_fake_mackesd(
            root / "mackesd-fp",
            fake_script(join_token(FIXTURE_BEARER, fp) + "\n"),
        )
        fp_dest = parent / "fp-dest"
        command(
            "--mackesd",
            str(fp_fake),
            "--mesh-id",
            MESH_ID,
            "--output",
            str(fp_dest),
        )
        assert fp_dest.read_bytes() == FIXTURE_BEARER.encode("ascii")

        wg = root / "workgroup"
        wg.mkdir(mode=0o700)
        seen = root / "seen-env"
        env_fake = write_fake_mackesd(
            root / "mackesd-env",
            (
                "#!/usr/bin/env python3\n"
                "import os,sys\n"
                f"open({str(seen)!r},'w').write(os.environ.get('MDE_WORKGROUP_ROOT',''))\n"
                f"sys.stdout.write({(join_token(FIXTURE_BEARER) + chr(10))!r})\n"
            ),
        )
        env_dest = parent / "env-dest"
        command(
            "--mackesd",
            str(env_fake),
            "--mesh-id",
            MESH_ID,
            "--workgroup-root",
            str(wg),
            "--output",
            str(env_dest),
        )
        assert seen.read_text(encoding="ascii") == str(wg.resolve())
        assert env_dest.read_bytes() == FIXTURE_BEARER.encode("ascii")

        assert_away_from_production(root, dest, sidecar, parent, fake, fp_dest, env_dest)
        assert not inside_repo(dest)
        assert PRODUCTION not in (root, dest, sidecar, parent)

    after = production_snapshot()
    if before is not None and after is not None:
        assert after == before, "self-test must never touch /root/mcnf-private"
    print("PASS")


if __name__ == "__main__":
    main()
