#!/usr/bin/env python3
"""Hermetic end-to-end test for the canonical release-output admission chain."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
PLAN = ROOT / "install-helpers/produce-release-output-plan.py"
COLLECT = ROOT / "install-helpers/collect-release-outputs.py"
SIGNER = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD0921C73"
PAYLOAD_DIGEST = "a" * 64
MEDIA = {
    "app-vm": "application/x-qemu-disk",
    "bootc-image": "application/vnd.mcnf.bootc-image-receipt+json",
    "browser-vm": "application/x-qemu-disk",
    "lighthouse-rpm": "application/x-rpm",
    "server-rpm": "application/x-rpm",
    "workstation-rpm": "application/x-rpm",
}


def write(path: Path, body: bytes, mode: int = 0o400) -> Path:
    path.write_bytes(body)
    path.chmod(mode)
    return path.resolve()


def canonical(path: Path, value: object) -> Path:
    return write(path, (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii"))


def tool(path: Path, body: str) -> None:
    path.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + body, encoding="utf-8")
    path.chmod(0o700)


def run(command: list[str], env: dict[str, str], *, ok: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, text=True, capture_output=True, env=env, check=False)
    assert (result.returncode == 0) == ok, (command, result.stdout, result.stderr)
    return result


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def rpm_manifest(path: Path, rpm: Path, role: str, revision: str) -> Path:
    names = {
        "workstation-rpm": ("magic-mesh-13.0.0-34.x86_64", "mcnf-app-vm-rpm-candidate-manifest", 2),
        "server-rpm": ("magic-mesh-server-13.0.0-23.x86_64", "mcnf-server-rpm-candidate-manifest", 1),
        "lighthouse-rpm": ("magic-mesh-lighthouse-13.0.0-11.x86_64", "mcnf-browser-vm-lighthouse-rpm-candidate-manifest", 1),
    }
    nevra, kind, schema = names[role]
    value: dict[str, object] = {
        "artifact": {"nevra": nevra, "payload_sha256": PAYLOAD_DIGEST, "rpm_sha256": digest(rpm)},
        "build_identity": {"source_revision": revision},
        "kind": kind,
        "schema_version": schema,
        "signing_fingerprint": SIGNER,
    }
    if role == "workstation-rpm":
        value["app_vm_target_identity"] = "mcnf-app-vm/wayland-standard-v1"
    elif role == "server-rpm":
        value["release_role"] = "server-rpm"
        value["server_variant_identity"] = "magic-mesh-server/headless-workstation-v1"
    else:
        value["browser_target_identity"] = "mcnf-browser-vm/browser-vm-chromium-v1"
        value["lighthouse_variant_identity"] = "magic-mesh-lighthouse/thin-control-plane-v1"
    return canonical(path, value)


def fixture(root: Path, env: dict[str, str], revision: str, epoch: str) -> tuple[Path, dict[str, Path]]:
    artifacts = root / "artifacts"
    artifacts.mkdir(mode=0o700)
    key = write(artifacts / "release.asc", b"governed release public key fixture\n")
    files: dict[str, Path] = {}
    for role in ("workstation-rpm", "server-rpm", "lighthouse-rpm"):
        rpm = write(artifacts / f"{role}.rpm", b"\xed\xab\xee\xdb" + role.encode() + b"\n")
        files[role] = rpm
        files[f"{role}-manifest"] = rpm_manifest(artifacts / f"{role}.json", rpm, role, revision)

    browser = write(artifacts / "browser.qcow2", b"QFI\xfbgoverned Browser fixture\n")
    profile_body = (ROOT / "packaging/browser-vm/profile.env").read_text(encoding="utf-8")
    lines = [f"BROWSER_VM_SOURCE_COMMIT={revision}" if line.startswith("BROWSER_VM_SOURCE_COMMIT=") else line
             for line in profile_body.splitlines()]
    profile = write(artifacts / "browser.env", ("\n".join(lines) + "\n").encode())
    browser_manifest = artifacts / "browser.qcow2.mcnf-manifest.json"
    run([
        str(ROOT / "packaging/browser-vm/verify-image-manifest.py"), "create",
        "--repo-root", str(ROOT), "--profile", str(profile), "--image", str(browser),
        "--manifest", str(browser_manifest), "--format", "qcow2",
    ], env)
    browser_manifest.chmod(0o400)
    files.update(browser=browser, browser_profile=profile, browser_manifest=browser_manifest.resolve())

    app = write(artifacts / "app.qcow2", b"QFI\xfbgoverned App fixture\n")
    app_manifest = canonical(artifacts / "app.json", {
        "artifact": {"filename": app.name, "sha256": "sha256:" + digest(app), "size": app.stat().st_size},
        "image_profile": "mcnf-app-vm/wayland-standard-v1", "kind": "mcnf-app-vm-image-manifest",
        "schema_version": 1, "source_revision": revision,
    })
    files.update(app=app, app_manifest=app_manifest)

    image_reference = "registry.invalid/mcnf/construct@sha256:" + "b" * 64
    bootc_receipt = canonical(artifacts / "bootc.json", {
        "architecture": "amd64", "commit_epoch": int(epoch), "image_reference": image_reference,
        "kind": "mcnf-bootc-image-digest", "manifest_media_type": "application/vnd.oci.image.manifest.v1+json",
        "os": "linux", "release_role": "all-roles", "resolved_digest": "sha256:" + "b" * 64,
        "schema_version": 1, "source_revision": revision,
    })
    files["bootc"] = bootc_receipt

    inputs = canonical(root / "inputs.json", {
        "commit_epoch": epoch, "kind": "mcnf-release-output-plan-input", "release_key": str(key),
        "schema_version": 1, "signing_identity": SIGNER, "source_revision": revision,
        "outputs": {
            "workstation-rpm": {"artifact": str(files["workstation-rpm"]), "candidate_manifest": str(files["workstation-rpm-manifest"])},
            "server-rpm": {"artifact": str(files["server-rpm"]), "candidate_manifest": str(files["server-rpm-manifest"])},
            "lighthouse-rpm": {"artifact": str(files["lighthouse-rpm"]), "candidate_manifest": str(files["lighthouse-rpm-manifest"])},
            "browser-vm": {"artifact": str(browser), "frozen_profile": str(profile), "manifest": str(browser_manifest)},
            "app-vm": {"artifact": str(app), "manifest": str(app_manifest)},
            "bootc-image": {"architecture": "amd64", "image_reference": image_reference,
                "receipt": str(bootc_receipt), "release_role": "all-roles"},
        },
    })
    return inputs, files


def main() -> None:
    revision = "0123456789abcdef0123456789abcdef01234567"
    epoch = "1700000000"
    with tempfile.TemporaryDirectory(prefix="release-output-e2e-") as raw:
        root = Path(raw); root.chmod(0o700)
        tools = root / "bin"; tools.mkdir(mode=0o700)
        tool(tools / "rpm", """
case " $* " in *' --initdb '*|*' --import '*) exit 0 ;; esac
rpm=${@: -1}; case "$rpm" in
  *workstation-rpm.rpm) name=magic-mesh; release=34 ;;
  *server-rpm-reverify-*/candidate.rpm|*server-rpm.rpm) name=magic-mesh-server; release=23 ;;
  *browser-lighthouse-verify-*/candidate.rpm|*lighthouse-rpm.rpm) name=magic-mesh-lighthouse; release=11 ;;
  *) exit 2 ;;
esac
printf '%s\\t0\\t13.0.0\\t%s\\tx86_64\\n8\\t%s\\n' "$name" "$release" "$(printf 'a%.0s' {1..64})"
""")
        tool(tools / "rpmkeys", "printf 'Header V4 RSA/SHA256 Signature, key ID d0921c73: OK\\n'\n")
        tool(tools / "gpg", f"printf 'pub:-:4096:1:00000000D0921C73::::::sc:::::::\\n'\nprintf 'fpr:::::::::{SIGNER}:\\n'\n")
        tool(tools / "rpm2cpio", "printf 'archive fixture\\n'\n")
        tool(tools / "cpio", f"cat >/dev/null\nprintf '\\177ELF13.0.0Construct{revision}bounded\\n'\n")
        tool(tools / "qemu-img", "printf '{\"format\":\"qcow2\",\"virtual-size\":68719476736}\\n'\n")
        tool(tools / "git", f"""
case "$3" in
  rev-parse) printf '{revision}\\n' ;;
  show) printf '{epoch}\\n' ;;
  ls-tree) printf '100755 blob 0000000000000000000000000000000000000000\\t%s\\n' "${{@: -1}}" ;;
  diff) exit 0 ;;
  *) exit 2 ;;
esac
""")
        env = dict(os.environ); env["PATH"] = f"{tools}:/usr/bin:/bin"
        inputs, files = fixture(root, env, revision, epoch)

        plan = root / "plan.json"
        run([sys.executable, str(PLAN), "--inputs", str(inputs), "--output", str(plan)], env)
        output = root / "release-outputs.json"
        run([sys.executable, str(COLLECT), "--plan", str(plan), "--output", str(output)], env)
        manifest = json.loads(output.read_text(encoding="utf-8"))
        assert manifest["promotion"] == "forbidden" and manifest["source_revision"] == revision
        rows = manifest["outputs"]
        assert len(rows) == 6 and {row["role"] for row in rows} == set(MEDIA)
        expected_files = {role: files[role] for role in ("workstation-rpm", "server-rpm", "lighthouse-rpm")}
        expected_files.update({"browser-vm": files["browser"], "app-vm": files["app"],
                               "bootc-image": files["bootc"]})
        for row in rows:
            artifact = expected_files[row["role"]]
            assert row["media_type"] == MEDIA[row["role"]]
            assert row["source_revision"] == revision
            if row["role"].endswith("-rpm"):
                assert row["signing_identity"] == SIGNER
            else:
                assert "signing_identity" not in row
            assert row["sha256"] == "sha256:" + digest(artifact) and row["size"] == artifact.stat().st_size
        assert stat.S_IMODE(output.stat().st_mode) == 0o400

        # The complete chain must reject a companion claimed across roles. The
        # producer's global inode ownership boundary should stop it before a
        # collection plan can be published.
        hostile = json.loads(inputs.read_text(encoding="utf-8"))
        hostile["outputs"]["workstation-rpm"]["candidate_manifest"] = str(files["server-rpm-manifest"])
        hostile_input = canonical(root / "cross-role-input.json", hostile)
        hostile_plan = root / "cross-role-plan.json"
        run([sys.executable, str(PLAN), "--inputs", str(hostile_input), "--output", str(hostile_plan)], env, ok=False)
        assert not hostile_plan.exists()

        # Mutation after plan publication must be detected by the repository
        # verifier/digest boundary, not admitted under the original identity.
        mutation_root = root / "mutation"; mutation_root.mkdir(mode=0o700)
        mutation_inputs, mutation_files = fixture(mutation_root, env, revision, epoch)
        mutation_plan = mutation_root / "plan.json"
        run([sys.executable, str(PLAN), "--inputs", str(mutation_inputs), "--output", str(mutation_plan)], env)
        mutation_files["app"].chmod(0o600)
        mutation_files["app"].write_bytes(mutation_files["app"].read_bytes() + b"mutation")
        mutation_files["app"].chmod(0o400)
        run([sys.executable, str(COLLECT), "--plan", str(mutation_plan),
             "--output", str(mutation_root / "output.json")], env, ok=False)
    print("release-output plan/collector six-role integration: PASS")


if __name__ == "__main__":
    main()
