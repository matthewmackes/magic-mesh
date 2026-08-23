#!/usr/bin/env python3
"""Hostile tests for mint-lifecycle-confirmation-dest.py. No production dest."""

from __future__ import annotations

import json
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
HELPER = HERE / "mint-lifecycle-confirmation-dest.py"
PRODUCTION = Path("/root/mcnf-private")


def invoke(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(HELPER), *args],
        text=True,
        capture_output=True,
    )


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="lifecycle-confirm-mint-") as raw:
        root = Path(raw)
        root.chmod(0o700)
        dest = root / "seed"
        sidecar = root / "sidecar.json"
        first = invoke("--output", str(dest), "--sidecar", str(sidecar))
        assert first.returncode == 0, first.stderr
        assert stat.S_IMODE(dest.stat().st_mode) == 0o600
        assert dest.stat().st_size == 32
        record = json.loads(sidecar.read_text(encoding="utf-8"))
        assert record["kind"] == "mcnf-lifecycle-confirmation-dest"
        assert record["production_admitted"] is False
        assert record["enroll_succeeded"] is False
        assert len(record["verifying_key_sha256"]) == 64
        assert dest.read_bytes().hex() not in first.stdout
        assert dest.read_bytes().hex() not in first.stderr
        again = invoke("--output", str(dest), "--sidecar", str(root / "other.json"))
        assert again.returncode == 2
        assert dest.stat().st_size == 32
        assert str(PRODUCTION) not in first.stdout
    print("test-mint-lifecycle-confirmation-dest: PASS")


if __name__ == "__main__":
    main()
