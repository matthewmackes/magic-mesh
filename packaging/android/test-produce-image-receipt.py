#!/usr/bin/env python3
"""Hostile producer/consumer tests for Cuttlefish image receipts."""

import json
import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

TOOL = Path(__file__).with_name("produce-image-receipt.py")


def main() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary); repo = root / "repo"; repo.mkdir()
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        subprocess.run(["git", "-C", str(repo), "config", "user.email", "fixture@example.invalid"], check=True)
        subprocess.run(["git", "-C", str(repo), "config", "user.name", "Fixture"], check=True)
        (repo / "seed").write_text("fixture\n")
        subprocess.run(["git", "-C", str(repo), "add", "seed"], check=True)
        env = os.environ | {"GIT_AUTHOR_DATE": "1700000000 +0000", "GIT_COMMITTER_DATE": "1700000000 +0000"}
        subprocess.run(["git", "-C", str(repo), "commit", "-qm", "fixture"], check=True, env=env)
        revision = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
        amd = "sha256:" + "a" * 64; arm = "sha256:" + "b" * 64
        manifest = json.dumps({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.index.v1+json", "manifests": [{"digest": amd, "platform": {"os": "linux", "architecture": "amd64"}}, {"digest": arm, "platform": {"os": "linux", "architecture": "arm64"}}]}, separators=(",", ":"))
        state = root / "manifest"; state.write_text(manifest)
        fake = root / "skopeo"
        fake.write_text("#!/bin/sh\n[ \"$2\" = --raw ] || exit 9\ncat \"$FAKE_MANIFEST\"\n"); fake.chmod(0o755)
        base = ("--repo", str(repo), "--skopeo", str(fake))
        common = ("--source-kind", "registry", "--original-source", "registry.invalid/android/cuttlefish:release", "--architecture", "amd64", "--provider-identity", "mcnf-cuttlefish", "--android-release-id", "android-15.0.0_r1", "--compatibility-id", "mcnf-cuttlefish-v1", "--source-revision", revision, "--commit-epoch", "1700000000")
        receipt = root / "receipt.json"; run_env = os.environ | {"FAKE_MANIFEST": str(state)}

        def invoke(*args: str, ok: bool = True):
            result = subprocess.run([sys.executable, str(TOOL), *args], text=True, capture_output=True, env=run_env)
            if (result.returncode == 0) != ok:
                raise AssertionError(result.stderr or result.stdout)
            return result

        invoke(*base, "produce", *common, "--output", str(receipt))
        value = json.loads(receipt.read_text())
        assert value["provider_identity"] == "mcnf-cuttlefish" and value["platform_digest"] == amd
        assert stat.S_IMODE(receipt.stat().st_mode) == 0o400
        invoke(*base, "inspect", *common, "--receipt", str(receipt))
        invoke(*base, "produce", *common, "--output", str(receipt), ok=False)
        changed = json.loads(state.read_text()); changed["manifests"][0]["digest"] = "sha256:" + "c" * 64
        state.write_text(json.dumps(changed, separators=(",", ":")))
        invoke(*base, "inspect", *common, "--receipt", str(receipt), ok=False)
        state.write_text(manifest)
        wrong = list(common); wrong[wrong.index("mcnf-cuttlefish-v1")] = "wrong-compatibility"
        invoke(*base, "inspect", *wrong, "--receipt", str(receipt), ok=False)
        link = root / "receipt-link"; link.symlink_to(receipt)
        invoke(*base, "inspect", *common, "--receipt", str(link), ok=False)

        artifact = root / "cuttlefish-image.tar"; artifact.write_bytes(b"immutable image bytes\n")
        artifact_common = ("--source-kind", "artifact", "--original-source", str(artifact), "--architecture", "amd64", "--provider-identity", "mcnf-cuttlefish", "--android-release-id", "android-15.0.0_r1", "--compatibility-id", "mcnf-cuttlefish-v1", "--source-revision", revision, "--commit-epoch", "1700000000", "--media-type", "application/vnd.mcnf.cuttlefish.image.v1+tar", "--artifact-format", "android-cuttlefish-image-archive")
        artifact_receipt = root / "artifact-receipt.json"
        invoke(*base, "produce", *artifact_common, "--output", str(artifact_receipt))
        invoke(*base, "inspect", *artifact_common, "--receipt", str(artifact_receipt))
        artifact.write_bytes(b"substituted same source\n")
        invoke(*base, "inspect", *artifact_common, "--receipt", str(artifact_receipt), ok=False)
        hardlink = root / "artifact-alias"; os.link(artifact, hardlink)
        invoke(*base, "produce", *artifact_common, "--output", str(root / "aliased.json"), ok=False)
    print("Cuttlefish image receipt hostile self-test: PASS")


if __name__ == "__main__":
    main()
