#!/usr/bin/env python3
"""Hostile end-to-end tests for the App VM RPM candidate producer."""

from __future__ import annotations

import json
import os
import stat
import subprocess
import tempfile
from pathlib import Path


REVISION = "0123456789abcdef0123456789abcdef01234567"
FINGERPRINT = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD0921C73"


def write_tool(path: Path, body: str) -> None:
    path.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + body, encoding="utf-8")
    path.chmod(0o700)


def main() -> int:
    app_vm = Path(__file__).resolve().parent
    producer = app_vm / "produce-rpm-candidate-manifest.py"
    verifier = app_vm / "verify-rpm-supply.sh"
    with tempfile.TemporaryDirectory(prefix="app-vm-candidate-producer-test-") as raw:
        root = Path(raw)
        root.chmod(0o700)
        tools = root / "bin"
        tools.mkdir(mode=0o700)
        write_tool(
            tools / "rpm",
            """
case " $* " in *' --initdb '*|*' --import '*) exit 0 ;; esac
printf 'magic-mesh\\t0\\t13.0.0\\t33\\tx86_64\\n8\\t%s\\n' "$(printf 'a%.0s' {1..64})"
""",
        )
        write_tool(
            tools / "rpmkeys",
            """
if [[ ${TEST_UNSIGNED:-0} == 1 ]]; then echo 'digests OK'; exit 1; fi
printf 'Header V4 RSA/SHA256 Signature, key ID d0921c73: OK\\n'
""",
        )
        write_tool(
            tools / "gpg",
            f"""
fingerprint={FINGERPRINT}
[[ ${{TEST_WRONG_GOVERNED_KEY:-0}} == 0 ]] || fingerprint=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB
printf 'pub:-:4096:1:00000000D0921C73::::::sc:::::::\\n'
printf 'fpr:::::::::%s:\\n' "$fingerprint"
""",
        )
        write_tool(tools / "rpm2cpio", "printf 'candidate.rpm\\n'\n")
        write_tool(
            tools / "cpio",
            f"""
read -r ignored
member=''
for value in "$@"; do member=$value; done
case "$member" in ./usr/bin/mackesd|./usr/bin/mde-shell-egui) ;; *) exit 2 ;; esac
printf '\\177ELF13.0.0Construct{REVISION}2026-08-13dev\\n'
""",
        )
        rpm = root / "magic-mesh.rpm"
        rpm.write_bytes(b"exact governed RPM fixture\n")
        rpm.chmod(0o400)
        key = root / "RPM-GPG-KEY-magic-mesh"
        key.write_text("governed public key fixture\n", encoding="utf-8")
        key.chmod(0o400)
        env = dict(os.environ)
        env["PATH"] = f"{tools}:/usr/bin:/bin"

        def invoke(name: str, extra: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
            selected = dict(env)
            selected.update(extra or {})
            return subprocess.run(
                [
                    str(producer), "--rpm", str(rpm), "--source-revision", REVISION,
                    "--release-key", str(key), "--output", str(root / name),
                ],
                text=True, capture_output=True, env=selected, check=False,
            )

        good = invoke("good")
        assert good.returncode == 0, good.stderr
        output = root / "good"
        manifest = output / "candidate-manifest.json"
        value = json.loads(manifest.read_text(encoding="utf-8"))
        assert value["app_vm_target_identity"] == "mcnf-app-vm/wayland-standard-v1"
        assert value["artifact"]["nevra"] == "magic-mesh-13.0.0-33.x86_64"
        assert value["build_identity"]["source_revision"] == REVISION
        assert value["signing_fingerprint"] == FINGERPRINT
        assert stat.S_IMODE(output.stat().st_mode) == 0o700
        assert stat.S_IMODE(manifest.stat().st_mode) == 0o400
        admitted = subprocess.run(
            [str(verifier), "--key", str(key), "--source-commit", REVISION,
             "--candidate-manifest", str(manifest), "--", str(rpm)],
            text=True, capture_output=True, env=env, check=False,
        )
        assert admitted.returncode == 0, admitted.stderr

        def verify(candidate_rpm: Path, candidate_manifest: Path) -> subprocess.CompletedProcess[str]:
            return subprocess.run(
                [str(verifier), "--key", str(key), "--source-commit", REVISION,
                 "--candidate-manifest", str(candidate_manifest), "--", str(candidate_rpm)],
                text=True, capture_output=True, env=env, check=False,
            )

        replaced_rpm = root / "replaced.rpm"
        replaced_rpm.write_bytes(rpm.read_bytes() + b"substitution\n")
        replaced_rpm.chmod(0o400)
        assert verify(replaced_rpm, manifest).returncode == 2

        for name, mutate in {
            "wrong-target": lambda item: item.update(app_vm_target_identity="mcnf-app-vm/other-v1"),
            "wrong-signer": lambda item: item.update(signing_fingerprint="B" * 40),
            "wrong-nevra": lambda item: item["artifact"].update(nevra="magic-mesh-99-1.x86_64"),
        }.items():
            value_copy = json.loads(json.dumps(value))
            mutate(value_copy)
            hostile_manifest = root / f"{name}.json"
            hostile_manifest.write_text(
                json.dumps(value_copy, sort_keys=True, separators=(",", ":")) + "\n",
                encoding="utf-8",
            )
            hostile_manifest.chmod(0o400)
            assert verify(rpm, hostile_manifest).returncode == 2, name

        for name, hostile in {
            "unsigned": {"TEST_UNSIGNED": "1"},
            "wrong-authority": {"TEST_WRONG_GOVERNED_KEY": "1"},
        }.items():
            result = invoke(name, hostile)
            assert result.returncode == 2, (name, result.stdout, result.stderr)
            assert "REFUSED[WL-FUNC-018/rpm-candidate-producer]" in result.stderr
            assert not (root / name).exists()

        # Change the requested revision to prove verifier-bound BuildInfo
        # refusal before output publication.
        stale = subprocess.run(
            [str(producer), "--rpm", str(rpm), "--source-revision", "1" + REVISION[1:],
             "--release-key", str(key), "--output", str(root / "stale")],
            text=True, capture_output=True, env=env, check=False,
        )
        assert stale.returncode == 2 and not (root / "stale").exists()

        existing = root / "existing"
        existing.mkdir(mode=0o700)
        marker = existing / "marker"
        marker.write_text("preserved\n", encoding="utf-8")
        refused = invoke("existing")
        assert refused.returncode == 2 and marker.read_text(encoding="utf-8") == "preserved\n"

        symlink = root / "symlink.rpm"
        symlink.symlink_to(rpm)
        refused = subprocess.run(
            [str(producer), "--rpm", str(symlink), "--source-revision", REVISION,
             "--release-key", str(key), "--output", str(root / "symlink-out")],
            text=True, capture_output=True, env=env, check=False,
        )
        assert refused.returncode == 2 and not (root / "symlink-out").exists()

        open_parent = root / "open"
        open_parent.mkdir(mode=0o777)
        open_parent.chmod(0o777)
        refused = subprocess.run(
            [str(producer), "--rpm", str(rpm), "--source-revision", REVISION,
             "--release-key", str(key), "--output", str(open_parent / "candidate")],
            text=True, capture_output=True, env=env, check=False,
        )
        assert refused.returncode == 2 and not (open_parent / "candidate").exists()
    print("test-produce-rpm-candidate-manifest: hostile producer/verifier integration passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
