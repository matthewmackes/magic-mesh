#!/bin/bash
# mesh-status-snapshot.sh — MESHSHELL SHELL-1: the data plane for the bash
# prompt + welcome greeting + mesh-help.
#
# Two jobs, run together by mesh-status.timer (~30s) on EVERY node:
#   1. Publish THIS node's services + version to the replicated workgroup dir
#      (`<wg>/<host>/shell-status.json`) so every node can see it.
#   2. Aggregate the replicated peer directory + every node's shell-status into
#      a single fast-to-read snapshot at /run/mde/mesh-status.json that the
#      prompt (cached read) and greeting (snapshot + bounded live refresh) use.
#
# Pure shell + python3 (already a platform dep). Degrades gracefully when the
# workgroup mount is absent (writes a self-only snapshot).
set -u

WG="${MDE_WORKGROUP_ROOT:-/mnt/mesh-storage}"
SELF="$(cat /proc/sys/kernel/hostname 2>/dev/null | tr -d '[:space:]')"
OUT=/run/mde/mesh-status.json
mkdir -p /run/mde 2>/dev/null || true

active() { systemctl is-active --quiet "$1" 2>/dev/null && echo true || echo false; }
running() { pgrep -x "$1" >/dev/null 2>&1 && echo true || echo false; }
yesno()  { [ "$1" = true ] && echo true || echo false; }

# ── 1. publish this node's services + version ───────────────────────────────
VER="$(rpm -q --qf '%{VERSION}' magic-mesh 2>/dev/null)"; [ -z "$VER" ] && VER="unknown"
ROLE="$(sed -n 's/^[[:space:]]*role[[:space:]]*=[[:space:]]*"\([a-z]*\)".*/\1/p' /var/lib/mde/role.toml 2>/dev/null)"
[ -z "$ROLE" ] && ROLE="unknown"

s_mackesd="$(active mackesd)"
s_nebula="$(active nebula)"
s_sync="$(active syncthing)"
s_bus="$([ -f /run/mde-bus/index.sqlite ] && echo true || echo false)"
s_dns="$s_mackesd"                                   # mesh DNS is a mackesd worker
s_voice="$(running mde-voice-hud)"
s_music="$(running mde-musicd)"
s_kdc="$([ "$ROLE" = workstation ] && [ "$s_mackesd" = true ] && echo true || echo false)"
# E12-14c — the Workbench is a plane inside the egui shell; the snapshot field
# name stays "workbench" (consumers key on it) but the probe is the live binary.
s_workbench="$(command -v mde-shell-egui >/dev/null 2>&1 && echo true || echo false)"

if [ -n "$SELF" ] && [ -d "$WG" ]; then
    mkdir -p "$WG/$SELF" 2>/dev/null || true
    STATUS_PATH="$WG/$SELF/shell-status.json"
    STATUS_JSON="$(cat <<EOF
{"version":"$VER","role":"$ROLE","services":{"mackesd":$s_mackesd,"nebula":$s_nebula,"sync":$s_sync,"bus":$s_bus,"dns":$s_dns,"voice":$s_voice,"music":$s_music,"kdc":$s_kdc,"workbench":$s_workbench},"updated_ms":$(( $(date +%s%3N) ))}
EOF
    )"
    # updated_ms is a producer heartbeat, not a peer-state change. Comparing
    # the stable payload before writing prevents Syncthing from propagating a
    # new mtime every timer tick while retaining the timestamp whenever the
    # service/version state actually changes. Missing, unreadable, or malformed
    # files still take the existing best-effort rewrite path.
    status_payload_unchanged() {
        [ -r "$STATUS_PATH" ] || return 1
        current_status="$(sed -E 's/"updated_ms":[0-9]+//' "$STATUS_PATH" 2>/dev/null)" || return 1
        candidate_status="$(printf '%s\n' "$STATUS_JSON" | sed -E 's/"updated_ms":[0-9]+//')" || return 1
        [ "$current_status" = "$candidate_status" ]
    }
    if ! status_payload_unchanged; then
        printf '%s\n' "$STATUS_JSON" > "$STATUS_PATH" 2>/dev/null || true
    fi
fi

# ── 2. aggregate the directory + every node's shell-status → snapshot ────────
# ── 1b. network overview data (SHELL-NET) — this node's overlay + routes +
#        external gateways, for the welcome banner's Network Overview. All
#        best-effort; empty fields render as "—". ────────────────────────────
NET_IF="$(ip -o -4 addr show 2>/dev/null | awk '$2 ~ /nebula|mde-neb/{print $2; exit}')"
NET_IP=""; NET_CIDR=""; NET_ROUTES=""
if [ -n "$NET_IF" ]; then
    NET_IP="$(ip -o -4 addr show dev "$NET_IF" 2>/dev/null | awk '{split($4,a,"/"); print a[1]; exit}')"
    # The connected (kernel) route on the overlay if IS the overlay subnet.
    NET_CIDR="$(ip route show dev "$NET_IF" proto kernel 2>/dev/null | awk '{print $1; exit}')"
    # Every subnet routable through the overlay (overlay subnet + unsafe_routes).
    NET_ROUTES="$(ip route show dev "$NET_IF" 2>/dev/null | awk '$1 ~ /\//{print $1}' | sort -u | head -12 | paste -sd, -)"
fi
NET_DEFGW="$(ip route show default 2>/dev/null | awk '{print $3; exit}')"
# This is deliberately the provider's terse device state only.  Do not ask
# NetworkManager for CONNECTION/SSID/profile fields and do not put modem
# bearer/APN/SIM data on the world-readable snapshot boundary.
NM_PROVIDER_STATUS="$(nmcli -t -f DEVICE,TYPE,STATE device status 2>/dev/null || true)"
MM_PROVIDER_STATUS="$(mmcli --modem=0 --output-keyvalue 2>/dev/null || true)"
PP_ACTIVE="$(powerprofilesctl get 2>/dev/null || true)"
PP_LIST="$(powerprofilesctl list 2>/dev/null || true)"
# BLUETOOTH-BOUNDARY — BlueZ contributes only bounded adapter/device counts and
# the aggregate powered state. Never serialize names, MAC addresses, trust,
# pairing keys, service UUIDs, or agent material into world-readable state.
BT_ADAPTERS=""
BT_POWERED=""
BT_DEVICES=""
if command -v bluetoothctl >/dev/null 2>&1; then
    BT_LIST="$(timeout 2s bluetoothctl list 2>/dev/null || true)"
    BT_ADAPTERS="$(printf '%s\n' "$BT_LIST" | grep -c '^Controller ' || true)"
    BT_SHOW="$(timeout 2s bluetoothctl show 2>/dev/null || true)"
    if printf '%s' "$BT_SHOW" | grep -q 'Powered: yes'; then
        BT_POWERED=true
    elif [ -n "$BT_SHOW" ]; then
        BT_POWERED=false
    fi
    BT_DEVICES="$(timeout 2s bluetoothctl devices 2>/dev/null | grep -c '^Device ' || true)"
fi
# AUDIO-RELEASE-GATE — publish only typed provider availability/counts.  Never
# cross raw pactl/pw-cli/wpctl output, device names, profiles, or usernames over
# the world-readable snapshot boundary.
AUDIO_PULSE=false
if command -v pactl >/dev/null 2>&1 && pactl info 2>/dev/null | grep -Eiq 'server name:.*(pulseaudio|pipewire)'; then
    AUDIO_PULSE=true
fi
AUDIO_PIPEWIRE=false
if command -v pw-cli >/dev/null 2>&1 && [ -n "$(pw-cli ls Node 2>/dev/null | head -c 65536)" ]; then
    AUDIO_PIPEWIRE=true
fi
AUDIO_WIREPLUMBER=false
if command -v wpctl >/dev/null 2>&1 && [ -n "$(wpctl status 2>/dev/null | head -c 65536)" ]; then
    AUDIO_WIREPLUMBER=true
fi
AUDIO_ALSA=0
if command -v aplay >/dev/null 2>&1; then
    AUDIO_ALSA="$(aplay -l 2>/dev/null | awk '/^card [0-9]+:/{count++} END{print count+0}')"
fi
AUDIO_PLAYBACK=false
[ "$AUDIO_ALSA" -gt 0 ] 2>/dev/null && AUDIO_PLAYBACK=true
AUDIO_CAPTURE=false
if command -v arecord >/dev/null 2>&1 && arecord -l 2>/dev/null | grep -q '^card [0-9]\+:'; then
    AUDIO_CAPTURE=true
fi
AUDIO_RECOVERY=""
if [ "$AUDIO_PULSE" = false ] && [ "$AUDIO_PIPEWIRE" = false ] && [ "$AUDIO_WIREPLUMBER" = false ]; then
    AUDIO_RECOVERY="Audio providers unavailable; refresh after PipeWire and WirePlumber start."
fi
# PRIVACY-BOUNDARY — publish only a mute bit for the default microphone and a
# bounded camera-device count. Device presence is not a privacy/permission
# claim, and no camera names, paths, application identities, or credentials
# cross the world-readable snapshot boundary. Camera privacy stays absent until
# a real privacy provider exists.
MICROPHONE_MUTED=""
if command -v wpctl >/dev/null 2>&1; then
    MIC_STATE="$(wpctl get-volume @DEFAULT_SOURCE@ 2>/dev/null || true)"
    if printf '%s' "$MIC_STATE" | grep -q 'MUTED'; then
        MICROPHONE_MUTED=true
    elif [ -n "$MIC_STATE" ]; then
        MICROPHONE_MUTED=false
    fi
fi
CAMERA_DEVICES="$(find /dev -maxdepth 1 -type c -name 'video[0-9]*' 2>/dev/null | head -64 | wc -l | tr -d ' ')"
# TELEMETRY-BOUNDARY — publish only bounded aggregate resource facts. No
# process names, command lines, mount paths, usernames, or device identifiers
# cross the world-readable snapshot boundary.
CPU_CORES="$(grep -c '^processor[[:space:]]*:' /proc/cpuinfo 2>/dev/null || true)"
LOAD_1M="$(awk '{print $1; exit}' /proc/loadavg 2>/dev/null || true)"
MEM_TOTAL_KIB="$(awk '/^MemTotal:[[:space:]]+[0-9]+/{print $2; exit}' /proc/meminfo 2>/dev/null || true)"
MEM_AVAILABLE_KIB="$(awk '/^MemAvailable:[[:space:]]+[0-9]+/{print $2; exit}' /proc/meminfo 2>/dev/null || true)"
ROOT_FS="$(df -P -k / 2>/dev/null | awk 'NR==2 {print $2, $3, $4, $5; exit}' || true)"
ROOT_TOTAL_KIB="$(printf '%s\n' "$ROOT_FS" | awk '{print $1}')"
ROOT_USED_KIB="$(printf '%s\n' "$ROOT_FS" | awk '{print $2}')"
ROOT_AVAILABLE_KIB="$(printf '%s\n' "$ROOT_FS" | awk '{print $3}')"
ROOT_USED_PERCENT="$(printf '%s\n' "$ROOT_FS" | awk '{gsub(/%/,"",$4); print $4}')"
# POWER-SOURCE-BOUNDARY — aggregate kernel power-supply facts only. Do not
# publish BAT*/AC* names, serials, model strings, or raw sysfs paths.
POWER_BATTERY_COUNT=0
POWER_BATTERY_SUM=0
POWER_BATTERY_SAMPLES=0
POWER_BATTERY_STATUS=""
for battery in /sys/class/power_supply/BAT*; do
    [ -d "$battery" ] || continue
    POWER_BATTERY_COUNT=$((POWER_BATTERY_COUNT + 1))
    if capacity="$(cat "$battery/capacity" 2>/dev/null)" \
        && [[ "$capacity" =~ ^[0-9]+$ ]] && [ "$capacity" -le 100 ]; then
        POWER_BATTERY_SUM=$((POWER_BATTERY_SUM + capacity))
        POWER_BATTERY_SAMPLES=$((POWER_BATTERY_SAMPLES + 1))
    fi
    if [ -z "$POWER_BATTERY_STATUS" ]; then
        POWER_BATTERY_STATUS="$(cat "$battery/status" 2>/dev/null | tr -cd '[:alpha:]' | head -c 32)"
    fi
done
POWER_BATTERY_PERCENT=""
if [ "$POWER_BATTERY_SAMPLES" -gt 0 ]; then
    POWER_BATTERY_PERCENT=$((POWER_BATTERY_SUM / POWER_BATTERY_SAMPLES))
fi
POWER_AC_ONLINE=""
for ac in /sys/class/power_supply/AC* /sys/class/power_supply/ADP*; do
    [ -d "$ac" ] || continue
    if online="$(cat "$ac/online" 2>/dev/null)" \
        && [[ "$online" = 0 || "$online" = 1 ]]; then
        [ "$online" = 1 ] && POWER_AC_ONLINE=true
        [ -z "$POWER_AC_ONLINE" ] && POWER_AC_ONLINE=false
        break
    fi
done
# DISPLAY/INPUT-OBSERVATION — aggregate DRM connector, mode, backlight, and
# evdev counts. Connector names, input names, sysfs paths, and serials stay
# local to the node and never enter the world-readable snapshot.
DISPLAY_CONNECTORS=0
DISPLAY_CONNECTED=0
DISPLAY_MODES=0
for status_file in /sys/class/drm/*/status; do
    [ -f "$status_file" ] || continue
    DISPLAY_CONNECTORS=$((DISPLAY_CONNECTORS + 1))
    [ "$(cat "$status_file" 2>/dev/null)" = connected ] \
        && DISPLAY_CONNECTED=$((DISPLAY_CONNECTED + 1))
    mode_file="${status_file%/status}/modes"
    if [ -f "$mode_file" ]; then
        mode_count="$(wc -l < "$mode_file" 2>/dev/null || echo 0)"
        [[ "$mode_count" =~ ^[0-9]+$ ]] \
            && DISPLAY_MODES=$((DISPLAY_MODES + mode_count))
    fi
done
BACKLIGHT_COUNT=0
BACKLIGHT_SUM=0
BACKLIGHT_SAMPLES=0
for backlight in /sys/class/backlight/*; do
    [ -d "$backlight" ] || continue
    BACKLIGHT_COUNT=$((BACKLIGHT_COUNT + 1))
    if current="$(cat "$backlight/brightness" 2>/dev/null)" \
        && maximum="$(cat "$backlight/max_brightness" 2>/dev/null)" \
        && [[ "$current" =~ ^[0-9]+$ ]] && [[ "$maximum" =~ ^[0-9]+$ ]] \
        && [ "$maximum" -gt 0 ] && [ "$current" -le "$maximum" ]; then
        BACKLIGHT_SUM=$((BACKLIGHT_SUM + current * 100 / maximum))
        BACKLIGHT_SAMPLES=$((BACKLIGHT_SAMPLES + 1))
    fi
done
BACKLIGHT_PERCENT=""
[ "$BACKLIGHT_SAMPLES" -gt 0 ] \
    && BACKLIGHT_PERCENT=$((BACKLIGHT_SUM / BACKLIGHT_SAMPLES))
INPUT_EVENT_DEVICES="$(find /dev/input -maxdepth 1 -type c -name 'event[0-9]*' 2>/dev/null | head -64 | wc -l | tr -d ' ')"
# HARDWARE-OBSERVATION — aggregate block capacity, removable-media count,
# thermal zones, and fan sensors. Never serialize block/hwmon names or paths.
STORAGE_DEVICES=0
STORAGE_TOTAL_BYTES=0
STORAGE_REMOVABLE=0
for block in /sys/block/*; do
    [ -d "$block" ] || continue
    sectors="$(cat "$block/size" 2>/dev/null)"
    [[ "$sectors" =~ ^[0-9]+$ ]] || continue
    [ "$sectors" -le $((1 << 42)) ] || continue
    STORAGE_DEVICES=$((STORAGE_DEVICES + 1))
    STORAGE_TOTAL_BYTES=$((STORAGE_TOTAL_BYTES + sectors * 512))
    [ "$(cat "$block/removable" 2>/dev/null)" = 1 ] \
        && STORAGE_REMOVABLE=$((STORAGE_REMOVABLE + 1))
done
THERMAL_ZONES=0
THERMAL_MAX_MILLI_C=""
for thermal in /sys/class/thermal/thermal_zone*; do
    [ -d "$thermal" ] || continue
    temperature="$(cat "$thermal/temp" 2>/dev/null)"
    [[ "$temperature" =~ ^-?[0-9]+$ ]] || continue
    [ "$temperature" -ge -100000 ] && [ "$temperature" -le 200000 ] || continue
    THERMAL_ZONES=$((THERMAL_ZONES + 1))
    if [ -z "$THERMAL_MAX_MILLI_C" ] || [ "$temperature" -gt "$THERMAL_MAX_MILLI_C" ]; then
        THERMAL_MAX_MILLI_C="$temperature"
    fi
done
FAN_DEVICES="$(find /sys/class/hwmon -maxdepth 2 -type f -name 'fan*_input' 2>/dev/null | head -32 | wc -l | tr -d ' ')"
# OS-ACCOUNT-BOUNDARY — publish only aggregate local account posture. Never
# serialize usernames, home paths, shells, group membership, or credentials.
USERS_PROVIDER=false
USER_ACCOUNT_COUNT=""
USER_LOGIN_COUNT=""
USER_ADMIN_GROUPS=""
if [ -r /etc/passwd ] && [ -r /etc/group ]; then
    USERS_PROVIDER=true
    USER_COUNTS="$(awk -F: '
        /^[^:]+:[^:]*:[0-9]+:/ {
            accounts++
            if (($3 == 0 || ($3 >= 1000 && $3 <= 60000)) &&
                $7 != "/usr/sbin/nologin" && $7 != "/sbin/nologin" &&
                $7 != "/bin/false" && $7 != "/usr/bin/false") login++
        }
        END { print accounts + 0, login + 0 }
    ' /etc/passwd 2>/dev/null || true)"
    read -r USER_ACCOUNT_COUNT USER_LOGIN_COUNT <<EOF
$USER_COUNTS
EOF
    USER_ADMIN_GROUPS="$(awk -F: '$1 == "sudo" || $1 == "wheel" { count++ } END { print count + 0 }' /etc/group 2>/dev/null || true)"
fi
# LIGHTHOUSE-9 / data accuracy — nebula loads a DIRECTORY config (`-config
# /etc/nebula`), merging the stock-RPM EXAMPLE `config.yml` (192.168.100.1 /
# 100.64.22.11) with mackesd's rendered REAL `config.yaml`. Reading both leaked
# the example placeholders into the cipher / gateway / lighthouse fields. Read
# the real rendered config only (fall back to the example if it's somehow absent).
NEB_CFG="/etc/nebula/config.yaml"; [ -f "$NEB_CFG" ] || NEB_CFG="/etc/nebula/config.yml"
# Nebula lighthouse public endpoints (external gateways) from static_host_map.
NET_GWEPS="$(grep -hoE '([0-9]{1,3}\.){3}[0-9]{1,3}:[0-9]+' "$NEB_CFG" 2>/dev/null | sort -u | head -8 | paste -sd, -)"
# LIGHTHOUSE-9 — the lighthouse OVERLAY IPs = the static_host_map KEYS (the line-
# leading IP, vs the values which are public ip:port). This is the authoritative
# "which nodes are lighthouses" signal (Nebula membership), independent of the
# deployment role.toml — the anchor nodes run as Server tier for storage, so
# `role==lighthouse` under-reports. The GUI matches a peer's overlay_ip against
# this set OR role==lighthouse.
NET_LHIPS="$(awk '/^static_host_map:/{f=1;next} f&&/^[^[:space:]#]/{f=0} f' "$NEB_CFG" 2>/dev/null | sed -nE 's/^[[:space:]]*"?([0-9]{1,3}(\.[0-9]{1,3}){3})"?[[:space:]]*:.*/\1/p' | sort -u | head -16 | paste -sd, -)"
# Nebula tunnel cipher strength (NEB-CRYPTO-LABEL). The snapshot runs as root so
# it can read the root-only config; the bell applet reads the friendly label here
# (world-readable /run/mde/mesh-status.json) instead of the unreadable config.
# Only reported when nebula is actually up; unset/`aes` = AES-256-GCM default.
NET_CIPHER=""
if systemctl is-active --quiet nebula 2>/dev/null; then
    NET_CIPHER_TOKEN="$(grep -hoE '^[[:space:]]*cipher:[[:space:]]*[A-Za-z0-9]+' "$NEB_CFG" 2>/dev/null | awk -F: '{gsub(/[[:space:]]/,"",$2); print $2}' | head -1)"
    case "$NET_CIPHER_TOKEN" in
        chachapoly|ChaChaPoly|chacha20*) NET_CIPHER="ChaCha20-Poly1305" ;;
        *)                                NET_CIPHER="AES-256-GCM" ;;
    esac
fi

# ── SUBSTRATE-9 — peers + leader from etcd when on the coordination plane ────
# The peer directory + leader lease live in etcd post-cutover (the fs
# peers/*.json glob + .mackesd-leader.lock are retired); per-node shell-status
# still rides the Syncthing-replicated share. Best-effort: needs etcdctl + the
# endpoints file; absent ⇒ ETCD_MODE empty ⇒ the python falls back to the fs glob.
ETCD_PEERS=""; ETCD_LEADER=""; ETCD_MODE=""
ENDPOINTS_FILE=/etc/mackesd/etcd-endpoints
if command -v etcdctl >/dev/null 2>&1 && [ -s "$ENDPOINTS_FILE" ]; then
    EPS="$(tr '\n' ',' < "$ENDPOINTS_FILE" | sed 's/,$//')"
    ETCD_PEERS="$(ETCDCTL_API=3 etcdctl --endpoints="$EPS" get --prefix /mesh/peers/ --print-value-only 2>/dev/null)"
    ETCD_LEADER="$(ETCDCTL_API=3 etcdctl --endpoints="$EPS" get /mesh/leader --print-value-only 2>/dev/null)"
    [ -n "$ETCD_PEERS$ETCD_LEADER" ] && ETCD_MODE=1
fi

# ── 2. aggregate the directory + every node's shell-status → snapshot ────────
WG="$WG" SELF="$SELF" SELF_VER="$VER" PLATFORM_VERSION="$VER" \
ETCD_MODE="$ETCD_MODE" ETCD_PEERS="$ETCD_PEERS" ETCD_LEADER="$ETCD_LEADER" \
NET_IF="$NET_IF" NET_IP="$NET_IP" NET_CIDR="$NET_CIDR" NET_ROUTES="$NET_ROUTES" \
NET_DEFGW="$NET_DEFGW" NET_GWEPS="$NET_GWEPS" NET_CIPHER="$NET_CIPHER" NET_LHIPS="$NET_LHIPS" \
NM_PROVIDER_STATUS="$NM_PROVIDER_STATUS" MM_PROVIDER_STATUS="$MM_PROVIDER_STATUS" \
PP_ACTIVE="$PP_ACTIVE" PP_LIST="$PP_LIST" \
AUDIO_PULSE="$AUDIO_PULSE" AUDIO_PIPEWIRE="$AUDIO_PIPEWIRE" \
AUDIO_WIREPLUMBER="$AUDIO_WIREPLUMBER" AUDIO_ALSA="$AUDIO_ALSA" \
AUDIO_PLAYBACK="$AUDIO_PLAYBACK" AUDIO_CAPTURE="$AUDIO_CAPTURE" \
AUDIO_RECOVERY="$AUDIO_RECOVERY" \
CPU_CORES="$CPU_CORES" LOAD_1M="$LOAD_1M" \
MEM_TOTAL_KIB="$MEM_TOTAL_KIB" MEM_AVAILABLE_KIB="$MEM_AVAILABLE_KIB" \
ROOT_TOTAL_KIB="$ROOT_TOTAL_KIB" ROOT_USED_KIB="$ROOT_USED_KIB" \
ROOT_AVAILABLE_KIB="$ROOT_AVAILABLE_KIB" ROOT_USED_PERCENT="$ROOT_USED_PERCENT" \
POWER_BATTERY_COUNT="$POWER_BATTERY_COUNT" POWER_BATTERY_PERCENT="$POWER_BATTERY_PERCENT" \
POWER_BATTERY_STATUS="$POWER_BATTERY_STATUS" POWER_AC_ONLINE="$POWER_AC_ONLINE" \
DISPLAY_CONNECTORS="$DISPLAY_CONNECTORS" DISPLAY_CONNECTED="$DISPLAY_CONNECTED" \
DISPLAY_MODES="$DISPLAY_MODES" BACKLIGHT_COUNT="$BACKLIGHT_COUNT" \
BACKLIGHT_PERCENT="$BACKLIGHT_PERCENT" INPUT_EVENT_DEVICES="$INPUT_EVENT_DEVICES" \
STORAGE_DEVICES="$STORAGE_DEVICES" STORAGE_TOTAL_BYTES="$STORAGE_TOTAL_BYTES" \
STORAGE_REMOVABLE="$STORAGE_REMOVABLE" THERMAL_ZONES="$THERMAL_ZONES" \
THERMAL_MAX_MILLI_C="$THERMAL_MAX_MILLI_C" FAN_DEVICES="$FAN_DEVICES" \
USERS_PROVIDER="$USERS_PROVIDER" USER_ACCOUNT_COUNT="$USER_ACCOUNT_COUNT" \
USER_LOGIN_COUNT="$USER_LOGIN_COUNT" USER_ADMIN_GROUPS="$USER_ADMIN_GROUPS" \
python3 - "$OUT" <<'PY' || true
import json, os, sys, glob, time, stat
wg=os.environ.get("WG","/mnt/mesh-storage"); self_host=os.environ.get("SELF","")
out=sys.argv[1]
def presence(h):
    return {"healthy":"online","degraded":"idle"}.get(h or "","offline")
nodes=[]; versions=set()
# SUBSTRATE-9 — peer records from etcd (one compact PeerRecord JSON per line)
# when on the coordination plane, else the replicated fs peers/*.json glob.
records=[]
if os.environ.get("ETCD_MODE"):
    for line in os.environ.get("ETCD_PEERS","").splitlines():
        line=line.strip()
        if not line: continue
        try: records.append(json.loads(line))
        except Exception: continue
else:
    for pf in sorted(glob.glob(os.path.join(wg,"peers","*.json"))):
        try: records.append(json.load(open(pf)))
        except Exception: continue
for d in records:
    host=d.get("hostname") or ""
    if not host: continue
    node={"hostname":host,"overlay_ip":d.get("overlay_ip") or "",
          "presence":presence(d.get("health")),"last_seen_ms":d.get("last_seen_ms") or 0,
          "version":None,"services":{},"role":d.get("role")}
    # Per-node shell-status still rides the Syncthing-replicated share.
    sf=os.path.join(wg,host,"shell-status.json")
    try:
        s=json.load(open(sf)); node["version"]=s.get("version"); node["services"]=s.get("services",{})
        node["role"]=s.get("role")
    except Exception: pass
    if node["version"]: versions.add(node["version"])
    nodes.append(node)
# Fallback: if the directory is empty (mount down), at least report self.
if not nodes and self_host:
    nodes=[{"hostname":self_host,"overlay_ip":"","presence":"online",
            "last_seen_ms":int(time.time()*1000),"version":os.environ.get("SELF_VER"),"services":{}}]
    if os.environ.get("SELF_VER"): versions.add(os.environ["SELF_VER"])
def vkey(v):
    try: return tuple(int(x) for x in v.split("."))
    except Exception: return (0,)
latest=max(versions,key=vkey) if versions else None
for n in nodes:
    n["update"]= bool(latest and n.get("version") and n["version"]!=latest)
# SHELL-NET — this node's network overview (overlay + routable subnets + gateways).
def _split(v):
    return [x for x in (os.environ.get(v,"") or "").split(",") if x]
def _leader():
    # The mesh leader = the leader-lease holder (node_id\trenewed_at_s\tepoch).
    # SUBSTRATE-9: from the etcd /mesh/leader value when on the coordination
    # plane (etcd auto-expires the key, so any value present = a live leader),
    # else the fs .mackesd-leader.lock with the 60s freshness check.
    try:
        if os.environ.get("ETCD_MODE"):
            line=(os.environ.get("ETCD_LEADER","") or "").strip()
            if not line: return ""
            nid=line.split("\t")[0]
            return nid[5:] if nid.startswith("peer:") else nid
        line=open(os.path.join(wg,".mackesd-leader.lock")).readline().strip()
        parts=line.split("\t")
        if len(parts)>=2 and (time.time()-float(parts[1]))<60:
            nid=parts[0]
            return nid[5:] if nid.startswith("peer:") else nid
    except Exception: pass
    return ""
network={"overlay_if":os.environ.get("NET_IF","") or "",
         "leader":_leader(),
         "overlay_ip":os.environ.get("NET_IP","") or "",
         "overlay_cidr":os.environ.get("NET_CIDR","") or "",
         "routes":_split("NET_ROUTES"),
         "default_gw":os.environ.get("NET_DEFGW","") or "",
         "gateway_endpoints":_split("NET_GWEPS"),
         "lighthouse_ips":_split("NET_LHIPS"),
         "cipher":os.environ.get("NET_CIPHER","") or ""}
# SHELL-NET provider observations — retain only typed link facts.  In
# particular, the raw NetworkManager/ModemManager output is never serialized;
# profile names, SSIDs, APNs, SIM/operator data, and credentials are dropped.
provider_map={"wifi":"wifi", "wlan":"wifi", "ethernet":"ethernet",
              "802-3-ethernet":"ethernet", "gsm":"cellular",
              "cdma":"cellular", "wwan":"cellular"}
state_map={"connected":"connected", "100 (connected)":"connected",
           "connecting":"connecting", "config":"connecting",
           "disconnected":"disconnected", "deactivating":"disconnected",
           "unavailable":"unavailable", "unmanaged":"unavailable"}
interfaces=[]
for line in (os.environ.get("NM_PROVIDER_STATUS","") or "").splitlines()[:8]:
    fields=[]
    for field in line.split(":"):
        if fields and fields[-1].endswith("\\"):
            fields[-1]=fields[-1][:-1]+":"+field
        else:
            fields.append(field)
    if len(fields)<3: continue
    name, kind, raw_state=(x.strip() for x in fields[:3])
    provider=provider_map.get(kind.lower())
    if not provider or not name or len(name)>128: continue
    normalized=raw_state.lower()
    status=state_map.get(normalized)
    if status is None:
        status="connecting" if ("connecting" in normalized or "config" in normalized) else "unknown"
    interfaces.append({"provider":provider,"name":name,"status":status,
                       "up":status=="connected"})
if not any(item["provider"]=="cellular" for item in interfaces):
    modem_state=""; modem_device=""
    for line in (os.environ.get("MM_PROVIDER_STATUS","") or "").splitlines():
        key, sep, value=line.partition("=")
        if not sep: continue
        if key.strip()=="modem.generic.state": modem_state=value.strip().lower()
        elif key.strip()=="modem.generic.device" and value.strip().startswith("/dev/"):
            modem_device=value.strip()[:128]
    if modem_state:
        status={"connected":"connected", "registered":"connecting",
                "connecting":"connecting", "disabled":"disconnected",
                "locked":"disconnected", "failed":"unavailable",
                "unknown":"unavailable"}.get(modem_state,"unknown")
        interfaces.append({"provider":"cellular","name":modem_device,
                           "status":status,"up":status=="connected"})
network["interfaces"]=interfaces[:8]
profiles=[]
for line in (os.environ.get("PP_LIST", "") or "").splitlines()[:16]:
    line=line.strip().lstrip("*").strip()
    if not line or ":" in line or len(line)>32: continue
    if all(ch.isalnum() or ch in "-_" for ch in line):
        profiles.append(line)
active=(os.environ.get("PP_ACTIVE", "") or "").strip()
if active and len(active)>32 or (active and not all(ch.isalnum() or ch in "-_" for ch in active)):
    active=""
power_profile={"active":active, "available":sorted(set(profiles))}
def env_bool(name):
    return os.environ.get(name, "false").lower() == "true"
def env_optional_bool(name):
    value=os.environ.get(name, "").lower()
    if value == "true": return True
    if value == "false": return False
    return None
def env_optional_count(name, maximum):
    value=os.environ.get(name, "")
    try:
        parsed=int(value)
    except ValueError:
        return None
    return max(0, min(maximum, parsed))
def env_optional_bytes_kib(name, maximum):
    value=env_optional_count(name, maximum // 1024)
    return None if value is None else value * 1024
def env_optional_float(name, maximum):
    try:
        parsed=float(os.environ.get(name, ""))
    except ValueError:
        return None
    return parsed if 0 <= parsed <= maximum else None
def bounded_pack_text(value, maximum=96):
    if not isinstance(value, str) or not value or len(value) > maximum:
        return None
    if any(ord(ch) < 32 or ord(ch) == 127 for ch in value):
        return None
    return value
def discover_vendor_packs():
    # Fixed, read-only package boundary. Manifests can describe capability
    # posture, but never create routes, commands, or executable controls.
    packs=[]
    for path in sorted(glob.glob("/usr/share/mde/vendor-packs/*.json"))[:8]:
        fd=None
        try:
            flags=os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
            fd=os.open(path, flags)
            file_stat=os.fstat(fd)
            if not stat.S_ISREG(file_stat.st_mode):
                continue
            if file_stat.st_size > 65536:
                continue
            with os.fdopen(fd) as stream:
                fd=None
                raw=stream.read(65537)
            if len(raw) > 65536:
                continue
            manifest=json.loads(raw)
            if not isinstance(manifest, dict):
                continue
            name=bounded_pack_text(manifest.get("name"))
            if not name: continue
            version=bounded_pack_text(manifest.get("version")) or "unknown"
            raw_status=manifest.get("status")
            status=("installed" if raw_status in ("installed", "current")
                    else "outdated" if raw_status == "outdated"
                    else "unavailable")
            capabilities=[]
            raw_capabilities=manifest.get("capabilities", [])
            if not isinstance(raw_capabilities, list):
                raw_capabilities=[]
            for capability in raw_capabilities:
                value=bounded_pack_text(capability)
                if value and value not in capabilities:
                    capabilities.append(value)
                if len(capabilities) == 8: break
            packs.append({"name":name,"version":version,"status":status,
                          "capabilities":capabilities})
        except (OSError, ValueError, TypeError):
            continue
        finally:
            if fd is not None:
                try: os.close(fd)
                except OSError: pass
    return packs
def discover_camera_devices():
    # Fixed, read-only device boundary. Publish only a bounded count; device
    # names, paths, and capture permissions never cross the snapshot boundary.
    count=0
    for path in sorted(glob.glob("/dev/video[0-9]*"))[:64]:
        fd=None
        try:
            fd=os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0))
            if stat.S_ISCHR(os.fstat(fd).st_mode):
                count += 1
        except OSError:
            continue
        finally:
            if fd is not None:
                try: os.close(fd)
                except OSError: pass
    return count
try:
    alsa_devices=max(0, min(256, int(os.environ.get("AUDIO_ALSA", "0"))))
except ValueError:
    alsa_devices=0
audio={"pulse_available":env_bool("AUDIO_PULSE"),
       "pipewire_graph":env_bool("AUDIO_PIPEWIRE"),
       "wireplumber_policy":env_bool("AUDIO_WIREPLUMBER"),
       "alsa_devices":alsa_devices,
       "playback":env_bool("AUDIO_PLAYBACK"),
       "capture":env_bool("AUDIO_CAPTURE"),
       "recovery":os.environ.get("AUDIO_RECOVERY", "")[:160]}
try:
    camera_devices=max(0, min(64, int(os.environ["CAMERA_DEVICES"])))
except ValueError:
    camera_devices=discover_camera_devices()
except KeyError:
    camera_devices=discover_camera_devices()
privacy={"microphone_muted":env_optional_bool("MICROPHONE_MUTED"),
         "camera_devices":camera_devices,
         "camera_privacy":None}
bluetooth={"adapters":env_optional_count("BT_ADAPTERS", 64),
           "powered":env_optional_bool("BT_POWERED"),
           "devices":env_optional_count("BT_DEVICES", 64)}
load_1m=env_optional_float("LOAD_1M", 256.0)
telemetry={"cpu_cores":env_optional_count("CPU_CORES", 256),
           "load_1m_milli":None if load_1m is None else round(load_1m * 1000),
           "memory_total_bytes":env_optional_bytes_kib("MEM_TOTAL_KIB", 1 << 44),
           "memory_available_bytes":env_optional_bytes_kib("MEM_AVAILABLE_KIB", 1 << 44),
           "root_total_bytes":env_optional_bytes_kib("ROOT_TOTAL_KIB", 1 << 44),
           "root_used_bytes":env_optional_bytes_kib("ROOT_USED_KIB", 1 << 44),
           "root_available_bytes":env_optional_bytes_kib("ROOT_AVAILABLE_KIB", 1 << 44),
           "root_used_percent":env_optional_count("ROOT_USED_PERCENT", 100)}
power_source={"battery_count":env_optional_count("POWER_BATTERY_COUNT", 16),
              "battery_percent":env_optional_count("POWER_BATTERY_PERCENT", 100),
              "battery_status":(os.environ.get("POWER_BATTERY_STATUS", "") or "")[:32],
              "ac_online":env_optional_bool("POWER_AC_ONLINE")}
display={"connectors":env_optional_count("DISPLAY_CONNECTORS", 64),
         "connected":env_optional_count("DISPLAY_CONNECTED", 64),
         "modes":env_optional_count("DISPLAY_MODES", 512),
         "backlights":env_optional_count("BACKLIGHT_COUNT", 16),
         "backlight_percent":env_optional_count("BACKLIGHT_PERCENT", 100)}
input_devices={"event_devices":env_optional_count("INPUT_EVENT_DEVICES", 64)}
hardware={"storage_devices":env_optional_count("STORAGE_DEVICES", 128),
          "storage_total_bytes":env_optional_bytes_kib("STORAGE_TOTAL_BYTES", 1 << 44),
          "storage_removable":env_optional_count("STORAGE_REMOVABLE", 32),
          "thermal_zones":env_optional_count("THERMAL_ZONES", 128),
          "thermal_max_milli_c":env_optional_count("THERMAL_MAX_MILLI_C", 200000),
          "fan_devices":env_optional_count("FAN_DEVICES", 32)}
vendor_packs=discover_vendor_packs()
users={"provider":env_bool("USERS_PROVIDER"),
       "account_count":env_optional_count("USER_ACCOUNT_COUNT", 4096),
       "login_count":env_optional_count("USER_LOGIN_COUNT", 4096),
       "admin_groups":env_optional_count("USER_ADMIN_GROUPS", 16)}
platform_version=os.environ.get("PLATFORM_VERSION") or "unknown"
snap={"generated_ms":int(time.time()*1000),"self":self_host,
      "platform_version":platform_version,"latest_version":latest,
      "online":sum(1 for n in nodes if n["presence"]=="online"),"total":len(nodes),
      "power_profile":power_profile,"audio":audio,"privacy":privacy,
      "bluetooth":bluetooth,
      "telemetry":telemetry,
      "power_source":power_source,
      "display":display,"input":input_devices,
      "hardware":hardware,
      "vendor_packs":vendor_packs,
      "users":users,
      "nodes":nodes,"network":network}
tmp=out+".tmp"
json.dump(snap,open(tmp,"w")); os.replace(tmp,out)
try: os.chmod(out,0o644)
except Exception: pass
PY
