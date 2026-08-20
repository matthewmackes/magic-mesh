#!/usr/bin/env python3
"""Focused hostile tests for the candidate-bound App VM catalog receipt."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
PRODUCER = ROOT / "packaging/app-vm/produce-catalog-receipt.py"
REVISION = "a" * 40
EPOCH = "1700000000"
PINNED = "org.example.App@sha256:" + "b" * 64


def invoke(catalog: Path, output: Path, revision: str = REVISION) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable, str(PRODUCER),
            "--catalog", str(catalog),
            "--source-revision", revision,
            "--source-epoch", EPOCH,
            "--output", str(output),
        ],
        text=True,
        capture_output=True,
        check=False,
    )


def main() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        catalog = root / "catalog.json"
        output = root / "receipt.json"

        catalog.write_text(
            json.dumps({"schema_version": 1, "remote": "curated", "refs": [PINNED]}) + "\n",
            encoding="utf-8",
        )
        catalog.chmod(0o444)
        result = invoke(catalog, output)
        if result.returncode != 0:
            raise SystemExit(f"valid catalog refused: {result.stderr}")
        if output.stat().st_mode & 0o777 != 0o400:
            raise SystemExit("receipt was not published mode 0400")
        receipt = json.loads(output.read_text(encoding="utf-8"))
        if receipt["source_revision"] != REVISION or receipt["refs"] != [PINNED]:
            raise SystemExit("receipt lost candidate identity or immutable ref")

        mutable = root / "mutable.json"
        mutable.write_text(
            json.dumps({"schema_version": 1, "remote": "curated", "refs": ["org.example.App:stable"]}),
            encoding="utf-8",
        )
        mutable.chmod(0o444)
        if invoke(mutable, root / "mutable-receipt.json").returncode == 0:
            raise SystemExit("mutable catalog ref was accepted")

        if invoke(catalog, root / "malformed-revision-receipt.json", "f" * 39).returncode == 0:
            raise SystemExit("malformed source revision was accepted")

        existing = root / "existing.json"
        existing.write_bytes(b"sentinel\n")
        existing.chmod(0o400)
        if invoke(catalog, existing).returncode == 0 or existing.read_bytes() != b"sentinel\n":
            raise SystemExit("existing receipt was overwritten")

    print("produce-catalog-receipt self-test: PASS")


if __name__ == "__main__":
    main()
