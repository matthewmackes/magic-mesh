#!/usr/bin/env python3
"""Hostile tests for the immutable release-output collector."""

from __future__ import annotations
import copy, json, os, shutil
from pathlib import Path
import subprocess, sys, tempfile

ROOT = Path(__file__).resolve().parents[1]
SIGNER = "A" * 40
ROLES = {
    "workstation-rpm": ("application/x-rpm", b"\xed\xab\xee\xdbrpm"),
    "server-rpm": ("application/x-rpm", b"\xed\xab\xee\xdbserver"),
    "lighthouse-rpm": ("application/x-rpm", b"\xed\xab\xee\xdblighthouse"),
    "browser-vm": ("application/x-qemu-disk", b"QFI\xfbbrowser"),
    "app-vm": ("application/x-qemu-disk", b"QFI\xfbapp"),
    "cuttlefish-image": ("application/vnd.mcnf.cuttlefish-image", b"cuttlefish"),
    "bootc-image": ("application/vnd.mcnf.bootc-image-receipt+json", b'{"resolved_digest":"sha256:fixture"}'),
}

def write(path: Path, data: bytes, mode: int = 0o400) -> None:
    path.write_bytes(data); path.chmod(mode)

def run(collector: Path, root: Path, plan: dict, name: str, ok: bool) -> subprocess.CompletedProcess[str]:
    plan_path = root / f"{name}.json"; plan_path.write_text(json.dumps(plan)); plan_path.chmod(0o400)
    result = subprocess.run([sys.executable, str(collector), "--plan", str(plan_path), "--output", str(root / f"{name}.out")], text=True, capture_output=True)
    assert (result.returncode == 0) == ok, (name, result.stdout, result.stderr)
    return result

def main() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw); root.chmod(0o700)
        repo = root / "repo"; helpers = repo / "install-helpers"; helpers.mkdir(parents=True)
        collector = helpers / "collect-release-outputs.py"
        shutil.copy2(ROOT / "install-helpers/collect-release-outputs.py", collector)
        verifier = helpers / "verify"
        verifier.write_text("#!/bin/sh\nset -eu\n[ -f \"$1\" ]\ncase \"$(basename \"$1\")\" in reject*) exit 1;; esac\n")
        verifier.chmod(0o755)
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        subprocess.run(["git", "-C", str(repo), "add", "install-helpers"], check=True)
        subprocess.run(["git", "-C", str(repo), "-c", "user.name=test", "-c", "user.email=test@example.invalid", "commit", "-qm", "fixture"], check=True)
        revision = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
        artifacts = root / "artifacts"; artifacts.mkdir(); artifacts.chmod(0o700)
        outputs = []
        for role, (media, payload) in ROLES.items():
            artifact = artifacts / role; write(artifact, payload)
            row = {"role": role, "path": str(artifact), "media_type": media,
                   "source_revision": revision, "companions": {},
                   "verifier": [str(verifier), "{artifact}", "{source_revision}"]}
            if media == "application/x-rpm":
                row["signing_identity"] = SIGNER
                row["verifier"].append("{signing_identity}")
            outputs.append(row)
        plan = {"schema_version": 1, "kind": "mcnf-release-output-collection-plan", "source_revision": revision, "outputs": outputs}
        run(collector, root, plan, "good", True)
        value = json.loads((root / "good.out").read_text())
        assert value["promotion"] == "forbidden" and len(value["outputs"]) == 7
        assert all(row["sha256"].startswith("sha256:") and row["size"] > 0 for row in value["outputs"])
        assert all(
            ("signing_identity" in row) == (row["media_type"] == "application/x-rpm")
            for row in value["outputs"]
        )

        mutations = {}
        duplicate = copy.deepcopy(plan); duplicate["outputs"][-1]["role"] = "app-vm"; mutations["duplicate-role"] = duplicate
        missing = copy.deepcopy(plan); missing["outputs"].pop(); mutations["missing"] = missing
        stale = copy.deepcopy(plan); stale["outputs"][0]["source_revision"] = "f" * 40; mutations["stale"] = stale
        unsigned = copy.deepcopy(plan); unsigned["outputs"][0]["signing_identity"] = "0" * 40; mutations["unsigned"] = unsigned
        no_artifact_arg = copy.deepcopy(plan); no_artifact_arg["outputs"][0]["verifier"] = [str(verifier), "literal", "{source_revision}", "{signing_identity}"]; mutations["unverified"] = no_artifact_arg
        no_signer = copy.deepcopy(plan); no_signer["outputs"][0]["verifier"] = [str(verifier), "{artifact}", "{source_revision}"]; mutations["unbound-signer"] = no_signer
        wrong_type = copy.deepcopy(plan); wrong_type["outputs"][0]["media_type"] = "application/x-qemu-disk"; mutations["wrong-type"] = wrong_type
        duplicate_file = copy.deepcopy(plan); duplicate_file["outputs"][-1]["path"] = duplicate_file["outputs"][0]["path"]; mutations["duplicate-file"] = duplicate_file
        external = copy.deepcopy(plan); external["outputs"][0]["verifier"] = ["/bin/true", "{artifact}", "{source_revision}", "{signing_identity}"]; mutations["external-verifier"] = external
        for name, item in mutations.items(): run(collector, root, item, name, False)

        rejected = copy.deepcopy(plan)
        reject_path = root / "reject-artifact"; write(reject_path, b"\xed\xab\xee\xdbno")
        rejected["outputs"][0]["path"] = str(reject_path)
        run(collector, root, rejected, "verifier-rejection", False)
        verifier.write_text(verifier.read_text() + "# replaced after source pin\n")
        run(collector, root, plan, "modified-verifier", False)
        subprocess.run(["git", "-C", str(repo), "checkout", "--", "install-helpers/verify"], check=True)
        run(collector, root, plan, "existing", True)
        second = subprocess.run([sys.executable, str(collector), "--plan", str(root / "existing.json"), "--output", str(root / "existing.out")], capture_output=True)
        assert second.returncode == 2
    print("release-output-collector hostile self-test: PASS")

if __name__ == "__main__": main()
