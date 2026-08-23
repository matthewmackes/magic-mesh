#!/usr/bin/env python3
"""Hostile tests for sign-lifecycle-confirmation.py. No production dest."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
MINT = HERE / "mint-lifecycle-confirmation-dest.py"
SIGN = HERE / "sign-lifecycle-confirmation.py"


def run(helper: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(helper), *args],
        text=True,
        capture_output=True,
    )


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="lifecycle-confirm-sign-") as raw:
        root = Path(raw)
        root.chmod(0o700)
        seed = root / "seed"
        sidecar = root / "sidecar.json"
        minted = run(MINT, "--output", str(seed), "--sidecar", str(sidecar))
        assert minted.returncode == 0, minted.stderr
        session = "offboard-peer:test-seat-1"
        scope = "ab" * 32
        dest = root / "confirmation.json"
        signed = run(
            SIGN,
            "--seed",
            str(seed),
            "--session-id",
            session,
            "--generation",
            "9",
            "--scope-digest-hex",
            scope,
            "--output",
            str(dest),
        )
        assert signed.returncode == 0, signed.stderr
        body = json.loads(dest.read_text(encoding="utf-8"))
        assert body["action"] == "offboard"
        assert body["session_id"] == session
        assert body["generation"] == 9
        assert body["phrase"] == "FORCE OFFBOARD 1 SYSTEMS"
        assert len(body["signature_hex"]) == 128
        assert seed.read_bytes().hex() not in signed.stdout
        again = run(
            SIGN,
            "--seed",
            str(seed),
            "--session-id",
            session,
            "--generation",
            "9",
            "--scope-digest-hex",
            scope,
            "--output",
            str(dest),
        )
        assert again.returncode == 2
        bad = run(
            SIGN,
            "--seed",
            str(seed),
            "--session-id",
            "wipe-all",
            "--generation",
            "9",
            "--scope-digest-hex",
            scope,
            "--output",
            str(root / "bad.json"),
        )
        assert bad.returncode == 2
    print("test-sign-lifecycle-confirmation: PASS")


if __name__ == "__main__":
    main()
