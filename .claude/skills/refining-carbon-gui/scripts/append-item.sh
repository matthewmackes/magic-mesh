#!/usr/bin/env bash
# append-item.sh — the admission chokepoint for the self-replicating worklist.
# Validate + dedup + value-threshold + global-budget candidate items, then append
# ONLY the admitted ones to docs/design/GUI-REFINE-WORKLIST.md. The single place
# new items enter the queue — converts "generate forever" into bounded progress.
#
# Usage: append-item.sh <changes.json> [worklist.md]
# Env:   RCG_MIN_VALUE (30) · RCG_MAX_ITEMS (40) · RCG_MAX_NEW (8)
# Stdout: {admitted:[...], rejected:[[change,reason],...], open_items_after:N}
set -uo pipefail
REPO="$(cd "$(dirname "$0")/../../../.." && pwd)"
CH="${1:-}"; WL="${2:-$REPO/docs/design/GUI-REFINE-WORKLIST.md}"
[ -f "$CH" ] || { echo "!! append-item: changes.json not found: $CH" >&2; exit 2; }
command -v python3 >/dev/null || { echo "!! append-item: python3 required" >&2; exit 2; }
mkdir -p "$(dirname "$WL")"; [ -f "$WL" ] || printf '# GUI-REFINE-WORKLIST\n\nSelf-replicating queue for refining-carbon-gui. Items admitted only via scripts/append-item.sh.\n' > "$WL"

MIN_VALUE="${RCG_MIN_VALUE:-30}"; MAX_ITEMS="${RCG_MAX_ITEMS:-40}"; MAX_NEW="${RCG_MAX_NEW:-8}"

python3 - "$CH" "$WL" "$MIN_VALUE" "$MAX_ITEMS" "$MAX_NEW" <<'PY'
import json,sys,re
ch,wl,minv,maxi,maxnew = sys.argv[1:6]; minv=int(minv); maxi=int(maxi); maxnew=int(maxnew)
try:
    cands = json.load(open(ch)).get("candidates",[])
except Exception as e:
    print(json.dumps({"error":f"bad changes.json: {e}"})); sys.exit(2)
body = open(wl).read()
open_items = len(re.findall(r'^- \[ \]', body, re.M))
req = ["surface","criterion","before","change","accept","value"]

def dup(surface, criterion, admitted):
    surf0 = surface.split(':')[0]
    # existing item lines carry "· <surface> · <criterion> ·"
    for line in body.splitlines():
        if surf0 in line and criterion in line and line.lstrip().startswith('- ['):
            return True
    return any(a["surface"].split(':')[0]==surf0 and a["criterion"]==criterion for a in admitted)

def next_id(surface):
    base = "RCG-" + re.sub(r'[^a-z0-9]+','-', surface.split(':')[0].replace('mde-','').lower()).strip('-')
    nums = [int(m) for m in re.findall(re.escape(base)+r'-(\d+)', body)]
    return f"{base}-{(max(nums)+1) if nums else 1:03d}"

admitted=[]; rejected=[]
for c in cands:
    miss=[k for k in req if k not in c or c.get(k) in (None,"")]
    if miss: rejected.append([c.get("change","?"),"missing "+",".join(miss)]); continue
    try: v=float(c["value"])
    except: rejected.append([c["change"],"value not numeric"]); continue
    if not 0<=v<=100: rejected.append([c["change"],"value out of [0,100]"]); continue
    if v<minv: rejected.append([c["change"],f"below threshold {minv}"]); continue
    if dup(c["surface"],c["criterion"],admitted): rejected.append([c["change"],"duplicate/overlap"]); continue
    if open_items+len(admitted)>=maxi: rejected.append([c["change"],f"MAX_ITEMS {maxi} reached"]); continue
    if len(admitted)>=maxnew: rejected.append([c["change"],f"MAX_NEW_PER_RUN {maxnew} reached"]); continue
    admitted.append(c)

if admitted:
    with open(wl,"a") as f:
        for c in admitted:
            i=next_id(c["surface"])
            f.write(f"\n- [ ] **{i} · {c['surface']} · {c['criterion']}** — {c['change']}.\n"
                    f"      before: {c['before']}; accept: {c['accept']}; value: {int(float(c['value']))}.\n")

print(json.dumps({"admitted":[c["change"] for c in admitted],
                  "rejected":rejected,
                  "open_items_after":open_items+len(admitted)}, indent=1))
PY
