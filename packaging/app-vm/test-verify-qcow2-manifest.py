#!/usr/bin/env python3
"""Hostile integration tests for the App VM qcow2 manifest verifier."""

from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile


HERE = Path(__file__).resolve().parent
VERIFY = HERE / "verify-qcow2-manifest.py"
REVISION = "0123456789abcdef0123456789abcdef01234567"


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="app-vm-qcow2-verify-") as raw:
        root = Path(raw); root.chmod(0o700)
        image = root / "app-vm-wayland-standard.qcow2"
        image.write_bytes(b"QFI\xfb" + b"governed App VM fixture")
        image.chmod(0o400)
        value = {
            "artifact": {"filename": image.name,
                         "sha256": "sha256:" + hashlib.sha256(image.read_bytes()).hexdigest(),
                         "size": image.stat().st_size},
            "image_profile": "mcnf-app-vm/wayland-standard-v1",
            "kind": "mcnf-app-vm-image-manifest",
            "schema_version": 1,
            "source_revision": REVISION,
        }

        def run(name: str, document: dict[str, object], candidate: Path = image,
                revision: str = REVISION, ok: bool = False) -> None:
            manifest = root / f"{name}.json"
            manifest.write_text(json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n")
            manifest.chmod(0o400)
            result = subprocess.run([str(VERIFY), "--image", str(candidate), "--manifest", str(manifest),
                                     "--source-revision", revision], text=True, capture_output=True)
            assert (result.returncode == 0) == ok, (name, result.stdout, result.stderr)

        run("good", value, ok=True)
        mutations: dict[str, dict[str, object]] = {}
        for name, update in {
            "revision": lambda item: item.update(source_revision="1" + REVISION[1:]),
            "profile": lambda item: item.update(image_profile="mcnf-app-vm/host"),
            "kind": lambda item: item.update(kind="generic-image"),
            "size": lambda item: item["artifact"].update(size=1),
            "digest": lambda item: item["artifact"].update(sha256="sha256:" + "a" * 64),
        }.items():
            hostile = copy.deepcopy(value); update(hostile); mutations[name] = hostile
        for name, document in mutations.items(): run(name, document)
        run("wrong-requested-revision", value, revision="1" + REVISION[1:])
        raw_image = root / "raw.img"; raw_image.write_bytes(b"not qcow2"); raw_image.chmod(0o400)
        raw_value = copy.deepcopy(value)
        raw_value["artifact"] = {"filename": raw_image.name,
                                 "sha256": "sha256:" + hashlib.sha256(raw_image.read_bytes()).hexdigest(),
                                 "size": raw_image.stat().st_size}
        run("raw-image", raw_value, candidate=raw_image)
        linked = root / "linked.qcow2"; linked.symlink_to(image)
        run("symlink-image", value, candidate=linked)
        duplicate = root / "duplicate.json"
        duplicate.write_text(json.dumps(value).replace('"schema_version": 1', '"schema_version": 1, "schema_version": 1'))
        duplicate.chmod(0o400)
        result = subprocess.run([str(VERIFY), "--image", str(image), "--manifest", str(duplicate),
                                 "--source-revision", REVISION])
        assert result.returncode == 2
    print("test-verify-app-vm-qcow2-manifest: hostile suite passed")


if __name__ == "__main__":
    main()
