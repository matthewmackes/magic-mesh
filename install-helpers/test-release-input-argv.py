#!/usr/bin/env python3
"""Focused hostile tests for the private release-input argv loader."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
LOADER = ROOT / "install-helpers/release-input-argv.py"


def check(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"release-input-argv self-test: {message}")


def invoke(loader: Path, document: Path, *identity: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(loader), str(document), *identity], text=True, capture_output=True, check=False
    )


def emit(loader: Path, document: Path, output: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(loader), str(document), "--emit-driver-arguments", str(output)],
        text=True,
        capture_output=True,
        check=False,
    )


def main() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        helpers = root / "install-helpers"
        inputs = root / "inputs"
        helpers.mkdir()
        inputs.mkdir()
        loader = helpers / LOADER.name
        shutil.copy2(LOADER, loader)
        loader.chmod(0o755)

        recorded = root / "recorded.json"
        preflight = helpers / "release-input-preflight.sh"
        preflight.write_text(
            "#!/usr/bin/env python3\n"
            "import json,sys\n"
            f"json.dump(sys.argv[1:],open({str(recorded)!r},'w'),separators=(',',':'))\n",
            encoding="utf-8",
        )
        preflight.chmod(0o755)

        names = (
            "maps-approval.json", "maps-verifier", "release.json", "relay", "agent", "rpm-receipt.json",
            "bootc-receipt.json", "app-base-receipt.json", "cuttlefish-receipt.json", "cuttlefish-image.tar",
            "mcnf-cuttlefish-readiness-relay.deb", "mcnf-cuttlefish-vdi-agent.deb",
        )
        for name in names:
            (inputs / name).write_text(name + "\n", encoding="utf-8")
        maps_root = inputs / "maps"
        maps_root.mkdir()

        document = {
            "schema_version": 1,
            "kind": "mcnf-release-input-argv",
            "source_revision": "1" * 40,
            "source_epoch": "1700000000",
            "maps_approval": str(inputs / "maps-approval.json"),
            "maps_tile_source_root": str(maps_root),
            "maps_quota_bytes": "4096",
            "maps_verifier": str(inputs / "maps-verifier"),
            "cuttlefish_declaration": str(inputs / "release.json"),
            "cuttlefish_readiness_relay": str(inputs / "relay"),
            "cuttlefish_vdi_agent": str(inputs / "agent"),
            "cuttlefish_guest_packages": [
                str(inputs / "mcnf-cuttlefish-readiness-relay.deb"),
                str(inputs / "mcnf-cuttlefish-vdi-agent.deb"),
            ],
            "rpm_signing_identity_receipt": str(inputs / "rpm-receipt.json"),
            "bootc_base_digest_receipt": str(inputs / "bootc-receipt.json"),
            "bootc_base_image_reference": "registry.invalid/mcnf/bootc@sha256:" + "2" * 64,
            "bootc_base_architecture": "amd64",
            "bootc_release_role": "unified-seat-server",
            "app_vm_base_image_receipt": str(inputs / "app-base-receipt.json"),
            "app_vm_base_image_reference": "registry.invalid/mcnf/app@sha256:" + "3" * 64,
            "app_vm_base_architecture": "amd64",
            "cuttlefish_image_receipt": str(inputs / "cuttlefish-receipt.json"),
            "cuttlefish_image_source_kind": "artifact",
            "cuttlefish_image_original_source": str(inputs / "cuttlefish-image.tar"),
            "cuttlefish_image_architecture": "amd64",
            "cuttlefish_provider_identity": "mcnf-cuttlefish",
            "cuttlefish_android_release_id": "android-15.0.0_r1",
            "cuttlefish_image_compatibility_id": "mcnf-cuttlefish-v1",
            "cuttlefish_image_media_type": "application/vnd.mcnf.cuttlefish.image.v1+tar",
            "cuttlefish_image_artifact_format": "android-cuttlefish-image-archive",
        }

        def publish(name: str, value: object, mode: int = 0o400) -> Path:
            path = root / name
            path.write_text(json.dumps(value, separators=(",", ":")), encoding="utf-8")
            path.chmod(mode)
            return path

        good = publish("good.json", document)
        result = invoke(loader, good)
        check(result.returncode == 0, f"valid document refused: {result.stderr}")
        args = json.loads(recorded.read_text(encoding="utf-8"))
        check(args.count("--cuttlefish-guest-package") == 2, "canonical argv omitted package paths")
        reference_index = args.index("--bootc-base-image-reference") + 1
        check(args[reference_index] == document["bootc_base_image_reference"], "image reference changed in transit")
        identity = (
            "--expected-source-revision", str(document["source_revision"]),
            "--expected-source-epoch", str(document["source_epoch"]),
        )
        check(invoke(loader, good, *identity).returncode == 0, "matching checkout identity was refused")
        wrong_identity = (*identity[:1], "f" * 40, *identity[2:])
        check(invoke(loader, good, *wrong_identity).returncode == 2, "cross-revision document was accepted")

        derived = root / "derived-arguments.json"
        check(emit(loader, good, derived).returncode == 0, "derived driver arguments were refused")
        derived_args = json.loads(derived.read_text(encoding="utf-8"))
        check(derived.stat().st_mode & 0o777 == 0o400, "derived arguments were not mode 0400")
        check("--source-revision" not in derived_args, "driver arguments duplicated source revision")
        check("--source-epoch" not in derived_args, "driver arguments duplicated source epoch")
        check(derived_args[0].endswith("/release-input-preflight.sh"), "derived preflight path is wrong")
        check(emit(loader, good, derived).returncode == 2, "existing derived output was overwritten")

        permissive = publish("permissive.json", document, 0o600)
        check(invoke(loader, permissive).returncode == 2, "permissive private-file mode was accepted")

        hard_link = root / "hard-link.json"
        os.link(good, hard_link)
        check(invoke(loader, good).returncode == 2, "multiply linked private file was accepted")
        hard_link.unlink()

        link = root / "link.json"
        link.symlink_to(good)
        check(invoke(loader, link).returncode == 2, "symlinked private file was accepted")
        check(invoke(loader, root / "missing.json").returncode == 2, "missing private file was accepted")

        missing_value = dict(document)
        del missing_value["rpm_signing_identity_receipt"]
        missing = publish("missing-field.json", missing_value)
        check(invoke(loader, missing).returncode == 2, "missing mandatory field was accepted")

        extra_value = dict(document)
        extra_value["credential"] = "must-not-be-accepted"
        extra = publish("extra.json", extra_value)
        check(invoke(loader, extra).returncode == 2, "extra/credential field was accepted")

        malformed = root / "malformed.json"
        malformed.write_text('{"schema_version":1,', encoding="utf-8")
        malformed.chmod(0o400)
        check(invoke(loader, malformed).returncode == 2, "malformed JSON was accepted")

        duplicate = root / "duplicate.json"
        duplicate.write_text('{"schema_version":1,"schema_version":1}', encoding="utf-8")
        duplicate.chmod(0o400)
        check(invoke(loader, duplicate).returncode == 2, "duplicate JSON field was accepted")

        mutable_reference = dict(document)
        mutable_reference["app_vm_base_image_reference"] = "registry.invalid/mcnf/app:latest"
        mutable = publish("mutable-reference.json", mutable_reference)
        check(invoke(loader, mutable).returncode == 2, "mutable image reference was accepted")

        substituted_value = dict(document)
        substituted_value["cuttlefish_image_original_source"] = str(inputs / "missing-image.tar")
        substituted = publish("substituted-reference.json", substituted_value)
        check(invoke(loader, substituted).returncode == 2, "substituted artifact reference was accepted")

    print("release-input-argv self-test: PASS")


if __name__ == "__main__":
    main()
