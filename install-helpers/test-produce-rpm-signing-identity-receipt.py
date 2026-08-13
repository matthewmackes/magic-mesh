#!/usr/bin/env python3
"""Hostile self-test for the canonical RPM signing identity receipt."""

from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

PRODUCER = Path(__file__).with_name("produce-rpm-signing-identity-receipt.py")


def command(*args: str, ok: bool = True, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    result = subprocess.run([sys.executable, str(PRODUCER), *args], text=True, capture_output=True, env=env)
    if ok and result.returncode != 0:
        raise AssertionError(result.stderr)
    if not ok and result.returncode == 0:
        raise AssertionError(f"unexpected admission: {' '.join(args)}")
    return result


def main() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        repo = root / "repo"
        (repo / "packaging/repo").mkdir(parents=True)
        shutil.copy2(PRODUCER.parent.parent / "packaging/repo/RPM-GPG-KEY-magic-mesh", repo / "packaging/repo/RPM-GPG-KEY-magic-mesh")
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        subprocess.run(["git", "-C", str(repo), "config", "user.email", "fixture@example.invalid"], check=True)
        subprocess.run(["git", "-C", str(repo), "config", "user.name", "Fixture"], check=True)
        subprocess.run(["git", "-C", str(repo), "add", "packaging/repo/RPM-GPG-KEY-magic-mesh"], check=True)
        fixture_env = os.environ.copy()
        fixture_env.update({"GIT_AUTHOR_DATE": "1700000000 +0000", "GIT_COMMITTER_DATE": "1700000000 +0000"})
        subprocess.run(["git", "-C", str(repo), "commit", "-qm", "fixture"], check=True, env=fixture_env)
        revision = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
        key = repo / "packaging/repo/RPM-GPG-KEY-magic-mesh"
        real_gpg = shutil.which("gpg")
        assert real_gpg
        public = subprocess.check_output([real_gpg, "--batch", "--no-options", "--with-colons", "--show-keys", str(key)], text=True)
        fingerprint = next(line.split(":")[9] for line in public.splitlines() if line.startswith("fpr:"))
        fake = root / "gpg"
        fake.write_text(
            "#!/bin/sh\n"
            "case \" $* \" in *' --show-keys '*) exec " + real_gpg + " \"$@\";; esac\n"
            "printf '%s\\n' 'sec:u:255:22:DEADBEEF:0:0:::::scSC:::::ed25519:::0:' "
            "'fpr:::::::::" + fingerprint + ":'\n"
            "if [ \"${FAKE_AMBIGUOUS:-0}\" = 1 ]; then printf '%s\\n' "
            "'sec:u:255:22:BAD:0:0:::::scSC:::::ed25519:::0:' "
            "'fpr:::::::::AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:'; fi\n",
            encoding="ascii",
        )
        fake.chmod(0o755)
        output = root / "receipt.json"
        base = ("--repo", str(repo), "--gpg", str(fake))
        command(*base, "produce", "--source-revision", revision, "--release-epoch", "1700000000", "--output", str(output))
        assert stat.S_IMODE(output.stat().st_mode) == 0o400
        value = json.loads(output.read_text())
        assert value["primary_fingerprint"] == fingerprint
        assert value["source_revision"] == revision and value["release_epoch"] == 1700000000
        assert "private" not in output.read_text().lower()
        command(*base, "inspect", "--receipt", str(output), "--expected-source-revision", revision, "--expected-release-epoch", "1700000000")
        command(*base, "inspect", "--receipt", str(output), "--expected-source-revision", revision, "--expected-release-epoch", "1700000000", "--signing-identity", "foreign configured label", ok=False)

        command(*base, "produce", "--source-revision", revision, "--release-epoch", "1700000000", "--output", str(output), ok=False)
        command(*base, "inspect", "--receipt", str(output), "--expected-source-revision", revision, "--expected-release-epoch", "1700000001", ok=False)
        tampered = root / "tampered.json"
        value["source_revision"] = "0" * 40
        tampered.write_text(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
        command(*base, "inspect", "--receipt", str(tampered), "--expected-source-revision", revision, "--expected-release-epoch", "1700000000", ok=False)
        ambiguous_env = os.environ.copy()
        ambiguous_env["FAKE_AMBIGUOUS"] = "1"
        command(*base, "produce", "--source-revision", revision, "--release-epoch", "1700000000", "--output", str(root / "ambiguous.json"), ok=False, env=ambiguous_env)
        foreign = root / "gpg-foreign"
        foreign.write_text(
            "#!/bin/sh\ncase \" $* \" in *' --show-keys '*) exec " + real_gpg + " \"$@\";; esac\n"
            "printf '%s\\n' 'sec:u:255:22:BAD:0:0:::::scSC:::::ed25519:::0:' 'fpr:::::::::AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:'\n"
        )
        foreign.chmod(0o755)
        command("--repo", str(repo), "--gpg", str(foreign), "produce", "--source-revision", revision, "--release-epoch", "1700000000", "--output", str(root / "foreign.json"), ok=False)
        symlink = root / "receipt-link"
        symlink.symlink_to(output)
        command(*base, "inspect", "--receipt", str(symlink), "--expected-source-revision", revision, "--expected-release-epoch", "1700000000", ok=False)
    print("RPM signing identity receipt hostile self-test: PASS")


if __name__ == "__main__":
    main()
