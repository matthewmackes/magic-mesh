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
    cat > "$WG/$SELF/shell-status.json" 2>/dev/null <<EOF
{"version":"$VER","role":"$ROLE","services":{"mackesd":$s_mackesd,"nebula":$s_nebula,"sync":$s_sync,"bus":$s_bus,"dns":$s_dns,"voice":$s_voice,"music":$s_music,"kdc":$s_kdc,"workbench":$s_workbench},"updated_ms":$(( $(date +%s%3N) ))}
EOF
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
python3 - "$OUT" <<'PY' || true
import json, os, sys, glob, time
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
platform_version=os.environ.get("PLATFORM_VERSION") or "unknown"
snap={"generated_ms":int(time.time()*1000),"self":self_host,
      "platform_version":platform_version,"latest_version":latest,
      "online":sum(1 for n in nodes if n["presence"]=="online"),"total":len(nodes),
      "power_profile":power_profile,"audio":audio,
      "nodes":nodes,"network":network}
tmp=out+".tmp"
json.dump(snap,open(tmp,"w")); os.replace(tmp,out)
try: os.chmod(out,0o644)
except Exception: pass
PY
