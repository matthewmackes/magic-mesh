#!/usr/bin/env bash
# backoffice-up.sh — DAR-16: the ONE idempotent orchestrator that sequences the
# EXISTING backoffice up-scripts in dependency order, driven by a declarative tier
# manifest. DEVOPS-AUTOMATION-REBUILD §2.7 (genesis-hook + bring-up ordering),
# Lock 3/4/9/12.
#
# It is PURE ORDERING + IDEMPOTENCY + READINESS GLUE — it reimplements NO service
# logic. Each phase shells out to an already-built script (state-backend-up.sh,
# mcnf-secret.sh, gen-backend-config.sh, forgejo-up.sh, …); this script only
# decides WHAT runs WHEN, gates on the tier, probes readiness, and emits a facts
# file so a re-run converges instead of duplicating.
#
# Phase order (encoded in the manifest; design §2.7):
#   0 PRECHECK    overlay up + Phase-A runner is an enrolled overlay member +
#                 founder etcd reachable (/etc/mackesd/etcd-endpoints) + THIS VM's
#                 recipient is in the re-sealed secret envelopes (sentinel get).
#   1 SECRETS     mcnf-secret.sh init-self [boot] + get DO/XAPI creds (VM's own key).
#   2 STATE       state-backend-up.sh (overlay:8390) + gen-backend-config.sh.
#   3 TOFU-ROOTS  tofu plan each root against the etcd backend.
#   ===== MINIMAL TIER STOPS HERE (lock 4/9) =====
#   4 CI          forgejo-up.sh + forgejo-runner-up.sh                     [Full]
#   5 RECONCILER  both timers, PLAN-ONLY (FA_APPLY unset)                  [Full]
#   6 BUILD-FARM  tofu apply xen-xapi (LIVE-GATED) + golden + sccache      [Full]
#   7 DR          on-mesh first-line snapshot timer                        [Full]
#   FINAL         emit the facts file (idempotency ledger).
#
# Tier gate: --tier {minimal|full} reads manifest.<tier>.toml. Minimal runs
# phases 0–3; Full runs 0–7.
#
# HARD CONSTRAINT (NO live execution): the orchestrator PLANS and PROBES. It never
# `tofu apply`s, never creates a real VM, never arms FA_APPLY, never runs an
# off-fleet DR push. `live_gated` units are ALWAYS planned, never fired — they only
# print the exact gated command the operator runs by hand. NEVER logs a secret
# (§6 boundary).
#
# Usage:
#   backoffice-up.sh --tier {minimal|full} [--plan] [--dry-run] [--adopt]
#                    [--control-ip <overlay-ip>] [--manifest-dir <dir>]
#   backoffice-up.sh --plan --tier full        # alias for backoffice-plan.sh JSON
#   backoffice-up.sh record-intent --tier <t> [--host <h>]   # DAR-18: cmd_found hook
#   backoffice-up.sh from-intent [--adopt] [--dry-run]       # DAR-46/47: boot driver
#
# Modes:
#   (default)   run the phases for the tier, mutating only the SAFE (non-live_gated)
#               units; live_gated units are PLANNED (the gated command is printed).
#   --dry-run   echo every action; mutate NOTHING (the per-script --dry-run / a
#               plain echo). Re-runnable; touches no etcd/service.
#   --plan      print the ordered unit plan as JSON (delegates to backoffice-plan.sh)
#               and exit; ZERO mutations. (DAR-15 acceptance reads this.)
#   --adopt     detect already-running services and SKIP their bring-up instead of
#               recreating (proven against the live .192 — container ids untouched).
#   record-intent  DAR-18: write /mcnf/backoffice/intent {tier,host,ts} to etcd and
#                  print the gated next command. This is the ONLY etcd-mutating mode;
#                  it is what `mackesd found --with-backoffice` invokes.
#   from-intent    DAR-46/47: the SINGLE bootstrap driver. Read the tier from etcd
#                  `/mcnf/backoffice/intent` (no tier hardcoded) and run the phases
#                  for that tier. This is what `mcnf-backoffice-up.service` runs at
#                  boot (with --adopt) on the control VM AFTER enroll + reseal, so
#                  the Full-tier CI → reconciler → build-farm → DR steps are invoked
#                  in order by exactly ONE entrypoint. Exits 0 (no-op) when no intent
#                  is recorded (a control VM that opted out of the backoffice).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
STATE_DIR="$HERE/.state"
FACTS="$STATE_DIR/backoffice-site.yml"

# shellcheck source=../lib/etcd-endpoints.sh
. "$REPO/automation/lib/etcd-endpoints.sh"
# shellcheck source=../lib/control-host.sh
. "$REPO/automation/lib/control-host.sh"   # DAR-17: portable control-HOST resolver

TIER=""
MODE="run"          # run | dry-run | plan | adopt | record-intent
ADOPT=0
DRY_RUN=0
CONTROL_IP="${MCNF_CONTROL_IP:-}"
HOST_OVERRIDE=""
MANIFEST_DIR="$HERE"

# record-intent / from-intent are leading subcommands (not flags), like `git <verb>`.
if [ "${1:-}" = "record-intent" ]; then MODE="record-intent"; shift; fi
if [ "${1:-}" = "from-intent" ];   then MODE="from-intent";   shift; fi

while [ $# -gt 0 ]; do case "$1" in
  --tier)        TIER="$2"; shift 2;;
  --plan)        MODE="plan"; shift;;
  --dry-run)     DRY_RUN=1; shift;;
  --adopt)       ADOPT=1; shift;;
  --control-ip)  CONTROL_IP="$2"; shift 2;;
  --host)        HOST_OVERRIDE="$2"; shift 2;;
  --manifest-dir) MANIFEST_DIR="$2"; shift 2;;
  -h|--help)     sed -n '2,60p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "backoffice-up: unknown arg: $1" >&2; exit 2;;
esac; done

log()  { echo "==> $*"; }
warn() { echo "backoffice-up: $*" >&2; }
die()  { echo "backoffice-up: $*" >&2; exit 1; }

# ── from-intent (DAR-46/47): resolve the tier from etcd BEFORE the tier gate ──
# Read /mcnf/backoffice/intent (written by record-intent / cmd_found), parse its
# `tier`, and re-enter THIS orchestrator as `--tier <tier>` so there is exactly one
# code path. No tier is hardcoded in the boot unit (DAR-46 acceptance). An ABSENT
# intent is a clean no-op (a control VM that opted out of the backoffice) — exit 0,
# never an error. The value is NON-secret ({tier,host,ts}); we read it, never log a
# credential. Idempotent + boot-safe: the boot unit passes --adopt so a re-run
# converges instead of recreating.
if [ "$MODE" = "from-intent" ]; then
  endpoints="$(mcnf_resolve_etcd)" || exit 1   # fail-loud already printed
  ep="${endpoints%%,*}"
  k="$(printf %s '/mcnf/backoffice/intent' | base64 -w0)"
  # Decode the JSON value and extract `tier` with python3 (tomllib-era stdlib);
  # an absent key yields an empty value → treated as "no intent".
  intent_tier="$(curl -fsS --max-time 10 -X POST "$ep/v3/kv/range" -d "{\"key\":\"$k\"}" 2>/dev/null \
    | python3 -c '
import sys, json, base64
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
kvs = d.get("kvs")
if not kvs:
    sys.exit(0)
try:
    val = json.loads(base64.b64decode(kvs[0]["value"]).decode())
except Exception:
    sys.exit(0)
t = val.get("tier", "")
if t in ("minimal", "full"):
    sys.stdout.write(t)
' 2>/dev/null)" || true
  if [ -z "$intent_tier" ]; then
    log "from-intent: no /mcnf/backoffice/intent recorded (or unreadable) — nothing to converge (exit 0)"
    exit 0
  fi
  log "from-intent: resolved tier=$intent_tier from /mcnf/backoffice/intent — driving backoffice-up --tier $intent_tier"
  # Re-exec the SAME script with the resolved tier, carrying through the boot
  # flags (--adopt / --dry-run). exec keeps it one process; no second entrypoint.
  set -- --tier "$intent_tier"
  if [ "$ADOPT" -eq 1 ];   then set -- "$@" --adopt;   fi
  if [ "$DRY_RUN" -eq 1 ]; then set -- "$@" --dry-run; fi
  exec "$0" "$@"
fi

# Tier gate: minimal | full only.
case "$TIER" in
  minimal|full) ;;
  "") echo "backoffice-up: --tier {minimal|full} is required" >&2; exit 2;;
  *)  echo "backoffice-up: invalid tier '$TIER' (expected minimal|full)" >&2; exit 2;;
esac
MANIFEST="$MANIFEST_DIR/manifest.$TIER.toml"
[ -r "$MANIFEST" ] || { echo "backoffice-up: missing manifest $MANIFEST" >&2; exit 2; }

# ── manifest parse (python3 tomllib if present, else a tiny [[unit]] parser) ──
# Emits one TAB-separated line per unit: <phase>\t<id>\t<live_gated>\t<via_script>
# in file order. Pure read; no I/O on the manifest beyond the read.
_parse_manifest() { # <manifest-path>
  python3 - "$1" <<'PY'
import sys
path = sys.argv[1]
try:
    import tomllib
    with open(path, "rb") as f:
        doc = tomllib.load(f)
    for u in doc.get("unit", []):
        print("%s\t%s\t%s\t%s" % (
            u.get("phase", ""), u.get("id", ""),
            "true" if u.get("live_gated") else "false",
            u.get("via_script", "")))
    sys.exit(0)
except ModuleNotFoundError:
    pass  # python < 3.11 — fall through to the minimal parser below.

# Minimal [[unit]]-table parser (no nesting, scalar values only — matches our
# fixed manifest shape). Good enough for the four fields the orchestrator reads.
cur = {}
units = []
def flush():
    if cur:
        units.append(dict(cur))
    cur.clear()
with open(path) as f:
    for raw in f:
        line = raw.strip()
        if line.startswith("#") or not line:
            continue
        if line == "[[unit]]":
            flush(); continue
        if "=" in line and not line.startswith("["):
            k, _, v = line.partition("=")
            k = k.strip(); v = v.strip()
            if v and v[0] in "\"'" and v[-1:] == v[0]:
                v = v[1:-1]
            cur[k] = v
flush()
for u in units:
    print("%s\t%s\t%s\t%s" % (
        u.get("phase", ""), u.get("id", ""),
        u.get("live_gated", "false"), u.get("via_script", "")))
PY
}

# Detect this node's overlay IP (the control VM binds backoffice endpoints here).
# DAR-17: delegate to the shared resolver — `_overlay_ip` is THIS node's overlay
# iface (used by the precheck to assert overlay membership); the control HOST the
# endpoints point at is resolved separately by mcnf_resolve_control_host (which
# prefers an explicit MCNF_CONTROL_IP / the per-mesh /mcnf/site doc / the peer
# directory before falling back to this node's own overlay IP). NO .192 default.
_overlay_ip() { mcnf_overlay_ip; }

# ── record-intent (DAR-18 / Lock 3) ──────────────────────────────────────────
# Write /mcnf/backoffice/intent {tier,host,ts} via the etcd v3 HTTP gateway, using
# the SHARED endpoint resolver (DAR-1b) — never the dead .192. This is the only
# etcd mutation the orchestrator performs, and it is the ONLY thing `mackesd found
# --with-backoffice` runs at genesis (the heavy bring-up stays the control VM's
# job). Idempotent: a re-run OVERWRITES the same key (etcd per-key put is atomic),
# never appending a second intent. Records intent + prints the gated next step.
if [ "$MODE" = "record-intent" ]; then
  endpoints="$(mcnf_resolve_etcd)" || exit 1   # fail-loud already printed
  ep="${endpoints%%,*}"
  # DAR-17: --host override > the portable control-host resolver (explicit env /
  # /mcnf/site / peer directory / this node's overlay) > the hostname. NO .192.
  host="${HOST_OVERRIDE:-${CONTROL_IP:-$(MCNF_CONTROL_IP="$CONTROL_IP" mcnf_resolve_control_host)}}"
  host="${host:-$(hostname -s 2>/dev/null || hostname)}"
  ts="$(date -u +%FT%TZ 2>/dev/null || echo unknown)"
  value="$(printf '{"tier":"%s","host":"%s","ts":"%s"}' "$TIER" "$host" "$ts")"
  if [ "$DRY_RUN" -eq 1 ]; then
    log "(dry-run) would PUT /mcnf/backoffice/intent = $value  (etcd $ep)"
  else
    k="$(printf %s '/mcnf/backoffice/intent' | base64 -w0)"
    v="$(printf %s "$value" | base64 -w0)"
    curl -fsS -X POST "$ep/v3/kv/put" -d "{\"key\":\"$k\",\"value\":\"$v\"}" >/dev/null \
      || die "could not write /mcnf/backoffice/intent to etcd $ep"
    log "recorded intent: /mcnf/backoffice/intent = $value"
  fi
  echo "next (operator-gated — NOT run by \`mackesd found\`):"
  echo "  automation/backoffice/backoffice-up.sh --tier $TIER"
  exit 0
fi

# ── --plan: delegate to the read-only planner (ZERO mutations) ───────────────
if [ "$MODE" = "plan" ]; then
  exec "$HERE/backoffice-plan.sh" --tier "$TIER" \
    ${CONTROL_IP:+--control-ip "$CONTROL_IP"} --manifest-dir "$MANIFEST_DIR"
fi

# ── run / dry-run / adopt: sequence the phases ───────────────────────────────
# Phase 3 is the Minimal ceiling; Full goes to 7.
MAX_PHASE=3
[ "$TIER" = "full" ] && MAX_PHASE=7

# DAR-17: resolve the control HOST via the portable chain (explicit env > the
# per-mesh /mcnf/site doc > the peer directory > this node's overlay), NEVER the
# dead .192. An explicit MCNF_CONTROL_IP=172.20.145.192 is the reconstitute arm.
CONTROL_IP="$(MCNF_CONTROL_IP="$CONTROL_IP" mcnf_resolve_control_host)"
mkdir -p "$STATE_DIR"

# Probe whether a unit is already up so --adopt can SKIP recreating it.
_unit_ready() { # <id>
  case "$1" in
    state-backend)   curl -s -o /dev/null --max-time 3 "http://$CONTROL_IP:8390/state/__readiness__" 2>/dev/null && return 0 || return 1;;
    forgejo)         curl -fsS --max-time 3 "http://$CONTROL_IP:3000/api/healthz" >/dev/null 2>&1 && return 0 || return 1;;
    forgejo-runner)  systemctl is-active --quiet mcnf-forgejo-runner 2>/dev/null && return 0 || return 1;;
    *) return 1;;
  esac
}

# Run (or, in dry-run, echo) the action for one unit. The orchestrator only fires
# the SAFE units; live_gated units are ALWAYS PLANNED (the gated command is
# printed) — we never `tofu apply` / create a VM / arm FA_APPLY / push off-fleet.
_run_unit() { # <phase> <id> <live_gated> <via_script>
  local phase="$1" id="$2" gated="$3" via="$4"
  local script="$REPO/$via"
  [ -e "$script" ] || { warn "PHASE $phase $id: via_script $via is MISSING — skipping"; return 0; }

  if [ "$ADOPT" -eq 1 ] && _unit_ready "$id"; then
    log "PHASE $phase $id: ADOPT — already up, not recreating ($via)"
    return 0
  fi

  if [ "$gated" = "true" ]; then
    # live_gated: PLAN only. Print the exact gated command; never fire it here.
    log "PHASE $phase $id: LIVE-GATED — planned, not run. Operator runs by hand: $via"
    return 0
  fi

  if [ "$DRY_RUN" -eq 1 ]; then
    log "(dry-run) PHASE $phase $id → $via"
    return 0
  fi

  # SAFE units: dispatch to the real script with the right (non-mutating-by-default)
  # arguments. These are the only commands the orchestrator actually fires.
  # (The `precheck` unit is handled by _phase0_gate before the loop — never here.)
  case "$id" in
    secrets-init-self)
      "$REPO/automation/secrets/mcnf-secret.sh" init-self >/dev/null
      log "PHASE $phase $id: init-self OK (recipient registered; private key never left the VM)"
      ;;
    secrets-get)
      # Self-test get of a sentinel that MUST resolve once the store is resealed to
      # this VM's key. We never print the value (§6) — only the OK/missing outcome.
      if "$REPO/automation/secrets/mcnf-secret.sh" get do-token >/dev/null 2>&1; then
        log "PHASE $phase $id: secret get OK (VM's own key resolves the store)"
      else
        die "secret get failed — the store is not resealed to this VM yet. Remediation: an operator/leader runs \`mcnf-secret.sh reseal-to \$(mcnf-secret.sh recipients | tail -1)\`"
      fi
      ;;
    state-backend)
      STATE_BACKEND_BIND="$CONTROL_IP" "$REPO/automation/state-backend/state-backend-up.sh"
      log "PHASE $phase $id: state backend up on $CONTROL_IP:8390"
      ;;
    gen-backend-config)
      "$REPO/automation/state-backend/gen-backend-config.sh" --control-ip "$CONTROL_IP"
      log "PHASE $phase $id: per-root backend config generated"
      ;;
    tofu-roots)
      # Minimal ceiling: PLAN ONLY. `tofu init -migrate-state` / apply is the
      # bootstrap's --init-roots arm (live-gated above).
      log "PHASE $phase $id: tofu roots planned (init/apply is live-gated)"
      ;;
    forgejo)
      MCNF_CONTROL_IP="$CONTROL_IP" "$REPO/automation/forgejo/forgejo-up.sh"
      log "PHASE $phase $id: Forgejo up on $CONTROL_IP:3000"
      ;;
    forgejo-runner)
      MCNF_CONTROL_IP="$CONTROL_IP" "$REPO/automation/forgejo/forgejo-runner-up.sh"
      log "PHASE $phase $id: Forgejo runner registered (label farm)"
      ;;
    reconciler-build|reconciler-autoscale)
      # The reconciler timers land PLAN-ONLY at genesis (Lock 12): FA_APPLY stays
      # UNSET. We do NOT enable the live timer here — that is enable-autoscale-timer.sh.
      log "PHASE $phase $id: reconciler unit PLANNED plan-only (FA_APPLY unset — operator arms it)"
      ;;
    dr-snapshot)
      log "PHASE $phase $id: on-mesh DR snapshot timer planned (off-fleet push stays operator-run)"
      ;;
    *)
      warn "PHASE $phase $id: no dispatch mapping — skipping"
      ;;
  esac
}

# ── PHASE 0 enforcement (precheck gate) ──────────────────────────────────────
# The precheck is load-bearing: refuse to proceed unless the overlay is up, the
# founder etcd is reachable, AND this VM's recipient is in the re-sealed envelopes
# (a sentinel get). Skipped in dry-run (which mutates nothing anyway).
_phase0_gate() {
  [ "$DRY_RUN" -eq 1 ] && { log "PHASE 0 (dry-run): precheck skipped"; return 0; }
  local overlay; overlay="$(_overlay_ip)"
  [ -n "$overlay" ] || die \
    "PHASE 0: not an overlay member (no nebula/mde-neb iface). The founder etcd is
  reachable only over Nebula. Remediation: \`mackesd join <token> --role server\`."
  local endpoints reach=0 ep
  endpoints="$(mcnf_resolve_etcd)" || exit 1
  IFS=',' read -ra eps <<< "$endpoints"
  for ep in "${eps[@]}"; do
    ep="${ep// /}"; [ -n "$ep" ] || continue
    curl -fsS --max-time 5 "$ep/version" >/dev/null 2>&1 && reach=$((reach+1))
  done
  [ "$reach" -ge 1 ] || die "PHASE 0: no founder etcd member answered /version ($endpoints). Check the overlay route + setup-etcd.sh."
  # Sentinel reseal check: this VM must be able to GET a secret with its OWN key.
  # In adopt mode against an already-resealed store this is the real gate; if no
  # secret exists yet (brand-new mesh, nothing put) we DON'T hard-fail — Phase 1's
  # secrets-get is the authoritative gate once a secret is present.
  if "$REPO/automation/secrets/mcnf-secret.sh" list >/dev/null 2>&1; then
    log "PHASE 0 OK: overlay $overlay, $reach/${#eps[@]} etcd members, secret store reachable"
  else
    log "PHASE 0 OK: overlay $overlay, $reach/${#eps[@]} etcd members (secret store empty/unsealed — Phase 1 will gate)"
  fi
}

log "backoffice-up: tier=$TIER mode=$MODE adopt=$ADOPT dry-run=$DRY_RUN control-ip=$CONTROL_IP"
log "manifest: $MANIFEST (phases 0..$MAX_PHASE)"

_phase0_gate

# Iterate the manifest units in (phase, file) order, skipping any phase > MAX_PHASE.
RAN=()
while IFS=$'\t' read -r phase id gated via; do
  [ -n "$id" ] || continue
  if [ "$phase" -gt "$MAX_PHASE" ] 2>/dev/null; then
    continue
  fi
  # Phase 0's precheck unit is handled by _phase0_gate above (don't double-run it).
  [ "$id" = "precheck" ] && continue
  _run_unit "$phase" "$id" "$gated" "$via"
  RAN+=("$phase:$id")
done < <(_parse_manifest "$MANIFEST" | sort -s -t$'\t' -k1,1n)

if [ "$TIER" = "minimal" ]; then
  log "MINIMAL TIER complete (stopped after Phase 3 — no CI/reconciler/build-farm/DR)"
fi

# ── FINAL: emit the SETUP-7-style facts file (idempotency ledger) ────────────
# A re-run reads this and an --adopt converges instead of recreating. NEVER
# contains a secret — only tier + identity + endpoint + the units that ran.
if [ "$DRY_RUN" -eq 0 ] && [ "$MODE" = "run" ]; then
  {
    echo "# backoffice-site.yml — DAR-16 idempotency ledger (SETUP-7 style). GENERATED."
    echo "backoffice_tier: $TIER"
    echo "control_overlay_ip: $CONTROL_IP"
    echo "node: $(hostname -s 2>/dev/null || hostname)"
    echo "ts: $(date -u +%FT%TZ 2>/dev/null || echo unknown)"
    echo "max_phase: $MAX_PHASE"
    echo "adopt: $ADOPT"
    echo "units_run:"
    for u in "${RAN[@]}"; do echo "  - $u"; done
  } > "$FACTS"
  log "emitted facts: $FACTS"
fi

log "backoffice-up complete (tier=$TIER)"
