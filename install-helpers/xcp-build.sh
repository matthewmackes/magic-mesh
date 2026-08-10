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
#   xcp-build.sh coverage             sync + the canonical 80% llvm-cov floor
#   xcp-build.sh rpm                  sync + release build + base/lighthouse RPMs; pull them local
#   xcp-build.sh container-rpm [args] sync + Fedora container RPM cut on the farm
#   xcp-build.sh pull <remote-glob>   rsync artifacts back (relative to the remote repo)
#   xcp-build.sh shell                interactive ssh into the build VM
#   xcp-build.sh route <cargo args>   print the shape-routed host + reason (dry; no sync/build)
#   xcp-build.sh --route-test         run the routing self-test (offline; no farm contact)
#   xcp-build.sh --rpm-target-test    run the RPM target-Fedora guard self-test (offline)
#   xcp-build.sh --check-features     print the canonical RPM feature set + --locked policy (build-deploy-3)
#
# Env overrides: MCNF_BUILD_HOST (for example 172.20.0.130), MCNF_BUILD_USER (mm),
#   MCNF_BUILD_SLOT (unset) — an isolated remote workspace+target on the SAME host
#   so multiple concurrent jobs run without colliding (scale workloads per node:
#   e.g. BigBoy's 12c/24G hosts 2-3 parallel builds). slot "2" → ~/magic-mesh-2.
#   MCNF_BUILD_SHAPE (big|small) — force the job shape, overriding the cargo-args
#   inference (FA-6 shape-aware routing).
#   MCNF_COVERAGE_FLOOR (default 80) — hard line-coverage floor for `coverage`.
#   MCNF_CARGO_LLVM_COV_VERSION (default 0.8.7) — pinned farm coverage tool.
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

# The FA-6 fallback build node is the always-on home-services build VM at
# 172.20.0.50. Shape routing sends big/release jobs to BigBoy at 172.20.0.130
# when the topology resolves; this fallback exists only for topology/autoscaler
# failures so a build still has a reachable fixed host. Override with
# MCNF_BUILD_HOST (an explicit host always wins).
DEFAULT_BUILD_HOST="172.20.0.50"
BUILD_USER="${MCNF_BUILD_USER:-mm}"
KEY="${MCNF_BUILD_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
# build-deploy-3 — the RPM cut's cargo feature list + the --locked policy live in
# ONE shared fragment so this farm cut path and build-rpm-fedora43.sh cannot
# drift. Sourced here (host side): the `rpm` recipe builds its remote command
# string locally, so $MDE_RPM_* expand on this host before being sent to the VM.
# shellcheck source=install-helpers/rpm-features.sh disable=SC1091
source "$REPO/install-helpers/rpm-features.sh"
SOURCE_RECEIPT_HELPER="$REPO/install-helpers/source-revision-receipt.sh"
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
MIN_SYNC_FREE_KIB="${MCNF_BUILD_MIN_SYNC_FREE_KIB:-8388608}"

log()  { echo "==> xcp-build: $*"; }
warn() { echo "==> xcp-build: $*" >&2; }

assert_sync_space() {
  local free_kib
  case "$MIN_SYNC_FREE_KIB" in
    ''|*[!0-9]*)
      warn "MCNF_BUILD_MIN_SYNC_FREE_KIB must be a non-negative integer (got '$MIN_SYNC_FREE_KIB')"
      return 2
      ;;
  esac
  free_kib="$("${SSH[@]}" "$DEST" 'df -Pk "$HOME" | awk "NR == 2 { print \$4 }"')"
  case "$free_kib" in
    ''|*[!0-9]*)
      warn "refusing sync: $BUILD_HOST did not report bounded free-space data"
      return 2
      ;;
  esac
  if [ "$free_kib" -lt "$MIN_SYNC_FREE_KIB" ]; then
    warn "refusing sync to $BUILD_HOST: /home has ${free_kib} KiB free; ${MIN_SYNC_FREE_KIB} KiB required"
    warn "remove only abandoned magic-mesh-farm slots or select another farm host"
    return 1
  fi
  log "remote sync capacity: ${free_kib} KiB free (minimum ${MIN_SYNC_FREE_KIB})"
}

do_sync() {
  assert_sync_space
  log "rsync working tree → $DEST:$REMOTE_DIR (excluding target*/)"
  # Exclude /.git entirely: farm builds need source files, not git history, and
  # syncing a worktree's .git-file (a broken gitdir pointer) or colliding with a
  # stale clone is how the working tree got reverted mid-build. A git-free build
  # dir cannot be `git reset` out from under a build.
  # Agent/model handoffs can restore a file with the same size and timestamp as
  # an older farm copy. Content checksums prevent rsync's quick-check heuristic
  # from silently compiling that stale source as if it were authoritative.
  rsync -az --checksum --delete -e "${SSH[*]}" \
    --exclude '/target' --exclude '/target-f43' --exclude '/target-f44' \
    --exclude '/.claude' \
    --exclude '/.git' \
    "$REPO/" "$DEST:$REMOTE_DIR/"
}

# A promotable package is built from Git's immutable committed snapshot, never
# from a concurrently changing working directory. The receipt resolver has
# already refused dirty/unresolvable source before this function is called.
do_sync_revision() {
  local revision="$1" snapshot
  [[ "$revision" =~ ^[0-9a-f]{40}$|^[0-9a-f]{64}$ ]] \
    || { warn "refusing immutable sync for malformed revision '$revision'"; return 2; }
  assert_sync_space
  snapshot="$(mktemp -d)"
  trap 'rm -rf -- "$snapshot"' EXIT
  git -C "$REPO" archive --format=tar "$revision" | tar -xf - -C "$snapshot"
  log "rsync immutable source revision $revision → $DEST:$REMOTE_DIR"
  rsync -az --checksum --delete -e "${SSH[*]}" \
    --exclude '/target' --exclude '/target-f43' --exclude '/target-f44' \
    --exclude '/.claude' --exclude '/.git' \
    "$snapshot/" "$DEST:$REMOTE_DIR/"
  rm -rf -- "$snapshot"
  trap - EXIT
}

# Run a command in the remote repo with the cargo env + the workspace config
# (mold linker, CMAKE policy) already present via the synced .cargo/config.toml.
remote() {
  local remote_env="" name quoted remote_script
  # Live VDI proof runners set one of these target variables in their local
  # environment. Forward only the bounded endpoint inputs approved by the
  # ignored live tests; arbitrary local environment must not cross the farm
  # boundary or alter a remote build.
  for name in MDE_SPICE_LIVE_TARGET MDE_VNC_LIVE_TARGET MDE_RDP_LIVE_TARGET \
    MCNF_APP_VM_SOURCE_COMMIT; do
    if [[ -n "${!name+x}" ]]; then
      printf -v quoted '%q' "${!name}"
      remote_env+=" $name=$quoted"
    fi
  done
  # Keep the complete recipe inside one remote Bash program. In particular,
  # the RPM path begins with the shell builtin `export`; executing `$*`
  # directly after `env` made `env` look for an external program named
  # `export`, then the following semicolon allowed an unprovenanced build to
  # continue. `%q` preserves the recipe as one `bash -lc` argument while the
  # existing `quote_args` layer keeps individual Cargo arguments as data.
  printf -v remote_script '%q' "$*"
  "${SSH[@]}" "$DEST" "source \$HOME/.cargo/env 2>/dev/null; cd $REMOTE_DIR && env$remote_env bash -lc $remote_script"
}

# RPM ELF dependencies are build-host facts, not adjustable header metadata.
# A media-enabled shell cut on Fedora 42, for example, needs FFmpeg-7 sonames
# and cannot safely be deployed to a Fedora 44 Workstation with FFmpeg-8.
# Every native cut must name its target release so this hard check cannot be
# silently bypassed:
#
#   MCNF_RPM_TARGET_FEDORA=44 MCNF_BUILD_HOST=172.20.0.131 xcp-build.sh rpm
#
# An intentional F42 cut remains available by explicitly naming Fedora 42.
# Container cuts select their Fedora image separately and do not use this
# native-builder guard.
valid_rpm_target_fedora() {
  case "${1:-}" in
    ''|*[!0-9]*) return 1 ;;
    *) return 0 ;;
  esac
}

builder_fedora_release() {
  "${SSH[@]}" "$DEST" 'rpm -E %fedora'
}

assert_rpm_target_fedora() {
  local wanted="${MCNF_RPM_TARGET_FEDORA:-}" actual
  if [ -z "$wanted" ]; then
    warn "refusing native RPM cut: MCNF_RPM_TARGET_FEDORA is required"
    warn "name the builder's Fedora release explicitly (for example MCNF_RPM_TARGET_FEDORA=42)"
    return 2
  fi
  valid_rpm_target_fedora "$wanted" || {
    warn "MCNF_RPM_TARGET_FEDORA must be a Fedora numeric release (got '$wanted')"
    return 2
  }
  actual="$(builder_fedora_release | tr -d '[:space:]')"
  if [ "$actual" != "$wanted" ]; then
    warn "refusing RPM cut: requested Fedora $wanted target but $BUILD_HOST is Fedora ${actual:-unknown}"
    warn "ELF sonames must be built for the target release; select a matching builder (F44 Workstations: 172.20.0.131)."
    return 2
  fi
  log "RPM target-Fedora guard: builder Fedora $actual matches requested target Fedora $wanted"
}

rpm_target_self_test() {
  local fails=0 marker mock_builder_fedora status
  marker="$(mktemp)"
  trap 'rm -f -- "$marker"' EXIT

  # Replace farm contact with a deterministic builder-release probe. The marker
  # proves a missing target is rejected before even inspecting the builder.
  builder_fedora_release() {
    printf x >>"$marker"
    printf '%s\n' "$mock_builder_fedora"
  }
  BUILD_HOST="self-test-builder"

  expect_preflight() { # description target|UNSET builder expected-status expected-probes
    local description="$1" target="$2" expected_status="$4" expected_probes="$5" probes
    mock_builder_fedora="$3"
    : >"$marker"
    if [ "$target" = UNSET ]; then
      (unset MCNF_RPM_TARGET_FEDORA; assert_rpm_target_fedora >/dev/null 2>&1) && status=0 || status=$?
    else
      MCNF_RPM_TARGET_FEDORA="$target" assert_rpm_target_fedora >/dev/null 2>&1 && status=0 || status=$?
    fi
    probes="$(wc -c <"$marker")"
    if [ "$status" -eq "$expected_status" ] && [ "$probes" -eq "$expected_probes" ]; then
      printf '  ok   %s\n' "$description"
    else
      printf '  FAIL %s: exit/probes [%s/%s] want [%s/%s]\n' \
        "$description" "$status" "$probes" "$expected_status" "$expected_probes"
      fails=$((fails + 1))
    fi
  }

  expect_preflight "omitted target is rejected before builder probe" UNSET 42 2 0
  expect_preflight "mismatched target is rejected during preflight" 44 42 2 1
  expect_preflight "matching target passes preflight" 44 44 0 1
  [ "$(unset MCNF_BUILD_SHAPE; infer_shape rpm)" = big ] \
    || { echo "  FAIL native RPM no longer routes as a big job"; fails=$((fails + 1)); }

  valid_rpm_target_fedora 44 || fails=$((fails + 1))
  ! valid_rpm_target_fedora f44 || fails=$((fails + 1))
  ! valid_rpm_target_fedora 44.0 || fails=$((fails + 1))
  ! valid_rpm_target_fedora '' || fails=$((fails + 1))

  rm -f -- "$marker"
  trap - EXIT
  if [ "$fails" -eq 0 ]; then
    echo "RPM target-Fedora guard self-test: ALL PASS"
    return 0
  fi
  echo "RPM target-Fedora guard self-test: $fails FAILED" >&2
  return 1
}

quote_args() {
  local arg quoted out=""
  for arg in "$@"; do
    printf -v quoted '%q' "$arg"
    out+=" $quoted"
  done
  printf '%s' "$out"
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
  # Cargo argv must remain data after it crosses SSH. A filter containing a
  # command separator previously could run a second command after green tests
  # (observed as a post-success `H: command not found`).
  check "cargo argv quotes command separators" \
    "$(quote_args test -p mackesd 'camera; H')" \
    " test -p mackesd camera\\;\\ H"
  # A promotable RPM recipe starts with `export`. It must remain a shell
  # builtin inside the remote Bash program, never become `env export ...`.
  local quoted_recipe
  printf -v quoted_recipe '%q' \
    'export MCNF_BUILD_SOURCE_REVISION=abc MCNF_BUILD_PROMOTABLE=1; cargo build'
  check "remote recipe quotes export as one Bash program" \
    "$quoted_recipe" \
    "export\\ MCNF_BUILD_SOURCE_REVISION=abc\\ MCNF_BUILD_PROMOTABLE=1\\;\\ cargo\\ build"
  printf -v quoted_recipe '%q' \
    'export MCNF_BUILD_SOURCE_REVISION=abc MCNF_BUILD_PROMOTABLE=1; printf "%s:%s" "$MCNF_BUILD_SOURCE_REVISION" "$MCNF_BUILD_PROMOTABLE"'
  check "remote recipe executes export inside nested Bash" \
    "$(bash -c "env bash -lc $quoted_recipe")" \
    "abc:1"
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
  --rpm-target-test) rpm_target_self_test; exit $? ;;
  route) shift; resolve_host "$@"; exit 0 ;; # resolve_host already logged host+reason
  # build-deploy-3 self-check: print the canonical RPM cut knobs THIS path uses.
  # Both cut paths source install-helpers/rpm-features.sh, so this doubles as the
  # authoritative value for build-rpm-fedora43.sh — diff the two to prove parity.
  --check-features)
    echo "MDE_RPM_SHELL_FEATURES=$MDE_RPM_SHELL_FEATURES"
    echo "MDE_RPM_LOCKED=$MDE_RPM_LOCKED"
    exit 0
    ;;
esac

# Every farm-contacting subcommand resolves its host from the job shape first.
# The cargo args (for `cargo`) drive the shape; sync/gates/rpm/pull/shell pass a
# representative arg list so the shape inference still classifies them correctly.
# Unknown/help args fall through WITHOUT host resolution → straight to usage below.
case "${1:-}" in
  cargo) resolve_host "${@:2}" ;;
  rpm)   resolve_host rpm ;;               # release cut → big
  container-rpm) resolve_host rpm ;;       # Fedora container cut → big
  # gates = fmt + clippy --all-targets + test --workspace: a heavy WHOLE-WORKSPACE
  # job (it compiles every crate twice over), so it claims the big VM like a
  # workspace build, NOT a small pool node (design L1 "whole-workspace → big").
  gates) MCNF_BUILD_SHAPE="${MCNF_BUILD_SHAPE:-big}" resolve_host gates ;;
  # Coverage instruments the full workspace and is the long-pole gate too.
  coverage) MCNF_BUILD_SHAPE="${MCNF_BUILD_SHAPE:-big}" resolve_host coverage ;;
  sync | pull | shell) resolve_host ;;     # default (small) routing
esac

case "${1:-}" in
  sync) do_sync ;;

  cargo)
    shift
    do_sync
    cargo_args="$(quote_args "$@")"
    remote "cargo$cargo_args"
    ;;

  gates)
    do_sync
    # Keep this direct farm gate aligned with ci-gate.sh: locked dependency
    # resolution, the async-services mackesd superset, and serial PTY/env-race
    # lanes. A default-parallel mackesd run can hang or cross-contaminate tests.
    remote "cargo fmt --all --check" \
      && remote "cargo clippy --workspace --all-targets --locked" \
      && remote "cargo test --workspace --exclude mackesd --exclude mde-term-egui --locked" \
      && remote "cargo test -p mackesd --features async-services --locked -- --test-threads=1" \
      && remote "cargo test -p mde-term-egui --locked -- --test-threads=1"
    ;;

  coverage)
    COV_VERSION="${MCNF_CARGO_LLVM_COV_VERSION:-0.8.7}"
    COV_FLOOR="${MCNF_COVERAGE_FLOOR:-80}"
    case "$COV_VERSION" in
      ''|*[!0-9.]*) warn "invalid MCNF_CARGO_LLVM_COV_VERSION=$COV_VERSION"; exit 2 ;;
    esac
    case "$COV_FLOOR" in
      ''|*[!0-9]*) warn "invalid MCNF_COVERAGE_FLOOR=$COV_FLOOR"; exit 2 ;;
    esac
    do_sync
    log "coverage gate on $BUILD_HOST (llvm-cov $COV_VERSION, floor ${COV_FLOOR}%)"
    # Fresh/reconciled VMs may have Rust but not the cargo subcommand. Make the
    # coverage lane self-contained and deterministic: install the pinned tool
    # only when absent or at a different version, then run one canonical script.
    remote "set -euo pipefail; rustup component add llvm-tools-preview >/dev/null; if ! command -v cargo-llvm-cov >/dev/null || ! cargo llvm-cov --version 2>/dev/null | grep -Fq 'cargo-llvm-cov $COV_VERSION'; then cargo install cargo-llvm-cov --version $COV_VERSION --locked --force; fi; MCNF_COVERAGE_FLOOR=$COV_FLOOR ./install-helpers/coverage-command.sh"
    ;;

  rpm)
    # This is deliberately the first native-cut action after host routing: an
    # omitted or mismatched target must fail before source sync, dependency
    # installation, vendoring, or compilation can create a promotable-looking
    # artifact. `container-rpm` selects its Fedora image in its own helper.
    assert_rpm_target_fedora
    IFS=$'\t' read -r MCNF_BUILD_SOURCE_REVISION SOURCE_DATE_EPOCH \
      < <("$SOURCE_RECEIPT_HELPER" --repo "$REPO")
    export MCNF_BUILD_SOURCE_REVISION SOURCE_DATE_EPOCH
    log "promotable source receipt: $MCNF_BUILD_SOURCE_REVISION (epoch $SOURCE_DATE_EPOCH)"
    do_sync_revision "$MCNF_BUILD_SOURCE_REVISION"
    # build-deploy-7 — cargo-generate-rpm is NOT installed per-cut on this path;
    # it is pinned at VM-PROVISIONING time to CGR_VERSION by
    # setup-build-vm-toolchain.sh + infra/ansible build-vm-toolchain.yml (pinned by
    # construction, matching build-rpm-fedora43.sh's container pin). Read the VM's
    # actual version into the cut log + warn (NON-FATAL, keeps the cut working) on
    # drift so a re-toolchained VM can't silently cut with a different packager.
    CGR_VERSION="${CGR_VERSION:-0.21.0}"
    got_cgr="$(remote "cargo install --list 2>/dev/null" | sed -n 's/^cargo-generate-rpm v\([0-9.]*\).*/\1/p' | head -1 || true)"
    log "cargo-generate-rpm on $BUILD_HOST: ${got_cgr:-unknown} (pinned $CGR_VERSION)"
    case "${got_cgr:-}" in
      "$CGR_VERSION") : ;;
      "") warn "build-deploy-7: could not read cargo-generate-rpm version on $BUILD_HOST (proceeding)" ;;
      *)  warn "build-deploy-7: cargo-generate-rpm on $BUILD_HOST is $got_cgr, expected $CGR_VERSION — re-run setup-build-vm-toolchain.sh to re-pin" ;;
    esac
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
    # + BUG-VIDEO-1 / MEDIA-2 phase 1 `mpv-libs-devel` (docs/gpu_encoder.md):
    # links the real libmpv2 engine for the `media-mpv` re-link below —
    # without it the shell would silently fall back to FakeMpv.
    # build-deploy-7 — these -devel deps are NOT version-pinned (unpinned dnf per
    # cut = residual non-hermeticity flagged for the operator). Unlike the fedora
    # container cut, the farm VM's package state PERSISTS across cuts, so it drifts
    # only when the VM's Fedora is updated; the intended set is provisioned once by
    # setup-build-vm-toolchain.sh. Full hermeticity here wants a versioned builder
    # image / LAN mirror snapshot (see docs/review PLATFORM-REVIEW build-deploy-7).
    remote "sudo dnf install -y --setopt=install_weak_deps=False clang llvm python3 fontconfig-devel freetype-devel harfbuzz-devel mesa-libEGL-devel mesa-libGL-devel mesa-libgbm-devel libxkbcommon-devel mpv-libs-devel"
    # E12-3 DRM: after the workspace build, re-link mde-shell-egui with --features drm
    # so it owns the bare KMS/DRM seat (no Wayland compositor). The workspace build
    # compiles all dependencies; this one-crate rebuild only re-links the final binary.
    # + E12-5 `live-vdi`: the RPM shell must also carry the in-shell IronRDP
    # transport, otherwise Desktop connects stay at the honest gated caption.
    # + BUG-VIDEO-1 `media-mpv`: the RPM shell must link the real mpv engine, or
    # the embedded Media surface ships silently backed by FakeMpv (simulated
    # playback, no real A/V — the live-verified 2026-07-03 Eagle finding);
    # `release_shell_configuration_enables_the_real_media_engine`
    # (mde-shell-egui) fails loudly if this feature is ever dropped here.
    # build-deploy-3 — feature list + --locked come from rpm-features.sh (sourced
    # above); $MDE_RPM_* expand HERE on the local host, so the literal flags land
    # in the remote command string identical to build-rpm-fedora43.sh's.
    remote "export MCNF_BUILD_SOURCE_REVISION=$MCNF_BUILD_SOURCE_REVISION MCNF_BUILD_PROMOTABLE=1 SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH; mkdir -p target/generate-rpm && rm -f target/generate-rpm/magic-mesh*.rpm && cargo build --workspace --release $MDE_RPM_LOCKED && cargo build --release $MDE_RPM_LOCKED -p mde-shell-egui --features $MDE_RPM_SHELL_FEATURES && cargo generate-rpm -p crates/mesh/mackesd && cargo generate-rpm -p crates/mesh/mackesd --variant lighthouse && ./install-helpers/verify-rpm-payload.sh size target/generate-rpm/magic-mesh-[0-9]*.rpm && ./install-helpers/verify-rpm-payload.sh size target/generate-rpm/magic-mesh-lighthouse-*.rpm"
    mkdir -p "$ARTIFACTS"
    rm -f "$ARTIFACTS"/magic-mesh*.rpm
    log "pulling RPM(s) → $ARTIFACTS"
    rsync -az -e "${SSH[*]}" "$DEST:$REMOTE_DIR/target/generate-rpm/*.rpm" "$ARTIFACTS/"
    for rpm in "$ARTIFACTS"/magic-mesh*.rpm; do
      [ -e "$rpm" ] || continue
      "$REPO/install-helpers/verify-rpm-payload.sh" size "$rpm"
    done
    ls -la "$ARTIFACTS"/*.rpm
    ;;

  container-rpm)
    shift
    IFS=$'\t' read -r MCNF_BUILD_SOURCE_REVISION SOURCE_DATE_EPOCH \
      < <("$SOURCE_RECEIPT_HELPER" --repo "$REPO")
    export MCNF_BUILD_SOURCE_REVISION SOURCE_DATE_EPOCH
    log "promotable source receipt: $MCNF_BUILD_SOURCE_REVISION (epoch $SOURCE_DATE_EPOCH)"
    do_sync_revision "$MCNF_BUILD_SOURCE_REVISION"
    log "Fedora container RPM cut on $BUILD_HOST (Podman stays on the farm)"
    args="$(quote_args "$@")"
    remote "MCNF_FARM_REMOTE=1 MCNF_BUILD_SOURCE_REVISION=$MCNF_BUILD_SOURCE_REVISION MCNF_BUILD_PROMOTABLE=1 SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH bash install-helpers/build-rpm-fedora43.sh$args"
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
