#!/usr/bin/env python3
"""Hostile tests for the governed App VM catalog-trust producer."""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tempfile
from pathlib import Path


def main() -> int:
    repo = Path(__file__).resolve().parents[1]
    producer = repo / "install-helpers/produce-app-vm-catalog-trust.py"
    fingerprint = "06B1C27EA0E08A225155EB3314018AA1497DDC7C"
    point = "40" + "3b2eee92223984dbf6bcfb44461669a577390810584023734561e7875ed7cc1d"
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        root.chmod(0o700)
        fixture_repo = root / "repo"
        (fixture_repo / "install-helpers").mkdir(parents=True)
        (fixture_repo / "packaging/repo").mkdir(parents=True)
        shutil.copy2(repo / "install-helpers/verify-app-vm-catalog-trust.py", fixture_repo / "install-helpers")
        shutil.copy2(repo / "packaging/repo/RPM-GPG-KEY-magic-mesh", fixture_repo / "packaging/repo")
        subprocess.run(["git", "-C", fixture_repo, "init", "-q"], check=True)
        subprocess.run(["git", "-C", fixture_repo, "config", "user.name", "FUNC-018 self-test"], check=True)
        subprocess.run(["git", "-C", fixture_repo, "config", "user.email", "func018-selftest.invalid"], check=True)
        subprocess.run(["git", "-C", fixture_repo, "add", "."], check=True)
        subprocess.run(["git", "-C", fixture_repo, "commit", "-qm", "fixture"], check=True)
        revision = subprocess.check_output(["git", "-C", fixture_repo, "rev-parse", "HEAD"], text=True).strip()
        fake = root / "gpg"
        fake.write_text(
            """#!/usr/bin/env python3
import os, sys
point = os.environ.get('TEST_POINT', '403b2eee92223984dbf6bcfb44461669a577390810584023734561e7875ed7cc1d')
kind = 'sec' if '--list-secret-keys' in sys.argv else 'pub'
fpr = os.environ.get('TEST_SECRET_FPR', '06B1C27EA0E08A225155EB3314018AA1497DDC7C') if kind == 'sec' else '06B1C27EA0E08A225155EB3314018AA1497DDC7C'
if kind == 'sec' and os.environ.get('TEST_NO_SECRET') == '1':
    print('gpg: no secret key', file=sys.stderr); raise SystemExit(2)
algo = os.environ.get('TEST_ALGO', '22')
curve = 'ed25519' if algo == '22' else ''
print(f'{kind}:u:255:{algo}:E6C820DAFBD1B07A:0:0::u:::scSC:::::{curve}:::0:')
print(f'fpr:::::::::{fpr}:')
print('pkd:0:80:092B06010401DA470F01:')
print(f'pkd:1:263:{point}:')
""",
            encoding="utf-8",
        )
        fake.chmod(0o700)

        def invoke(name: str, extra: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
            env = dict(os.environ)
            env.update(extra or {})
            return subprocess.run(
                [str(producer), "--repo", str(fixture_repo), "--gpg", str(fake), "--source-revision", revision, "--out-dir", str(root / name)],
                text=True,
                capture_output=True,
                env=env,
                check=False,
            )

        good = invoke("good")
        assert good.returncode == 0, good.stderr
        out = root / "good"
        assert stat.S_IMODE(out.stat().st_mode) == 0o700
        assert stat.S_IMODE((out / "catalog-trust-receipt.json").stat().st_mode) == 0o400
        assert stat.S_IMODE((out / "catalog-verification.key").stat().st_mode) == 0o400
        assert (out / "catalog-verification.key").read_text() == point[2:] + "\n"
        assert fingerprint in (out / "catalog-trust-receipt.json").read_text()
        assert revision in (out / "catalog-trust-receipt.json").read_text()
        assert not any("PRIVATE" in path.read_text(errors="ignore") for path in out.iterdir())

        cases = {
            "missing-secret": {"TEST_NO_SECRET": "1"},
            "wrong-secret": {"TEST_SECRET_FPR": "A" * 40},
            "wrong-algorithm": {"TEST_ALGO": "1"},
            "bad-point": {"TEST_POINT": "41" + point[2:]},
        }
        for name, env in cases.items():
            result = invoke(name, env)
            assert result.returncode == 2, (name, result.stdout, result.stderr)
            assert "REFUSED[WL-FUNC-018/catalog-trust-producer]" in result.stderr
            assert not (root / name).exists(), name

        existing = root / "existing"
        existing.mkdir()
        marker = existing / "marker"
        marker.write_text("preserved\n")
        refused = invoke("existing")
        assert refused.returncode == 2 and marker.read_text() == "preserved\n"

        bad_parent = root / "open"
        bad_parent.mkdir(mode=0o777)
        bad_parent.chmod(0o777)
        env = dict(os.environ)
        result = subprocess.run(
            [str(producer), "--repo", str(fixture_repo), "--gpg", str(fake), "--source-revision", revision, "--out-dir", str(bad_parent / "trust")],
            text=True,
            capture_output=True,
            env=env,
            check=False,
        )
        assert result.returncode == 2 and not (bad_parent / "trust").exists()
        shutil.rmtree(bad_parent)
    print("test-produce-app-vm-catalog-trust: hostile self-test passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
