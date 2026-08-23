#!/usr/bin/env python3
"""Hostile self-test for require-seat-mutation-warning.py.

Does not publish a toast and does not sleep five seconds. The live
seat-update-warning.sh is admitted, not executed.
"""

from __future__ import annotations

import importlib.util
import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
HELPER = HERE / "require-seat-mutation-warning.py"
LIVE = HERE / "seat-update-warning.sh"


def command(*args: str, refused: bool = False) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        [sys.executable, str(HELPER), *args],
        check=False,
        capture_output=True,
        text=True,
    )
    if refused:
        assert result.returncode == 2, result.stderr or result.stdout
        assert result.stdout == "", result.stdout
        assert "REFUSED:" in result.stderr
    else:
        assert result.returncode == 0, result.stderr
        assert "REFUSED:" not in result.stderr
    return result


def write_helper(path: Path, body: str, mode: int = 0o700) -> None:
    path.write_text(body, encoding="utf-8")
    os.chmod(path, mode)


def main() -> None:
    live = command("--admit-only", "--helper", str(LIVE))
    assert "toast not published" in live.stdout
    assert stat.S_IMODE(LIVE.stat().st_mode) & 0o111

    with tempfile.TemporaryDirectory(prefix="mcnf-seat-warning-") as temporary:
        root = Path(temporary)
        missing = root / "missing.sh"
        command("--admit-only", "--helper", str(missing), refused=True)

        no_flag = root / "no-flag.sh"
        write_helper(no_flag, "#!/bin/sh\nWAIT_SECONDS=5\nexit 0\n")
        command("--admit-only", "--helper", str(no_flag), refused=True)

        no_wait = root / "no-wait.sh"
        write_helper(no_wait, "#!/bin/sh\n# AI-GENERATED-ALERT\nexit 0\n")
        command("--admit-only", "--helper", str(no_wait), refused=True)

        linked = root / "linked.sh"
        linked.symlink_to(LIVE)
        command("--admit-only", "--helper", str(linked), refused=True)

        ok = root / "ok.sh"
        write_helper(
            ok,
            "#!/bin/sh\nWAIT_SECONDS=5\n# AI-GENERATED-ALERT\nexit 0\n",
        )
        ran = command("--helper", str(ok))
        assert "warning completed" in ran.stdout

        fail = root / "fail.sh"
        write_helper(
            fail,
            "#!/bin/sh\nWAIT_SECONDS=5\n# AI-GENERATED-ALERT\nexit 1\n",
        )
        failed = command("--helper", str(fail), refused=True)
        assert "mutation was not started" in failed.stderr

        loaded = importlib.util.spec_from_file_location("require_warning", HELPER)
        module = importlib.util.module_from_spec(loaded)
        assert loaded.loader is not None
        loaded.loader.exec_module(module)
        os.environ[module.HELPER_ENV] = str(ok)
        try:
            override = module.resolve_warning_helper()
            pinned = module.resolve_warning_helper(for_production_mutation=True)
        finally:
            os.environ.pop(module.HELPER_ENV, None)
        assert override == ok.resolve()
        assert pinned == LIVE.resolve()
        admitted = module.admit_warning_helper(for_production_mutation=True)
        assert admitted == LIVE.resolve()

    print("require seat mutation warning hostile suite passed")


if __name__ == "__main__":
    main()
