#!/usr/bin/env python3
"""Verify the WL-ARCH-009 mackesd process-boundary packaging contract.

This is a source/package validator, not a runtime substitute.  It deliberately
fails while the repository only ships the monolithic ``mackesd.service``.  A
passing result requires the six independently supervised group units and an
aggregating ``mackesd.target``; it does not create units, start services, or
claim that process isolation has been proven live.

Usage:
  verify-mackesd-process-boundary.py [--repo-root PATH]
  verify-mackesd-process-boundary.py --self-test
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import shlex
import sys
import tempfile


GROUPS = (
    "control",
    "observation",
    "actions",
    "data",
    "compute",
    "integrations",
)
TARGET = "mackesd.target"
MONOLITH = "mackesd.service"


@dataclass(frozen=True)
class UnitFile:
    path: Path
    sections: dict[str, dict[str, list[str]]]


def read_unit(path: Path) -> UnitFile:
    """Read the small systemd subset this boundary owns.

    Repeated directives are preserved because systemd permits them and target
    dependencies are commonly split over several lines.  The parser rejects
    malformed assignments rather than silently treating a typo as a pass.
    """

    sections: dict[str, dict[str, list[str]]] = {}
    current: dict[str, list[str]] | None = None
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise ValueError(f"cannot read {path}: {exc}") from exc
    for number, raw in enumerate(lines, start=1):
        line = raw.strip()
        if not line or line.startswith(("#", ";")):
            continue
        if line.startswith("[") and line.endswith("]"):
            name = line[1:-1]
            if not name:
                raise ValueError(f"{path}:{number}: empty section name")
            current = sections.setdefault(name, {})
            continue
        if current is None or "=" not in line:
            raise ValueError(f"{path}:{number}: expected a section or key=value")
        key, value = line.split("=", 1)
        key = key.strip()
        if not key:
            raise ValueError(f"{path}:{number}: empty directive name")
        current.setdefault(key, []).append(value.strip())
    return UnitFile(path, sections)


def values(unit: UnitFile, section: str, key: str) -> list[str]:
    return unit.sections.get(section, {}).get(key, [])


def words(unit: UnitFile, section: str, key: str) -> set[str]:
    result: set[str] = set()
    for value in values(unit, section, key):
        result.update(value.split())
    return result


def exec_tokens(value: str, path: Path) -> list[str]:
    command = value.lstrip("-@:+!")
    try:
        return shlex.split(command, posix=True)
    except ValueError as exc:
        raise ValueError(f"{path}: malformed ExecStart: {exc}") from exc


def validate_group(unit: UnitFile, group: str) -> list[str]:
    errors: list[str] = []
    command_lines = values(unit, "Service", "ExecStart")
    tokens: list[str] = []
    if len(command_lines) != 1:
        errors.append(f"{unit.path.name}: require exactly one [Service] ExecStart")
    else:
        tokens = exec_tokens(command_lines[0], unit.path)
        if len(tokens) < 4 or tokens[0:2] != ["/usr/bin/mackesd", "serve"]:
            errors.append(
                f"{unit.path.name}: ExecStart must invoke '/usr/bin/mackesd serve --group {group}'"
            )
        elif tokens.count("--group") != 1:
            errors.append(f"{unit.path.name}: ExecStart must contain exactly one --group")
        else:
            index = tokens.index("--group")
            if index + 1 >= len(tokens) or tokens[index + 1] != group:
                errors.append(f"{unit.path.name}: ExecStart must bind --group {group}")
        has_writer = "--sqlite-writer" in tokens
        if group == "data" and not has_writer:
            errors.append(f"{unit.path.name}: data is the sole SQLite writer; add --sqlite-writer")
        if group != "data" and has_writer:
            errors.append(f"{unit.path.name}: only mackesd-data.service may use --sqlite-writer")

    if TARGET not in words(unit, "Unit", "PartOf"):
        errors.append(f"{unit.path.name}: [Unit] PartOf must include {TARGET}")
    return errors


def validate_source(repo_root: Path) -> list[str]:
    unit_dir = repo_root / "packaging" / "systemd"
    errors: list[str] = []
    if not unit_dir.is_dir():
        return [f"missing systemd packaging directory: {unit_dir}"]

    target_path = unit_dir / TARGET
    if not target_path.is_file():
        errors.append(
            f"missing {TARGET}; ARCH-009 requires an aggregate target for the six process groups"
        )
    else:
        try:
            target = read_unit(target_path)
            required = set(GROUPS)
            declared = words(target, "Unit", "Requires")
            missing = {f"mackesd-{group}.service" for group in required} - declared
            if missing:
                errors.append(f"{TARGET}: [Unit] Requires is missing {', '.join(sorted(missing))}")
            if values(target, "Service", "ExecStart"):
                errors.append(f"{TARGET}: targets must aggregate units, not run an ExecStart")
            if "multi-user.target" not in words(target, "Install", "WantedBy"):
                errors.append(f"{TARGET}: [Install] WantedBy must include multi-user.target")
        except ValueError as exc:
            errors.append(str(exc))

    for group in GROUPS:
        path = unit_dir / f"mackesd-{group}.service"
        if not path.is_file():
            errors.append(f"missing {path.name}; expected independent {group} entrypoint")
            continue
        try:
            errors.extend(validate_group(read_unit(path), group))
        except ValueError as exc:
            errors.append(str(exc))

    monolith_path = unit_dir / MONOLITH
    if monolith_path.is_file():
        try:
            monolith = read_unit(monolith_path)
            if any(
                tokens[:2] == ["/usr/bin/mackesd", "serve"]
                for value in values(monolith, "Service", "ExecStart")
                for tokens in [exec_tokens(value, monolith_path)]
            ):
                errors.append(
                    "monolithic mackesd.service still starts '/usr/bin/mackesd serve'; "
                    "remove it from the packaged runtime before declaring ARCH-009 S4 complete"
                )
        except ValueError as exc:
            errors.append(str(exc))
    return errors


def write_fixture(root: Path, *, valid: bool = True) -> None:
    unit_dir = root / "packaging" / "systemd"
    unit_dir.mkdir(parents=True)
    requirements = " ".join(f"mackesd-{group}.service" for group in GROUPS)
    (unit_dir / TARGET).write_text(
        f"[Unit]\nRequires={requirements}\n\n[Install]\nWantedBy=multi-user.target\n",
        encoding="utf-8",
    )
    for group in GROUPS:
        writer = " --sqlite-writer" if group == "data" else ""
        (unit_dir / f"mackesd-{group}.service").write_text(
            "[Unit]\nPartOf=mackesd.target\n\n[Service]\n"
            f"ExecStart=/usr/bin/mackesd serve --group {group}{writer}\n",
            encoding="utf-8",
        )
    if not valid:
        (unit_dir / "mackesd-data.service").write_text(
            "[Unit]\nPartOf=mackesd.target\n\n[Service]\n"
            "ExecStart=/usr/bin/mackesd serve --group data\n",
            encoding="utf-8",
        )


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="mackesd-process-boundary-") as temporary:
        root = Path(temporary)
        write_fixture(root)
        assert not validate_source(root), "complete isolated fixture must pass"

        writer_errors_root = root / "writer-errors"
        write_fixture(writer_errors_root, valid=False)
        writer_errors = validate_source(writer_errors_root)
        assert any("sole SQLite writer" in error for error in writer_errors), writer_errors

        source_like_root = root / "source-like"
        source_units = source_like_root / "packaging" / "systemd"
        source_units.mkdir(parents=True)
        (source_units / MONOLITH).write_text(
            "[Service]\nExecStart=/usr/bin/mackesd serve\n", encoding="utf-8"
        )
        source_errors = validate_source(source_like_root)
        assert any("missing mackesd.target" in error for error in source_errors), source_errors
        assert any("monolithic mackesd.service" in error for error in source_errors), source_errors

    print("verify-mackesd-process-boundary.py: self-test passed")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, help="repository root (defaults to this helper's parent)")
    parser.add_argument("--self-test", action="store_true", help="validate generated temporary fixtures")
    args = parser.parse_args(argv)
    if args.self_test and args.repo_root is not None:
        parser.error("--self-test does not accept --repo-root")
    return args


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.self_test:
        self_test()
        return 0
    root = (args.repo_root or Path(__file__).resolve().parent.parent).resolve()
    errors = validate_source(root)
    if errors:
        print("mackesd process boundary: FAIL", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("mackesd process boundary: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
