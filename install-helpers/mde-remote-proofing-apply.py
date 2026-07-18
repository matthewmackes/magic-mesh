#!/usr/bin/env python3
# WL-FUNC-009 - Remote Proofing Sunshine/Moonlight plan bridge.
#
# This helper is intentionally render-free: it consumes the shell-owned
# Remote Proofing settings file and the mesh-status snapshot, then writes a
# deterministic Sunshine/firewall lifecycle intent for the service layer. It
# never starts Sunshine and never opens firewall ports by itself.

from __future__ import annotations

import argparse
import ipaddress
import json
import os
import pwd
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


CONFIG_FILE = "settings-remote-proofing.json"
SYSTEM_BUS_ROOT = Path("/run/mde-bus")
DEFAULT_MESH_STATUS = Path("/run/mde/mesh-status.json")
DEFAULT_RUNTIME_DIR = Path("/run/mde/remote-proofing")
DEFAULT_PLAN_PATH = DEFAULT_RUNTIME_DIR / "plan.json"
DEFAULT_SUNSHINE_CONF = DEFAULT_RUNTIME_DIR / "sunshine.conf"
DEFAULT_LIFECYCLE_PATH = DEFAULT_RUNTIME_DIR / "lifecycle.json"
DEFAULT_FIREWALL_STATE = Path("/var/lib/mde/remote-proofing/firewalld-state.json")

SUNSHINE_SERVICE_UNIT = "sunshine.service"
SUNSHINE_SERVICE_SCOPE = "user"
FIREWALL_BACKEND = "firewalld"
FIREWALL_ZONE = "public"

SUNSHINE_TCP_PORTS = [47984, 47989, 47990, 48010]
SUNSHINE_UDP_PORT_RANGES = ["47998-48010"]

EXPOSURES = {"mesh_only", "lan", "public"}
CAPTURES = {"auto", "kms", "wlr", "x11"}
ENCODERS = {"auto", "vaapi", "nvenc", "amdvce", "software"}


def _env_path(name: str) -> Path | None:
    value = os.environ.get(name)
    return Path(value) if value else None


def default_settings_path() -> Path:
    if path := _env_path("MDE_REMOTE_PROOFING_SETTINGS"):
        return path
    if root := _env_path("MDE_BUS_ROOT"):
        return root / CONFIG_FILE
    if (SYSTEM_BUS_ROOT / "index.sqlite").exists():
        return SYSTEM_BUS_ROOT / CONFIG_FILE
    xdg = os.environ.get("XDG_DATA_HOME")
    data_home = Path(xdg) if xdg else Path.home() / ".local" / "share"
    return data_home / "mde" / "bus" / CONFIG_FILE


def default_mesh_status_path() -> Path:
    return _env_path("MDE_MESH_STATUS") or DEFAULT_MESH_STATUS


def _load_json(path: Path) -> tuple[dict[str, Any], bool, str | None]:
    try:
        raw = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return {}, False, None
    except OSError as exc:
        return {}, False, str(exc)
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as exc:
        return {}, False, f"{exc.msg} at line {exc.lineno} column {exc.colno}"
    if not isinstance(value, dict):
        return {}, False, "top-level JSON value is not an object"
    return value, True, None


def _bool_field(
    data: dict[str, Any],
    key: str,
    default: bool,
    warnings: list[str],
) -> bool:
    value = data.get(key, default)
    if isinstance(value, bool):
        return value
    warnings.append(f"Invalid {key}; using {str(default).lower()}.")
    return default


def _choice_field(
    data: dict[str, Any],
    key: str,
    choices: set[str],
    default: str,
    warnings: list[str],
) -> str:
    value = data.get(key, default)
    if isinstance(value, str) and value in choices:
        return value
    warnings.append(f"Invalid {key}; using {default}.")
    return default


def _fps_field(data: dict[str, Any], warnings: list[str]) -> int:
    value = data.get("min_fps_target", 30)
    if isinstance(value, bool) or not isinstance(value, int):
        warnings.append("Invalid min_fps_target; using 30.")
        return 30
    clamped = max(15, min(120, value))
    if clamped != value:
        warnings.append(f"Clamped min_fps_target from {value} to {clamped}.")
    return clamped


def normalize_config(raw: dict[str, Any]) -> tuple[dict[str, Any], list[str]]:
    warnings: list[str] = []
    cfg = {
        "enabled": _bool_field(raw, "enabled", False, warnings),
        "exposure": _choice_field(raw, "exposure", EXPOSURES, "mesh_only", warnings),
        "capture": _choice_field(raw, "capture", CAPTURES, "kms", warnings),
        "encoder": _choice_field(raw, "encoder", ENCODERS, "auto", warnings),
        "native_pairing_prompt": _bool_field(raw, "native_pairing_prompt", True, warnings),
        "require_local_approval": _bool_field(raw, "require_local_approval", True, warnings),
        "show_shadowing_indicator": _bool_field(
            raw, "show_shadowing_indicator", True, warnings
        ),
        "allow_remote_input": _bool_field(raw, "allow_remote_input", True, warnings),
        "vnc_fallback": _bool_field(raw, "vnc_fallback", True, warnings),
        "min_fps_target": _fps_field(raw, warnings),
    }
    return cfg, warnings


def mesh_facts(raw: dict[str, Any]) -> dict[str, str | None]:
    network = raw.get("network")
    if not isinstance(network, dict):
        network = {}
    own: dict[str, Any] = {}
    self_name = raw.get("self")
    nodes = raw.get("nodes")
    if isinstance(nodes, list):
        for node in nodes:
            if isinstance(node, dict) and node.get("hostname") == self_name:
                own = node
                break

    def text(source: dict[str, Any], key: str) -> str | None:
        value = source.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
        return None

    return {
        "overlay_ip": text(own, "overlay_ip") or text(network, "overlay_ip"),
        "overlay_cidr": text(network, "overlay_cidr"),
        "default_gw": text(network, "default_gw"),
    }


def _firewall_intent(
    enabled: bool,
    policy: str,
    facts: dict[str, str | None],
) -> dict[str, Any]:
    intent: dict[str, Any] = {
        "policy": policy,
        "tcp_ports": [],
        "udp_port_ranges": [],
        "allow_sources": [],
        "notes": [],
    }
    if not enabled:
        intent["notes"].append("Remote Proofing is disabled; keep Sunshine ports closed.")
        return intent

    intent["tcp_ports"] = SUNSHINE_TCP_PORTS
    intent["udp_port_ranges"] = SUNSHINE_UDP_PORT_RANGES
    if policy == "mesh_overlay_only":
        if facts.get("overlay_cidr"):
            intent["allow_sources"] = [facts["overlay_cidr"]]
        else:
            intent["notes"].append("Overlay CIDR is unavailable; do not open ports yet.")
    elif policy == "trusted_lan_only":
        intent["allow_sources"] = ["trusted-lan"]
        intent["notes"].append("Resolve trusted LAN interface before applying firewall rules.")
    elif policy == "public_explicit":
        intent["allow_sources"] = ["0.0.0.0/0"]
        intent["notes"].append("Public exposure requires explicit operator acceptance.")
    return intent


def build_plan(settings_path: Path, mesh_status_path: Path) -> dict[str, Any]:
    raw_settings, settings_loaded, settings_error = _load_json(settings_path)
    raw_mesh, mesh_loaded, mesh_error = _load_json(mesh_status_path)
    cfg, warnings = normalize_config(raw_settings)
    facts = mesh_facts(raw_mesh)

    if settings_error:
        warnings.append(f"Settings file unreadable; using disabled defaults: {settings_error}.")
    if cfg["enabled"] and mesh_error:
        warnings.append(f"Mesh status unreadable: {mesh_error}.")

    if not cfg["enabled"]:
        bind_scope = "disabled"
        bind_address = None
        firewall_policy = "closed"
    elif cfg["exposure"] == "mesh_only":
        bind_scope = "mesh_overlay"
        bind_address = facts["overlay_ip"]
        firewall_policy = "mesh_overlay_only"
        if not facts["overlay_ip"]:
            warnings.append(
                "Mesh address is not visible yet; keep the service degraded until the overlay address is known."
            )
        if not facts["overlay_cidr"]:
            warnings.append("Mesh overlay CIDR is not visible yet; firewall ports must stay closed.")
    elif cfg["exposure"] == "lan":
        bind_scope = "trusted_lan"
        bind_address = None
        firewall_policy = "trusted_lan_only"
        if not facts["default_gw"]:
            warnings.append("LAN exposure needs a trusted local interface before the service starts.")
    else:
        bind_scope = "all_interfaces"
        bind_address = "0.0.0.0"
        firewall_policy = "public_explicit"
        warnings.append(
            "All-interfaces exposure must keep the firewall warning, local approval, and on-seat indicator visible."
        )

    if cfg["enabled"] and not cfg["require_local_approval"]:
        warnings.append("Local approval is off; use only for controlled proofing.")
    if cfg["enabled"] and not cfg["show_shadowing_indicator"]:
        warnings.append("The on-seat shadowing indicator is off; remote viewers may be hidden.")
    if cfg["enabled"] and not cfg["allow_remote_input"]:
        warnings.append("Remote viewers can watch only; keyboard and mouse input are blocked.")

    plan = {
        "source": {
            "settings_path": str(settings_path),
            "settings_loaded": settings_loaded,
            "mesh_status_path": str(mesh_status_path),
            "mesh_loaded": mesh_loaded,
        },
        "enabled": cfg["enabled"],
        "bind_scope": bind_scope,
        "bind_address": bind_address,
        "firewall_policy": firewall_policy,
        "sunshine": {
            "capture": cfg["capture"],
            "encoder": cfg["encoder"],
            "minimum_fps_target": cfg["min_fps_target"],
            "upnp": "disabled",
        },
        "controls": {
            "native_pairing_prompt": cfg["native_pairing_prompt"],
            "require_local_approval": cfg["require_local_approval"],
            "show_shadowing_indicator": cfg["show_shadowing_indicator"],
            "allow_remote_input": cfg["allow_remote_input"],
            "vnc_fallback": cfg["vnc_fallback"],
        },
        "firewall": _firewall_intent(cfg["enabled"], firewall_policy, facts),
        "network_facts": facts,
        "warnings": warnings,
    }
    plan["degraded"] = bool(cfg["enabled"] and warnings)
    return plan


def build_lifecycle(plan: dict[str, Any], config_path: Path) -> dict[str, Any]:
    """Render the side-effect-free service/firewall lifecycle contract.

    The next service layer should be able to consume this artifact without
    reverse-engineering Sunshine comments or rerunning policy normalization.
    """

    firewall = plan["firewall"]
    controls = plan["controls"]
    settings_loaded = bool(plan["source"].get("settings_loaded"))
    sunshine_blockers: list[str] = []
    firewall_blockers: list[str] = []

    if not settings_loaded:
        desired_service_state = "unmanaged"
        sunshine_blockers.append("Remote Proofing settings are absent; leaving Sunshine untouched.")
        firewall_desired_state = "unmanaged"
    elif not plan["enabled"]:
        desired_service_state = "stopped"
        sunshine_blockers.append("Remote Proofing is disabled.")
        firewall_desired_state = "closed"
    else:
        desired_service_state = "ready"
        firewall_desired_state = "applied"
        if plan["bind_scope"] == "mesh_overlay":
            if not plan["bind_address"]:
                sunshine_blockers.append("Mesh overlay bind address is unavailable.")
            if not firewall["allow_sources"]:
                firewall_blockers.append("Mesh overlay CIDR is unavailable.")
        elif plan["bind_scope"] == "trusted_lan":
            sunshine_blockers.append(
                "Trusted LAN bind address must be resolved before Sunshine starts."
            )
            firewall_blockers.append(
                "Trusted LAN interface must be resolved before Sunshine ports open."
            )
            firewall_desired_state = "needs_network_resolution"

        if sunshine_blockers or firewall_blockers:
            desired_service_state = "blocked"
            if firewall_desired_state == "applied":
                firewall_desired_state = "blocked"

    return {
        "schema_version": 1,
        "source": plan["source"],
        "network_facts": plan["network_facts"],
        "enabled": plan["enabled"],
        "degraded": bool(
            settings_loaded
            and plan["enabled"]
            and (plan["warnings"] or sunshine_blockers or firewall_blockers)
        ),
        "managed": settings_loaded,
        "sunshine": {
            "service_unit": SUNSHINE_SERVICE_UNIT,
            "service_scope": SUNSHINE_SERVICE_SCOPE,
            "desired_state": desired_service_state,
            "start_allowed": settings_loaded
            and plan["enabled"]
            and not sunshine_blockers
            and not firewall_blockers,
            "stop_required": settings_loaded and not plan["enabled"],
            "restart_after_config": settings_loaded
            and plan["enabled"]
            and not sunshine_blockers
            and not firewall_blockers,
            "config_fragment": str(config_path),
            "bind_scope": plan["bind_scope"],
            "bind_address": plan["bind_address"],
            "capture": plan["sunshine"]["capture"],
            "encoder": plan["sunshine"]["encoder"],
            "minimum_fps_target": plan["sunshine"]["minimum_fps_target"],
            "blockers": sunshine_blockers,
        },
        "firewall": {
            "backend": FIREWALL_BACKEND,
            "zone": FIREWALL_ZONE,
            "desired_state": firewall_desired_state,
            "apply_allowed": settings_loaded and plan["enabled"] and not firewall_blockers,
            "policy": plan["firewall_policy"],
            "tcp_ports": firewall["tcp_ports"],
            "udp_port_ranges": firewall["udp_port_ranges"],
            "allow_sources": firewall["allow_sources"],
            "blockers": firewall_blockers,
            "notes": firewall["notes"],
        },
        "controls": {
            "native_pairing_prompt": controls["native_pairing_prompt"],
            "require_local_approval": controls["require_local_approval"],
            "show_shadowing_indicator": controls["show_shadowing_indicator"],
            "allow_remote_input": controls["allow_remote_input"],
            "vnc_fallback": controls["vnc_fallback"],
        },
        "warnings": plan["warnings"],
    }


def sunshine_origin_web_ui(plan: dict[str, Any]) -> str:
    if plan["firewall_policy"] == "public_explicit":
        return "wan"
    return "lan"


def sunshine_origin_web_ui_for_policy(policy: str) -> str:
    if policy == "public_explicit":
        return "wan"
    return "lan"


def render_sunshine_config_from_lifecycle(lifecycle: dict[str, Any]) -> str:
    lines = [
        "# Generated by mde-remote-proofing-apply.",
        "# Source of truth: Mesh & System -> Remote Proofing.",
        "# Consume this with lifecycle.json; Magic Mesh policy metadata lives there.",
    ]
    if not lifecycle["managed"]:
        lines.append("# Remote Proofing is unmanaged; leave any existing Sunshine state untouched.")
        return "\n".join(lines) + "\n"
    if not lifecycle["enabled"]:
        lines.append("# Remote Proofing is disabled; Sunshine should remain stopped.")
        return "\n".join(lines) + "\n"

    sunshine = lifecycle["sunshine"]
    lines.extend(
        [
            "upnp = disabled",
            f"capture = {sunshine['capture']}",
            f"encoder = {sunshine['encoder']}",
            f"minimum_fps_target = {sunshine['minimum_fps_target']}",
            "address_family = ipv4",
            f"origin_web_ui_allowed = {sunshine_origin_web_ui_for_policy(lifecycle['firewall']['policy'])}",
        ]
    )
    if sunshine["bind_address"]:
        lines.append(f"bind_address = {sunshine['bind_address']}")
    else:
        lines.append("# bind_address is unresolved; apply firewall policy before starting Sunshine.")
    for warning in lifecycle["warnings"]:
        lines.append(f"# warning: {warning}")
    for blocker in sunshine["blockers"]:
        lines.append(f"# blocker: {blocker}")
    return "\n".join(lines) + "\n"


def render_sunshine_config(plan: dict[str, Any]) -> str:
    lines = [
        "# Generated by mde-remote-proofing-apply.",
        "# Source of truth: Mesh & System -> Remote Proofing.",
        "# Consume this with lifecycle.json; Magic Mesh policy metadata lives there.",
    ]
    if not plan["enabled"]:
        lines.extend(
            [
                "# Remote Proofing is disabled; Sunshine should remain stopped or firewalled.",
            ]
        )
        return "\n".join(lines) + "\n"

    sunshine = plan["sunshine"]
    lines.extend(
        [
            "upnp = disabled",
            f"capture = {sunshine['capture']}",
            f"encoder = {sunshine['encoder']}",
            f"minimum_fps_target = {sunshine['minimum_fps_target']}",
            "address_family = ipv4",
            f"origin_web_ui_allowed = {sunshine_origin_web_ui(plan)}",
        ]
    )
    if plan["bind_address"]:
        lines.append(f"bind_address = {plan['bind_address']}")
    else:
        lines.append("# bind_address is unresolved; apply firewall policy before starting Sunshine.")
    for warning in plan["warnings"]:
        lines.append(f"# warning: {warning}")
    return "\n".join(lines) + "\n"


def _json_command(argv: list[str]) -> Any | None:
    try:
        completed = subprocess.run(argv, text=True, capture_output=True, check=False)
    except OSError:
        return None
    if completed.returncode != 0:
        return None
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError:
        return None


def resolve_trusted_lan(default_gw: str | None) -> dict[str, str] | None:
    if not default_gw:
        return None
    route = _json_command(["ip", "-j", "route", "get", default_gw])
    if not isinstance(route, list) or not route:
        return None
    hop = route[0]
    if not isinstance(hop, dict):
        return None
    bind_address = hop.get("prefsrc")
    dev = hop.get("dev")
    if not isinstance(bind_address, str) or not isinstance(dev, str):
        return None
    addr = _json_command(["ip", "-j", "addr", "show", "dev", dev])
    if not isinstance(addr, list):
        return None
    for interface in addr:
        if not isinstance(interface, dict):
            continue
        for item in interface.get("addr_info", []):
            if not isinstance(item, dict):
                continue
            if item.get("family") != "inet" or item.get("local") != bind_address:
                continue
            prefix = item.get("prefixlen")
            if not isinstance(prefix, int):
                continue
            network = ipaddress.ip_network(f"{bind_address}/{prefix}", strict=False)
            return {
                "bind_address": bind_address,
                "allow_source": str(network),
                "interface": dev,
            }
    return None


def resolve_trusted_lan_lifecycle(lifecycle: dict[str, Any]) -> bool:
    sunshine = lifecycle["sunshine"]
    firewall = lifecycle["firewall"]
    if sunshine["bind_scope"] != "trusted_lan":
        return False
    resolved = resolve_trusted_lan(lifecycle["network_facts"].get("default_gw"))
    if not resolved:
        return False
    sunshine["bind_address"] = resolved["bind_address"]
    sunshine["desired_state"] = "ready"
    sunshine["start_allowed"] = lifecycle["managed"] and lifecycle["enabled"]
    sunshine["restart_after_config"] = lifecycle["managed"] and lifecycle["enabled"]
    sunshine["blockers"] = [
        blocker for blocker in sunshine["blockers"] if not blocker.startswith("Trusted LAN")
    ]
    firewall["allow_sources"] = [resolved["allow_source"]]
    firewall["desired_state"] = "applied"
    firewall["apply_allowed"] = lifecycle["managed"] and lifecycle["enabled"]
    firewall["blockers"] = [
        blocker for blocker in firewall["blockers"] if not blocker.startswith("Trusted LAN")
    ]
    firewall["notes"] = [
        note for note in firewall["notes"] if not note.startswith("Resolve trusted LAN")
    ]
    firewall.setdefault("resolved", {})["interface"] = resolved["interface"]
    firewall["resolved"]["bind_address"] = resolved["bind_address"]
    firewall["resolved"]["allow_source"] = resolved["allow_source"]
    lifecycle["degraded"] = bool(lifecycle["warnings"] or sunshine["blockers"] or firewall["blockers"])
    return True


def sync_plan_from_lifecycle(plan: dict[str, Any], lifecycle: dict[str, Any]) -> None:
    """Make the summary plan reflect the effective resolved lifecycle state."""

    sunshine = lifecycle["sunshine"]
    firewall = lifecycle["firewall"]
    plan["bind_address"] = sunshine["bind_address"]
    plan["degraded"] = lifecycle["degraded"]
    plan["firewall"]["allow_sources"] = firewall["allow_sources"]
    plan["firewall"]["notes"] = firewall["notes"]
    if "resolved" in firewall:
        plan["firewall"]["resolved"] = firewall["resolved"]
    else:
        plan["firewall"].pop("resolved", None)


def firewall_rich_rule(port: str, protocol: str, source: str | None) -> str:
    source_clause = "" if not source or source == "0.0.0.0/0" else f' source address="{source}"'
    return f'rule family="ipv4"{source_clause} port port="{port}" protocol="{protocol}" accept'


def desired_firewall_rules(lifecycle: dict[str, Any]) -> list[str]:
    firewall = lifecycle["firewall"]
    if not firewall["apply_allowed"]:
        return []
    sources = firewall["allow_sources"] or [None]
    rules: list[str] = []
    for source in sources:
        if source == "trusted-lan":
            continue
        for port in firewall["tcp_ports"]:
            rules.append(firewall_rich_rule(str(port), "tcp", source))
        for port_range in firewall["udp_port_ranges"]:
            rules.append(firewall_rich_rule(str(port_range), "udp", source))
    return rules


def _load_state(path: Path) -> dict[str, Any]:
    data, loaded, _ = _load_json(path)
    return data if loaded else {}


def _write_json(path: Path, value: dict[str, Any]) -> None:
    _atomic_write(path, json.dumps(value, indent=2, sort_keys=True) + "\n")


def _run(
    argv: list[str],
    result: dict[str, Any],
    dry_run: bool,
    *,
    env: dict[str, str] | None = None,
) -> bool:
    result["commands"].append(argv)
    if dry_run:
        return True
    completed = subprocess.run(argv, env=env, text=True, capture_output=True, check=False)
    record = {
        "argv": argv,
        "returncode": completed.returncode,
        "stdout": completed.stdout.strip(),
        "stderr": completed.stderr.strip(),
    }
    result["command_results"].append(record)
    return completed.returncode == 0


def _desktop_users() -> list[pwd.struct_passwd]:
    users = []
    for entry in pwd.getpwall():
        if (
            1000 <= entry.pw_uid < 60000
            and entry.pw_dir.startswith("/home/")
            and entry.pw_shell not in {"/sbin/nologin", "/usr/sbin/nologin", "/bin/false"}
        ):
            users.append(entry)
    users.sort(key=lambda user: user.pw_uid)
    return users


def is_desktop_user(user: pwd.struct_passwd) -> bool:
    return (
        1000 <= user.pw_uid < 60000
        and user.pw_dir.startswith("/home/")
        and user.pw_shell not in {"/sbin/nologin", "/usr/sbin/nologin", "/bin/false"}
    )


def resolve_desktop_user(name: str | None) -> pwd.struct_passwd | None:
    if name:
        try:
            user = pwd.getpwnam(name)
        except KeyError:
            return None
        return user if is_desktop_user(user) else None
    if env_name := os.environ.get("MDE_REMOTE_PROOFING_USER"):
        return resolve_desktop_user(env_name)
    users = _desktop_users()
    return users[0] if users else None


def user_systemctl_env(user: pwd.struct_passwd) -> dict[str, str]:
    runtime = f"/run/user/{user.pw_uid}"
    env = os.environ.copy()
    env.update(
        {
            "HOME": user.pw_dir,
            "USER": user.pw_name,
            "LOGNAME": user.pw_name,
            "XDG_RUNTIME_DIR": runtime,
            "DBUS_SESSION_BUS_ADDRESS": f"unix:path={runtime}/bus",
        }
    )
    return env


def user_systemctl_command(user: pwd.struct_passwd, action: str) -> list[str]:
    runtime = f"/run/user/{user.pw_uid}"
    return [
        "runuser",
        "-u",
        user.pw_name,
        "--",
        "env",
        f"XDG_RUNTIME_DIR={runtime}",
        f"DBUS_SESSION_BUS_ADDRESS=unix:path={runtime}/bus",
        "systemctl",
        "--user",
        action,
        SUNSHINE_SERVICE_UNIT,
    ]


def write_user_sunshine_config(
    user: pwd.struct_passwd,
    config_text: str,
    dry_run: bool,
) -> Path:
    target = Path(user.pw_dir) / ".config" / "sunshine" / "sunshine.conf"
    if dry_run:
        return target
    target.parent.mkdir(parents=True, exist_ok=True)
    backup = target.with_suffix(target.suffix + ".mde-backup")
    if target.exists() and not backup.exists():
        try:
            backup.write_bytes(target.read_bytes())
            os.chown(backup, user.pw_uid, user.pw_gid)
            os.chmod(backup, 0o600)
        except OSError:
            pass
    _atomic_write(target, config_text)
    os.chown(target, user.pw_uid, user.pw_gid)
    os.chmod(target, 0o600)
    return target


def reconcile_firewall(
    lifecycle: dict[str, Any],
    state_path: Path,
    result: dict[str, Any],
    dry_run: bool,
) -> bool:
    previous = _load_state(state_path)
    previous_rules = set(previous.get("rich_rules") or [])
    desired_rules = set(desired_firewall_rules(lifecycle))
    remove_rules = sorted(previous_rules - desired_rules)
    add_rules = sorted(desired_rules - previous_rules)

    if not remove_rules and not add_rules:
        result["firewall"]["changed"] = False
        return True
    if not dry_run and not shutil.which("firewall-cmd"):
        result["warnings"].append("firewall-cmd is unavailable; firewall rules not applied.")
        return False

    ok = True
    for rule in remove_rules:
        ok = (
            _run(
                ["firewall-cmd", "--permanent", f"--zone={FIREWALL_ZONE}", f"--remove-rich-rule={rule}"],
                result,
                dry_run,
            )
            and ok
        )
    for rule in add_rules:
        ok = (
            _run(
                ["firewall-cmd", "--permanent", f"--zone={FIREWALL_ZONE}", f"--add-rich-rule={rule}"],
                result,
                dry_run,
            )
            and ok
        )
    if ok:
        ok = _run(["firewall-cmd", "--reload"], result, dry_run) and ok
        if not dry_run:
            _write_json(
                state_path,
                {
                    "backend": FIREWALL_BACKEND,
                    "zone": FIREWALL_ZONE,
                    "rich_rules": sorted(desired_rules),
                },
            )
    result["firewall"].update(
        {
            "changed": bool(remove_rules or add_rules),
            "added_rules": add_rules,
            "removed_rules": remove_rules,
        }
    )
    return ok


def apply_lifecycle(
    lifecycle: dict[str, Any],
    config_text: str,
    state_path: Path,
    desktop_user: str | None,
    dry_run: bool,
) -> dict[str, Any]:
    result: dict[str, Any] = {
        "managed": lifecycle["managed"],
        "dry_run": dry_run,
        "commands": [],
        "command_results": [],
        "warnings": [],
        "sunshine": {"changed": False},
        "firewall": {"changed": False},
    }
    if not lifecycle["managed"]:
        result["warnings"].append("Remote Proofing settings are absent; lifecycle apply skipped.")
        return result

    user = resolve_desktop_user(desktop_user)
    if user is None:
        result["warnings"].append("No desktop user found; Sunshine user service not managed.")
        return result
    result["sunshine"]["user"] = user.pw_name
    result["sunshine"]["uid"] = user.pw_uid

    firewall_ok = reconcile_firewall(lifecycle, state_path, result, dry_run)
    sunshine = lifecycle["sunshine"]
    if sunshine["start_allowed"] and firewall_ok:
        target = write_user_sunshine_config(user, config_text, dry_run)
        result["sunshine"]["config_path"] = str(target)
        result["sunshine"]["changed"] = True
        if not Path(f"/run/user/{user.pw_uid}/bus").exists() and not dry_run:
            result["warnings"].append("Desktop user bus is unavailable; Sunshine restart skipped.")
            return result
        if not _run(user_systemctl_command(user, "restart"), result, dry_run, env=user_systemctl_env(user)):
            result["warnings"].append("Sunshine restart failed.")
    elif sunshine["start_allowed"] and not firewall_ok:
        result["sunshine"]["changed"] = True
        result["warnings"].append("Sunshine start skipped because firewall reconciliation failed.")
        if Path(f"/run/user/{user.pw_uid}/bus").exists() or dry_run:
            if not _run(user_systemctl_command(user, "stop"), result, dry_run, env=user_systemctl_env(user)):
                result["warnings"].append("Sunshine stop failed.")
    elif sunshine["stop_required"] or sunshine["desired_state"] == "blocked":
        result["sunshine"]["changed"] = True
        if not Path(f"/run/user/{user.pw_uid}/bus").exists() and not dry_run:
            result["warnings"].append("Desktop user bus is unavailable; Sunshine stop skipped.")
            return result
        if not _run(user_systemctl_command(user, "stop"), result, dry_run, env=user_systemctl_env(user)):
            result["warnings"].append("Sunshine stop failed.")
    else:
        result["warnings"].extend(sunshine.get("blockers", []))
    return result


def _atomic_write(path: Path, data: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=path.name + ".", dir=str(path.parent), text=True)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as tmp:
            tmp.write(data)
            tmp.flush()
            os.fsync(tmp.fileno())
        os.replace(tmp_name, path)
    except Exception:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


def run_self_test() -> None:
    with tempfile.TemporaryDirectory() as td:
        root = Path(td)
        settings = root / CONFIG_FILE
        mesh = root / "mesh-status.json"
        settings.write_text(
            json.dumps(
                {
                    "enabled": True,
                    "exposure": "mesh_only",
                    "capture": "kms",
                    "encoder": "vaapi",
                    "min_fps_target": 45,
                }
            ),
            encoding="utf-8",
        )
        mesh.write_text(
            json.dumps(
                {
                    "self": "this-node",
                    "nodes": [{"hostname": "this-node", "overlay_ip": "10.42.0.8"}],
                    "network": {
                        "overlay_ip": "10.42.0.8",
                        "overlay_cidr": "10.42.0.0/16",
                        "default_gw": "172.20.0.1",
                    },
                }
            ),
            encoding="utf-8",
        )
        plan = build_plan(settings, mesh)
        assert plan["enabled"]
        assert plan["bind_scope"] == "mesh_overlay"
        assert plan["bind_address"] == "10.42.0.8"
        assert plan["firewall"]["allow_sources"] == ["10.42.0.0/16"]
        rendered = render_sunshine_config(plan)
        assert "capture = kms" in rendered
        assert "encoder = vaapi" in rendered
        assert "minimum_fps_target = 45" in rendered
        assert "address_family = ipv4" in rendered
        assert "origin_web_ui_allowed = lan" in rendered
        assert "mde_remote_proofing_enabled" not in rendered
        lifecycle = build_lifecycle(plan, root / "sunshine.conf")
        assert lifecycle["sunshine"]["desired_state"] == "ready"
        assert lifecycle["sunshine"]["start_allowed"]
        assert lifecycle["firewall"]["apply_allowed"]
        assert lifecycle["firewall"]["allow_sources"] == ["10.42.0.0/16"]

        settings.write_text(
            json.dumps({"enabled": True, "exposure": "public", "min_fps_target": 250}),
            encoding="utf-8",
        )
        public_plan = build_plan(settings, mesh)
        assert public_plan["bind_address"] == "0.0.0.0"
        assert public_plan["sunshine"]["minimum_fps_target"] == 120
        assert public_plan["degraded"]
        public_lifecycle = build_lifecycle(public_plan, root / "sunshine.conf")
        assert public_lifecycle["sunshine"]["start_allowed"]
        assert public_lifecycle["firewall"]["apply_allowed"]
        assert "origin_web_ui_allowed = wan" in render_sunshine_config(public_plan)

        settings.write_text(json.dumps({"enabled": True, "exposure": "lan"}), encoding="utf-8")
        lan_plan = build_plan(settings, mesh)
        lan_lifecycle = build_lifecycle(lan_plan, root / "sunshine.conf")
        assert not lan_lifecycle["sunshine"]["start_allowed"]
        assert lan_lifecycle["firewall"]["desired_state"] == "needs_network_resolution"
        lan_lifecycle["sunshine"]["bind_address"] = "172.20.0.15"
        lan_lifecycle["firewall"]["allow_sources"] = ["172.20.0.0/16"]
        lan_lifecycle["firewall"]["notes"] = []
        lan_lifecycle["firewall"]["resolved"] = {
            "allow_source": "172.20.0.0/16",
            "bind_address": "172.20.0.15",
            "interface": "eno1",
        }
        lan_lifecycle["degraded"] = False
        sync_plan_from_lifecycle(lan_plan, lan_lifecycle)
        assert lan_plan["bind_address"] == "172.20.0.15"
        assert lan_plan["firewall"]["allow_sources"] == ["172.20.0.0/16"]
        assert lan_plan["firewall"]["notes"] == []
        assert lan_plan["firewall"]["resolved"]["interface"] == "eno1"
        assert not lan_plan["degraded"]

        missing_plan = build_plan(root / "missing.json", root / "missing-mesh.json")
        assert not missing_plan["enabled"]
        assert missing_plan["firewall_policy"] == "closed"
        missing_lifecycle = build_lifecycle(missing_plan, root / "sunshine.conf")
        assert not missing_lifecycle["managed"]
        assert missing_lifecycle["sunshine"]["desired_state"] == "unmanaged"
        assert missing_lifecycle["firewall"]["desired_state"] == "unmanaged"
        assert desired_firewall_rules(missing_lifecycle) == []

        disabled_settings = root / "disabled.json"
        disabled_settings.write_text(json.dumps({"enabled": False}), encoding="utf-8")
        disabled_plan = build_plan(disabled_settings, mesh)
        disabled_lifecycle = build_lifecycle(disabled_plan, root / "sunshine.conf")
        assert disabled_lifecycle["managed"]
        assert disabled_lifecycle["sunshine"]["stop_required"]
        assert disabled_lifecycle["firewall"]["desired_state"] == "closed"
        assert desired_firewall_rules(disabled_lifecycle) == []


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Render the Magic Mesh Remote Proofing Sunshine/firewall plan."
    )
    parser.add_argument("--settings", type=Path, default=default_settings_path())
    parser.add_argument("--mesh-status", type=Path, default=default_mesh_status_path())
    parser.add_argument("--print-json", action="store_true")
    parser.add_argument("--print-config", action="store_true")
    parser.add_argument("--print-lifecycle", action="store_true")
    parser.add_argument("--write-plan", type=Path, nargs="?", const=DEFAULT_PLAN_PATH)
    parser.add_argument("--write-config", type=Path, nargs="?", const=DEFAULT_SUNSHINE_CONF)
    parser.add_argument("--write-lifecycle", type=Path, nargs="?", const=DEFAULT_LIFECYCLE_PATH)
    parser.add_argument("--apply-lifecycle", action="store_true")
    parser.add_argument("--apply-dry-run", action="store_true")
    parser.add_argument("--print-apply-result", action="store_true")
    parser.add_argument("--desktop-user")
    parser.add_argument("--firewall-state", type=Path, default=DEFAULT_FIREWALL_STATE)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.self_test:
        run_self_test()
        return 0

    plan = build_plan(args.settings, args.mesh_status)
    lifecycle_config_path = args.write_config or DEFAULT_SUNSHINE_CONF
    lifecycle = build_lifecycle(plan, lifecycle_config_path)
    if args.apply_lifecycle or args.apply_dry_run:
        resolve_trusted_lan_lifecycle(lifecycle)
        sync_plan_from_lifecycle(plan, lifecycle)
    config_text = render_sunshine_config_from_lifecycle(lifecycle)
    did_write = False
    if args.write_plan:
        _atomic_write(args.write_plan, json.dumps(plan, indent=2, sort_keys=True) + "\n")
        did_write = True
    if args.write_config:
        _atomic_write(args.write_config, config_text)
        did_write = True
    if args.write_lifecycle:
        _atomic_write(args.write_lifecycle, json.dumps(lifecycle, indent=2, sort_keys=True) + "\n")
        did_write = True
    apply_result = None
    if args.apply_lifecycle or args.apply_dry_run:
        apply_result = apply_lifecycle(
            lifecycle,
            config_text,
            args.firewall_state,
            args.desktop_user,
            args.apply_dry_run,
        )
        did_write = True
    if args.print_config:
        sys.stdout.write(config_text)
    if args.print_lifecycle:
        sys.stdout.write(json.dumps(lifecycle, indent=2, sort_keys=True) + "\n")
    if args.print_apply_result:
        if apply_result is None:
            apply_result = apply_lifecycle(
                lifecycle,
                config_text,
                args.firewall_state,
                args.desktop_user,
                True,
            )
        sys.stdout.write(json.dumps(apply_result, indent=2, sort_keys=True) + "\n")
    if (
        args.print_json
        or not args.print_config
        and not args.print_lifecycle
        and not args.print_apply_result
        and not did_write
    ):
        sys.stdout.write(json.dumps(plan, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
