#!/usr/bin/env bash
# xcp-build.sh — farm workspace builds out to the XCP build VM, keeping heavy
# compute OFF the local AI/dev host (operator directive 2026-06-20: "only AI
# work local; farm all other work to XCP; make better use of the compute you
# have access to"). The local host kept hitting 100% disk + slow contended
# builds; the build VM (mcnf-build) is a dedicated 4-vCPU / 16 GB Fedora guest
# on the idle XCP host XEN-HOME-SERVICES (172.20.0.9).
#
# It rsyncs the working tree (dirty or clean — no commit needed) to the VM,
# runs the build there, and pulls artifacts back. target*/ stay on the VM (the
# 200 GB+ of build output never touches the local disk again).
#
# Usage:
#   xcp-build.sh sync                 rsync the working tree to the VM
#   xcp-build.sh cargo <args...>      sync + run `cargo <args>` on the VM
#   xcp-build.sh gates                sync + fmt-check + clippy + test (the ship/release gates)
#   xcp-build.sh rpm                  sync + release build + cargo generate-rpm; pull the RPM local
#   xcp-build.sh pull <remote-glob>   rsync artifacts back (relative to the remote repo)
#   xcp-build.sh shell                interactive ssh into the build VM
#   xcp-build.sh route <cargo args>   print the shape-routed host + reason (dry; no sync/build)
#   xcp-build.sh --route-test         run the routing self-test (offline; no farm contact)
#
# Env overrides: MCNF_BUILD_HOST (172.20.0.52), MCNF_BUILD_USER (mm),
#   MCNF_BUILD_SLOT (unset) — an isolated remote workspace+target on the SAME host
#   so multiple concurrent jobs run without colliding (scale workloads per node:
#   e.g. BigBoy's 12c/24G hosts 2-3 parallel builds). slot "2" → ~/magic-mesh-2.
#   MCNF_BUILD_SHAPE (big|small) — force the job shape, overriding the cargo-args
#   inference (FA-6 shape-aware routing).
#
# FA-6 shape-aware routing (docs/design/farm-autoscale.md): the build farm is now
# ELASTIC — the autoscaler (install-helpers/farm-autoscale.sh) provisions per-dom0
# VMs in one of two shapes (`big` = one whole-host VM on XEN-BIGBOY, `small×N` = a
# pool). A job declares its shape (whole-workspace/release/rpm = BIG → BigBoy's big
# VM; per-crate build / agent pod = SMALL → spread across the small pool) and this
# script picks the matching provisioned VM from the live topology. If the
# autoscaler is paused / no matching VM is provisioned / the topology is
# unreadable, routing DEGRADES to the fixed BigBoy default below so a build never
# fails to route. The chosen host + reason are always logged.
set -euo pipefail

# The FA-6 fallback build node: the always-on standalone BigBoy build VM at
# 172.20.0.52 (the pre-autoscale fixed `mcnf-build-52` host), per the operator
# directive "worklist work → BIGBOY, testing → the two other Xen hosts"
# (2026-06-22). Routing degrades to THIS host whenever shape routing can't resolve
# an elastic VM (autoscaler paused / no matching shape / topology unreadable), so
# a build never fails to route. It must be a FIXED, always-present, non-elastic
# build VM (works even when the autoscaler has provisioned NOTHING). That is
# `mcnf-build-50` at **172.20.0.50** — NOT the autoscaler's elastic BigBoy `big`
# VM (.130). Corrected 2026-06-25 from a stale `.52`: per docs/BUILD-ENVIRONMENT.md
# §3 there is no live `.52` (the VM *named* mcnf-build-52 is at .130) — probing .52
# gives "No route to host", so the old fallback could never route (a silent
# work-stops landmine). Override with MCNF_BUILD_HOST (an explicit host always wins).
DEFAULT_BUILD_HOST="172.20.0.50"
BUILD_USER="${MCNF_BUILD_USER:-mm}"
KEY="${MCNF_BUILD_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
TOFU_DIR="${MCNF_TOFU_DIR:-$REPO/infra/tofu}"
# Per-slot remote dir lets concurrent agents share one VM (each its own target/).
# Base is `magic-mesh-farm` (NOT the bare `magic-mesh`): the build VMs carry a
# stale Forgejo-mirror clone at ~/magic-mesh whose origin/master sits at an old
# commit, and a CI git-reset there reverts the working tree mid-build (it broke
# the 11.0.6 + 11.0.8 generate-rpm step — Cargo.toml snapped back to 11.0.1). A
# dedicated, git-free build dir is immune. Override with MCNF_BUILD_DIR.
REMOTE_DIR="${MCNF_BUILD_DIR:-magic-mesh-farm}${MCNF_BUILD_SLOT:+-$MCNF_BUILD_SLOT}"
ARTIFACTS="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"
SSH=(ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 -o BatchMode=yes)

log()  { echo "==> xcp-build: $*"; }
warn() { echo "==> xcp-build: $*" >&2; }

do_sync() {
  log "rsync working tree → $DEST:$REMOTE_DIR (excluding target*/)"
  # Exclude /.git entirely: farm builds need source files, not git history, and
  # syncing a worktree's .git-file (a broken gitdir pointer) or colliding with a
  # stale clone is how the working tree got reverted mid-build. A git-free build
  # dir cannot be `git reset` out from under a build.
  rsync -az --delete -e "${SSH[*]}" \
    --exclude '/target' --exclude '/target-f43' --exclude '/target-f44' \
    --exclude '/.git' \
    "$REPO/" "$DEST:$REMOTE_DIR/"
}

# Run a command in the remote repo with the cargo env + the workspace config
# (mold linker, CMAKE policy) already present via the synced .cargo/config.toml.
remote() {
  "${SSH[@]}" "$DEST" "source \$HOME/.cargo/env 2>/dev/null; cd $REMOTE_DIR && $*"
}

# ============================================================================
# FA-6 — shape-aware build routing
# ============================================================================
# A job is BIG (whole-workspace/release/rpm → wants a whole dom0) or SMALL (a
# per-crate build / agent pod → spreads across the small pool). We pick the VM
# the autoscaler provisioned for that shape from the live topology, degrading to
# the fixed BigBoy default if nothing matches (a build NEVER fails to route).

# infer_shape <cargo-args...> — classify a job's shape from its cargo args, the
# same rule the autoscaler/design uses (docs/design/farm-autoscale.md L1):
#   build --workspace | --release | the `rpm`/`generate-rpm` subcommand → big
#   build -p <crate> | test -p <crate>                                  → small
#   anything else (a bare `cargo build`, `cargo test --workspace`, fmt…)  → small
# Whole-workspace TEST is treated SMALL (it's the gates path, not a release cut);
# only a whole-workspace/release *build* or an rpm cut claims the whole host.
# Pure: reads only its args + MCNF_BUILD_SHAPE; prints "big" or "small".
infer_shape() {
  # Explicit override wins (and is validated; a bad value falls through to infer).
  case "${MCNF_BUILD_SHAPE:-}" in
    big | small) printf '%s\n' "$MCNF_BUILD_SHAPE"; return 0 ;;
  esac
  local args=" $* " has_workspace=0 has_release=0 has_p=0 is_build=0 is_rpm=0
  case "$args" in *" --workspace "*) has_workspace=1 ;; esac
  case "$args" in *" --release "*) has_release=1 ;; esac
  case "$args" in *" -p "*) has_p=1 ;; esac
  case "$args" in *" build "*) is_build=1 ;; esac
  case "$args" in *" rpm "* | *" generate-rpm "*) is_rpm=1 ;; esac
  # A release/rpm cut, or a whole-workspace BUILD, is BIG. A per-crate (-p) job is
  # SMALL even if it carries --release (a single crate doesn't need the whole host).
  if [ "$is_rpm" -eq 1 ]; then
    printf 'big\n'
  elif [ "$has_p" -eq 1 ]; then
    printf 'small\n'
  elif [ "$is_build" -eq 1 ] && { [ "$has_workspace" -eq 1 ] || [ "$has_release" -eq 1 ]; }; then
    printf 'big\n'
  else
    printf 'small\n'
  fi
}

# read_topology — gather the live farm topology as newline-delimited records:
#   <shape> <ip>
# one per provisioned build VM, derived from the autoscaler's decision. Prefers a
# live `tofu output` (resolved IPs incl. any vm_overrides); falls back to parsing
# the autoscaler's generated *.auto.tfvars + the cold-fact IP scheme from main.tf
# (ip_base per dom0, +10 per small index — kept in sync with infra/tofu/main.tf).
# Empty output (nothing provisioned / unreadable) → caller degrades to default.
# This is the ONLY I/O in routing; the decision itself is the pure pick_host().
read_topology() {
  local tfvars="$TOFU_DIR/farm-autoscale.auto.tfvars"
  local tfvars_text=""
  [ -f "$tfvars" ] && tfvars_text="$(cat "$tfvars")"
  # 1) Live tofu output — authoritative for the resolved IPs (honours vm_overrides).
  #    Best-effort and fast-failing: a missing/locked state or unreachable XO just
  #    yields nothing here and we fall through to the cheap tfvars parse. Never
  #    blocks a build. We do NOT guess shape from vcpus (a `big` VM on home/xcp1 is
  #    only 3 vCPU, smaller than a 4-vCPU `small` — vcpus can't tell them apart);
  #    shape is read from the autoscaler's authoritative per-dom0 shape map, keyed
  #    by each VM's dom0. A VM whose dom0 isn't in the map (shouldn't happen) is
  #    treated `small` so it still routes a real IP rather than vanishing.
  if [ -n "${MCNF_ROUTE_NO_TOFU:-}" ]; then
    : # test/offline hook — skip the tofu probe entirely
  elif command -v tofu >/dev/null 2>&1 && command -v jq >/dev/null 2>&1 && [ -d "$TOFU_DIR" ]; then
    local out shape_json records
    if out="$( cd "$TOFU_DIR" && tofu output -json build_topology 2>/dev/null )" \
        && [ -n "$out" ] && [ "$out" != "null" ] && printf '%s' "$out" | jq -e 'length > 0' >/dev/null 2>&1; then
      shape_json="$(dom0_shape_json "$tfvars_text")"
      # `// "small"` (default) on BOTH the shape lookup and the ip_cidr split keeps
      # jq from aborting on a record that's mid-provision (null/absent dom0 or ip) —
      # one bad VM degrades to a small IP, it never collapses the whole topology.
      if records="$(printf '%s' "$out" | jq -r --argjson sh "$shape_json" \
            'to_entries[] | (($sh[.value.dom0 // ""] // "small") + " " + ((.value.ip_cidr // "") | sub("/.*";"")))' \
            2>/dev/null)" && [ -n "$records" ]; then
        printf '%s\n' "$records"
        return 0
      fi
    fi
  fi
  # 2) Fallback: parse the autoscaler's generated shape vars + the main.tf IP scheme.
  [ -n "$tfvars_text" ] || return 0
  topology_from_tfvars "$tfvars_text"
}

# dom0_shape_json <tfvars-text> — PURE: extract the autoscaler's `shape = {...}`
# decision into a compact JSON object {"<dom0>":"<shape>",...} for the jq join in
# read_topology. Empty/absent → "{}" (every dom0 then defaults to small downstream).
dom0_shape_json() {
  local text="$1" dk shape first=1 out="{"
  for dk in xen-bigboy xen-home-services kvm-xcp1; do
    shape="$(dom0_shape "$text" "$dk")"
    [ "$shape" = "off" ] && continue # off dom0s have no VM to classify
    [ "$first" -eq 1 ] || out="$out,"
    out="$out\"$dk\":\"$shape\""
    first=0
  done
  printf '%s}\n' "$out"
}

# dom0_shape <tfvars-text> <dom0-key> — PURE: the shape ("big"|"small"|"off") the
# autoscaler decided for one dom0, read from the `shape = {...}` map. Absent → off.
dom0_shape() {
  local shape
  # `{s/.../p;q}` prints the first match then quits sed — no `| head` pipeline (so
  # no SIGPIPE/pipefail interaction), and the first `shape = {...}` entry wins.
  shape="$(printf '%s\n' "$1" | sed -n "/\"$2\"[[:space:]]*=[[:space:]]*\"\\(big\\|small\\|off\\)\"/{s/.*\"$2\"[[:space:]]*=[[:space:]]*\"\\(big\\|small\\|off\\)\".*/\\1/p;q}")"
  printf '%s\n' "${shape:-off}"
}

# topology_from_tfvars <tfvars-text> — PURE: turn the autoscaler's HCL shape vars
# into "<shape> <ip>" records using the same cold-fact IP scheme as main.tf
# (per-dom0 ip_base; the big VM and small-0 share ip_base, +10 per extra small).
# No I/O — given the same text it always yields the same records (self-testable).
topology_from_tfvars() {
  local text="$1" dk shape n base i ip
  # The 3 elastic-managed dom0s + their build-VM ip_base (cold facts, main.tf). The
  # farm's 4th dom0 XEN-194 (build VM .170) is NOT in the autoscaler tfvars
  # (infra/tofu/variables.tf validates only these 3 keys — a known IaC gap), so routing
  # here covers these 3 + the fixed DEFAULT_BUILD_HOST fallback; pin
  # MCNF_BUILD_HOST=172.20.0.170 to target .170. Canonical roster: farm-topology.sh.
  for dk in xen-bigboy xen-home-services kvm-xcp1; do
    case "$dk" in
      xen-bigboy)        base="172.20.0.130" ;;
      xen-home-services) base="172.20.0.50" ;;
      kvm-xcp1)          base="172.20.0.90" ;;
    esac
    shape="$(dom0_shape "$text" "$dk")"
    case "$shape" in
      big)
        printf 'big %s\n' "$base" ;;
      small)
        # small_count for this dom0 (a number, not quoted); default 1 if absent.
        # The small_count map only carries `small` dom0s, so the FIRST numeric match
        # for this key (sed prints then `q`uits) is its count — no `| head` pipe.
        n="$(printf '%s\n' "$text" | sed -n "/\"$dk\"[[:space:]]*=[[:space:]]*[0-9]/{s/.*\"$dk\"[[:space:]]*=[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p;q}")"
        [ -n "$n" ] || n=1
        i=0
        while [ "$i" -lt "$n" ]; do
          ip="$(ip_plus "$base" $(( i * 10 )))"
          printf 'small %s\n' "$ip"
          i=$(( i + 1 ))
        done
        ;;
      off) : ;; # scale-to-zero — no VM
    esac
  done
}

# ip_plus <a.b.c.d> <n> — add n to the last octet (the +10 small-VM step). Pure.
ip_plus() {
  local ip="$1" add="$2" pre last
  pre="${ip%.*}"; last="${ip##*.}"
  printf '%s.%s\n' "$pre" "$(( last + add ))"
}

# pick_host <shape> <topology-text> <default-host> <slot> — THE PURE ROUTING
# DECISION (FA-6 DoD): given a shape, the topology records (one "<shape> <ip>" per
# line), the fallback host, and a slot/job key for spreading, print two lines:
#   <chosen-ip>
#   <reason>
# Rules:
#   big   → the first `big` VM in the topology (BigBoy's whole-host VM).
#   small → spread across the `small` pool by a stable hash of the slot/job, so
#           concurrent smalls land on DIFFERENT pool VMs.
#   no matching VM / empty topology → the default host (graceful degrade).
# Pure: no I/O, deterministic in its four args (so the self-test can assert it).
pick_host() {
  local shape="$1" topo="$2" default_host="$3" slot="$4"
  local -a pool=()
  local s ip
  while IFS=' ' read -r s ip; do
    [ -n "$s" ] || continue
    if [ "$s" = "$shape" ]; then pool+=("$ip"); fi
  done <<EOF
$topo
EOF
  if [ "${#pool[@]}" -eq 0 ]; then
    printf '%s\n' "$default_host"
    printf 'no %s VM in live topology — degrade to default %s\n' "$shape" "$default_host"
    return 0
  fi
  if [ "$shape" = "big" ]; then
    printf '%s\n' "${pool[0]}"
    printf 'big job → big VM %s (whole-host)\n' "${pool[0]}"
    return 0
  fi
  # small: stable spread. Hash the slot/job key to an index into the pool so the
  # SAME job always lands on the same VM (idempotent re-runs reuse its warm
  # target/) while concurrent DIFFERENT slots fan out across the pool.
  local h idx
  h="$(str_hash "$slot")"
  idx=$(( h % ${#pool[@]} ))
  printf '%s\n' "${pool[$idx]}"
  printf 'small job (slot=%s) → pool VM %s [%d/%d]\n' "${slot:-none}" "${pool[$idx]}" "$idx" "${#pool[@]}"
}

# str_hash <s> — a small stable non-negative integer hash of a string (djb2-ish),
# pure bash so the spread is deterministic without cksum/md5 process spawns.
str_hash() {
  local s="$1" h=5381 i c
  for (( i = 0; i < ${#s}; i++ )); do
    printf -v c '%d' "'${s:$i:1}"
    h=$(( ( (h * 33) + c ) & 0x7fffffff ))
  done
  printf '%s\n' "$h"
}

# resolve_host <cargo-args...> — the routing ENTRYPOINT used by the dispatch.
# Sets the global BUILD_HOST + DEST. An explicit MCNF_BUILD_HOST short-circuits
# everything (operator pin wins). Otherwise: infer shape → read live topology →
# pick_host → log the choice + reason.
resolve_host() {
  if [ -n "${MCNF_BUILD_HOST:-}" ]; then
    BUILD_HOST="$MCNF_BUILD_HOST"
    log "route: MCNF_BUILD_HOST pinned → $BUILD_HOST (shape routing skipped)"
  else
    local shape topo result
    shape="$(infer_shape "$@")"
    topo="$(read_topology)"
    result="$(pick_host "$shape" "$topo" "$DEFAULT_BUILD_HOST" "${MCNF_BUILD_SLOT:-0}")"
    BUILD_HOST="$(printf '%s\n' "$result" | sed -n '1p')"
    log "route: shape=$shape → $BUILD_HOST ($(printf '%s\n' "$result" | sed -n '2p'))"
  fi
  DEST="$BUILD_USER@$BUILD_HOST"
}

# route_self_test — offline assertions of the PURE routing pieces (FA-6 DoD).
# No farm contact; exercises infer_shape / topology_from_tfvars / pick_host only.
route_self_test() {
  local fails=0
  check() { # <desc> <got> <want>
    if [ "$2" = "$3" ]; then
      printf '  ok   %s\n' "$1"
    else
      printf '  FAIL %s: got [%s] want [%s]\n' "$1" "$2" "$3"; fails=$(( fails + 1 ))
    fi
  }
  # A live-ish topology: BigBoy big VM + a 3-wide small pool on home/xcp1.
  local TOPO; TOPO="$(printf 'big 172.20.0.130\nsmall 172.20.0.50\nsmall 172.20.0.90\nsmall 172.20.0.100\n')"

  # --- shape inference (cargo args) ---
  check "workspace build → big"   "$(unset MCNF_BUILD_SHAPE; infer_shape build --workspace --release)" big
  check "release build → big"     "$(unset MCNF_BUILD_SHAPE; infer_shape build --release)" big
  check "rpm subcommand → big"    "$(unset MCNF_BUILD_SHAPE; infer_shape rpm)" big
  check "generate-rpm → big"      "$(unset MCNF_BUILD_SHAPE; infer_shape generate-rpm -p crates/mesh/mackesd)" big
  check "build -p crate → small"  "$(unset MCNF_BUILD_SHAPE; infer_shape build -p mackesd)" small
  check "test -p crate → small"   "$(unset MCNF_BUILD_SHAPE; infer_shape test -p mackesd)" small
  check "workspace test → small"  "$(unset MCNF_BUILD_SHAPE; infer_shape test --workspace)" small
  check "bare build → small"      "$(unset MCNF_BUILD_SHAPE; infer_shape build)" small
  check "per-crate release→small" "$(unset MCNF_BUILD_SHAPE; infer_shape build -p mackesd --release)" small
  check "MCNF_BUILD_SHAPE=big"    "$(MCNF_BUILD_SHAPE=big infer_shape build -p mackesd)" big
  check "MCNF_BUILD_SHAPE=small"  "$(MCNF_BUILD_SHAPE=small infer_shape build --workspace)" small

  # --- pick_host routing ---
  check "big → BigBoy big VM" "$(pick_host big "$TOPO" "$DEFAULT_BUILD_HOST" slotA | sed -n 1p)" 172.20.0.130
  # A small lands SOMEWHERE in the pool (one of the three) — and is stable per slot.
  local s1 s1b
  s1="$(pick_host small "$TOPO" "$DEFAULT_BUILD_HOST" slot-1 | sed -n 1p)"
  s1b="$(pick_host small "$TOPO" "$DEFAULT_BUILD_HOST" slot-1 | sed -n 1p)"
  check "small is stable per slot" "$s1" "$s1b"
  case "$s1" in 172.20.0.50 | 172.20.0.90 | 172.20.0.100) check "small lands in pool" yes yes ;; *) check "small lands in pool" "$s1" "(pool)" ;; esac
  # Concurrent smalls SPREAD: across enough distinct slots we hit >1 distinct VM.
  local seen; seen="$(for j in a b c d e f g h; do pick_host small "$TOPO" "$DEFAULT_BUILD_HOST" "slot-$j" | sed -n 1p; done | sort -u | wc -l)"
  if [ "$seen" -ge 2 ]; then check "smalls spread across pool" yes yes; else check "smalls spread across pool" "$seen distinct" ">=2 distinct"; fi

  # --- graceful degrade ---
  check "big, empty topo → default"   "$(pick_host big "" "$DEFAULT_BUILD_HOST" slotA | sed -n 1p)" "$DEFAULT_BUILD_HOST"
  check "small, empty topo → default" "$(pick_host small "" "$DEFAULT_BUILD_HOST" slotA | sed -n 1p)" "$DEFAULT_BUILD_HOST"
  # Shape present but no MATCH (only smalls, want big) → default.
  local SMALLONLY; SMALLONLY="$(printf 'small 172.20.0.50\nsmall 172.20.0.90\n')"
  check "big, small-only topo → default" "$(pick_host big "$SMALLONLY" "$DEFAULT_BUILD_HOST" slotA | sed -n 1p)" "$DEFAULT_BUILD_HOST"

  # --- tfvars → topology parse (the autoscaler's generated HCL) ---
  local TFV; TFV="$(printf 'shape = {\n  "xen-bigboy" = "off"\n  "xen-home-services" = "small"\n  "kvm-xcp1" = "off"\n}\nsmall_count = {\n  "xen-home-services" = 3\n}\n')"
  # home small×3 → ip_base .50, +10, +20.
  check "tfvars: small×3 home pool" "$(topology_from_tfvars "$TFV" | tr '\n' '|')" "small 172.20.0.50|small 172.20.0.60|small 172.20.0.70|"
  local TFV2; TFV2="$(printf 'shape = {\n  "xen-bigboy" = "big"\n  "xen-home-services" = "off"\n  "kvm-xcp1" = "off"\n}\nsmall_count = {}\n')"
  check "tfvars: bigboy big" "$(topology_from_tfvars "$TFV2" | tr '\n' '|')" "big 172.20.0.130|"
  # End-to-end: a workspace build over a bigboy-big tfvars routes to .130.
  check "e2e: workspace build → .130" \
    "$(pick_host "$(unset MCNF_BUILD_SHAPE; infer_shape build --workspace)" "$(topology_from_tfvars "$TFV2")" "$DEFAULT_BUILD_HOST" s | sed -n 1p)" \
    172.20.0.130

  # --- authoritative shape map (the tofu-output classifier's join source) ---
  check "dom0_shape: bigboy big"  "$(dom0_shape "$TFV2" xen-bigboy)" big
  check "dom0_shape: home off"    "$(dom0_shape "$TFV2" xen-home-services)" off
  check "dom0_shape: absent→off"  "$(dom0_shape "" kvm-xcp1)" off
  check "dom0_shape_json bigboy"  "$(dom0_shape_json "$TFV2")" '{"xen-bigboy":"big"}'
  check "dom0_shape_json home×3"  "$(dom0_shape_json "$TFV")" '{"xen-home-services":"small"}'
  # The tofu-output classifier joins shape-by-dom0 (NOT vcpus): a `big` VM on
  # home/xcp1 (only 3 vCPU, < a small's 4) must still classify big, and a record
  # mid-provision (null vcpus/ip) must NOT abort the whole render. Exercise the
  # exact jq the live path runs, against a shape map + a topology JSON.
  if command -v jq >/dev/null 2>&1; then
    local SJ TOPO_JSON got
    SJ="$(dom0_shape_json "$(printf 'shape = {\n  "xen-home-services" = "big"\n  "xen-bigboy" = "off"\n  "kvm-xcp1" = "off"\n}\n')")"
    # vm "a": a 3-vCPU big on home; vm "b": a malformed record (no vcpus, no dom0).
    TOPO_JSON='{"xen-home-services":{"dom0":"xen-home-services","ip_cidr":"172.20.0.50/16","vcpus":3},"orphan":{"ip_cidr":"172.20.0.99/16"}}'
    got="$(printf '%s' "$TOPO_JSON" | jq -r --argjson sh "$SJ" 'to_entries[] | (($sh[.value.dom0 // ""] // "small") + " " + ((.value.ip_cidr // "") | sub("/.*";"")))' | tr '\n' '|')"
    check "tofu-join: 3vCPU big→big, orphan→small (no abort)" "$got" "big 172.20.0.50|small 172.20.0.99|"
  else
    check "tofu-join (jq unavailable — skipped)" skip skip
  fi

  echo
  if [ "$fails" -eq 0 ]; then
    log "route self-test: ALL PASS"
    return 0
  fi
  warn "route self-test: $fails FAILED"
  return 1
}

# --- offline subcommands (no host resolution / farm contact) -------------------
case "${1:-}" in
  --route-test | route-test) route_self_test; exit $? ;;
  route) shift; resolve_host "$@"; exit 0 ;; # resolve_host already logged host+reason
esac

# Every farm-contacting subcommand resolves its host from the job shape first.
# The cargo args (for `cargo`) drive the shape; sync/gates/rpm/pull/shell pass a
# representative arg list so the shape inference still classifies them correctly.
# Unknown/help args fall through WITHOUT host resolution → straight to usage below.
case "${1:-}" in
  cargo) resolve_host "${@:2}" ;;
  rpm)   resolve_host rpm ;;               # release cut → big
  # gates = fmt + clippy --all-targets + test --workspace: a heavy WHOLE-WORKSPACE
  # job (it compiles every crate twice over), so it claims the big VM like a
  # workspace build, NOT a small pool node (design L1 "whole-workspace → big").
  gates) MCNF_BUILD_SHAPE="${MCNF_BUILD_SHAPE:-big}" resolve_host gates ;;
  sync | pull | shell) resolve_host ;;     # default (small) routing
esac

case "${1:-}" in
  sync) do_sync ;;

  cargo) shift; do_sync; remote "cargo $*" ;;

  gates)
    do_sync
    remote "cargo fmt --all --check" \
      && remote "cargo clippy --all-targets" \
      && remote "cargo test --workspace"
    ;;

  rpm)
    do_sync
    # Stage the air-gapped vendored assets the generate-rpm `assets` array ships
    # — without these the VM has no vendor/birthright/ and generate-rpm dies
    # "Asset file not found" (BUILD-PLATFORM-4 RPM-cut gap, 2026-06-22). Mirror
    # build-rpm-fedora43.sh exactly so the farm RPM is byte-faithful to the
    # canonical cut: birthright blobs (ntfy/starship, fetched + sha256-verified).
    # Runs on the VM (it has network egress + podman) so the fetch stays off the
    # local host; idempotent.
    log "vendoring birthright blobs on the VM (off the local host)"
    remote "./install-helpers/vendor-birthright-blobs.sh"
    log "release build + generate-rpm on the VM (heavy — runs on XCP, not local)"
    # E12-3 DRM: after the workspace build, re-link mde-shell-egui with --features drm
    # so it owns the bare KMS/DRM seat (no Wayland compositor). The workspace build
    # compiles all dependencies; this one-crate rebuild only re-links the final binary.
    # + BOOKMARKS-6 `live-helper`: the RPM ships /usr/bin/mde-web-preview, so the
    # shipped shell must be able to spawn it — without this feature the Browser
    # surface is permanently the gated EmptyState (the live 2026-07-05 finding).
    remote "cargo build --workspace --release && cargo build --release -p mde-shell-egui --features drm,live-helper && cargo generate-rpm -p crates/mesh/mackesd"
    mkdir -p "$ARTIFACTS"
    log "pulling RPM(s) → $ARTIFACTS"
    rsync -az -e "${SSH[*]}" "$DEST:$REMOTE_DIR/target/generate-rpm/*.rpm" "$ARTIFACTS/"
    ls -la "$ARTIFACTS"/*.rpm
    ;;

  pull)
    shift; mkdir -p "$ARTIFACTS"
    rsync -az -e "${SSH[*]}" "$DEST:$REMOTE_DIR/$1" "$ARTIFACTS/"
    ;;

  shell) exec "${SSH[@]}" -o BatchMode=no "$DEST" ;;

  *)
    # Print the "# Usage:" comment block (content-addressed so it survives header
    # edits): from the Usage: line to the first blank/non-comment line after it.
    sed -n '/^# Usage:/,/^[^#]/p' "$0" | sed '/^[^#]/d; s/^# \{0,1\}//'
    exit 1
    ;;
esac
