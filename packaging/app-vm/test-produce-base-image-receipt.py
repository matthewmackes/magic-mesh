#!/usr/bin/env python3
"""Hostile producer and revalidation tests for App base-image receipts."""

import json, os, stat, subprocess, sys, tempfile
from pathlib import Path

TOOL = Path(__file__).with_name("produce-base-image-receipt.py")

def call(*args: str, ok: bool = True):
    result = subprocess.run([sys.executable, str(TOOL), *args], text=True, capture_output=True)
    if (result.returncode == 0) != ok:
        raise AssertionError(result.stderr or result.stdout)
    return result

def main() -> None:
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp); repo = root / "repo"; repo.mkdir()
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        subprocess.run(["git", "-C", str(repo), "config", "user.email", "fixture@example.invalid"], check=True)
        subprocess.run(["git", "-C", str(repo), "config", "user.name", "Fixture"], check=True)
        (repo / "seed").write_text("fixture\n")
        subprocess.run(["git", "-C", str(repo), "add", "seed"], check=True)
        env = os.environ | {"GIT_AUTHOR_DATE":"1700000000 +0000", "GIT_COMMITTER_DATE":"1700000000 +0000"}
        subprocess.run(["git", "-C", str(repo), "commit", "-qm", "fixture"], check=True, env=env)
        revision = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
        amd = "sha256:" + "a" * 64; arm = "sha256:" + "b" * 64
        manifest = json.dumps({"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{"digest":amd,"platform":{"os":"linux","architecture":"amd64"}},{"digest":arm,"platform":{"os":"linux","architecture":"arm64"}}]}, separators=(",", ":"))
        state = root / "manifest"; state.write_text(manifest)
        fake = root / "skopeo"
        fake.write_text("#!/bin/sh\n[ \"$2\" = --raw ] || exit 9\ncat \"$FAKE_MANIFEST\"\n"); fake.chmod(0o755)
        base = ("--repo",str(repo),"--skopeo",str(fake)); common=("--image-reference","registry.invalid/fedora/app-vm-base:44","--architecture","amd64","--source-revision",revision,"--commit-epoch","1700000000")
        receipt = root / "receipt.json"
        with_env = os.environ | {"FAKE_MANIFEST":str(state)}
        def invoke(*args: str, ok: bool = True):
            result = subprocess.run([sys.executable,str(TOOL),*args],text=True,capture_output=True,env=with_env)
            if (result.returncode == 0) != ok: raise AssertionError(result.stderr or result.stdout)
        invoke(*base,"produce",*common,"--output",str(receipt))
        value=json.loads(receipt.read_text()); assert value["app_vm_target"]=="mcnf-app-vm/wayland-standard-v1"; assert value["app_vm_profile"]=="wayland-standard"; assert value["platform_digest"]==amd; assert stat.S_IMODE(receipt.stat().st_mode)==0o400
        invoke(*base,"inspect",*common,"--receipt",str(receipt))
        invoke(*base,"produce",*common,"--output",str(receipt),ok=False)
        wrong=list(common); wrong[wrong.index("amd64")+0]="arm64"
        invoke(*base,"inspect",*wrong,"--receipt",str(receipt),ok=False)
        changed=json.loads(state.read_text()); changed["manifests"][0]["digest"]="sha256:"+"c"*64; state.write_text(json.dumps(changed,separators=(",",":")))
        invoke(*base,"inspect",*common,"--receipt",str(receipt),ok=False)
        state.write_text(manifest)
        tampered=root/"tampered.json"; value["source_revision"]="0"+revision[1:]; tampered.write_text(json.dumps(value)); tampered.chmod(0o400)
        invoke(*base,"inspect",*common,"--receipt",str(tampered),ok=False)
        link=root/"link"; link.symlink_to(receipt); invoke(*base,"inspect",*common,"--receipt",str(link),ok=False)
        duplicate=json.dumps({"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{"digest":amd,"platform":{"os":"linux","architecture":"amd64"}},{"digest":arm,"platform":{"os":"linux","architecture":"amd64"}}]},separators=(",",":")); state.write_text(duplicate)
        invoke(*base,"produce",*common,"--output",str(root/"duplicate.json"),ok=False)
        state.write_text(""); invoke(*base,"produce",*common,"--output",str(root/"empty.json"),ok=False); assert not (root/"empty.json").exists()
    print("App base-image receipt hostile self-test: PASS")

if __name__ == "__main__": main()
