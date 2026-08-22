#!/usr/bin/env python3
"""Hostile self-test for immutable bootc digest receipt production."""

from __future__ import annotations
import json, os, stat, subprocess, sys, tempfile
from pathlib import Path

TOOL = Path(__file__).with_name("produce-bootc-digest-receipt.py")

def call(*args: str, ok: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run([sys.executable, str(TOOL), *args], text=True, capture_output=True)
    if ok != (result.returncode == 0):
        raise AssertionError(result.stderr or result.stdout)
    return result

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
        manifest = json.dumps({"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{"digest":amd,"platform":{"os":"linux","architecture":"amd64"}},{"digest":arm,"platform":{"os":"linux","architecture":"arm64"}}]}, separators=(",", ":"))
        fake = root / "skopeo"
        fake.write_text("#!/bin/sh\n[ \"${FAIL_REGISTRY:-0}\" = 0 ] || { echo unavailable >&2; exit 1; }\nprintf '%s' '" + manifest + "'\n")
        fake.chmod(0o755)
        receipt = root / "receipt.json"
        base = ("--repo",str(repo),"--skopeo",str(fake))
        produce = ("produce","--image-reference","registry.invalid/mcnf/bootc:release","--architecture","amd64","--source-revision",revision,"--commit-epoch","1700000000","--release-role","all-roles","--output",str(receipt))
        call(*base,*produce)
        assert stat.S_IMODE(receipt.stat().st_mode) == 0o400
        value = json.loads(receipt.read_text())
        assert value["resolved_digest"] == "sha256:" + __import__("hashlib").sha256(manifest.encode()).hexdigest()
        inspect = ("inspect","--receipt",str(receipt),"--expected-image-reference","registry.invalid/mcnf/bootc:release","--expected-architecture","amd64","--expected-source-revision",revision,"--expected-commit-epoch","1700000000","--expected-release-role","all-roles")
        call(*base,*inspect)
        call(*base,*produce,ok=False)  # no replacement
        call(*base,*inspect[:-1],"foreign-role",ok=False)
        for legacy in ("base", "unified-seat-server"):
            refused = call(*base,*inspect[:-1],legacy,ok=False)
            assert "legacy" in refused.stderr and legacy in refused.stderr
            produce_legacy = list(produce)
            produce_legacy[produce_legacy.index("--release-role") + 1] = legacy
            produce_legacy[-1] = str(root / f"{legacy}.json")
            produced = call(*base,*produce_legacy,ok=False)
            assert "legacy" in produced.stderr and legacy in produced.stderr
            assert not Path(produce_legacy[-1]).exists()
        call(*base,*inspect[:-3],"1700000001",*inspect[-2:],ok=False)
        changed = root / "changed.json"; value["architecture"]="arm64"; changed.write_text(json.dumps(value,sort_keys=True,separators=(",",":"))+"\n")
        changed_inspect = list(inspect); changed_inspect[2] = str(changed)
        call(*base,*changed_inspect,ok=False)
        link = root / "link"; link.symlink_to(receipt); linked = list(inspect); linked[2]=str(link)
        call(*base,*linked,ok=False)
        duplicate = root / "duplicate-skopeo"
        duplicate_manifest = json.dumps({"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{"digest":amd,"platform":{"os":"linux","architecture":"amd64"}},{"digest":arm,"platform":{"os":"linux","architecture":"amd64"}}]},separators=(",",":"))
        duplicate.write_text("#!/bin/sh\nprintf '%s' '"+duplicate_manifest+"'\n"); duplicate.chmod(0o755)
        call("--repo",str(repo),"--skopeo",str(duplicate),*produce[:-1],str(root/"duplicate.json"),ok=False)
        failing = root / "failing"; failing.write_text("#!/bin/sh\necho registry-unavailable >&2\nexit 1\n"); failing.chmod(0o755)
        call("--repo",str(repo),"--skopeo",str(failing),*produce[:-1],str(root/"unavailable.json"),ok=False)
        assert not (root/"unavailable.json").exists()
    print("bootc digest receipt hostile self-test: PASS")

if __name__ == "__main__": main()
