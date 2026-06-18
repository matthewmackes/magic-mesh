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
s_lizardfs="$(mountpoint -q "$WG" 2>/dev/null && echo true || echo false)"
s_bus="$([ -f /run/mde-bus/index.sqlite ] && echo true || echo false)"
s_dns="$s_mackesd"                                   # mesh DNS is a mackesd worker
s_voice="$(running mde-voice-hud)"
s_music="$(running mde-musicd)"
s_kdc="$([ "$ROLE" = workstation ] && [ "$s_mackesd" = true ] && echo true || echo false)"
s_workbench="$(command -v mde-workbench >/dev/null 2>&1 && echo true || echo false)"

if [ -n "$SELF" ] && [ -d "$WG" ] && mountpoint -q "$WG" 2>/dev/null; then
    mkdir -p "$WG/$SELF" 2>/dev/null || true
    cat > "$WG/$SELF/shell-status.json" 2>/dev/null <<EOF
{"version":"$VER","role":"$ROLE","services":{"mackesd":$s_mackesd,"nebula":$s_nebula,"lizardfs":$s_lizardfs,"bus":$s_bus,"dns":$s_dns,"voice":$s_voice,"music":$s_music,"kdc":$s_kdc,"workbench":$s_workbench},"updated_ms":$(( $(date +%s%3N) ))}
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
# Nebula lighthouse public endpoints (external gateways) from static_host_map.
NET_GWEPS="$(grep -hoE '([0-9]{1,3}\.){3}[0-9]{1,3}:[0-9]+' /etc/nebula/config.yml /etc/nebula/config.yaml 2>/dev/null | sort -u | head -8 | paste -sd, -)"
# Nebula tunnel cipher strength (NEB-CRYPTO-LABEL). The snapshot runs as root so
# it can read the root-only config; the bell applet reads the friendly label here
# (world-readable /run/mde/mesh-status.json) instead of the unreadable config.
# Only reported when nebula is actually up; unset/`aes` = AES-256-GCM default.
NET_CIPHER=""
if systemctl is-active --quiet nebula 2>/dev/null; then
    NET_CIPHER_TOKEN="$(grep -hoE '^[[:space:]]*cipher:[[:space:]]*[A-Za-z0-9]+' /etc/nebula/config.yml /etc/nebula/config.yaml 2>/dev/null | awk -F: '{gsub(/[[:space:]]/,"",$2); print $2}' | head -1)"
    case "$NET_CIPHER_TOKEN" in
        chachapoly|ChaChaPoly|chacha20*) NET_CIPHER="ChaCha20-Poly1305" ;;
        *)                                NET_CIPHER="AES-256-GCM" ;;
    esac
fi

# ── 2. aggregate the directory + every node's shell-status → snapshot ────────
WG="$WG" SELF="$SELF" SELF_VER="$VER" \
NET_IF="$NET_IF" NET_IP="$NET_IP" NET_CIDR="$NET_CIDR" NET_ROUTES="$NET_ROUTES" \
NET_DEFGW="$NET_DEFGW" NET_GWEPS="$NET_GWEPS" NET_CIPHER="$NET_CIPHER" \
python3 - "$OUT" <<'PY' || true
import json, os, sys, glob, time
wg=os.environ.get("WG","/mnt/mesh-storage"); self_host=os.environ.get("SELF","")
out=sys.argv[1]
def presence(h):
    return {"healthy":"online","degraded":"idle"}.get(h or "","offline")
nodes=[]; versions=set()
peers=sorted(glob.glob(os.path.join(wg,"peers","*.json")))
for pf in peers:
    try: d=json.load(open(pf))
    except Exception: continue
    host=d.get("hostname") or os.path.splitext(os.path.basename(pf))[0]
    node={"hostname":host,"overlay_ip":d.get("overlay_ip") or "",
          "presence":presence(d.get("health")),"last_seen_ms":d.get("last_seen_ms") or 0,
          "version":None,"services":{}}
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
network={"overlay_if":os.environ.get("NET_IF","") or "",
         "overlay_ip":os.environ.get("NET_IP","") or "",
         "overlay_cidr":os.environ.get("NET_CIDR","") or "",
         "routes":_split("NET_ROUTES"),
         "default_gw":os.environ.get("NET_DEFGW","") or "",
         "gateway_endpoints":_split("NET_GWEPS"),
         "cipher":os.environ.get("NET_CIPHER","") or ""}
snap={"generated_ms":int(time.time()*1000),"self":self_host,"latest_version":latest,
      "online":sum(1 for n in nodes if n["presence"]=="online"),"total":len(nodes),
      "nodes":nodes,"network":network}
tmp=out+".tmp"
json.dump(snap,open(tmp,"w")); os.replace(tmp,out)
try: os.chmod(out,0o644)
except Exception: pass
PY
