#!/usr/bin/env python3
"""Verify the WL-ARCH-009 mackesd process-boundary packaging contract.

This is a source/package validator, not a runtime substitute.  A passing result
requires the six independently supervised group units and an aggregating
``mackesd.target``; it does not create units, start services, or claim that
process isolation has been proven live.

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
GROUP_UNITS = frozenset(f"mackesd-{group}.service" for group in GROUPS)

# These dependencies couple a unit's lifecycle to another unit.  Soft startup
# ordering through Wants=/After= is intentional, but no grouped daemon may stop,
# restart, or be held active because of another grouped daemon.
PEER_LIFECYCLE_DIRECTIVES = (
    "Requires",
    "Requisite",
    "BindsTo",
    "PartOf",
    "Upholds",
    "Conflicts",
    "PropagatesStopTo",
    "StopPropagatedFrom",
)

# A canonical ExecStart path is not a binary-identity guarantee when the unit
# can replace the filesystem namespace seen by the process.  Keep the grouped
# owners in the host image namespace so /usr/bin/mackesd cannot be rebound to a
# second implementation while preserving the expected command text.
EXECUTABLE_NAMESPACE_DIRECTIVES = (
    "RootDirectory",
    "RootImage",
    "BindPaths",
    "BindReadOnlyPaths",
    "TemporaryFileSystem",
    "MountImages",
    "ExtensionImages",
    "ExtensionDirectories",
)

GROUP_RESOURCE_POLICY = {
    "control": {
        "CPUQuota": "100%",
        "IOWeight": "200",
        "MemoryHigh": "384M",
        "MemoryMax": "512M",
        "TasksMax": "512",
    },
    "observation": {
        "CPUQuota": "75%",
        "IOWeight": "100",
        "MemoryHigh": "256M",
        "MemoryMax": "384M",
        "TasksMax": "512",
    },
    "actions": {
        "CPUQuota": "100%",
        "IOWeight": "200",
        "MemoryHigh": "384M",
        "MemoryMax": "512M",
        "TasksMax": "512",
    },
    "data": {
        "CPUQuota": "100%",
        "IOWeight": "300",
        "MemoryHigh": "512M",
        "MemoryMax": "768M",
        "TasksMax": "512",
    },
    "compute": {
        "CPUQuota": "150%",
        "IOWeight": "300",
        "MemoryHigh": "512M",
        "MemoryMax": "768M",
        "TasksMax": "1024",
    },
    "integrations": {
        "CPUQuota": "100%",
        "IOWeight": "200",
        "MemoryHigh": "384M",
        "MemoryMax": "512M",
        "TasksMax": "768",
    },
}


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
        expected = ["/usr/bin/mackesd", "serve", "--group", group]
        if tokens != expected:
            errors.append(
                f"{unit.path.name}: ExecStart must be exactly '/usr/bin/mackesd serve --group {group}'"
            )
    for directive in EXECUTABLE_NAMESPACE_DIRECTIVES:
        if values(unit, "Service", directive):
            errors.append(
                f"{unit.path.name}: [Service] {directive} must not remap "
                "/usr/bin/mackesd binary identity"
            )

    if TARGET not in words(unit, "Unit", "PartOf"):
        errors.append(f"{unit.path.name}: [Unit] PartOf must include {TARGET}")
    for directive in PEER_LIFECYCLE_DIRECTIVES:
        coupled_peers = words(unit, "Unit", directive) & GROUP_UNITS
        if coupled_peers:
            errors.append(
                f"{unit.path.name}: [Unit] {directive} must not couple grouped peers "
                f"{', '.join(sorted(coupled_peers))}"
            )
    if group != "control":
        owner = "mackesd-control.service"
        if owner not in words(unit, "Unit", "Wants"):
            errors.append(f"{unit.path.name}: [Unit] Wants must include {owner}")
        if owner not in words(unit, "Unit", "After"):
            errors.append(f"{unit.path.name}: [Unit] After must include {owner}")

    service_policy = {
        "Type": "notify",
        "WatchdogSec": "180",
        "Restart": "on-failure",
        "CPUAccounting": "true",
        "MemoryAccounting": "true",
        **GROUP_RESOURCE_POLICY[group],
    }
    for directive, expected in service_policy.items():
        actual = values(unit, "Service", directive)
        if actual != [expected]:
            rendered = ", ".join(repr(value) for value in actual) or "missing"
            errors.append(
                f"{unit.path.name}: [Service] {directive} must be exactly "
                f"{expected!r}, got {rendered}"
            )
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
            required = {f"mackesd-{group}.service" for group in GROUPS}
            declared = words(target, "Unit", "Wants")
            declared_groups = {
                unit for unit in declared if unit.startswith("mackesd-") and unit.endswith(".service")
            }
            missing = required - declared_groups
            if missing:
                errors.append(f"{TARGET}: [Unit] Wants is missing {', '.join(sorted(missing))}")
            extra = declared_groups - required
            if extra:
                errors.append(f"{TARGET}: [Unit] Wants has unknown groups {', '.join(sorted(extra))}")
            required_groups = {
                unit
                for unit in words(target, "Unit", "Requires")
                if unit.startswith("mackesd-") and unit.endswith(".service")
            }
            if required_groups:
                errors.append(
                    f"{TARGET}: [Unit] Requires creates crash cascades for "
                    f"{', '.join(sorted(required_groups))}; use Wants with After ordering"
                )
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

    expected_group_files = {f"mackesd-{group}.service" for group in GROUPS}
    actual_group_files = {path.name for path in unit_dir.glob("mackesd-*.service")}
    extra_group_files = actual_group_files - expected_group_files
    if extra_group_files:
        errors.append(f"unknown grouped unit files: {', '.join(sorted(extra_group_files))}")

    # Unit names are not an authority boundary.  A differently named packaged
    # service can launch the same grouped daemon and preserve a second owner
    # while evading both the retired monolith check and the mackesd-* census.
    for path in sorted(unit_dir.glob("*.service")):
        if path.name in expected_group_files:
            continue
        try:
            unit = read_unit(path)
            command_lines = values(unit, "Service", "ExecStart")
            launches_grouped_daemon = any(
                exec_tokens(command, path)[:2] == ["/usr/bin/mackesd", "serve"]
                for command in command_lines
            )
        except ValueError as exc:
            errors.append(str(exc))
            continue
        if launches_grouped_daemon:
            errors.append(
                f"{path.name}: unowned mackesd serve launcher bypasses the exact six group units"
            )

    monolith_path = unit_dir / MONOLITH
    if monolith_path.exists():
        errors.append(
            "retired mackesd.service still exists; package and enable mackesd.target instead"
        )
    return errors


def validate_bootc_packaging(repo_root: Path) -> list[str]:
    """Prove that the immutable-image lane installs and enables the boundary."""

    errors: list[str] = []
    preset_path = repo_root / "packaging" / "bootc" / "system-preset" / "45-mcnf-quasar.preset"
    containerfile_path = repo_root / "packaging" / "bootc" / "Containerfile"
    verifier_path = repo_root / "packaging" / "bootc" / "verify-image.sh"
    try:
        preset = preset_path.read_text(encoding="utf-8")
        containerfile = containerfile_path.read_text(encoding="utf-8")
        verifier = verifier_path.read_text(encoding="utf-8")
    except OSError as exc:
        return [f"cannot read bootc package contract: {exc}"]

    if "enable mackesd.target" not in preset.splitlines():
        errors.append(f"{preset_path.name}: must enable mackesd.target")
    if "disable mackesd.service" not in preset.splitlines():
        errors.append(f"{preset_path.name}: must disable retired mackesd.service")
    if "systemctl enable mackesd.target" not in containerfile:
        errors.append("bootc Containerfile must enable mackesd.target")
    if "rm -f /usr/lib/systemd/system/mackesd.service" not in containerfile:
        errors.append("bootc Containerfile must remove the RPM-installed monolithic unit")

    for name in (TARGET, *(f"mackesd-{group}.service" for group in GROUPS)):
        copy = f"COPY packaging/systemd/{name} /usr/lib/systemd/system/{name}"
        if copy not in containerfile:
            errors.append(f"bootc Containerfile does not install {name}")
    group_loop = f"for group in {' '.join(GROUPS)}; do"
    if TARGET not in verifier or group_loop not in verifier or 'unit="mackesd-$group.service"' not in verifier:
        errors.append("bootc image verifier does not assert the target and exact six group units")
    return errors


def write_fixture(root: Path, *, valid: bool = True) -> None:
    unit_dir = root / "packaging" / "systemd"
    unit_dir.mkdir(parents=True)
    requirements = " ".join(f"mackesd-{group}.service" for group in GROUPS)
    (unit_dir / TARGET).write_text(
        f"[Unit]\nWants={requirements}\n\n[Install]\nWantedBy=multi-user.target\n",
        encoding="utf-8",
    )
    for group in GROUPS:
        owner_order = ""
        if group != "control":
            owner_order = (
                "Wants=mackesd-control.service\n"
                "After=mackesd-control.service\n"
            )
        policy = GROUP_RESOURCE_POLICY[group]
        (unit_dir / f"mackesd-{group}.service").write_text(
            f"[Unit]\nPartOf=mackesd.target\n{owner_order}\n[Service]\n"
            f"ExecStart=/usr/bin/mackesd serve --group {group}\n"
            "Type=notify\nWatchdogSec=180\nRestart=on-failure\n"
            "CPUAccounting=true\nMemoryAccounting=true\n"
            f"MemoryHigh={policy['MemoryHigh']}\nMemoryMax={policy['MemoryMax']}\n"
            f"CPUQuota={policy['CPUQuota']}\nTasksMax={policy['TasksMax']}\n"
            f"IOWeight={policy['IOWeight']}\n",
            encoding="utf-8",
        )
    if not valid:
        (unit_dir / "mackesd-data.service").write_text(
            "[Unit]\nPartOf=mackesd.target\n\n[Service]\n"
            "ExecStart=/usr/bin/mackesd serve --group data --sqlite-writer\n"
            "Type=notify\nWatchdogSec=180\nRestart=on-failure\n"
            "CPUAccounting=true\nMemoryAccounting=true\n"
            "MemoryHigh=512M\nMemoryMax=768M\nCPUQuota=100%\n"
            "TasksMax=512\nIOWeight=300\n",
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
        assert any("must be exactly" in error for error in writer_errors), writer_errors

        cascade_root = root / "cascade"
        write_fixture(cascade_root)
        cascade_target = cascade_root / "packaging" / "systemd" / TARGET
        cascade_target.write_text(
            cascade_target.read_text(encoding="utf-8").replace("Wants=", "Requires=", 1),
            encoding="utf-8",
        )
        cascade_group = cascade_root / "packaging" / "systemd" / "mackesd-actions.service"
        cascade_group.write_text(
            cascade_group.read_text(encoding="utf-8").replace(
                "Wants=mackesd-control.service", "Requires=mackesd-control.service"
            ),
            encoding="utf-8",
        )
        cascade_errors = validate_source(cascade_root)
        assert any("Requires creates crash cascades" in error for error in cascade_errors), cascade_errors
        assert any("Requires must not couple grouped peers" in error for error in cascade_errors), cascade_errors

        peer_coupling_root = root / "peer-coupling"
        write_fixture(peer_coupling_root)
        peer_unit = peer_coupling_root / "packaging" / "systemd" / "mackesd-actions.service"
        peer_unit.write_text(
            peer_unit.read_text(encoding="utf-8").replace(
                "After=mackesd-control.service",
                "After=mackesd-control.service\nBindsTo=mackesd-compute.service",
            ),
            encoding="utf-8",
        )
        peer_errors = validate_source(peer_coupling_root)
        assert any("BindsTo must not couple grouped peers" in error for error in peer_errors), peer_errors

        unlimited_root = root / "unlimited"
        write_fixture(unlimited_root)
        unlimited_unit = unlimited_root / "packaging" / "systemd" / "mackesd-data.service"
        unlimited_unit.write_text(
            unlimited_unit.read_text(encoding="utf-8").replace(
                "MemoryMax=768M", "MemoryMax=infinity"
            ),
            encoding="utf-8",
        )
        unlimited_errors = validate_source(unlimited_root)
        assert any("MemoryMax must be exactly" in error for error in unlimited_errors), unlimited_errors

        renamed_launcher_root = root / "renamed-launcher"
        write_fixture(renamed_launcher_root)
        renamed_launcher = (
            renamed_launcher_root / "packaging" / "systemd" / "mesh-control-recovery.service"
        )
        renamed_launcher.write_text(
            "[Service]\nExecStart=/usr/bin/mackesd serve --group control\n",
            encoding="utf-8",
        )
        renamed_launcher_errors = validate_source(renamed_launcher_root)
        assert any(
            "mesh-control-recovery.service: unowned mackesd serve launcher" in error
            for error in renamed_launcher_errors
        ), renamed_launcher_errors

        binary_remap_root = root / "binary-remap"
        write_fixture(binary_remap_root)
        binary_remap_unit = (
            binary_remap_root / "packaging" / "systemd" / "mackesd-control.service"
        )
        binary_remap_unit.write_text(
            binary_remap_unit.read_text(encoding="utf-8").replace(
                "ExecStart=/usr/bin/mackesd serve --group control",
                "BindReadOnlyPaths=-/opt/hostile/mackesd:/usr/bin/mackesd\n"
                "ExecStart=/usr/bin/mackesd serve --group control",
            ),
            encoding="utf-8",
        )
        binary_remap_errors = validate_source(binary_remap_root)
        assert any(
            "BindReadOnlyPaths must not remap /usr/bin/mackesd binary identity" in error
            for error in binary_remap_errors
        ), binary_remap_errors

        source_like_root = root / "source-like"
        source_units = source_like_root / "packaging" / "systemd"
        source_units.mkdir(parents=True)
        (source_units / MONOLITH).write_text(
            "[Service]\nExecStart=/usr/bin/mackesd serve\n", encoding="utf-8"
        )
        source_errors = validate_source(source_like_root)
        assert any("missing mackesd.target" in error for error in source_errors), source_errors
        assert any("retired mackesd.service" in error for error in source_errors), source_errors

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
    errors.extend(validate_bootc_packaging(root))
    if errors:
        print("mackesd process boundary: FAIL", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("mackesd process boundary: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
