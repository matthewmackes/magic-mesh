#!/usr/bin/env bash
# Bounded live-seat proof for one daemon-admitted Internet-radio station.
# The default path is read-only. Playback is possible only with --play-probe,
# uses the typed Music workspace action, and is stopped before this helper exits.
set -euo pipefail

readonly EXPECTED_PACKAGE_ARCH='x86_64'
readonly DEFAULT_STATION='C-SPAN Radio'
readonly MUSIC_CREDENTIAL='/etc/credstore.encrypted/music-action-private-key'

fail() { printf '[FAIL] %s\n' "$1" >&2; exit 2; }

bounded_integer() {
    local value=$1 minimum=$2 maximum=$3
    [[ "$value" =~ ^[0-9]+$ ]] && (( value >= minimum && value <= maximum ))
}

validate_station_name() {
    local value=$1
    [[ -n "$value" && ${#value} -le 128 &&
        "$value" != *[$'\001'-$'\037'$'\177']* ]]
}

validate_bus_root() {
    [[ "$1" =~ ^/[A-Za-z0-9._/-]{1,511}$ && "$1" != *'/../'* &&
        "$1" != */.. && "$1" != *'/./'* && "$1" != */. ]]
}

declared_release_version() {
    local cargo_toml=$1
    [[ -f "$cargo_toml" && ! -L "$cargo_toml" ]] || return 1
    awk '
        $0 ~ /^\[workspace\.package\][[:space:]]*$/ { in_workspace = 1; next }
        in_workspace && $0 ~ /^\[/ { exit }
        in_workspace && $0 ~ /^[[:space:]]*version[[:space:]]*=/ {
            value = $0
            sub(/^[^"]*"/, "", value)
            sub(/".*$/, "", value)
            if (value != "") { print value; found = 1; exit }
        }
        END { if (!found) exit 1 }
    ' "$cargo_toml"
}

declared_rpm_release() {
    local cargo_toml=$1
    [[ -f "$cargo_toml" && ! -L "$cargo_toml" ]] || return 1
    awk '
        $0 ~ /^\[package\.metadata\.generate-rpm\][[:space:]]*$/ { in_rpm = 1; next }
        in_rpm && $0 ~ /^\[/ { exit }
        in_rpm && $0 ~ /^[[:space:]]*release[[:space:]]*=/ {
            value = $0
            sub(/^[^"]*"/, "", value)
            sub(/".*$/, "", value)
            if (value != "") { print value; found = 1; exit }
        }
        END { if (!found) exit 1 }
    ' "$cargo_toml"
}

validate_release_version() {
    [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]
}

validate_rpm_release() {
    [[ "$1" =~ ^[0-9][0-9A-Za-z._+~-]*$ ]]
}

# Select exactly one named station and validate its provider stream locator.
# The selected URL is written only to a mode-0600 temporary file and is never
# echoed, because provider URLs may contain short-lived query credentials.
parse_station_reply() {
    local reply_path=$1 station_name=$2 output_path=$3
    python3 - "$reply_path" "$station_name" "$output_path" <<'PY'
import json
import os
import stat
import sys
import urllib.parse

reply_path, wanted, output_path = sys.argv[1:]

def die(message):
    raise SystemExit(message)

try:
    with open(reply_path, "r", encoding="utf-8") as handle:
        value = json.load(handle)
except (OSError, json.JSONDecodeError):
    die("list-radio reply is not valid JSON")
if isinstance(value, dict) and isinstance(value.get("body"), str):
    try:
        value = json.loads(value["body"])
    except json.JSONDecodeError:
        die("list-radio reply body is not valid JSON")
if not isinstance(value, dict) or value.get("ok") is False:
    die("list-radio did not return a successful reply")
result = value.get("result", value)
rows = result.get("radio") if isinstance(result, dict) else None
if not isinstance(rows, list) or len(rows) > 500:
    die("list-radio has no bounded radio collection")
matches = []
for row in rows:
    if not isinstance(row, dict):
        continue
    name = row.get("name", row.get("title"))
    if isinstance(name, str) and name.strip().casefold() == wanted.strip().casefold():
        matches.append(row)
if not matches:
    die("named station is absent from list-radio")
if len(matches) != 1:
    die("named station is ambiguous in list-radio")
row = matches[0]
name = row.get("name", row.get("title"))
station_id = row.get("id")
url = row.get("streamUrl", row.get("stream_url"))
if not isinstance(name, str) or not name.strip() or len(name) > 128 or any(c.isspace() and c not in " " for c in name) or any(ord(c) < 32 for c in name):
    die("named station has a malformed display name")
if not isinstance(station_id, str) or not station_id.strip() or len(station_id) > 256 or any(ord(c) < 32 for c in station_id):
    die("named station has a malformed provider id")
if not isinstance(url, str) or not url or len(url) > 2048 or any(c.isspace() or ord(c) < 32 for c in url):
    die("named station has an empty or malformed stream URL")
try:
    parsed = urllib.parse.urlsplit(url)
    port = parsed.port
except ValueError:
    die("named station has an empty or malformed stream URL")
if parsed.scheme.lower() not in ("http", "https") or not parsed.netloc or not parsed.hostname:
    die("named station stream URL is not bounded http(s)")
if parsed.username is not None or parsed.password is not None or parsed.fragment:
    die("named station stream URL contains forbidden authority or fragment data")
if port is not None and not 1 <= port <= 65535:
    die("named station stream URL has an invalid port")
if len(parsed.hostname) > 253 or any(c.isspace() or ord(c) < 32 for c in parsed.hostname):
    die("named station stream URL has a malformed host")
selected = {"name": name.strip(), "id": station_id, "stream_url": url}
with open(output_path, "w", encoding="utf-8") as handle:
    json.dump(selected, handle, separators=(",", ":"))
    handle.write("\n")
os.chmod(output_path, stat.S_IRUSR | stat.S_IWUSR)
PY
}

# Resolve the list-radio row to the daemon's retained ContentRef. This proves
# typed admission, rather than treating a raw provider URL as playable merely
# because it was syntactically valid.
parse_workspace_admission() {
    local history_path=$1 station_path=$2 output_path=$3
    python3 - "$history_path" "$station_path" "$output_path" <<'PY'
import json
import os
import stat
import sys

history_path, station_path, output_path = sys.argv[1:]

def die(message):
    raise SystemExit(message)

try:
    with open(station_path, "r", encoding="utf-8") as handle:
        station = json.load(handle)
    with open(history_path, "r", encoding="utf-8") as handle:
        lines = [line for line in handle if line.strip()]
    stored = json.loads(lines[-1])
except (OSError, IndexError, json.JSONDecodeError):
    die("retained Music workspace snapshot is unavailable")
snapshot = stored.get("body") if isinstance(stored, dict) else None
if isinstance(snapshot, str):
    try:
        snapshot = json.loads(snapshot)
    except json.JSONDecodeError:
        die("retained Music workspace body is malformed")
if not isinstance(snapshot, dict):
    die("retained Music workspace body is absent")
collections = snapshot.get("collections")
if not isinstance(collections, list) or len(collections) > 32:
    die("retained Music workspace collections are malformed")
matches = []
for collection in collections:
    if not isinstance(collection, dict) or collection.get("key") != "radio":
        continue
    items = collection.get("items")
    if not isinstance(items, list) or len(items) > 500:
        die("retained radio collection is malformed or unbounded")
    for item in items:
        if not isinstance(item, dict) or item.get("kind") != "radio":
            continue
        if str(item.get("title", "")).strip().casefold() != station["name"].casefold():
            continue
        variants = item.get("variants")
        if not isinstance(variants, list) or len(variants) > 32:
            die("retained radio variants are malformed or unbounded")
        for variant in variants:
            content = variant.get("content") if isinstance(variant, dict) else None
            if not isinstance(content, dict):
                continue
            if content.get("kind") == "radio" and content.get("remote_id") == station["stream_url"]:
                cached = variant.get("cached") is True
                reachable = variant.get("reachable") is True
                if cached or reachable:
                    priority = variant.get("operator_priority", 0)
                    latency = variant.get("latency_ms")
                    if not isinstance(priority, int) or priority < 0:
                        die("retained radio variant has malformed priority")
                    if latency is not None and (not isinstance(latency, int) or latency < 0):
                        die("retained radio variant has malformed latency")
                    matches.append((
                        not cached,
                        not reachable,
                        -priority,
                        latency is None,
                        latency if latency is not None else 0,
                        content,
                    ))
if not matches:
    die("named station has no reachable or cached typed radio variant")
# Python's sort is stable, matching Rust's stable slice sort for variants whose
# cache/reachability/priority/latency keys are equal.
matches.sort(key=lambda candidate: candidate[:-1])
content = matches[0][-1]
source_id = content.get("source_id")
remote_id = content.get("remote_id")
if not isinstance(source_id, str) or not source_id.strip() or len(source_id) > 256 or any(ord(c) < 32 for c in source_id):
    die("retained station has a malformed source identity")
if not isinstance(remote_id, str) or remote_id != station["stream_url"]:
    die("retained station URL differs from list-radio")
with open(output_path, "w", encoding="utf-8") as handle:
    json.dump({"source_id": source_id, "remote_id": remote_id, "kind": "radio"}, handle, separators=(",", ":"))
    handle.write("\n")
os.chmod(output_path, stat.S_IRUSR | stat.S_IWUSR)
PY
}

validate_state_reply() {
    local reply_path=$1 mode=$2 station_path=${3:-}
    python3 - "$reply_path" "$mode" "$station_path" <<'PY'
import json
import sys

reply_path, mode, station_path = sys.argv[1:]
try:
    with open(reply_path, "r", encoding="utf-8") as handle:
        value = json.load(handle)
    if isinstance(value, dict) and isinstance(value.get("body"), str):
        value = json.loads(value["body"])
except (OSError, json.JSONDecodeError):
    raise SystemExit("get-state reply is malformed")
if not isinstance(value, dict) or value.get("ok") is False:
    raise SystemExit("get-state reply is unsuccessful")
if not all(key in value for key in ("active", "playing", "audio_available")):
    raise SystemExit("get-state reply lacks engine state")
if mode == "idle":
    if not value.get("audio_available"):
        raise SystemExit("seat has no admitted audio engine")
    if value.get("active") or value.get("playing"):
        raise SystemExit("seat already has active playback; refusing to disturb it")
elif mode == "station":
    with open(station_path, "r", encoding="utf-8") as handle:
        station = json.load(handle)
    if not value.get("active") or not value.get("playing") or value.get("song_id") != station["stream_url"]:
        raise SystemExit("engine does not report the selected station active")
elif mode == "stopped":
    if value.get("active") or value.get("playing"):
        raise SystemExit("playback engine remained active after cleanup")
else:
    raise SystemExit("unknown state validation mode")
PY
}

validate_workspace_reply() {
    local reply_path=$1 request_id=$2
    python3 - "$reply_path" "$request_id" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], "r", encoding="utf-8") as handle:
        value = json.load(handle)
    if isinstance(value, dict) and isinstance(value.get("body"), str):
        value = json.loads(value["body"])
except (OSError, json.JSONDecodeError):
    raise SystemExit("typed workspace reply is malformed")
if not isinstance(value, dict) or value.get("accepted") is not True:
    code = value.get("error_code") if isinstance(value, dict) else None
    if not isinstance(code, str) or not code or len(code) > 64 or not all(
        c.isascii() and (c.isalnum() or c in "_-") for c in code
    ):
        code = "unspecified"
    raise SystemExit(f"typed workspace action was not admitted ({code})")
if value.get("request_id") != sys.argv[2]:
    raise SystemExit("typed workspace reply has the wrong request id")
PY
}

build_action_body() {
    local action=$1 request_id=$2 content_path=$3 output_path=$4
    python3 - "$action" "$request_id" "$content_path" "$output_path" <<'PY'
import json
import os
import stat
import sys

action, request_id, content_path, output_path = sys.argv[1:]
body = {"schema_version": 1, "request_id": request_id, "action": action}
if content_path:
    with open(content_path, "r", encoding="utf-8") as handle:
        body["content"] = json.load(handle)
with open(output_path, "w", encoding="utf-8") as handle:
    json.dump(body, handle, separators=(",", ":"))
    handle.write("\n")
os.chmod(output_path, stat.S_IRUSR | stat.S_IWUSR)
PY
}

# Run only for an explicit playback probe. The root-only encrypted credential
# is decrypted inside a root temporary directory, used to mint one short-lived
# exact-body Ed25519 capability, and removed before the signer exits. Only the
# signed request (never the seed) reaches stdout.
sign_workspace_action() {
    local unsigned_path=$1 signed_path=$2
    local signed_body
    signed_body=$(sudo -n python3 - "$unsigned_path" "$MUSIC_CREDENTIAL" <<'PY'
import hashlib
import json
import os
import secrets
import stat
import subprocess
import sys
import tempfile
import time

unsigned_path, credential_path = sys.argv[1:]
with open(unsigned_path, "r", encoding="utf-8") as handle:
    document = json.load(handle)
canonical = json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
digest = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
nonce = secrets.token_hex(32)
expires = int(time.time() * 1000) + 20_000
node = subprocess.check_output(["hostname"], text=True).strip()
if not node:
    raise SystemExit("local hostname is unavailable")
token = {
    "schema_version": 1,
    "key_id": "music-action-ed25519-v1",
    "nonce": nonce,
    "expires_at_ms": expires,
    "verb": "music-workspace",
    "node": node,
    "target": "workspace",
    "request_sha256": digest,
}
payload = "|".join([
    "magic-mesh:music-action-ed25519:v1", "1", token["key_id"], nonce,
    str(expires), token["verb"], node, token["target"], digest,
]).encode("utf-8")
with tempfile.TemporaryDirectory(prefix="mcnf-radio-sign-", dir="/run") as temporary:
    seed_path = os.path.join(temporary, "seed")
    key_path = os.path.join(temporary, "key.der")
    payload_path = os.path.join(temporary, "payload")
    signature_path = os.path.join(temporary, "signature")
    subprocess.run(
        ["systemd-creds", "decrypt", credential_path, seed_path],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    with open(seed_path, "r", encoding="ascii") as handle:
        seed_hex = handle.read().strip()
    if len(seed_hex) != 64 or any(c not in "0123456789abcdefABCDEF" for c in seed_hex):
        raise SystemExit("Music action credential is malformed")
    with open(key_path, "wb") as handle:
        handle.write(bytes.fromhex("302e020100300506032b657004220420") + bytes.fromhex(seed_hex))
    with open(payload_path, "wb") as handle:
        handle.write(payload)
    os.chmod(key_path, stat.S_IRUSR | stat.S_IWUSR)
    subprocess.run(
        ["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", key_path,
         "-keyform", "DER", "-in", payload_path, "-out", signature_path],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    with open(signature_path, "rb") as handle:
        signature = handle.read()
if len(signature) != 64:
    raise SystemExit("Music action signature has the wrong length")
token["signature"] = signature.hex()
document["music_auth"] = token
print(json.dumps(document, separators=(",", ":"), ensure_ascii=False))
PY
    )
    printf '%s\n' "$signed_body" >"$signed_path"
    unset signed_body
    chmod 0600 "$signed_path"
}

remote_request() {
    local topic=$1 body_path=${2:-} output_path=$3
    local uid
    uid=$(id -u)
    if [[ -n "$body_path" ]]; then
        timeout --signal=TERM --kill-after=2s "${COMMAND_TIMEOUT}s" \
            env XDG_RUNTIME_DIR="/run/user/$uid" mde-bus request "$topic" \
            "$(<"$body_path")" --bus-root "$BUS_ROOT" \
            --timeout-secs "$COMMAND_TIMEOUT" --json >"$output_path" 2>/dev/null
    else
        timeout --signal=TERM --kill-after=2s "${COMMAND_TIMEOUT}s" \
            env XDG_RUNTIME_DIR="/run/user/$uid" mde-bus request "$topic" \
            --bus-root "$BUS_ROOT" --timeout-secs "$COMMAND_TIMEOUT" \
            --json >"$output_path" 2>/dev/null
    fi
}

remote_run() {
    [[ $# == 7 ]] || fail 'invalid internal remote invocation'
    local station_b64=$1
    BUS_ROOT=$2
    COMMAND_TIMEOUT=$3
    local play_probe=$4
    PROBE_SECONDS=$5
    local expected_version=$6 expected_rpm_release=$7
    local station
    station=$(printf '%s' "$station_b64" | base64 --decode) || fail 'invalid encoded station name'
    validate_station_name "$station" || fail 'station name is empty, oversized, or contains control characters'
    validate_bus_root "$BUS_ROOT" || fail 'Bus root must be a bounded absolute path without traversal components'
    bounded_integer "$COMMAND_TIMEOUT" 1 30 || fail 'command timeout must be 1..30 seconds'
    [[ "$play_probe" == 0 || "$play_probe" == 1 ]] || fail 'invalid internal playback mode'
    bounded_integer "$PROBE_SECONDS" 2 15 || fail 'probe duration must be 2..15 seconds'
    validate_release_version "$expected_version" || fail 'invalid expected platform version'
    validate_rpm_release "$expected_rpm_release" || fail 'invalid expected RPM release'
    command -v timeout >/dev/null 2>&1 || fail 'timeout is required on the seat'
    command -v python3 >/dev/null 2>&1 || fail 'python3 is required on the seat'
    command -v mde-bus >/dev/null 2>&1 || fail 'mde-bus is unavailable on the seat'

    proof_dir=$(mktemp -d /tmp/mcnf-radio-proof.XXXXXX)
    chmod 0700 "$proof_dir"
    cleanup_required=0
    cleanup_failed=0

    cleanup_probe() {
        local unsigned_path="$proof_dir/stop-unsigned.json"
        local signed_path="$proof_dir/stop-signed.json"
        local reply_path="$proof_dir/stop-reply.json"
        local stopped_path="$proof_dir/stopped-state.json"
        local request_id
        request_id="radio-proof-stop-$(date +%s)-$$-$RANDOM"
        # A probe starts only after an idle-state gate. Once admitted, always
        # send its paired typed stop, including from the EXIT/TERM trap.
        build_action_body stop "$request_id" '' "$unsigned_path" || return 1
        sign_workspace_action "$unsigned_path" "$signed_path" || return 1
        remote_request action/music/workspace "$signed_path" "$reply_path" || return 1
        validate_workspace_reply "$reply_path" "$request_id" || return 1
        remote_request action/music/get-state '' "$stopped_path" || return 1
        validate_state_reply "$stopped_path" stopped || return 1
        cleanup_required=0
        printf '[OK] explicit radio probe stopped through the typed workspace action; temporary capability files removed\n'
    }

    finish() {
        local rc=$?
        trap - EXIT INT TERM
        if (( cleanup_required )); then
            if ! cleanup_probe; then
                printf '[FAIL] explicit radio probe cleanup could not prove the engine stopped\n' >&2
                cleanup_failed=1
            fi
        fi
        rm -rf -- "$proof_dir"
        if (( cleanup_failed )); then rc=1; fi
        exit "$rc"
    }
    trap finish EXIT
    trap 'exit 130' INT TERM

    local service_pid service_exe restarts package_identity owner_identity
    if ! timeout --signal=TERM --kill-after=2s "${COMMAND_TIMEOUT}s" \
        systemctl --user is-active --quiet mde-musicd.service; then
        fail 'mde-musicd.service is not active'
    fi
    service_pid=$(timeout --signal=TERM --kill-after=2s "${COMMAND_TIMEOUT}s" \
        systemctl --user show mde-musicd.service -p MainPID --value)
    restarts=$(timeout --signal=TERM --kill-after=2s "${COMMAND_TIMEOUT}s" \
        systemctl --user show mde-musicd.service -p NRestarts --value)
    [[ "$service_pid" =~ ^[1-9][0-9]*$ && "$restarts" == 0 ]] ||
        fail 'mde-musicd.service lacks a live PID or has restarted'
    service_exe=$(readlink -f -- "/proc/$service_pid/exe")
    [[ "$service_exe" == /usr/bin/mde-musicd ]] || fail 'mde-musicd.service is not executing /usr/bin/mde-musicd'
    printf '[OK] service preflight: mde-musicd.service active (NRestarts=0)\n'

    package_identity=$(timeout --signal=TERM --kill-after=2s "${COMMAND_TIMEOUT}s" \
        rpm -q --qf '%{NAME}\t%{VERSION}\t%{RELEASE}\t%{ARCH}' magic-mesh)
    owner_identity=$(timeout --signal=TERM --kill-after=2s "${COMMAND_TIMEOUT}s" \
        rpm -qf --qf '%{NAME}\t%{VERSION}\t%{RELEASE}\t%{ARCH}' "$service_exe")
    local expected_identity="magic-mesh"$'\t'"$expected_version"$'\t'"$expected_rpm_release"$'\t'"$EXPECTED_PACKAGE_ARCH"
    [[ "$package_identity" == "$expected_identity" && "$owner_identity" == "$expected_identity" ]] ||
        fail 'installed package or active daemon does not match the checked-out release identity'
    local verify_path="$proof_dir/rpm-verify" verify_rc=0 verify_line
    timeout --signal=TERM --kill-after=2s "${COMMAND_TIMEOUT}s" \
        rpm -V magic-mesh >"$verify_path" 2>/dev/null || verify_rc=$?
    if (( verify_rc == 0 )); then
        [[ ! -s "$verify_path" ]] || fail 'rpm verification returned unexpected output with success status'
    elif (( verify_rc == 1 )); then
        while IFS= read -r verify_line; do
            [[ "$verify_line" == 'S.5....T.    /opt/mcnf/automation/secrets/mcnf-secret.sh' ]] ||
                fail 'rpm verification found an unexpected installed-file difference'
        done <"$verify_path"
        [[ -s "$verify_path" ]] || fail 'rpm verification failed without a bounded diagnostic'
    else
        fail 'rpm verification command failed'
    fi
    printf '[OK] package preflight: magic-mesh-%s-%s.%s owns the active daemon and has no unapproved rpm -V differences\n' \
        "$expected_version" "$expected_rpm_release" "$EXPECTED_PACKAGE_ARCH"

    local ping_path="$proof_dir/ping"
    timeout --signal=TERM --kill-after=2s "${COMMAND_TIMEOUT}s" \
        mde-musicd ping --retry 0 >"$ping_path" 2>/dev/null || fail 'mde-musicd provider ping failed'
    [[ -s "$ping_path" ]] || fail 'mde-musicd provider ping returned no result'
    printf '[OK] provider preflight: mde-musicd ping answered\n'

    local radio_reply="$proof_dir/list-radio.json" station_path="$proof_dir/station.json"
    remote_request action/music/list-radio '' "$radio_reply" || fail 'bounded action/music/list-radio request failed'
    parse_station_reply "$radio_reply" "$station" "$station_path" || fail 'named station failed list-radio validation'
    printf '[PROOF] catalog: PASS — %s is present and has one bounded http(s) stream URL (URL redacted)\n' "$station"

    local history_path="$proof_dir/workspace-history.jsonl" content_path="$proof_dir/content.json"
    local deadline=$((SECONDS + COMMAND_TIMEOUT)) admitted=0
    while (( SECONDS <= deadline )); do
        if timeout --signal=TERM --kill-after=2s "${COMMAND_TIMEOUT}s" \
            mde-bus history state/music/workspace --count 1 --reverse \
            --bus-root "$BUS_ROOT" --json >"$history_path" 2>/dev/null &&
            parse_workspace_admission "$history_path" "$station_path" "$content_path" >/dev/null 2>&1; then
            admitted=1
            break
        fi
        sleep 1
    done
    (( admitted )) || fail 'catalog station did not converge into the retained typed radio catalog'
    printf '[PROOF] typed admission: PASS — retained radio ContentRef matches the list-radio URL and admitted source\n'

    if (( ! play_probe )); then
        printf '[INFO] playback probe disabled; pass --play-probe to permit a short typed play/stop mutation\n'
        printf '[NOT PROVEN] audible/rendered output: catalog and admission checks do not prove decoded sound, speaker output, or rendered UI\n'
        printf 'verify-music-live-radio: PASS (read-only catalog/admission proof only)\n'
        return 0
    fi

    command -v sudo >/dev/null 2>&1 || fail 'sudo is required for explicit typed playback signing'
    command -v openssl >/dev/null 2>&1 || fail 'openssl is required for explicit typed playback signing'
    command -v systemd-creds >/dev/null 2>&1 || fail 'systemd-creds is required for explicit typed playback signing'
    sudo -n test -r "$MUSIC_CREDENTIAL" || fail 'root Music action credential is unavailable for explicit playback'
    local initial_state="$proof_dir/initial-state.json"
    remote_request action/music/get-state '' "$initial_state" || fail 'could not read initial playback state'
    validate_state_reply "$initial_state" idle "$station_path" || fail 'seat is not idle with an available audio engine'

    local play_request_id
    play_request_id="radio-proof-play-$(date +%s)-$$-$RANDOM"
    local play_unsigned="$proof_dir/play-unsigned.json" play_signed="$proof_dir/play-signed.json"
    local play_reply="$proof_dir/play-reply.json" probe_state="$proof_dir/probe-state.json"
    build_action_body play "$play_request_id" "$content_path" "$play_unsigned"
    sign_workspace_action "$play_unsigned" "$play_signed" || fail 'could not mint the short-lived Music playback capability'
    remote_request action/music/workspace "$play_signed" "$play_reply" || fail 'typed radio play request failed'
    validate_workspace_reply "$play_reply" "$play_request_id" || fail 'typed radio play request was not admitted'
    cleanup_required=1

    local probe_deadline=$((SECONDS + PROBE_SECONDS)) active_samples=0
    while (( SECONDS <= probe_deadline )); do
        if remote_request action/music/get-state '' "$probe_state" &&
            validate_state_reply "$probe_state" station "$station_path" >/dev/null 2>&1; then
            active_samples=$((active_samples + 1))
            (( active_samples >= 2 )) && break
        else
            active_samples=0
        fi
        sleep 1
    done
    (( active_samples >= 2 )) || fail 'typed play was admitted but the selected station did not remain active for two samples'
    printf '[PROOF] playback admission: PASS — typed play was accepted and daemon engine state remained active for two samples\n'
    cleanup_probe || fail 'typed stop/cleanup did not complete'
    printf '[NOT PROVEN] audible/rendered output: active daemon state is not microphone/sink capture, human audibility, speaker, or UI-render proof\n'
    printf 'verify-music-live-radio: PASS (bounded typed playback probe; not audible/rendered proof)\n'
}

self_test() {
    command -v python3 >/dev/null 2>&1 || fail 'python3 is required for self-test'
    local test_dir
    test_dir=$(mktemp -d)
    trap 'rm -rf -- "$test_dir"' EXIT
    local valid="$test_dir/valid.json" selected="$test_dir/selected.json"
    printf '%s\n' '{"body":"{\"ok\":true,\"result\":{\"radio\":[{\"id\":\"station-1\",\"name\":\"C-SPAN Radio\",\"streamUrl\":\"https://radio.example/live?token=redacted\"}]}}"}' >"$valid"
    parse_station_reply "$valid" 'c-span radio' "$selected"
    python3 - "$selected" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    value = json.load(handle)
assert value["name"] == "C-SPAN Radio"
assert value["stream_url"].startswith("https://")
PY
    local malformed="$test_dir/malformed.json"
    for fixture in \
        '{"ok":true,"result":{"radio":[{"id":"x","name":"C-SPAN Radio","streamUrl":""}]}}' \
        '{"ok":true,"result":{"radio":[{"id":"x","name":"C-SPAN Radio","streamUrl":"ftp://radio.example/live"}]}}' \
        '{"ok":true,"result":{"radio":[{"id":"x","name":"C-SPAN Radio","streamUrl":"https://user:pass@radio.example/live"}]}}' \
        '{"ok":true,"result":{"radio":[{"id":"x","name":"C-SPAN Radio","streamUrl":"not-a-url"}]}}'; do
        printf '%s\n' "$fixture" >"$malformed"
        if parse_station_reply "$malformed" 'C-SPAN Radio' "$selected" >/dev/null 2>&1; then
            fail 'self-test accepted an empty or malformed stream URL'
        fi
    done
    printf '%s\n' '{"ok":true,"result":{"radio":[{"id":"a","name":"C-SPAN Radio","streamUrl":"https://a.example/live"},{"id":"b","name":"c-span radio","streamUrl":"https://b.example/live"}]}}' >"$malformed"
    if parse_station_reply "$malformed" 'C-SPAN Radio' "$selected" >/dev/null 2>&1; then
        fail 'self-test accepted an ambiguous station name'
    fi
    local history="$test_dir/history.jsonl" content="$test_dir/content.json"
    printf '%s\n' '{"body":"{\"schema_version\":1,\"collections\":[{\"key\":\"radio\",\"items\":[{\"kind\":\"radio\",\"title\":\"C-SPAN Radio\",\"variants\":[{\"content\":{\"source_id\":\"airsonic:https://music.example\",\"remote_id\":\"https://radio.example/live?token=redacted\",\"kind\":\"radio\"},\"cached\":false,\"reachable\":true,\"operator_priority\":0,\"latency_ms\":null},{\"content\":{\"source_id\":\"airsonic:https://backup.example\",\"remote_id\":\"https://radio.example/live?token=redacted\",\"kind\":\"radio\"},\"cached\":true,\"reachable\":false,\"operator_priority\":0,\"latency_ms\":null}]}]}]}"}' >"$history"
    parse_workspace_admission "$history" "$selected" "$content"
    python3 - "$content" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    value = json.load(handle)
assert value["source_id"] == "airsonic:https://backup.example"
assert value["kind"] == "radio"
PY
    printf '%s\n' '{"body":"{\"ok\":true,\"active\":false,\"playing\":false,\"audio_available\":true}"}' >"$valid"
    validate_state_reply "$valid" idle
    printf '%s\n' '{"body":"{\"ok\":true,\"active\":true,\"playing\":true,\"audio_available\":true,\"song_id\":\"https://radio.example/live?token=redacted\"}"}' >"$valid"
    validate_state_reply "$valid" station "$selected"
    bounded_integer 2 2 15
    bounded_integer 15 2 15
    if bounded_integer 1 2 15 || bounded_integer 16 2 15 || bounded_integer nope 2 15; then
        fail 'self-test accepted an invalid probe duration'
    fi
    validate_station_name 'C-SPAN Radio'
    for bad_name in $'bad\nstation' $'bad\033station' $'bad\177station'; do
        if validate_station_name "$bad_name"; then fail 'self-test accepted a control-bearing station name'; fi
    done
    validate_bus_root /run/mde-bus
    for bad_root in relative /run/mde/../secret '/run/mde bus' /run/mde/./bus; do
        if validate_bus_root "$bad_root"; then fail 'self-test accepted a malformed Bus root'; fi
    done
    rm -rf -- "$test_dir"
    trap - EXIT
    printf 'verify-music-live-radio: self-test passed (no SSH or playback attempted)\n'
}

usage() {
    cat >&2 <<'EOF'
usage: verify-music-live-radio.sh [--station NAME] [--play-probe] [--self-test]

Default: bounded, read-only checks of mde-musicd.service, installed package
identity, provider ping, action/music/list-radio, the named station's stream
URL shape, and its retained typed ContentRef admission. The station defaults
to "C-SPAN Radio".

--play-probe explicitly permits a 2..15 second typed play probe (8 seconds by
default). The helper refuses to disturb existing playback and always submits
its paired typed stop before exit. Engine-active state is reported separately
from audible/speaker/rendered proof and never promoted to such proof.

Environment: MUSIC_LIVE_HOST, MUSIC_LIVE_USER, MUSIC_LIVE_SSH_KEY,
MUSIC_LIVE_BUS_ROOT, MUSIC_LIVE_RADIO_STATION,
MUSIC_LIVE_RADIO_SSH_TIMEOUT_SECONDS (1..120),
MUSIC_LIVE_RADIO_COMMAND_TIMEOUT_SECONDS (1..30), and
MUSIC_LIVE_RADIO_PROBE_SECONDS (2..15).
EOF
}

local_main() {
    local script_dir repo_root
    script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
    repo_root=$(cd -- "$script_dir/.." && pwd -P)
    local host=${MUSIC_LIVE_HOST:-172.20.0.15}
    local user_=${MUSIC_LIVE_USER:-mm}
    local ssh_key=${MUSIC_LIVE_SSH_KEY:-${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}}
    local bus_root=${MUSIC_LIVE_BUS_ROOT:-/run/mde-bus}
    local station=${MUSIC_LIVE_RADIO_STATION:-$DEFAULT_STATION}
    local ssh_timeout=${MUSIC_LIVE_RADIO_SSH_TIMEOUT_SECONDS:-45}
    local command_timeout=${MUSIC_LIVE_RADIO_COMMAND_TIMEOUT_SECONDS:-8}
    local probe_seconds=${MUSIC_LIVE_RADIO_PROBE_SECONDS:-8}
    local play_probe=0

    if [[ ${1:-} == --self-test ]]; then
        [[ $# == 1 ]] || fail '--self-test takes no additional arguments'
        self_test
        return 0
    fi
    if [[ ${1:-} == --help || ${1:-} == -h ]]; then usage; return 0; fi
    while (($#)); do
        case $1 in
            --station)
                [[ $# -ge 2 ]] || fail '--station requires NAME'
                station=$2
                shift 2
                ;;
            --play-probe)
                play_probe=1
                shift
                ;;
            *) usage; fail "unknown argument: $1" ;;
        esac
    done
    [[ -n "$host" && -n "$user_" && -n "$ssh_key" && -n "$bus_root" ]] ||
        fail 'host, user, SSH key, and Bus root must be non-empty'
    [[ "$host" =~ ^[A-Za-z0-9.:-]+$ ]] || fail 'host contains unsupported characters'
    [[ "$user_" =~ ^[A-Za-z0-9._-]+$ ]] || fail 'user contains unsupported characters'
    validate_bus_root "$bus_root" || fail 'Bus root must be a bounded absolute path without traversal components'
    validate_station_name "$station" || fail 'station name is empty, oversized, or contains control characters'
    bounded_integer "$ssh_timeout" 1 120 || fail 'SSH timeout must be 1..120 seconds'
    bounded_integer "$command_timeout" 1 30 || fail 'command timeout must be 1..30 seconds'
    bounded_integer "$probe_seconds" 2 15 || fail 'probe duration must be 2..15 seconds'
    command -v ssh >/dev/null 2>&1 || fail 'ssh is required'
    command -v timeout >/dev/null 2>&1 || fail 'timeout is required'
    command -v base64 >/dev/null 2>&1 || fail 'base64 is required'
    [[ -r "$ssh_key" ]] || fail 'configured SSH key is unavailable'

    local release_version rpm_release station_b64
    release_version=$(declared_release_version "$repo_root/Cargo.toml") ||
        fail 'could not read [workspace.package].version from root Cargo.toml'
    rpm_release=$(declared_rpm_release "$repo_root/crates/mesh/mackesd/Cargo.toml") ||
        fail 'could not read RPM release from mackesd Cargo.toml'
    validate_release_version "$release_version" || fail 'declared platform version is malformed'
    validate_rpm_release "$rpm_release" || fail 'declared RPM release is malformed'
    station_b64=$(printf '%s' "$station" | base64 --wrap=0)

    printf '== Music live radio verification (%s@%s) ==\n' "$user_" "$host"
    printf '[INFO] station: %s; playback probe: %s\n' "$station" "$([[ $play_probe == 1 ]] && printf enabled || printf disabled)"
    local remote_rc=0
    timeout --signal=TERM --kill-after=3s "${ssh_timeout}s" \
        ssh -i "$ssh_key" -o BatchMode=yes -o ConnectTimeout=10 \
        -o ServerAliveInterval=5 -o ServerAliveCountMax=1 \
        -o StrictHostKeyChecking=accept-new "$user_@$host" bash -s -- \
        --remote-run "$station_b64" "$bus_root" "$command_timeout" \
        "$play_probe" "$probe_seconds" "$release_version" "$rpm_release" \
        <"${BASH_SOURCE[0]}" || remote_rc=$?
    if (( remote_rc == 0 )); then
        printf 'verify-music-live-radio: remote verification complete\n'
    else
        printf 'verify-music-live-radio: FAIL (bounded SSH rc=%s)\n' "$remote_rc" >&2
    fi
    return "$remote_rc"
}

if [[ ${1:-} == --remote-run ]]; then
    shift
    remote_run "$@"
else
    local_main "$@"
fi
