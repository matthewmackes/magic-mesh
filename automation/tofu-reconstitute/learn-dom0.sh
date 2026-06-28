#!/usr/bin/env bash
# learn-dom0.sh — DAR-32: the xen-xapi dom0-LEARN flow. Register a NEW XCP-ng dom0
# into the build-farm's declarative `dom0_registry` by probing its XAPI once, so a
# new machine joins the farm WITHOUT hand-coding `local.dom0` or a provider alias.
# DEVOPS-AUTOMATION-REBUILD §2.8 (build-farm-provision), Lock:
#   "xen-xapi/build-vms learn a new dom0 from a declarative dom0_registry map
#    discovered by a one-shot XAPI probe (learn-dom0.sh); provider aliases RENDERED
#    from a providers.tf.tmpl (HCL can't for_each a provider block)."
#
# WHAT IT DOES (one-shot, idempotent):
#   1. Unseal the XAPI password from the mesh secret store (mcnf-secret.sh get
#      xapi-password) — NEVER logged, NEVER written to the registry.
#   2. Probe the dom0 over `xe` (SSH; mesh key, else sshpass): pool name_label,
#      the management PIF's eth0 network_uuid, the Local SR uuid.
#   3. Allocate the NEXT FREE 40-wide IP lane in 172.20.0.0/16 that does NOT
#      overlap any lane already in the registry (lanes are spaced 40 apart so no
#      dom0's build-VM range collides — matches build-vms.tf's ip_base spacing).
#   4. Append (or, for a KNOWN key, update-in-place) the registry entry. The
#      rendered providers (gen-tfvars.sh, DAR-33) + the build-farm for_each consume
#      this map. Re-running for a known dom0 is idempotent (same key → no second
#      entry, the assigned lane is PRESERVED, fields refresh from the live probe).
#
# The registry is PER-MESH generated state (the dom0 set differs per mesh), so it
# is gitignored — never committed. It carries NO secret (only pool name / UUIDs /
# IP lane / sizing); the XAPI password stays in the store.
#
# Usage:
#   learn-dom0.sh <key> <xapi_host> [--provider-alias <a>] [--registry <path>]
#                 [--ssh-user root] [--xcp-pass <pw>] [--big-vcpus N] [--big-mem-gib N]
#                 [--print] [--dry-run]
#
#   <key>        the dom0 registry key (e.g. xen-home-services | kvm-xcp1 | new-box).
#   <xapi_host>  the dom0 management IP (the XAPI endpoint; provider host=https://<ip>).
#   --provider-alias  the aliased `xenserver` provider name (HCL ident; default: a
#                     safe slug of <key>). One provider block per registry entry.
#   --print      after writing, print the resolved entry as JSON to stdout.
#   --dry-run    probe + compute the lane but DO NOT write the registry (echo only).
#
# Env:
#   MCNF_DOM0_REGISTRY   registry path override (default infra/tofu/xen-xapi/dom0_registry.json)
#   SSH_KEY              mesh key for the dom0 (default ~/.ssh/mackes_mesh_ed25519)
#   TF_VAR_xapi_password / XCP_PASS  pre-supplied password (else mcnf-secret.sh get)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"

REGISTRY="${MCNF_DOM0_REGISTRY:-$REPO/infra/tofu/xen-xapi/dom0_registry.json}"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
SSH_USER="root"
XCP_PASS="${XCP_PASS:-}"
PROVIDER_ALIAS=""
BIG_VCPUS=""
BIG_MEM_GIB=""
PRINT=0
DRY_RUN=0

KEY=""
XAPI_HOST=""

# Positional: <key> <xapi_host> first, then flags (like the other helpers).
[ $# -ge 2 ] || { sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
KEY="$1"; shift
XAPI_HOST="$1"; shift
while [ $# -gt 0 ]; do case "$1" in
  --provider-alias) PROVIDER_ALIAS="$2"; shift 2;;
  --registry)       REGISTRY="$2"; shift 2;;
  --ssh-user)       SSH_USER="$2"; shift 2;;
  --xcp-pass)       XCP_PASS="$2"; shift 2;;
  --big-vcpus)      BIG_VCPUS="$2"; shift 2;;
  --big-mem-gib)    BIG_MEM_GIB="$2"; shift 2;;
  --print)          PRINT=1; shift;;
  --dry-run)        DRY_RUN=1; shift;;
  -h|--help)        sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "learn-dom0: unknown arg: $1" >&2; exit 2;;
esac; done

log()  { echo "==> learn-dom0: $*"; }
die()  { echo "learn-dom0: $*" >&2; exit 1; }

[ -n "$KEY" ] || die "need <key>"
[ -n "$XAPI_HOST" ] || die "need <xapi_host>"
command -v python3 >/dev/null || die "python3 required (registry JSON read/write)"

# Default the provider alias to a HCL-safe slug of the key (lowercase, [a-z0-9_],
# leading letter). One aliased `xenserver` provider is rendered per entry.
if [ -z "$PROVIDER_ALIAS" ]; then
  PROVIDER_ALIAS="$(printf '%s' "$KEY" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9_' '_' | sed 's/^[^a-z]/p&/')"
fi

# ── XAPI password (NEVER logged / never stored in the registry) ──────────────
# Prefer a pre-supplied env (TF_VAR_xapi_password / XCP_PASS / --xcp-pass), else
# unseal from the mesh secret store with the node's own key.
SECRET="$REPO/automation/secrets/mcnf-secret.sh"
if [ -z "$XCP_PASS" ] && [ -n "${TF_VAR_xapi_password:-}" ]; then
  XCP_PASS="$TF_VAR_xapi_password"
fi
if [ -z "$XCP_PASS" ]; then
  if [ -x "$SECRET" ] && XCP_PASS="$("$SECRET" get xapi-password 2>/dev/null)" && [ -n "$XCP_PASS" ]; then
    :  # unsealed from the store
  else
    die "no XAPI password — supply --xcp-pass / TF_VAR_xapi_password, or put one with \`mcnf-secret.sh put xapi-password\` and reseal to this node"
  fi
fi

# ── SSH transport to the dom0: mesh key if it works, else sshpass ────────────
SSHBASE="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=15"
if ssh -i "$SSH_KEY" $SSHBASE -o BatchMode=yes "$SSH_USER@$XAPI_HOST" true 2>/dev/null; then
  RUN() { ssh -i "$SSH_KEY" $SSHBASE "$SSH_USER@$XAPI_HOST" "$@"; }
else
  command -v sshpass >/dev/null || die "mesh key not authorized on $XAPI_HOST and sshpass is missing (needed for password auth)"
  export SSHPASS="$XCP_PASS"
  RUN() { sshpass -e ssh $SSHBASE "$SSH_USER@$XAPI_HOST" "$@"; }
fi
# ssh re-splits the remote command on spaces → %q-quote each xe arg so a value
# with spaces (name-label='Local storage') arrives intact (same as the golden-
# template helper).
xe() { local _c="xe" _a; for _a in "$@"; do _c="$_c $(printf '%q' "$_a")"; done; RUN "$_c"; }

# ── one-shot XAPI probe: pool name, eth0 network_uuid, Local SR ──────────────
log "probing dom0 '$KEY' over XAPI at $XAPI_HOST (alias=$PROVIDER_ALIAS)"
POOL_NAME="$(xe pool-list params=name-label --minimal 2>/dev/null | tr -d '\r' || true)"
[ -n "$POOL_NAME" ] || POOL_NAME="$KEY"  # standalone hosts can have an empty pool name-label

NET_UUID="$(xe pif-list management=true params=network-uuid --minimal 2>/dev/null | tr -d '\r' | tr ',' '\n' | head -1)"
[ -n "$NET_UUID" ] || die "could not resolve the management PIF network_uuid on $XAPI_HOST (xe pif-list management=true)"

# Resolve a local SR by name, then by type (portability — same fallback chain as
# setup-xcp-golden-template.sh): not every host labels it 'Local storage'.
SR_UUID="$(xe sr-list name-label='Local storage' params=uuid --minimal 2>/dev/null | tr -d '\r' | tr ',' '\n' | head -1)"
[ -n "$SR_UUID" ] || SR_UUID="$(xe sr-list type=ext params=uuid --minimal 2>/dev/null | tr -d '\r' | tr ',' '\n' | head -1)"
[ -n "$SR_UUID" ] || SR_UUID="$(xe sr-list type=lvm params=uuid --minimal 2>/dev/null | tr -d '\r' | tr ',' '\n' | head -1)"
[ -n "$SR_UUID" ] || die "no local SR on $XAPI_HOST (xe sr-list — tried 'Local storage', type=ext, type=lvm)"

log "probe OK: pool='$POOL_NAME' network_uuid=$NET_UUID local_sr=$SR_UUID"

# ── allocate the next free 40-wide IP lane (172.20.0.0/16; lanes spaced 40) ───
# Done in python3 alongside the registry merge so the read + the lane scan see the
# SAME state atomically. Existing lanes are read from the registry's ip_base last-
# octets; the chosen base for a KNOWN key is PRESERVED (idempotent). The default
# big sizing mirrors build-vms.tf (3 vCPU / 18 GiB) unless overridden.
ENTRY_JSON="$(
  REGISTRY="$REGISTRY" KEY="$KEY" XAPI_HOST="$XAPI_HOST" \
  PROVIDER_ALIAS="$PROVIDER_ALIAS" POOL_NAME="$POOL_NAME" \
  NET_UUID="$NET_UUID" SR_UUID="$SR_UUID" \
  BIG_VCPUS="$BIG_VCPUS" BIG_MEM_GIB="$BIG_MEM_GIB" DRY_RUN="$DRY_RUN" \
  python3 - <<'PY'
import json, os, sys

reg_path = os.environ["REGISTRY"]
key = os.environ["KEY"]
dry = os.environ.get("DRY_RUN", "0") == "1"

# Lane geometry: 172.20.0.0/16, 40-wide lanes starting at .50, stepping +40.
# (Matches build-vms.tf: ip_bases .50/.90/.130 are spaced 40 apart so no dom0's
# build-VM range overlaps; small VMs step the last octet +10, capped at 4 → 40 wide.)
PREFIX3 = "172.20.0"
LANE_START = 50
LANE_STEP = 40
LANE_MAX = 250  # last octet ceiling

def load(path):
    try:
        with open(path) as f:
            d = json.load(f)
    except FileNotFoundError:
        d = {}
    except Exception as e:
        sys.stderr.write("learn-dom0: registry %s is unreadable: %s\n" % (path, e))
        sys.exit(1)
    return d.get("dom0", {}) if isinstance(d, dict) and "dom0" in d else (d if isinstance(d, dict) else {})

dom0 = load(reg_path)

def last_octet(ip):
    try:
        return int(ip.split(".")[-1])
    except Exception:
        return None

used = set()
for k, v in dom0.items():
    if k == key:
        continue  # our own (idempotent re-run) doesn't block its own lane
    o = last_octet(v.get("ip_base", ""))
    if o is not None:
        used.add(o)

# Preserve a known key's existing lane (idempotent); else allocate the next free.
if key in dom0 and dom0[key].get("ip_base"):
    base_octet = last_octet(dom0[key]["ip_base"])
else:
    base_octet = None
    o = LANE_START
    while o <= LANE_MAX:
        # A lane [o, o+LANE_STEP) is free if no used base falls within it AND o
        # itself is not used (bases are lane starts, so a simple non-overlap check).
        if all(not (o <= u < o + LANE_STEP) for u in used):
            base_octet = o
            break
        o += LANE_STEP
    if base_octet is None:
        sys.stderr.write("learn-dom0: no free 40-wide IP lane left in %s.0/16\n" % PREFIX3)
        sys.exit(1)

ip_base = "%s.%d" % (PREFIX3, base_octet)

def num(envname, default):
    v = os.environ.get(envname, "")
    return int(v) if v.strip() else default

# Preserve existing sizing for a known key unless explicitly overridden.
prev = dom0.get(key, {})
big_vcpus = num("BIG_VCPUS", prev.get("big_vcpus", 3))
big_mem_gib = num("BIG_MEM_GIB", prev.get("big_mem_gib", 18))

entry = {
    "provider_alias": os.environ["PROVIDER_ALIAS"],
    "xapi_host": os.environ["XAPI_HOST"],
    "pool_name": os.environ["POOL_NAME"],
    "network_uuid": os.environ["NET_UUID"],
    "local_sr_uuid": os.environ["SR_UUID"],
    "ip_base": ip_base,
    "big_name": "mcnf-build-big-%s" % key,
    "small_name": "mcnf-build-%s" % key,
    "big_vcpus": big_vcpus,
    "big_mem_gib": big_mem_gib,
}

dom0[key] = entry

if not dry:
    out = {"dom0": dom0}
    os.makedirs(os.path.dirname(os.path.abspath(reg_path)), exist_ok=True)
    tmp = reg_path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(out, f, indent=2, sort_keys=True)
        f.write("\n")
    os.replace(tmp, reg_path)

# Emit the resolved entry (for --print + the caller's log).
print(json.dumps({key: entry}, indent=2, sort_keys=True))
PY
)" || die "registry merge failed"

if [ "$DRY_RUN" -eq 1 ]; then
  log "(dry-run) computed entry for '$KEY' — registry NOT written:"
  echo "$ENTRY_JSON"
  exit 0
fi

log "registry updated: $REGISTRY (key '$KEY')"
if [ "$PRINT" -eq 1 ]; then
  echo "$ENTRY_JSON"
fi
echo "next: gen-tfvars.sh re-renders providers.tf (alias '$PROVIDER_ALIAS') + the local.dom0 map; \`tofu plan\` resolves the new dom0's data sources."
