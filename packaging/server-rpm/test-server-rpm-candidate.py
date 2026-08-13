#!/usr/bin/env python3
"""Hostile integration tests for the governed Server RPM candidate contract."""

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
    producer = Path(__file__).resolve().with_name("produce-server-rpm-candidate.py")
    with tempfile.TemporaryDirectory(prefix="server-rpm-candidate-test-") as raw:
        root = Path(raw); root.chmod(0o700)
        tools = root / "bin"; tools.mkdir(mode=0o700)
        tool(tools / "rpm", """
case " $* " in *' --initdb '*|*' --import '*) exit 0 ;; esac
name=${TEST_RPM_NAME:-magic-mesh-server}; arch=${TEST_RPM_ARCH:-x86_64}
printf '%s\t0\t12.1.6\t23\t%s\n8\t%s\n' "$name" "$arch" "$(printf 'a%.0s' {1..64})"
""")
        tool(tools / "rpmkeys", """
[[ ${TEST_UNSIGNED:-0} == 0 ]] || { echo 'digests OK'; exit 1; }
key_id=${TEST_KEY_ID:-d0921c73}
printf 'Header V4 RSA/SHA256 Signature, key ID %s: OK\n' "$key_id"
""")
        tool(tools / "gpg", f"""
fingerprint={FINGERPRINT}
[[ ${{TEST_WRONG_KEY:-0}} == 0 ]] || fingerprint=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB
printf 'pub:-:4096:1:00000000D0921C73::::::sc:::::::\n'
printf 'fpr:::::::::%s:\n' "$fingerprint"
""")
        tool(tools / "rpm2cpio", "printf 'archive fixture'\n")
        tool(tools / "cpio", f"cat >/dev/null\nprintf '\\177ELF12.1.6Construct{REVISION}bounded\\n'\n")
        rpm = root / "magic-mesh-server.rpm"
        rpm.write_bytes(b"\xed\xab\xee\xdbsigned server fixture\n"); rpm.chmod(0o644)
        key = root / "RPM-GPG-KEY-magic-mesh"
        key.write_text("governed release public key\n", encoding="utf-8"); key.chmod(0o644)
        env = dict(os.environ); env["PATH"] = f"{tools}:/usr/bin:/bin"

        def produce(name: str, extra: dict[str, str] | None = None):
            selected = dict(env); selected.update(extra or {})
            return subprocess.run(
                [str(producer), "produce", "--rpm", str(rpm), "--source-revision", REVISION,
                 "--expected-signing-fingerprint", FINGERPRINT, "--release-key", str(key),
                 "--output", str(root / name)], text=True, capture_output=True,
                env=selected, check=False,
            )

        good = produce("good")
        assert good.returncode == 0, good.stderr
        candidate = root / "good/candidate.rpm"
        manifest = root / "good/candidate-manifest.json"
        value = json.loads(manifest.read_text(encoding="utf-8"))
        assert value["release_role"] == "server-rpm"
        assert value["server_variant_identity"] == "magic-mesh-server/headless-workstation-v1"
        assert value["artifact"]["nevra"] == "magic-mesh-server-12.1.6-23.x86_64"
        assert value["build_identity"]["source_revision"] == REVISION
        assert value["signing_fingerprint"] == FINGERPRINT
        assert candidate.read_bytes() == rpm.read_bytes()
        assert stat.S_IMODE(candidate.stat().st_mode) == 0o400
        assert stat.S_IMODE(manifest.stat().st_mode) == 0o400

        def verify(candidate_path: Path, manifest_path: Path, *, revision: str = REVISION,
                   signer: str = FINGERPRINT, extra: dict[str, str] | None = None):
            selected = dict(env); selected.update(extra or {})
            return subprocess.run(
                [str(producer), "reverify", "--rpm", str(candidate_path),
                 "--source-revision", revision, "--expected-signing-fingerprint", signer,
                 "--release-key", str(key), "--manifest", str(manifest_path)],
                text=True, capture_output=True, env=selected, check=False,
            )

        assert verify(candidate, manifest).returncode == 0
        replacement = root / "replacement.rpm"
        replacement.write_bytes(candidate.read_bytes() + b"mutation"); replacement.chmod(0o400)
        assert verify(replacement, manifest).returncode == 2
        assert verify(candidate, manifest, revision="1" + REVISION[1:]).returncode == 2
        assert verify(candidate, manifest, signer="B" * 40).returncode == 2

        mutations = {
            "role": lambda item: item.update(release_role="workstation-rpm"),
            "variant": lambda item: item.update(server_variant_identity="magic-mesh-lighthouse/thin-control-plane-v1"),
            "revision": lambda item: item["build_identity"].update(source_revision="1" + REVISION[1:]),
            "signer": lambda item: item.update(signing_fingerprint="B" * 40),
            "nevra": lambda item: item["artifact"].update(nevra="magic-mesh-12.1.6-23.x86_64"),
            "digest": lambda item: item["artifact"].update(rpm_sha256="0" * 64),
        }
        for name, mutate in mutations.items():
            hostile = json.loads(json.dumps(value)); mutate(hostile)
            path = root / f"{name}.json"
            path.write_text(json.dumps(hostile, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
            path.chmod(0o400)
            assert verify(candidate, path).returncode == 2, name

        duplicate = root / "duplicate.json"
        duplicate.write_text(manifest.read_text().replace('"schema_version":1', '"schema_version":1,"schema_version":1'), encoding="utf-8")
        duplicate.chmod(0o400)
        assert verify(candidate, duplicate).returncode == 2
        malformed = root / "malformed.json"; malformed.write_text('{"broken":', encoding="utf-8"); malformed.chmod(0o400)
        assert verify(candidate, malformed).returncode == 2

        for name, hostile in {
            "unsigned": {"TEST_UNSIGNED": "1"}, "wrong-key": {"TEST_WRONG_KEY": "1"},
            "wrong-role": {"TEST_RPM_NAME": "magic-mesh"}, "wrong-arch": {"TEST_RPM_ARCH": "aarch64"},
        }.items():
            result = produce(name, hostile)
            assert result.returncode == 2 and not (root / name).exists(), (name, result.stderr)

        rpm.chmod(0o664)
        assert produce("writable-rpm").returncode == 2
        rpm.chmod(0o644)
        key.chmod(0o664)
        assert produce("writable-key").returncode == 2
        key.chmod(0o644)
        writable_manifest = root / "writable.json"
        writable_manifest.write_bytes(manifest.read_bytes()); writable_manifest.chmod(0o660)
        assert verify(candidate, writable_manifest).returncode == 2

        rpm_link = root / "rpm-link"; rpm_link.symlink_to(rpm)
        assert subprocess.run(
            [str(producer), "produce", "--rpm", str(rpm_link), "--source-revision", REVISION,
             "--expected-signing-fingerprint", FINGERPRINT, "--release-key", str(key),
             "--output", str(root / "linked")], env=env, capture_output=True, check=False,
        ).returncode == 2
        manifest_link = root / "manifest-link"; manifest_link.symlink_to(manifest)
        assert verify(candidate, manifest_link).returncode == 2
        key_link = root / "key-link"; key_link.symlink_to(key)
        assert subprocess.run(
            [str(producer), "reverify", "--rpm", str(candidate), "--source-revision", REVISION,
             "--expected-signing-fingerprint", FINGERPRINT, "--release-key", str(key_link),
             "--manifest", str(manifest)], env=env, capture_output=True, check=False,
        ).returncode == 2
    print("test-server-rpm-candidate: hostile producer/reverifier integration passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
