#!/usr/bin/env python3
"""Hostile test for the governed resource-publisher receipt producer."""

from __future__ import annotations

import json
import os
import stat
import subprocess
import tempfile
from pathlib import Path


def run(*args: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, text=True, capture_output=True, check=False, env=env)


def main() -> int:
    repo = Path(__file__).resolve().parents[1]
    producer = repo / "install-helpers/produce-resource-publisher-credential.py"
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        root.chmod(0o700)
        fixture = root / "repo"
        fixture.mkdir()
        subprocess.run(["git", "-C", fixture, "init", "-q"], check=True)
        subprocess.run(["git", "-C", fixture, "config", "user.name", "FUNC-019 self-test"], check=True)
        subprocess.run(["git", "-C", fixture, "config", "user.email", "func019-selftest.invalid"], check=True)
        (fixture / "tracked").write_text("fixture\n")
        subprocess.run(["git", "-C", fixture, "add", "."], check=True)
        subprocess.run(["git", "-C", fixture, "commit", "-qm", "fixture"], check=True)
        revision = subprocess.check_output(["git", "-C", fixture, "rev-parse", "HEAD"], text=True).strip()
        gnupg = root / "gnupg"
        gnupg.mkdir(mode=0o700)
        env = dict(os.environ, GNUPGHOME=str(gnupg))
        generated = run("gpg", "--batch", "--passphrase", "", "--quick-generate-key", "FUNC-019 fixture <fixture.invalid>", "ed25519", "sign", "0", env=env)
        assert generated.returncode == 0, generated.stderr
        fingerprint = subprocess.check_output(["gpg", "--batch", "--with-colons", "--list-secret-keys"], text=True, env=env).split("fpr:::::::::", 1)[1].split(":", 1)[0]
        public_key = root / "release.asc"
        with public_key.open("w") as stream:
            subprocess.run(["gpg", "--batch", "--armor", "--export", fingerprint], check=True, text=True, stdout=stream, env=env)
        public_key.chmod(0o400)
        credential = root / "publisher.secret"
        credential.write_text("publisher-secret-fixture")
        credential.chmod(0o400)

        def invoke(name: str, *extra: str) -> subprocess.CompletedProcess[str]:
            return run(str(producer), "--repo", str(fixture), "--release-public-key", str(public_key), "--release-key-id", fingerprint, "--credential", str(credential), "--target-node", "peer:seat-a", "--target-role", "workstation", "--source-revision", revision, "--out-dir", str(root / name), *extra, env=env)

        good = invoke("good")
        assert good.returncode == 0, good.stderr
        receipt = json.loads((root / "good/resource-publisher-receipt.json").read_text())
        assert receipt["publisher_identity"] == f"openpgp-primary:{fingerprint}"
        assert receipt["source_revision"] == revision and receipt["target_node"] == "peer:seat-a"
        assert stat.S_IMODE((root / "good").stat().st_mode) == 0o700
        assert all(stat.S_IMODE(path.stat().st_mode) == 0o400 for path in (root / "good").iterdir())
        assert "publisher-secret-fixture" not in json.dumps(receipt)

        missing = credential.with_name("missing")
        refused = run(str(producer), "--repo", str(fixture), "--release-public-key", str(public_key), "--release-key-id", fingerprint, "--credential", str(missing), "--target-node", "peer:seat-a", "--target-role", "workstation", "--source-revision", revision, "--out-dir", str(root / "missing"), env=env)
        assert refused.returncode == 2 and not (root / "missing").exists()
        credential.chmod(0o600)
        credential.write_text("bad\nsecret")
        credential.chmod(0o400)
        bad = invoke("control-byte")
        assert bad.returncode == 2 and not (root / "control-byte").exists()
        credential.chmod(0o600)
        credential.write_text("publisher-secret-fixture")
        credential.chmod(0o400)
        wrong_node = invoke("wrong-node", "--target-node", "not-a-peer")
        assert wrong_node.returncode == 2 and not (root / "wrong-node").exists()
        existing = root / "existing"
        existing.mkdir()
        (existing / "marker").write_text("preserved\n")
        collision = invoke("existing")
        assert collision.returncode == 2 and (existing / "marker").read_text() == "preserved\n"

        other_home = root / "other-gnupg"
        other_home.mkdir(mode=0o700)
        other_env = dict(env, GNUPGHOME=str(other_home))
        mismatch = run(str(producer), "--repo", str(fixture), "--release-public-key", str(public_key), "--release-key-id", fingerprint, "--credential", str(credential), "--target-node", "peer:seat-a", "--target-role", "workstation", "--source-revision", revision, "--out-dir", str(root / "wrong-key"), env=other_env)
        assert mismatch.returncode == 2 and not (root / "wrong-key").exists()
    print("test-produce-resource-publisher-credential: hostile self-test passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
