#!/usr/bin/env python3
"""Hostile tests for the canonical six-role release-output plan producer."""

from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
PRODUCER = ROOT / "install-helpers/produce-release-output-plan.py"
REVISION = "1" * 40
SIGNER = "A" * 40


def immutable(path: Path, payload: bytes = b"fixture\n") -> str:
    path.write_bytes(payload)
    path.chmod(0o644)
    return str(path.resolve())


def invoke(root: Path, value: object, name: str, ok: bool) -> tuple[subprocess.CompletedProcess[str], Path]:
    source = root / f"{name}.input.json"
    source.write_text(json.dumps(value), encoding="utf-8")
    source.chmod(0o644)
    output = root / f"{name}.plan.json"
    result = subprocess.run(
        [sys.executable, str(PRODUCER), "--inputs", str(source), "--output", str(output)],
        text=True, capture_output=True,
    )
    assert (result.returncode == 0) == ok, (name, result.stdout, result.stderr)
    return result, output


def fixture(root: Path) -> dict[str, object]:
    artifacts = root / "artifacts"
    artifacts.mkdir(mode=0o700)
    files = {}
    for name in (
        "workstation.rpm", "workstation.json", "server.rpm", "server.json",
        "lighthouse.rpm", "lighthouse.json", "browser.qcow2", "browser.json",
        "browser.env", "app.qcow2", "app.json",
        "bootc.json", "release.asc",
    ):
        payload = b"QFI\xfbfixture" if name.endswith(".qcow2") else b"fixture\n"
        files[name] = immutable(artifacts / name, payload)
    return {
        "schema_version": 1,
        "kind": "mcnf-release-output-plan-input",
        "source_revision": REVISION,
        "commit_epoch": "1700000000",
        "signing_identity": SIGNER,
        "release_key": files["release.asc"],
        "outputs": {
            "workstation-rpm": {"artifact": files["workstation.rpm"], "candidate_manifest": files["workstation.json"]},
            "server-rpm": {"artifact": files["server.rpm"], "candidate_manifest": files["server.json"]},
            "lighthouse-rpm": {"artifact": files["lighthouse.rpm"], "candidate_manifest": files["lighthouse.json"]},
            "browser-vm": {"artifact": files["browser.qcow2"], "manifest": files["browser.json"], "frozen_profile": files["browser.env"]},
            "app-vm": {"artifact": files["app.qcow2"], "manifest": files["app.json"]},
            "bootc-image": {
                "receipt": files["bootc.json"],
                "image_reference": "registry.invalid/mcnf/construct@sha256:" + "b" * 64,
                "architecture": "amd64", "release_role": "all-roles",
            },
        },
    }


def assert_schema(value: dict[str, object], source: dict[str, object]) -> None:
    assert set(value) == {"schema_version", "kind", "source_revision", "outputs"}
    assert value["schema_version"] == 1 and value["kind"] == "mcnf-release-output-collection-plan"
    assert value["source_revision"] == REVISION
    rows = value["outputs"]
    assert isinstance(rows, list) and [row["role"] for row in rows] == [
        "workstation-rpm", "server-rpm", "lighthouse-rpm", "browser-vm",
        "app-vm", "bootc-image",
    ]
    by_role = {row["role"]: row for row in rows}
    common = {"role", "path", "media_type", "source_revision", "companions", "verifier"}
    rpm_roles = {"workstation-rpm", "server-rpm", "lighthouse-rpm"}
    assert all(row["source_revision"] == REVISION for row in rows)
    assert all(
        set(row) == common | {"signing_identity"} and row["signing_identity"] == SIGNER
        for row in rows if row["role"] in rpm_roles
    )
    assert all(set(row) == common for row in rows if row["role"] not in rpm_roles)
    assert by_role["workstation-rpm"]["verifier"] == [
        str(ROOT / "packaging/app-vm/verify-rpm-supply.sh"), "--key", "{companion:release_key}",
        "--source-commit", "{source_revision}", "--candidate-manifest", "{companion:candidate_manifest}",
        "--expected-signing-fingerprint", "{signing_identity}", "--", "{artifact}",
    ]
    assert by_role["server-rpm"]["verifier"][1:4] == ["reverify", "--rpm", "{artifact}"]
    assert by_role["lighthouse-rpm"]["verifier"][1:4] == ["verify", "--rpm", "{artifact}"]
    browser_argv = by_role["browser-vm"]["verifier"]
    assert browser_argv[-2:] == ["--source-revision", "{source_revision}"]
    assert browser_argv[1] == "verify" and "{companion:frozen_profile}" in browser_argv and "{companion:manifest}" in browser_argv
    assert by_role["app-vm"]["verifier"][-2:] == ["--source-revision", "{source_revision}"]
    assert all(
        "{signing_identity}" not in row["verifier"]
        for row in rows if row["role"] not in rpm_roles
    )
    bootc = by_role["bootc-image"]
    bootc_input = source["outputs"]["bootc-image"]
    assert bootc["media_type"] == "application/vnd.mcnf.bootc-image-receipt+json"
    assert bootc["path"] == bootc_input["receipt"] and bootc["companions"] == {}
    assert bootc["verifier"][bootc["verifier"].index("--receipt") + 1] == "{artifact}"
    assert all("verifier" not in row for row in source["outputs"].values())


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="release-output-plan-test-") as raw:
        root = Path(raw)
        root.chmod(0o700)
        good = fixture(root)
        _, output = invoke(root, good, "good", True)
        document = json.loads(output.read_text(encoding="utf-8"))
        assert_schema(document, good)
        assert stat.S_IMODE(output.stat().st_mode) == 0o400
        previous = output.read_bytes()
        result = subprocess.run(
            [sys.executable, str(PRODUCER), "--inputs", str(root / "good.input.json"), "--output", str(output)],
            text=True, capture_output=True,
        )
        assert result.returncode == 2 and output.read_bytes() == previous

        mutations: dict[str, object] = {}
        missing = copy.deepcopy(good); del missing["outputs"]["bootc-image"]; mutations["missing-role"] = missing
        extra = copy.deepcopy(good); extra["outputs"]["extra"] = {}; mutations["extra-role"] = extra
        caller_argv = copy.deepcopy(good); caller_argv["outputs"]["app-vm"]["verifier"] = ["/bin/true"]; mutations["caller-verifier"] = caller_argv
        stale = copy.deepcopy(good); stale["source_revision"] = "0" * 40; mutations["null-revision"] = stale
        unsigned = copy.deepcopy(good); unsigned["signing_identity"] = "0" * 40; mutations["null-signer"] = unsigned
        bad_epoch = copy.deepcopy(good); bad_epoch["commit_epoch"] = 1700000000; mutations["numeric-epoch"] = bad_epoch
        transport = copy.deepcopy(good); transport["outputs"]["bootc-image"]["image_reference"] = "docker://bad"; mutations["transport-reference"] = transport
        duplicate = copy.deepcopy(good); duplicate["outputs"]["app-vm"]["manifest"] = duplicate["outputs"]["browser-vm"]["manifest"]; mutations["duplicate-file"] = duplicate
        duplicate_key = copy.deepcopy(good); duplicate_key["release_key"] = duplicate_key["outputs"]["app-vm"]["manifest"]; mutations["duplicate-release-key"] = duplicate_key
        relative = copy.deepcopy(good); relative["outputs"]["app-vm"]["artifact"] = "relative.qcow2"; mutations["relative-path"] = relative
        for name, value in mutations.items():
            invoke(root, value, name, False)

        writable = copy.deepcopy(good)
        writable_path = Path(writable["outputs"]["app-vm"]["artifact"])
        writable_path.chmod(0o664)
        invoke(root, writable, "writable-artifact", False)
        writable_path.chmod(0o644)

        symlink = root / "artifact-link"
        symlink.symlink_to(good["outputs"]["app-vm"]["artifact"])
        linked = copy.deepcopy(good); linked["outputs"]["app-vm"]["artifact"] = str(symlink)
        invoke(root, linked, "symlink-artifact", False)

        duplicate_json = root / "duplicate-json.input.json"
        duplicate_json.write_text('{"schema_version":1,"schema_version":1}\n', encoding="utf-8")
        duplicate_json.chmod(0o644)
        result = subprocess.run(
            [sys.executable, str(PRODUCER), "--inputs", str(duplicate_json), "--output", str(root / "duplicate-json.out")],
            text=True, capture_output=True,
        )
        assert result.returncode == 2
    print("release-output plan producer hostile self-test: PASS")


if __name__ == "__main__":
    main()
