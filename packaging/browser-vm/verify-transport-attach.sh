#!/usr/bin/env bash
# Validate the typed Browser VM attach envelope consumed by the shell's VDI
# broker mirror. This is intentionally host-side verification; the guest never
# receives a command, credential, URL, or arbitrary transport configuration.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
CONTRACT="$ROOT/packaging/browser-vm/browser-vm-transport-attach.schema.json"
EXAMPLE="$ROOT/packaging/browser-vm/browser-vm-transport-attach.example.json"

die() {
    echo "verify-browser-vm-transport-attach: $*" >&2
    exit 1
}

[ -f "$CONTRACT" ] || die "attach schema is missing"
[ -f "$EXAMPLE" ] || die "attach example is missing"
[ ! -L "$CONTRACT" ] || die "attach schema must not be a symlink"
[ ! -L "$EXAMPLE" ] || die "attach example must not be a symlink"

if command -v jq >/dev/null 2>&1; then
    json_valid() { jq -e . "$1" >/dev/null; }
else
    command -v python3 >/dev/null 2>&1 || die "jq or python3 is required for typed JSON validation"
    json_valid() {
        ATTACH_FILE=$1 python3 - <<'PY'
import json
import os

with open(os.environ["ATTACH_FILE"], encoding="utf-8") as stream:
    json.load(stream)
PY
    }
fi

json_valid "$CONTRACT" || die "attach schema is not valid JSON"
json_valid "$EXAMPLE" || die "attach example is not valid JSON"

attach_filter=''
attach_filter+='(keys == ["generation","schema_version","session_id","status","surface","transport","workload"]) and '
attach_filter+='(.schema_version == 1 and .workload == "browser-vm" and .surface == "browser") and '
attach_filter+='(.session_id | (type == "string" and test("^session:[A-Za-z0-9._:-]{1,127}$"))) and '
attach_filter+='(.generation | (type == "number" and floor == . and . >= 1 and . <= 9223372036854775807)) and '
attach_filter+='(.transport == "rdp" or .transport == "spice") and '
attach_filter+='(.status.protocol == .transport) and '
attach_filter+='(.status | keys == ["host","port","protocol","state"] and '
attach_filter+='  .state == "brokered" and (.protocol == "rdp" or .protocol == "spice") and '
attach_filter+='  (.host | (type == "string" and test("^[A-Za-z0-9._:-]{1,253}$"))) and '
attach_filter+='  (.port | (type == "number" and floor == . and . >= 1 and . <= 65535)))'

validate_attach() {
    local file=$1
    if command -v jq >/dev/null 2>&1; then
        jq -e "$attach_filter" "$file" >/dev/null
    else
        ATTACH_FILE=$file python3 - <<'PY'
import json
import os
import re

with open(os.environ["ATTACH_FILE"], encoding="utf-8") as stream:
    value = json.load(stream)
required = {"generation", "schema_version", "session_id", "status", "surface", "transport", "workload"}
if set(value) != required:
    raise SystemExit(1)
if value["schema_version"] != 1 or value["workload"] != "browser-vm" or value["surface"] != "browser":
    raise SystemExit(1)
if not isinstance(value["session_id"], str) or not re.fullmatch(r"session:[A-Za-z0-9._:-]{1,127}", value["session_id"]):
    raise SystemExit(1)
if not isinstance(value["generation"], int) or not 1 <= value["generation"] <= 9223372036854775807:
    raise SystemExit(1)
if value["transport"] not in {"rdp", "spice"}:
    raise SystemExit(1)
status = value["status"]
if set(status) != {"host", "port", "protocol", "state"}:
    raise SystemExit(1)
if status["state"] != "brokered" or status["protocol"] not in {"rdp", "spice"}:
    raise SystemExit(1)
if status["protocol"] != value["transport"]:
    raise SystemExit(1)
if not isinstance(status["host"], str) or not re.fullmatch(r"[A-Za-z0-9._:-]{1,253}", status["host"]):
    raise SystemExit(1)
if not isinstance(status["port"], int) or not 1 <= status["port"] <= 65535:
    raise SystemExit(1)
PY
    fi
}

validate_attach "$EXAMPLE" || die "attach example violates the typed contract"

fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT

reject() {
    local label=$1 file=$2
    if validate_attach "$file" >/dev/null 2>&1; then
        die "accepted invalid $label fixture"
    fi
}

printf '%s\n' '{"schema_version":1,"workload":"browser-vm","surface":"browser","session_id":"session:browser-vm-00000001","generation":1,"transport":"rdp","status":{"state":"brokered","protocol":"rdp","host":"https://attacker.invalid","port":3389}}' > "$fixture/url.json"
reject "URL endpoint" "$fixture/url.json"

printf '%s\n' '{"schema_version":1,"workload":"browser-vm","surface":"browser","session_id":"session:browser-vm-00000001","generation":1,"transport":"sunshine","status":{"state":"brokered","protocol":"sunshine","host":"10.42.0.50","port":3389}}' > "$fixture/unsupported.json"
reject "unsupported Sunshine-only shell attach" "$fixture/unsupported.json"

printf '%s\n' '{"schema_version":1,"workload":"browser-vm","surface":"browser","session_id":"session:browser-vm-00000001","generation":1,"transport":"rdp","status":{"state":"brokered","protocol":"spice","host":"10.42.0.50","port":5900}}' > "$fixture/mismatched-protocol.json"
reject "transport/protocol mismatch" "$fixture/mismatched-protocol.json"

printf '%s\n' '{"schema_version":1,"workload":"browser-vm","surface":"browser","session_id":"session:browser-vm-00000001","generation":1,"transport":"rdp","command":"flatpak run","status":{"state":"brokered","protocol":"rdp","host":"10.42.0.50","port":3389}}' > "$fixture/command.json"
reject "executable host input" "$fixture/command.json"

echo "Browser VM transport attach contract passed: state/vdi/console RDP/SPICE envelope"
