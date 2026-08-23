#!/usr/bin/env python3
"""Hostile self-test for admit-unpublished-signed-candidate.py.

No network. No live RPM sign. Tests use temp dirs outside the helper git
worktree and must never write /root/mcnf-private/. Fixture RPM bytes are
not a production candidate.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
HELPER = HERE / "admit-unpublished-signed-candidate.py"
PRODUCTION = Path("/root/mcnf-private")
SIGNER = "06B1C27EA0E08A225155EB3314018AA1497DDC7C"


def resolve_repo() -> Path:
    result = subprocess.run(
        ["git", "-C", str(HERE), "rev-parse", "--show-toplevel"],
        check=False,
        capture_output=True,
        text=True,
    )
    root = result.stdout.strip()
    if result.returncode == 0 and root:
        return Path(root).resolve()
    return HERE.parent.resolve()


REPO = resolve_repo()


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_rpm(path: Path, body: bytes) -> str:
    path.write_bytes(body)
    os.chmod(path, 0o400)
    return digest(body)


def sidecar(
    roles: dict[str, dict[str, str]],
    *,
    kind: str = "mcnf-unpublished-signed-candidate",
    published: bool = False,
    production_admitted: bool = False,
    signer: str = SIGNER,
) -> dict[str, object]:
    return {
        "kind": kind,
        "schema_version": 1,
        "published": published,
        "production_admitted": production_admitted,
        "signer_fingerprint": signer,
        "roles": roles,
    }


def write_sidecar(path: Path, record: dict[str, object]) -> None:
    path.write_text(
        json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="ascii",
    )
    os.chmod(path, 0o400)


def command(*args: str, dest: Path | None = None, refused: bool = False) -> subprocess.CompletedProcess[str]:
    argv = [sys.executable, str(HELPER)]
    if dest is not None:
        argv.extend(["--dest", str(dest)])
    argv.extend(args)
    result = subprocess.run(argv, check=False, capture_output=True, text=True)
    if refused:
        assert result.returncode == 2, result.stderr or result.stdout
        assert result.stdout == "", result.stdout
        assert "REFUSED:" in result.stderr
        assert "{{JOIN_TOKEN}}" not in result.stderr
    else:
        assert result.returncode == 0, result.stderr
        assert "REFUSED:" not in result.stderr, result.stderr
    return result


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="mcnf-admit-candidate-") as temporary:
        root = Path(temporary)
        missing = root / "missing.json"
        missing_result = command(dest=missing, refused=True)
        assert "unpublished signed candidate is absent" in missing_result.stderr

        rpms = {}
        role_records = {}
        for name, body in (
            ("workstation", b"ws-rpm-bytes"),
            ("server", b"server-rpm-bytes"),
            ("lighthouse", b"lh-rpm-bytes"),
        ):
            path = root / f"magic-mesh-{name}.rpm"
            role_records[name] = {
                "path": str(path),
                "sha256": write_rpm(path, body),
                "nevra": {
                    "workstation": "magic-mesh-13.0.0-1.fc44.x86_64",
                    "server": "magic-mesh-server-13.0.0-1.fc44.x86_64",
                    "lighthouse": "magic-mesh-lighthouse-13.0.0-1.fc44.x86_64",
                }[name],
            }
            rpms[name] = path

        dest = root / "unpublished-signed-candidate.json"
        write_sidecar(dest, sidecar(role_records))
        admitted = command(dest=dest)
        assert "production_admitted=false" in admitted.stdout

        dest_pub = root / "published.json"
        write_sidecar(dest_pub, sidecar(role_records, published=True))
        command(dest=dest_pub, refused=True)

        dest_prod = root / "production.json"
        write_sidecar(dest_prod, sidecar(role_records, production_admitted=True))
        command(dest=dest_prod, refused=True)

        dest_kind = root / "kind.json"
        write_sidecar(dest_kind, sidecar(role_records, kind="mcnf-maps-mbtiles-receipt"))
        command(dest=dest_kind, refused=True)

        dest_signer = root / "signer.json"
        write_sidecar(dest_signer, sidecar(role_records, signer="0" * 40))
        command(dest=dest_signer, refused=True)

        dest_old = root / "old-nevra.json"
        old = dict(role_records)
        old["workstation"] = dict(role_records["workstation"])
        old["workstation"]["nevra"] = "magic-mesh-12.1.6-35.x86_64"
        write_sidecar(dest_old, sidecar(old))
        old_result = command(dest=dest_old, refused=True)
        assert "13.0.0" in old_result.stderr

        dest_digest = root / "digest.json"
        bad = dict(role_records)
        bad["server"] = dict(role_records["server"])
        bad["server"]["sha256"] = "ab" * 32
        write_sidecar(dest_digest, sidecar(bad))
        command(dest=dest_digest, refused=True)

        dest_extra = root / "extra.json"
        extra = dict(role_records)
        extra["browser"] = role_records["workstation"]
        write_sidecar(dest_extra, sidecar(extra))
        command(dest=dest_extra, refused=True)

        linked = root / "linked.json"
        linked.symlink_to(dest)
        command(dest=linked, refused=True)

        inside = REPO / "install-helpers" / f".qu0026ad-cand-{os.getpid()}.json"
        try:
            write_sidecar(inside, sidecar(role_records))
            command(dest=inside, refused=True)
        finally:
            if inside.exists():
                inside.unlink()

        token = root / "{{JOIN_TOKEN}}.json"
        write_sidecar(token, sidecar(role_records))
        token_result = command(dest=token, refused=True)
        assert "{{JOIN_TOKEN}}" not in token_result.stderr

        for path in rpms.values():
            assert stat.S_IMODE(path.stat().st_mode) == 0o400
        assert not str(root).startswith(str(PRODUCTION))

        spec = importlib.util.spec_from_file_location("admit_candidate", HELPER)
        module = importlib.util.module_from_spec(spec)
        assert spec.loader is not None
        spec.loader.exec_module(module)
        os.environ[module.DEST_ENV] = str(dest)
        try:
            try:
                module.admit_unpublished_signed_candidate(for_production_mutation=True)
            except module.Refusal as error:
                assert "unpublished signed candidate is absent" in str(error)
            else:
                raise AssertionError("env override must not unlock production mutation")
            try:
                module.verify_rpm_signature(rpms["workstation"])
            except module.Refusal as error:
                assert "GPG-signed" in str(error) or "signature" in str(error).lower()
            else:
                raise AssertionError("fixture bytes must not count as GPG-signed")
        finally:
            os.environ.pop(module.DEST_ENV, None)

        historical = Path("/root/mcnf-release-artifacts/magic-mesh-12.1.6-35.x86_64.rpm")
        if historical.is_file():
            try:
                module.verify_rpm_signature(historical)
            except module.Refusal as error:
                text = str(error)
                assert "GPG-signed" in text or "governed fingerprint" in text
            else:
                raise AssertionError("unsigned 12.1.6 RPM must not count as governed-signed")

    print("admit unpublished signed candidate hostile suite passed")


if __name__ == "__main__":
    main()
