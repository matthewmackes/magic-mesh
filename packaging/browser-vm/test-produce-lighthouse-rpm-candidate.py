#!/usr/bin/env python3
"""Hostile producer/verifier integration for the Browser Lighthouse RPM input."""

from __future__ import annotations

import json
import os
import stat
import subprocess
import tempfile
from pathlib import Path


REVISION = "0123456789abcdef0123456789abcdef01234567"
FINGERPRINT = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD0921C73"


def tool(path: Path, body: str) -> None:
    path.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + body, encoding="utf-8")
    path.chmod(0o700)


def main() -> int:
    producer = Path(__file__).resolve().with_name("produce-lighthouse-rpm-candidate.py")
    with tempfile.TemporaryDirectory(prefix="browser-lighthouse-candidate-test-") as raw:
        root = Path(raw)
        root.chmod(0o700)
        tools = root / "bin"
        tools.mkdir(mode=0o700)
        tool(tools / "rpm", """
case " $* " in *' --initdb '*|*' --import '*) exit 0 ;; esac
name=${TEST_RPM_NAME:-magic-mesh-lighthouse}
arch=${TEST_RPM_ARCH:-x86_64}
printf '%s\t0\t12.1.6\t11\t%s\n8\t%s\n' "$name" "$arch" "$(printf 'a%.0s' {1..64})"
""")
        tool(tools / "rpmkeys", """
[[ ${TEST_UNSIGNED:-0} == 0 ]] || { echo 'digests OK'; exit 1; }
printf 'Header V4 RSA/SHA256 Signature, key ID d0921c73: OK\n'
""")
        tool(tools / "gpg", f"""
fingerprint={FINGERPRINT}
[[ ${{TEST_WRONG_KEY:-0}} == 0 ]] || fingerprint=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB
printf 'pub:-:4096:1:00000000D0921C73::::::sc:::::::\n'
printf 'fpr:::::::::%s:\n' "$fingerprint"
""")
        tool(tools / "rpm2cpio", "printf 'archive fixture'\n")
        tool(tools / "cpio", f"""
cat >/dev/null
printf '\177ELF12.1.6Construct{REVISION}bounded\n'
""")
        rpm = root / "magic-mesh-lighthouse.rpm"
        rpm.write_bytes(b"signed lighthouse fixture\n")
        rpm.chmod(0o400)
        key = root / "RPM-GPG-KEY-magic-mesh"
        key.write_text("governed public key\n", encoding="utf-8")
        key.chmod(0o400)
        env = dict(os.environ)
        env["PATH"] = f"{tools}:/usr/bin:/bin"

        def produce(name: str, extra: dict[str, str] | None = None):
            selected = dict(env)
            selected.update(extra or {})
            return subprocess.run(
                [str(producer), "produce", "--rpm", str(rpm), "--source-revision", REVISION,
                 "--release-key", str(key), "--output", str(root / name)],
                text=True, capture_output=True, env=selected, check=False,
            )

        good = produce("good")
        assert good.returncode == 0, good.stderr
        manifest = root / "good/candidate-manifest.json"
        value = json.loads(manifest.read_text(encoding="utf-8"))
        assert value["artifact"]["nevra"] == "magic-mesh-lighthouse-12.1.6-11.x86_64"
        assert value["browser_target_identity"] == "mcnf-browser-vm/browser-vm-chromium-v1"
        assert value["lighthouse_variant_identity"] == "magic-mesh-lighthouse/thin-control-plane-v1"
        assert value["build_identity"]["source_revision"] == REVISION
        assert value["signing_fingerprint"] == FINGERPRINT
        assert stat.S_IMODE(manifest.stat().st_mode) == 0o400

        def verify(candidate: Path, document: Path, extra: dict[str, str] | None = None):
            selected = dict(env)
            selected.update(extra or {})
            return subprocess.run(
                [str(producer), "verify", "--rpm", str(candidate), "--source-revision", REVISION,
                 "--release-key", str(key), "--manifest", str(document)],
                text=True, capture_output=True, env=selected, check=False,
            )

        assert verify(rpm, manifest).returncode == 0
        replacement = root / "replacement.rpm"
        replacement.write_bytes(rpm.read_bytes() + b"changed")
        replacement.chmod(0o400)
        assert verify(replacement, manifest).returncode == 2

        for name, mutate in {
            "target": lambda item: item.update(browser_target_identity="mcnf-app-vm/wayland-standard-v1"),
            "variant": lambda item: item.update(lighthouse_variant_identity="magic-mesh/workstation-v1"),
            "revision": lambda item: item["build_identity"].update(source_revision="1" + REVISION[1:]),
            "nevra": lambda item: item["artifact"].update(nevra="magic-mesh-12.1.6-11.x86_64"),
            "signer": lambda item: item.update(signing_fingerprint="B" * 40),
        }.items():
            hostile = json.loads(json.dumps(value))
            mutate(hostile)
            path = root / f"{name}.json"
            path.write_text(json.dumps(hostile, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
            path.chmod(0o400)
            assert verify(rpm, path).returncode == 2, name

        for name, hostile in {
            "unsigned": {"TEST_UNSIGNED": "1"},
            "wrong-key": {"TEST_WRONG_KEY": "1"},
            "wrong-name": {"TEST_RPM_NAME": "magic-mesh"},
            "wrong-arch": {"TEST_RPM_ARCH": "aarch64"},
        }.items():
            result = produce(name, hostile)
            assert result.returncode == 2, (name, result.stdout, result.stderr)
            assert not (root / name).exists()

        stale_env = dict(env)
        stale_env["TEST_RPM_NAME"] = "magic-mesh-lighthouse"
        stale = subprocess.run(
            [str(producer), "produce", "--rpm", str(rpm), "--source-revision", "1" + REVISION[1:],
             "--release-key", str(key), "--output", str(root / "stale")],
            text=True, capture_output=True, env=stale_env, check=False,
        )
        assert stale.returncode == 2 and not (root / "stale").exists()

        duplicate = root / "duplicate.json"
        duplicate.write_text(manifest.read_text(encoding="utf-8").replace('"schema_version":1', '"schema_version":1,"schema_version":1'), encoding="utf-8")
        duplicate.chmod(0o400)
        assert verify(rpm, duplicate).returncode == 2

        manifest_link = root / "manifest-link.json"
        manifest_link.symlink_to(manifest)
        assert verify(rpm, manifest_link).returncode == 2
        key_link = root / "key-link"
        key_link.symlink_to(key)
        linked_key = subprocess.run(
            [str(producer), "verify", "--rpm", str(rpm), "--source-revision", REVISION,
             "--release-key", str(key_link), "--manifest", str(manifest)],
            text=True, capture_output=True, env=env, check=False,
        )
        assert linked_key.returncode == 2
    print("test-produce-lighthouse-rpm-candidate: hostile producer/verifier integration passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
