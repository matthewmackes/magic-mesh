#!/usr/bin/env python3
"""Hostile tests for acquire-flathub-curated-catalog.py. No production dest."""

from __future__ import annotations

import importlib.util
import io
import json
import stat
import sys
import tempfile
from pathlib import Path
from urllib.error import URLError

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("acquire_flathub", HERE / "acquire-flathub-curated-catalog.py")
assert SPEC and SPEC.loader
MOD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MOD)

COMMIT = "ff822a56a9fa65aad0296c0ed07b4ac6e6faf8ed2af93771b5a26b1acd82a303"


class FakeResponse:
    def __init__(self, body: bytes) -> None:
        self._body = body

    def read(self) -> bytes:
        return self._body

    def __enter__(self) -> "FakeResponse":
        return self

    def __exit__(self, *_args: object) -> None:
        return None


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="flathub-catalog-") as raw:
        root = Path(raw)
        root.chmod(0o700)
        dest = root / "catalog.json"
        sidecar = root / "sidecar.json"

        def fake_open(_url: str, timeout: int = 0) -> FakeResponse:
            return FakeResponse((COMMIT + "\n").encode("ascii"))

        MOD.urllib.request.urlopen = fake_open  # type: ignore[method-assign]
        record = MOD.acquire("org.libreoffice.LibreOffice", "https://example.test/ref", dest, sidecar)
        assert record["production_admitted"] is False
        catalog = json.loads(dest.read_text(encoding="utf-8"))
        assert catalog["remote"] == "curated"
        assert catalog["refs"] == [f"org.libreoffice.LibreOffice@sha256:{COMMIT}"]
        assert stat.S_IMODE(dest.stat().st_mode) == 0o444
        assert sidecar.is_file()

        def fail_open(_url: str, timeout: int = 0) -> FakeResponse:
            raise URLError("offline")

        MOD.urllib.request.urlopen = fail_open  # type: ignore[method-assign]
        try:
            MOD.acquire("org.libreoffice.LibreOffice", "https://example.test/ref", root / "b.json", root / "b.side")
        except MOD.Refusal as error:
            assert "fetch failed" in str(error)
        else:
            raise SystemExit("offline fetch was admitted")

        try:
            MOD.acquire("org.example.App", "https://example.test/ref", root / "c.json", root / "c.side")
        except MOD.Refusal as error:
            assert "fixture" in str(error)
        else:
            raise SystemExit("fixture app id was admitted")
    print("test-acquire-flathub-curated-catalog: PASS")


if __name__ == "__main__":
    main()
