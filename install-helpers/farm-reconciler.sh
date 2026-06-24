#!/usr/bin/env bash
# farm-reconciler.sh — FARM-AUTO reconciler loop (FARM-AUTOSCALE design L2).
#
# The no-AI, timer-drivable reconcile that ties the FA components into the live
# build lifecycle. One tick = one reconcile:
#
#   1. QUEUE SOURCE (no AI): determine the live per-dom0 build demand by reusing
#      what already exists — the worklist's active @farm:{…} jobs (parsed by
#      automation/lib/farm-jobs.sh) PLUS the LIVE in-flight builds on each build VM
#      (a running cargo/rustc/cc1plus — a true "a job is on this dom0 right now"
#      signal, so a busy dom0 is never scaled to zero out from under its build).
#      Each worklist job's command is classified BIG (whole-workspace / release /
#      rpm) or SMALL (per-crate / agent pod) by the SAME heuristic xcp-build.sh
#      uses (infer_shape, FA-6) — sourced, never re-implemented; each live build is
#      counted as one in-flight SMALL on its dom0. The counts are bucketed per dom0
#      into the "big:small[:pods]" the autoscaler expects (BIG → BigBoy; SMALL →
#      spread across the small-capable dom0s).
#   2. DECIDE: call install-helpers/farm-autoscale.sh with those counts to compute
#      + commit the per-dom0 shapes (FA-4 hysteresis/drain + FA-5 pod budget). The
#      autoscaler writes the tofu vars and runs `tofu plan` — it NEVER applies.
#   3. APPLY — GATED (L2, design-locked "operator/reconciler-gated"). Default is
#      PLAN-ONLY (never applies). Apply runs ONLY when FA_APPLY=1 AND the tofu
#      state is sane AND XO is reachable. When applying: `tofu apply` the committed
#      shapes (clone from MDE-VM-golden, scale-to-zero idle), then make each KEPT
#      build VM BUILD-READY (toolchain-on-first-provision, below), and between jobs
#      farm-vm-snapshot.sh snapshot-reverts each VM to its clean baseline (L3).
#
#      BUILD-READINESS — toolchain-on-first-provision + snapshot baseline:
#      MDE-VM-golden is a BASE template with NO Rust toolchain — a fresh clone has
#      cargo/rustc MISSING and CANNOT build. So after a successful apply, for each
#      build VM the autoscaler KEPT this tick, provision_build_ready (gated behind
#      the SAME apply gate) checks whether the VM already carries a `clean` baseline
#      snapshot (the one the inter-job reset reverts to). If it does, the VM is
#      build-ready and we SKIP — the toolchain cost is paid ONCE per VM, not per
#      tick, and the existing inter-job revert keeps it clean. If NOT (freshly
#      provisioned, no clean snapshot), we run infra/ansible/build-vm-toolchain.yml
#      against that VM's IP to install rust 1.94 + dev libs + mold, THEN take the
#      `clean` baseline snapshot so every future tick reverts to a TOOLCHAINED
#      baseline. Any failure (ansible missing / VM unreachable / playbook or
#      snapshot failure) WARNs loudly + skips that VM for the next tick to retry —
#      it NEVER crashes the tick or strands a running build. Plan-only / FA_APPLY=0
#      / --dry-run installs NOTHING (no ansible, no snapshot) — only the gated apply
#      path provisions.
#        FOLLOW-UP (faster): bake the toolchain INTO MDE-VM-golden so every clone is
#        instantly build-ready (zero per-VM toolchain cost on first provision). Then
#        provision_build_ready collapses to just taking the baseline snapshot.
#   4. DEGRADE GRACEFULLY (REQUIRED — the CURRENT live state): XO is presently
#      UNREACHABLE (ws://172.20.145.192:8080 connection-refused) and tofu has no
#      state. The reconciler DETECTS XO-unreachable / no-state, keeps the last-good
#      topology, logs loudly, and NEVER strands a running build or crashes — a
#      plan/apply failure DEGRADES the tick, it does not abort the loop.
#   5. OBSERVE (FA-7 tie-in): every tick logs its decision + reason + apply-gate
#      status. Modes for the timer/operator below.
#
# Modes:
#   farm-reconciler.sh --once               one reconcile (the systemd-timer entry)
#   farm-reconciler.sh --once --dry-run      one reconcile, decide-only (no commit,
#                                            no apply — succeeds even with XO down)
#   farm-reconciler.sh --status              current topology + queue + gate state
#   farm-reconciler.sh --self-test           pure-function assertions (no farm I/O)
#
# Apply prerequisites (ALL required before a real apply happens — honest defaults):
#   - FA_APPLY=1            opt in to apply (default OFF → plan-only, safe)
#   - XO reachable          the XO websocket host:port accepts a TCP connect
#   - tofu state present     `tofu state list` succeeds with ≥0 resources (sane)
#   - golden template set    var.golden_template_name is NON-EMPTY (default
#                            MDE-VM-golden) — i.e. apply WOULD create VMs at all (an
#                            operator blanks it for a connect-only plan). NB: this
#                            gate checks CONFIG, not template existence — that the
#                            template actually exists on XCP-2 is enforced by `tofu
#                            apply` itself failing loudly (the reconciler degrades).
# Until ALL hold, the reconciler stays PLAN-ONLY and says so on every tick. There
# is NO fake apply and NO pretend-provisioned VM — the live apply is OFF by default.
#
# Env: MCNF_REPO (default <repo>), MCNF_TOFU_DIR (default <repo>/infra/tofu),
#      MCNF_TOFU (default `tofu`), MCNF_WORKLIST (default <repo>/docs/WORKLIST.md),
#      MCNF_XO_URL (default ws://172.20.145.192:8080 — XO reachability probe),
#      MCNF_BUILD_USER (default mm), MCNF_FARM_KEY (default ~/.ssh/mackes_mesh_ed25519),
#      FA_APPLY (default 0 — the apply gate), FA_NOW (epoch; injectable for tests),
#      FA_PROBE_TIMEOUT (default 4s — XO TCP probe timeout),
#      FA_NO_SLOTS (set to skip the per-VM in-flight-slot probe — offline/tests),
#      MCNF_TOOLCHAIN_PLAYBOOK (default <repo>/infra/ansible/build-vm-toolchain.yml —
#        the build-ready provisioning playbook; overridable for tests).
#      OVERCOMMIT GUARD (safe hybrid mode): FA_BUILD_MEM_GIB (default 4) +
#        FA_BUILD_VCPUS (default 4) size the elastic burst VMs FIT-TO-HEADROOM so they
#        coexist with each dom0's always-on baseline VM rather than overcommitting it;
#        FA_MAX_SMALL defaults to 1 HERE (one elastic VM per dom0). Full-elastic (big
#        shapes) is the operator-driven alternative AFTER decommissioning the baselines.
#      Autoscaler tunables (FA_MAX_SMALL/FA_DWELL_SECS/FA_POD_BUDGET) pass through.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="${MCNF_REPO:-$(cd "$HERE/.." && pwd)}"
TOFU_DIR="${MCNF_TOFU_DIR:-$REPO_ROOT/infra/tofu}"
TOFU="${MCNF_TOFU:-tofu}"
WORKLIST="${MCNF_WORKLIST:-$REPO_ROOT/docs/WORKLIST.md}"
AUTOSCALE="$HERE/farm-autoscale.sh"
SNAPSHOT="$HERE/farm-vm-snapshot.sh"
XCP_BUILD="$HERE/xcp-build.sh"
TOOLCHAIN_PLAYBOOK="${MCNF_TOOLCHAIN_PLAYBOOK:-$REPO_ROOT/infra/ansible/build-vm-toolchain.yml}"
FARM_JOBS="$REPO_ROOT/automation/lib/farm-jobs.sh"
XO_URL="${MCNF_XO_URL:-ws://172.20.145.192:8080}"
BUILD_USER="${MCNF_BUILD_USER:-mm}"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
PROBE_TIMEOUT="${FA_PROBE_TIMEOUT:-4}"
APPLY="${FA_APPLY:-0}"

# --- OVERCOMMIT GUARD (safe hybrid mode) -------------------------------------
# The three dom0s each already run an ALWAYS-ON baseline build VM (16-24 GiB) with
# only a few GiB of headroom. tofu's default elastic VM is build_memory_gib=16 — so
# provisioning an elastic burst VM at 16 GiB (let alone several per dom0) would
# OVERCOMMIT the dom0 and the new VM would fail to boot. To make FA_APPLY=1 safe we
# bound the burst to FIT the headroom:
#   - fit-to-headroom SIZE: TF_VAR_build_memory_gib / TF_VAR_build_vcpus are EXPORTED
#     so EVERY tofu invocation this tick — the autoscaler's `tofu plan` AND the
#     reconciler's `tofu apply` — sizes elastic VMs the same, small enough to coexist
#     with the always-on baseline (defaults 4 GiB / 4 vCPU; override via
#     FA_BUILD_MEM_GIB / FA_BUILD_VCPUS). Exporting (vs a one-shot `-var` on apply
#     only) keeps PLAN and APPLY in agreement so the preview an operator gates on
#     matches what apply lands.
#   - bounded COUNT: FA_MAX_SMALL defaults to 1 HERE (the reconciler caps the
#     autoscaler at one small VM per dom0), so a burst is exactly one fit-sized
#     elastic VM per dom0 ALONGSIDE the always-on baseline — never multiple.
# This is the SAFE HYBRID MODE: always-on baseline + bounded elastic burst. The
# full-elastic migration (decommission the always-on VMs, then run big shapes) is
# the operator-driven alternative — see the apply step's note. Operators raise these
# only after confirming real dom0 headroom.
#
# fa_posint <value> <default> — echo <value> if it is a positive integer, else WARN
# and fall back to <default>. Guards an operator override (FA_BUILD_MEM_GIB=64, a
# typo, or "") from passing a bad/unsafe size straight into tofu — a non-integer
# would fail the apply, and a silently-huge value would defeat the guard's purpose.
fa_posint() {
  case "$1" in
    '' | *[!0-9]* ) echo "==> farm-reconciler: ignoring non-positive-integer override '$1' — using default $2" >&2; echo "$2" ;;
    0 )             echo "==> farm-reconciler: ignoring zero override — using default $2" >&2; echo "$2" ;;
    * )             echo "$1" ;;
  esac
}
BUILD_MEM_GIB="$(fa_posint "${FA_BUILD_MEM_GIB:-4}" 4)"
BUILD_VCPUS="$(fa_posint "${FA_BUILD_VCPUS:-4}" 4)"
# Export as TF_VARs so plan (autoscaler child) and apply (here) agree on the size.
export TF_VAR_build_memory_gib="$BUILD_MEM_GIB"
export TF_VAR_build_vcpus="$BUILD_VCPUS"
# Cap the autoscaler at one small VM per dom0 by default (overridable via FA_MAX_SMALL
# in the environment) so the elastic burst can never overcommit a dom0. Exported so
# the autoscaler child process honours it without threading a flag through every call.
export FA_MAX_SMALL="${FA_MAX_SMALL:-1}"

# Stable dom0 print/iterate order (matches the design doc + farm-autoscale.sh).
ORDER=("xen-bigboy" "xen-home-services" "kvm-xcp1")
# dom0 → the build-VM IPs to probe for in-flight work (best-effort, degrades).
# Per dom0 we probe BOTH:
#   - the elastic lane the autoscaler provisions (ip_base + the +10 small steps,
#     cold facts from infra/tofu/main.tf local.dom0 — a 4-wide small pool),
#   - AND the legacy fixed build VM (xcp-build.sh's DEFAULT_BUILD_HOST .52 on
#     BigBoy, .50/.51 historically), so the probe sees real builds in the CURRENT
#     live state too (XO down → nothing elastic provisioned → jobs route to .52).
# Unreachable IPs cost ~one probe-timeout each and contribute 0. Kept here as a
# small explicit list rather than re-deriving the +10 scheme so the probe stays a
# few cheap TCP checks; xcp-build.sh::topology_from_tfvars owns the authoritative
# IP math for ROUTING (this is only a liveness probe, so a superset is safe).
declare -A DOM0_IPS=(
  ["xen-bigboy"]="172.20.0.130 172.20.0.140 172.20.0.150 172.20.0.160 172.20.0.52"
  ["xen-home-services"]="172.20.0.50 172.20.0.60 172.20.0.70 172.20.0.80"
  ["kvm-xcp1"]="172.20.0.90 172.20.0.100 172.20.0.110 172.20.0.120 172.20.0.51"
)
# dom0 → its hypervisor (dom0) host, for the inter-job snapshot-revert (the dom0
# runs `xe`). Cold facts from install-helpers/farm.sh's fleet + main.tf pool names.
declare -A DOM0_HOST=(
  ["xen-bigboy"]="172.20.145.165"
  ["xen-home-services"]="172.20.0.9"
  ["kvm-xcp1"]="172.20.145.193"
)
# dom0 → the VM name-labels per shape (cold facts from infra/tofu/main.tf
# local.dom0: big_name / small_name; the nth small is "<small_name>-<n>" for n≥1).
declare -A DOM0_BIG_NAME=(
  ["xen-bigboy"]="mcnf-build-big-52"
  ["xen-home-services"]="mcnf-build-big-50"
  ["kvm-xcp1"]="mcnf-build-big-51"
)
declare -A DOM0_SMALL_NAME=(
  ["xen-bigboy"]="mcnf-build-52"
  ["xen-home-services"]="mcnf-build-50"
  ["kvm-xcp1"]="mcnf-build-51"
)
# dom0 → the autoscaler flag that carries its queue spec.
declare -A DOM0_FLAG=(
  ["xen-bigboy"]="--bigboy"
  ["xen-home-services"]="--home"
  ["kvm-xcp1"]="--xcp1"
)

usage() { sed -n '2,55p' "$0" | sed 's/^# \{0,1\}//'; }

# =============================================================================
# PURE helpers — no I/O, no globals; exercised by --self-test.
# =============================================================================

# classify_command <cargo-command-string> — BIG | SMALL for a single @farm job's
# command. Defers to xcp-build.sh's infer_shape (FA-6) when sourceable so the rule
# can NEVER drift from the router; the small inline fallback mirrors it 1:1 for the
# self-test / when xcp-build.sh is absent. The command is the text inside @farm:{…}
# (e.g. "cargo build --workspace --release" or "cargo build -p mde-bus"). Echoes
# "big" or "small".
classify_command() {
  local cmd="$1"
  # Drop a leading "cargo" so infer_shape sees the subcommand+args it expects.
  local args="${cmd#cargo }"
  if declare -f infer_shape >/dev/null 2>&1; then
    # infer_shape reads MCNF_BUILD_SHAPE; unset it so the COMMAND drives the shape
    # (a stray env from the caller must not pin every job's class). $args is split
    # ON PURPOSE — infer_shape wants the cargo args as separate positionals.
    # shellcheck disable=SC2086
    ( unset MCNF_BUILD_SHAPE; infer_shape $args )
    return
  fi
  # Fallback (xcp-build.sh not sourced): the same rule, inline.
  local a=" $args " ws=0 rel=0 hp=0 isb=0 isr=0
  case "$a" in *" --workspace "*) ws=1 ;; esac
  case "$a" in *" --release "*) rel=1 ;; esac
  case "$a" in *" -p "*) hp=1 ;; esac
  case "$a" in *" build "*) isb=1 ;; esac
  case "$a" in *" rpm "* | *" generate-rpm "*) isr=1 ;; esac
  if [ "$isr" -eq 1 ]; then echo big
  elif [ "$hp" -eq 1 ]; then echo small
  elif [ "$isb" -eq 1 ] && { [ "$ws" -eq 1 ] || [ "$rel" -eq 1 ]; }; then echo big
  else echo small; fi
}

# bucket_demand <big-total> <small-total> <pod-total> — PURE: turn the WHOLE-FLEET
# big/small/pod job totals into the per-dom0 "big:small:pods" specs the autoscaler
# takes, on stdout as three lines IN ORDER (xen-bigboy, xen-home-services, kvm-xcp1).
# Placement rule (mirrors xcp-build.sh routing + design L1/L4):
#   - BIG jobs want a whole host → ALL counted on xen-bigboy (the only true big iron;
#     a big VM on home/xcp1 is only 3 vCPU). The autoscaler then runs BigBoy `big`.
#   - SMALL jobs + pods spread across the small-capable dom0s. We split smalls
#     round-robin-ish across home + xcp1 (BigBoy carries the bigs); pods follow the
#     smalls so a pod-heavy queue biases the dom0s toward small×N (FA-5).
#   - If there ARE bigs, BigBoy is claimed by big (L4 mutual exclusion) and its
#     smalls fold onto home/xcp1; with no bigs, BigBoy can also take smalls.
# Deterministic in its 3 args, so the self-test can assert exact specs.
bucket_demand() {
  local big="$1" small="$2" pods="$3"
  local bb_big=0 bb_small=0 hm_small=0 x1_small=0
  local bb_pods=0 hm_pods=0 x1_pods=0
  # BIG → BigBoy (whole host). A nonzero big count claims BigBoy's big shape.
  bb_big="$big"
  # SMALL pool dom0s: home + xcp1 always; BigBoy too ONLY when no bigs claim it.
  local -a pool=("xen-home-services" "kvm-xcp1")
  [ "$big" -eq 0 ] && pool=("xen-bigboy" "xen-home-services" "kvm-xcp1")
  local n="${#pool[@]}" i=0 dk
  # Spread smalls + pods round-robin across the pool (deterministic order).
  local s=0 p=0
  while [ "$s" -lt "$small" ] || [ "$p" -lt "$pods" ]; do
    dk="${pool[$(( i % n ))]}"
    if [ "$s" -lt "$small" ]; then
      case "$dk" in
        xen-bigboy) bb_small=$(( bb_small + 1 )) ;;
        xen-home-services) hm_small=$(( hm_small + 1 )) ;;
        kvm-xcp1) x1_small=$(( x1_small + 1 )) ;;
      esac
      s=$(( s + 1 ))
    fi
    if [ "$p" -lt "$pods" ]; then
      case "$dk" in
        xen-bigboy) bb_pods=$(( bb_pods + 1 )) ;;
        xen-home-services) hm_pods=$(( hm_pods + 1 )) ;;
        kvm-xcp1) x1_pods=$(( x1_pods + 1 )) ;;
      esac
      p=$(( p + 1 ))
    fi
    i=$(( i + 1 ))
  done
  printf '%s:%s:%s\n' "$bb_big" "$bb_small" "$bb_pods"
  printf '%s:%s:%s\n' "0" "$hm_small" "$hm_pods"
  printf '%s:%s:%s\n' "0" "$x1_small" "$x1_pods"
}

# apply_gate <fa_apply> <xo_reachable> <state_sane> <golden_set> — PURE: the apply
# decision (L2). Echoes "apply" iff ALL four hold, else "plan-only:<reason>" naming
# the FIRST failing prerequisite (so the tick logs exactly why it stayed plan-only).
# Each input is "1"/"0". Default-safe: any 0 → plan-only.
apply_gate() {
  local fa="$1" xo="$2" state="$3" golden="$4"
  if [ "$fa" != "1" ];     then echo "plan-only:FA_APPLY!=1 (apply opt-in off)"; return; fi
  if [ "$xo" != "1" ];     then echo "plan-only:XO-unreachable"; return; fi
  if [ "$state" != "1" ];  then echo "plan-only:tofu-state-unsafe"; return; fi
  if [ "$golden" != "1" ]; then echo "plan-only:no-golden-template"; return; fi
  echo "apply"
}

# host_port_from_xo_url <ws://host:port[/...]> — PURE: extract "host port" for the
# TCP reachability probe. ws://172.20.145.192:8080 → "172.20.145.192 8080".
# Handles a bracketed IPv6 literal (ws://[::1]:8080 → "::1 8080") so an IPv6 XO
# isn't pinned plan-only by a misparse. Defaults the port to 80 (ws) if absent.
host_port_from_xo_url() {
  local u="$1" rest host port
  rest="${u#*://}"          # strip scheme
  rest="${rest%%/*}"        # drop any /path
  case "$rest" in
    \[*\]*)                 # [ipv6] or [ipv6]:port
      host="${rest#\[}"; host="${host%%\]*}"
      port="${rest#*\]}"    # ":port" or ""
      port="${port#:}"
      [ -n "$port" ] || port=80
      ;;
    *)
      host="${rest%%:*}"
      if [ "$rest" = "$host" ]; then port=80; else port="${rest##*:}"; fi
      ;;
  esac
  printf '%s %s\n' "$host" "$port"
}

# vm_is_build_ready <has-clean-probe-rc> — PURE: decide build-readiness from the EXIT
# CODE of the `farm-vm-snapshot.sh has-clean` probe for one VM. A build VM is BUILD-
# READY iff it already carries a `clean` baseline snapshot (the one reset_running_vms
# reverts to — taken only AFTER the toolchain is installed, so its presence means the
# toolchain is baked in). rc 0 = has a clean snapshot → "ready"; ANY nonzero (no
# snapshot / absent VM / dom0-unreachable) → "not-ready", which re-provisions — the
# SAFE direction, since the toolchain playbook is idempotent. Pure in its one arg so
# the self-test can assert both branches without touching a dom0.
vm_is_build_ready() {
  if [ "$1" = "0" ]; then echo ready; else echo not-ready; fi
}

# provision_enabled <gate-verdict> — PURE: may provision_build_ready install the
# toolchain this tick? ONLY when the apply gate verdict is exactly "apply" (the same
# verdict that authorised the tofu apply). Any plan-only:* verdict — FA_APPLY=0,
# XO-unreachable, dry-run, etc. — returns nonzero so the step is a hard no-op: NO
# ansible, NO snapshot. This is the structural guarantee that plan-only/--dry-run
# never provisions. Echoes nothing; the rc IS the answer (0 = provision).
provision_enabled() { [ "$1" = "apply" ]; }

# toolchain_inventory_line <ip> <user> <key> — PURE: build the single host line for a
# one-host inventory FILE, pinning the VM's IP + ssh user + key so the run is self-
# contained (no dependence on infra/ansible/inventory.ini, which only lists the 3
# FIXED build VMs — an elastically-cloned VM's IP wouldn't be in it). The caller
# writes it under a `[build_vms]` header (the group build-vm-toolchain.yml targets);
# an inline `-i host,` would put the host in `ungrouped` and the play would match
# nothing. Matches the inventory.ini schema (ansible_host/ansible_user/
# ansible_ssh_private_key_file). Pure string-builder so the self-test asserts it.
toolchain_inventory_line() {
  local ip="$1" user="$2" key="$3"
  printf '%s ansible_host=%s ansible_user=%s ansible_ssh_private_key_file=%s\n' \
    "$ip" "$ip" "$user" "$key"
}

# =============================================================================
# --self-test — pure-function assertions (no farm I/O). Run first, exits.
# =============================================================================
if [ "${1:-}" = "--self-test" ]; then
  fails=0
  check() { # check <label> <got> <want>
    if [ "$2" = "$3" ]; then echo "  ok: $1"
    else echo "  FAIL: $1 — got '$2' want '$3'" >&2; fails=$((fails + 1)); fi
  }
  echo "farm-reconciler --self-test:"

  # --- classify_command (the @farm-job shape rule; mirrors infer_shape FA-6) ---
  check "workspace build → big"   "$(classify_command 'cargo build --workspace --release')" big
  check "release build → big"     "$(classify_command 'cargo build --release')" big
  check "rpm cut → big"           "$(classify_command 'cargo generate-rpm -p crates/mesh/mackesd')" big
  check "per-crate build → small" "$(classify_command 'cargo build -p mde-bus')" small
  check "per-crate test → small"  "$(classify_command 'cargo test -p mde-theme')" small
  check "workspace test → small"  "$(classify_command 'cargo test --workspace')" small
  check "per-crate +release small" "$(classify_command 'cargo build -p mackesd --release')" small

  # --- bucket_demand (whole-fleet totals → per-dom0 specs) ---
  # One big job → BigBoy big, nothing else.
  check "1 big → bigboy big" "$(bucket_demand 1 0 0 | tr '\n' '|')" "1:0:0|0:0:0|0:0:0|"
  # Two smalls, no bigs → BigBoy + home take one each (round-robin from the 3-pool).
  check "2 small no-big spread" "$(bucket_demand 0 2 0 | tr '\n' '|')" "0:1:0|0:1:0|0:0:0|"
  # Bigs present → BigBoy claimed by big; smalls fold onto home+xcp1 only (L4).
  check "big preempts: smalls to home+xcp1" "$(bucket_demand 1 2 0 | tr '\n' '|')" "1:0:0|0:1:0|0:1:0|"
  # Pods follow the smalls; a pod-heavy queue lands pods on the small pool (FA-5).
  check "pods spread to small pool" "$(bucket_demand 0 0 3 | tr '\n' '|')" "0:0:1|0:0:1|0:0:1|"
  # Idle fleet → every dom0 off (0:0:0).
  check "idle → all off" "$(bucket_demand 0 0 0 | tr '\n' '|')" "0:0:0|0:0:0|0:0:0|"

  # --- apply_gate (L2: all four prereqs must hold) ---
  check "gate: all ok → apply"        "$(apply_gate 1 1 1 1)" apply
  check "gate: FA_APPLY off"          "$(apply_gate 0 1 1 1)" "plan-only:FA_APPLY!=1 (apply opt-in off)"
  check "gate: XO down (current live)" "$(apply_gate 1 0 1 1)" "plan-only:XO-unreachable"
  check "gate: no state"              "$(apply_gate 1 1 0 1)" "plan-only:tofu-state-unsafe"
  check "gate: no golden"             "$(apply_gate 1 1 1 0)" "plan-only:no-golden-template"
  # The CURRENT live state (XO down, no state) is plan-only even if opted in.
  check "gate: live state → plan-only" "$(apply_gate 1 0 0 1)" "plan-only:XO-unreachable"

  # --- host_port_from_xo_url ---
  check "xo url host:port"      "$(host_port_from_xo_url 'ws://172.20.145.192:8080')" "172.20.145.192 8080"
  check "xo url with path"      "$(host_port_from_xo_url 'ws://10.0.0.1:8080/api')" "10.0.0.1 8080"
  check "xo url no port → 80"   "$(host_port_from_xo_url 'ws://10.0.0.1')" "10.0.0.1 80"
  check "xo url ipv6 host:port" "$(host_port_from_xo_url 'ws://[::1]:8080')" "::1 8080"
  check "xo url ipv6 no port"   "$(host_port_from_xo_url 'ws://[fe80::1]')" "fe80::1 80"

  # --- vm_is_build_ready (toolchain-on-first-provision readiness from the probe rc) ---
  # has-clean rc 0 = a clean snapshot exists → already toolchained → ready → SKIP.
  check "probe rc 0 → ready"            "$(vm_is_build_ready 0)" ready
  # ANY nonzero (no snapshot / absent VM / dom0 down) → not-ready → provision RUNS.
  check "probe rc 1 → run"              "$(vm_is_build_ready 1)" not-ready
  check "probe rc 255 (ssh fail) → run" "$(vm_is_build_ready 255)" not-ready

  # --- toolchain_inventory_line (one [build_vms]-group host line for the clone) ---
  check "inventory line for a clone" \
    "$(toolchain_inventory_line 172.20.0.130 mm /root/.ssh/mackes_mesh_ed25519)" \
    "172.20.0.130 ansible_host=172.20.0.130 ansible_user=mm ansible_ssh_private_key_file=/root/.ssh/mackes_mesh_ed25519"

  # --- provision_enabled: the structural "dry-run/plan-only NEVER provisions" guard.
  # Only the exact "apply" verdict authorises installing the toolchain; every
  # plan-only:* verdict (FA_APPLY=0, XO-down, dry-run) is a hard no-op.
  check "gate apply → provision"        "$(provision_enabled apply && echo yes || echo no)" yes
  check "FA_APPLY=0 → no provision"     "$(provision_enabled 'plan-only:FA_APPLY!=1 (apply opt-in off)' && echo yes || echo no)" no
  check "XO-down → no provision"        "$(provision_enabled 'plan-only:XO-unreachable' && echo yes || echo no)" no

  # --- demand contribution (queue-accuracy bug A): a @farm:{…} payload contributes
  # to demand ONLY when it is a real `cargo …` build command. Templates/placeholders
  # (crate,verify / …) contribute 0. We assert this end-to-end through the REAL
  # farm-jobs.sh active path (the same path collect_worklist_demand uses), counting
  # the active jobs for a synthetic worklist. Skipped (not failed) if farm-jobs.sh
  # is absent, so the pure self-test stays runnable anywhere.
  if [ -x "$FARM_JOBS" ]; then
    demand_for() { # demand_for <worklist-body> → number of active @farm jobs
      local wl; wl="$(mktemp "${TMPDIR:-/tmp}/mcnf-fa-st.XXXXXX")" || { echo 0; return; }
      printf '%s\n' "$1" >"$wl"
      MCNF_WORKLIST="$wl" "$FARM_JOBS" active 2>/dev/null | grep -c . || true
      rm -f "$wl"
    }
    check "template payload → 0 demand" \
      "$(demand_for '- [ ] **DRAIN-4: x** @farm:{crate,verify}')" 0
    check "ellipsis placeholder → 0 demand" \
      "$(demand_for '- [>] **FOO-1: x** @farm:{…}')" 0
    check "real cargo job → 1 demand" \
      "$(demand_for '- [>] **FOO-2: x** @farm:{cargo build -p mde-bus}')" 1
    check "mixed: template + 2 real → 2 demand" \
      "$(demand_for '- [>] **FOO-3: x** @farm:{crate,verify} @farm:{cargo build -p a} @farm:{cargo test -p b}')" 2
  else
    echo "  skip: demand-contribution (no farm-jobs.sh at $FARM_JOBS)"
  fi

  # --- overcommit guard (bug B): the apply path is bounded by default — a
  # fit-to-headroom size + one small VM per dom0, so a burst can't overcommit a dom0.
  # Assert the DEFAULT-DERIVATION logic in a controlled (unset) sub-env, NOT the live
  # globals — so an operator who legitimately overrode FA_MAX_SMALL/FA_BUILD_* in their
  # shell still sees a green self-test (the script is correct; only the defaults differ).
  # SC2016: the single-quotes are DELIBERATE — the ${VAR:-default} must expand inside
  # the `env -u`-cleared CHILD shell (with the var unset), not in this parent shell.
  # shellcheck disable=SC2016
  check "default elastic mem fit-to-headroom" \
    "$(env -u FA_BUILD_MEM_GIB bash -c 'echo "${FA_BUILD_MEM_GIB:-4}"')" 4
  # shellcheck disable=SC2016
  check "default elastic vcpus fit-to-headroom" \
    "$(env -u FA_BUILD_VCPUS bash -c 'echo "${FA_BUILD_VCPUS:-4}"')" 4
  # shellcheck disable=SC2016
  check "max_small capped to 1/dom0 by default" \
    "$(env -u FA_MAX_SMALL bash -c 'echo "${FA_MAX_SMALL:-1}"')" 1
  # And the override-validation guard: a bad override falls back to the default + warns.
  check "non-int mem override → default" "$(fa_posint abc 4 2>/dev/null)" 4
  check "zero mem override → default"    "$(fa_posint 0 4 2>/dev/null)" 4
  check "empty mem override → default"   "$(fa_posint '' 4 2>/dev/null)" 4
  check "valid mem override kept"        "$(fa_posint 8 4 2>/dev/null)" 8

  if [ "$fails" -eq 0 ]; then
    echo "farm-reconciler: self-test passed"; exit 0
  fi
  echo "farm-reconciler: SELF-TEST FAILED ($fails)" >&2; exit 1
fi

# =============================================================================
# Arg parse (modes). Default with no args = --status (read-only, safe).
# =============================================================================
MODE="status"
DRY_RUN=0
while [ $# -gt 0 ]; do case "$1" in
  --once)    MODE="once"; shift;;
  --status)  MODE="status"; shift;;
  --dry-run) DRY_RUN=1; shift;;
  -h | --help | help) usage; exit 0;;
  *) echo "farm-reconciler: unknown arg: $1" >&2; usage; exit 2;;
esac; done

# All diagnostics go to STDERR (visible in the systemd-timer journal). STDOUT is
# reserved for DATA the helpers capture via `< <(…)` (the per-dom0 specs from
# collect_demand) — a diagnostic on stdout would be slurped in as a fake spec.
log()  { echo "==> farm-reconciler: $*" >&2; }
warn() { echo "==> farm-reconciler: $*" >&2; }
die()  { warn "$*"; exit 2; }

NOW="${FA_NOW:-$(date +%s)}"
SSH=(ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=8)

# Reuse xcp-build.sh's canonical helpers (infer_shape FA-6, dom0_shape) by eval'ing
# JUST their definitions here, instead of re-implementing them (no drift). We can't
# plain-`source` xcp-build.sh — its top-level dispatch `case` ends in `*) … exit 1`,
# which a source would propagate into us. So extract each function's text and eval.
#
# source_fn_from <file> <fnname> — define <fnname> in THIS shell from its definition
# in <file>. Captures from `^<fnname>() {` to the matching `^}` at COLUMN 0 (these
# helpers are flat — no column-0 `}` inside the body — so the first col-0 `}` is the
# function's own close). Returns nonzero (caller warns + uses the inline fallback) if
# the file is unreadable or the function isn't found, so a refactor that renames the
# upstream function degrades loudly to the mirror rather than silently misbehaving.
source_fn_from() {
  local file="$1" fn="$2" body
  [ -r "$file" ] || return 1
  body="$(awk -v fn="$fn" '
    $0 ~ "^" fn "\\(\\) \\{" { f=1 }
    f { print }
    f && /^\}/ { exit }
  ' "$file")"
  # Must have captured a complete block: starts with the def and ends with a col-0 }.
  case "$body" in
    "$fn"'() {'*) ;; *) return 1 ;;
  esac
  printf '%s\n' "$body" | grep -qE '^\}$' || return 1
  eval "$body"
}
source_fn_from "$XCP_BUILD" infer_shape || warn "could not reuse infer_shape from $XCP_BUILD — using the inline mirror"
# dom0_shape lets the reset step read the committed per-dom0 shape from the tfvars;
# if it can't be sourced the reset step degrades to a no-op (logged), never crashes.
source_fn_from "$XCP_BUILD" dom0_shape || warn "could not reuse dom0_shape from $XCP_BUILD — inter-job reset will be skipped"

# --- XO reachability probe (graceful-degrade signal #1) -----------------------
# A pure TCP connect to the XO websocket host:port. The CURRENT live state is
# connection-refused (XO down) → returns 1, and the reconciler degrades. Never
# blocks the loop: bounded by FA_PROBE_TIMEOUT.
xo_reachable() {
  local hp host port
  hp="$(host_port_from_xo_url "$XO_URL")"
  host="${hp%% *}"; port="${hp##* }"
  timeout "$PROBE_TIMEOUT" bash -c "cat </dev/null >/dev/tcp/$host/$port" 2>/dev/null
}

# --- tofu state sanity (graceful-degrade signal #2) ---------------------------
# State is "sane" iff tofu is installed, the dir initialised, and `tofu state list`
# succeeds (an empty list is fine — it means zero managed resources, not a fault;
# a missing/locked/corrupt backend FAILS the command → unsafe → plan-only). The
# CURRENT live state has NO tofu state → unsafe → plan-only. Best-effort + bounded.
tofu_state_sane() {
  command -v "$TOFU" >/dev/null 2>&1 || return 1
  [ -d "$TOFU_DIR" ] || return 1
  ( cd "$TOFU_DIR" && timeout 30 "$TOFU" state list >/dev/null 2>&1 )
}

# --- golden-template gate -----------------------------------------------------
# HONEST SCOPE: this checks the CONFIG — is var.golden_template_name non-empty, so
# the build-VM resources are NOT inert (count 0)? It does NOT, and cannot from the
# control host alone, prove the template was actually built on XCP-2. That deeper
# truth is enforced where it MUST be: `tofu apply` clones the template by name and
# FAILS LOUDLY if it doesn't exist on the pool — and the reconciler degrades on an
# apply failure (keeps last-good, never strands a build). So this gate's job is
# only "would apply create VMs at all" (an operator blanks the name for a
# connect-only plan → off); template existence is the apply's own gate.
#
# Effective value = env/tfvars override if present, else the variables.tf default.
# An override can ADD or BLANK it; we honour both so a deliberate "" → off.
golden_template_set() {
  local val
  # 1) explicit env (TF_VAR_golden_template_name) wins.
  if [ -n "${TF_VAR_golden_template_name+x}" ]; then
    val="${TF_VAR_golden_template_name}"
  # 2) an *.auto.tfvars override (operator-set), if any carries the key. The grep
  #    GATES this branch (key present), so a blank override (`= ""`) → val="" → off,
  #    while a key-absent tfvars falls through to the default.
  elif grep -qhE 'golden_template_name' "$TOFU_DIR"/*.auto.tfvars 2>/dev/null; then
    val="$(grep -hoE 'golden_template_name[[:space:]]*=[[:space:]]*"[^"]*"' \
        "$TOFU_DIR"/*.auto.tfvars 2>/dev/null | sed -E 's/.*"([^"]*)".*/\1/' | tail -1)"
  else
    # 3) the variables.tf default (the conservative floor for a clean checkout).
    # Scope sed to the `variable "golden_template_name" { … }` block so we read ITS
    # default, not some other variable's.
    val="$(sed -nE '/variable "golden_template_name"/,/^}/{ s/.*default[[:space:]]*=[[:space:]]*"([^"]*)".*/\1/p }' \
            "$TOFU_DIR/variables.tf" 2>/dev/null | tail -1)"
  fi
  [ -n "$val" ]
}

# eval_gate — run the three apply prerequisites' probes once and set the globals
# GATE_XO / GATE_STATE / GATE_GOLDEN (each 0/1) + GATE (the apply_gate verdict).
# Each probe is bounded + never throws (a failure is the safe 0 = plan-only). Used
# by both --status (report) and --once (decide), so the probe logic lives in ONE
# place and the two paths can't disagree.
GATE_XO=0; GATE_STATE=0; GATE_GOLDEN=0; GATE=""
eval_gate() {
  GATE_XO=0; GATE_STATE=0; GATE_GOLDEN=0
  if xo_reachable;        then GATE_XO=1;     fi
  if tofu_state_sane;     then GATE_STATE=1;  fi
  if golden_template_set; then GATE_GOLDEN=1; fi
  GATE="$(apply_gate "$APPLY" "$GATE_XO" "$GATE_STATE" "$GATE_GOLDEN")"
}

# =============================================================================
# QUEUE SOURCE — live per-dom0 demand, no AI.
# =============================================================================
# Two signals, summed:
#   (a) worklist @farm:{…} ACTIVE jobs (farm-jobs.sh active) — each command
#       classified BIG/SMALL by classify_command (infer_shape, FA-6).
#   (b) in-flight build SLOTS on each build VM (~/magic-mesh-* dirs) — a live
#       proxy for jobs ALREADY running (so we don't scale a dom0 to zero out from
#       under a build). Best-effort over SSH; an unreachable VM contributes 0 (it
#       can't be hosting a slot if it's unreachable) and NEVER fails the tick.
# We emit whole-fleet totals (big, small, pods) then bucket_demand them per dom0.

# collect_worklist_demand — echo "big small pods" whole-fleet totals from the
# worklist's active @farm jobs. Pods: an @farm command that is NOT a cargo build/
# test (e.g. an agent-pod spawn) counts as a pod; here every @farm job is a cargo
# build/test, so pods stay 0 from this signal (slots may add pods later). Degrades
# to 0/0/0 if the worklist or farm-jobs.sh is missing.
collect_worklist_demand() {
  local big=0 small=0 pods=0
  if [ -x "$FARM_JOBS" ] && [ -f "$WORKLIST" ]; then
    # farm-jobs.sh emits "<jobid>\t<status>\t<task>\t<command>"; we only need the
    # command to classify the job's shape — the rest is discarded with `_`.
    local _jid _status _task cmd shape
    while IFS=$'\t' read -r _jid _status _task cmd; do
      [ -n "$cmd" ] || continue
      shape="$(classify_command "$cmd")"
      case "$shape" in
        big)   big=$(( big + 1 )) ;;
        small) small=$(( small + 1 )) ;;
      esac
    done < <(MCNF_WORKLIST="$WORKLIST" "$FARM_JOBS" active 2>/dev/null || true)
  else
    warn "no farm-jobs.sh / worklist — worklist demand = 0 (slots may still drive)"
  fi
  printf '%s %s %s\n' "$big" "$small" "$pods"
}

# collect_slot_demand — echo "big small pods" from the LIVE in-flight builds on the
# build VMs. We count RUNNING build processes (cargo/rustc/cc1plus/cc1), NOT the
# ~/magic-mesh-* slot DIRS: those dirs persist forever (xcp-build.sh keeps target/
# on the VM permanently and never removes a slot), so a finished job's dir would
# inflate demand for good. A running compiler is a TRUE in-flight signal — it tells
# us a job is on this dom0 RIGHT NOW so the autoscaler must not scale it to zero out
# from under the build. Each VM with a live build counts as 1 in-flight SMALL job on
# its dom0 (a build VM runs one logical job; the per-VM POD/parallelism is the
# autoscaler's small_count concern, not a demand multiplier here).
#   - Probe every IP in the dom0's lane + its legacy fixed host (DOM0_IPS) so we see
#     builds in the elastic AND the degraded/fallback (.52) state.
#   - Best-effort: an unreachable IP costs ~one probe-timeout and contributes 0.
#   - FA_NO_SLOTS skips the probe entirely (offline / --self-test / CI).
collect_slot_demand() {
  local big=0 small=0 pods=0
  if [ -n "${FA_NO_SLOTS:-}" ]; then printf '0 0 0\n'; return; fi
  local dk ip busy
  for dk in "${ORDER[@]}"; do
    for ip in ${DOM0_IPS[$dk]}; do
      # TCP-probe first (bounded) so a down VM costs ~probe-timeout, not the ssh wait.
      timeout "$PROBE_TIMEOUT" bash -c "cat </dev/null >/dev/tcp/$ip/22" 2>/dev/null || continue
      # Is a build compiler running? pgrep exits 0 (match) / 1 (no match) / 2 (err).
      # We only care MATCH vs not — a running cargo/rustc/cc1plus = one live build.
      # $(id -u) is intentionally single-quoted: it must expand on the REMOTE VM
      # (the mm user's uid there), not on the control host.
      # shellcheck disable=SC2016
      if "${SSH[@]}" -n "$BUILD_USER@$ip" \
            'pgrep -x -u "$(id -u)" cargo >/dev/null 2>&1 || pgrep -x rustc >/dev/null 2>&1 || pgrep -x cc1plus >/dev/null 2>&1' \
            >/dev/null 2>&1; then
        busy=1
      else
        busy=0
      fi
      [ "$busy" -eq 1 ] && small=$(( small + 1 ))
    done
  done
  printf '%s %s %s\n' "$big" "$small" "$pods"
}

# collect_demand — sum the worklist + slot signals into the per-dom0 specs, echoed
# as three lines (xen-bigboy, home, xcp1) of "big:small:pods" IN ORDER. Also stashes
# the whole-fleet totals into the FLEET_* globals for the status/log line.
FLEET_BIG=0; FLEET_SMALL=0; FLEET_PODS=0
collect_demand() {
  local wl sl wb ws wp sb ss sp
  wl="$(collect_worklist_demand)"; read -r wb ws wp <<<"$wl"
  sl="$(collect_slot_demand)";     read -r sb ss sp <<<"$sl"
  FLEET_BIG=$(( wb + sb )); FLEET_SMALL=$(( ws + ss )); FLEET_PODS=$(( wp + sp ))
  log "queue: worklist(big=$wb small=$ws pods=$wp) + slots(big=$sb small=$ss pods=$sp)" \
      "→ fleet(big=$FLEET_BIG small=$FLEET_SMALL pods=$FLEET_PODS)"
  bucket_demand "$FLEET_BIG" "$FLEET_SMALL" "$FLEET_PODS"
}

# =============================================================================
# --status — read-only: current committed topology + the live queue it WOULD act
# on + the apply-gate state. Mutates NOTHING (defers to farm-autoscale --topology,
# which is itself read-only). Safe to run anytime / from a panel.
# =============================================================================
if [ "$MODE" = "status" ]; then
  log "FARM-AUTO reconciler status @ $(date -u -d "@$NOW" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u +%Y-%m-%dT%H:%M:%SZ)"

  # Gate state (each prereq + the resulting decision).
  eval_gate
  log "apply-gate: FA_APPLY=$APPLY xo_reachable=$GATE_XO state_sane=$GATE_STATE golden_set=$GATE_GOLDEN → ${GATE%%:*}"
  case "$GATE" in plan-only:*) log "  plan-only reason: ${GATE#plan-only:}";; esac

  # Live queue → per-dom0 specs (best-effort; degrades to 0s if signals are down).
  declare -a SPECS=()
  while IFS= read -r line; do [ -n "$line" ] && SPECS+=("$line"); done < <(collect_demand)

  # Show the autoscaler's committed topology + what THIS queue would decide next.
  if [ -x "$AUTOSCALE" ]; then
    args=(); i=0
    for dk in "${ORDER[@]}"; do args+=("${DOM0_FLAG[$dk]}" "${SPECS[$i]:-0:0:0}"); i=$(( i + 1 )); done
    log "current topology (committed) + drift preview for the live queue:"
    FA_NOW="$NOW" "$AUTOSCALE" "${args[@]}" --topology || warn "autoscale --topology failed (degraded)"
  else
    warn "autoscaler not found at $AUTOSCALE — cannot show topology"
  fi
  exit 0
fi

# =============================================================================
# --once — one reconcile tick.
# =============================================================================
[ "$MODE" = "once" ] || die "internal: unexpected mode '$MODE'"
[ -x "$AUTOSCALE" ] || die "autoscaler not found/executable: $AUTOSCALE"

STAMP="$(date -u -d "@$NOW" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u +%Y-%m-%dT%H:%M:%SZ)"
log "tick $STAMP (dry-run=$DRY_RUN, FA_APPLY=$APPLY)"

# --- 1) QUEUE SOURCE ----------------------------------------------------------
declare -a SPECS=()
while IFS= read -r line; do [ -n "$line" ] && SPECS+=("$line"); done < <(collect_demand)

# --- 2) DECIDE (+ optionally commit) via farm-autoscale.sh --------------------
# Build the per-dom0 flag args once; reuse for the decide call.
AS_ARGS=(); i=0
for dk in "${ORDER[@]}"; do AS_ARGS+=("${DOM0_FLAG[$dk]}" "${SPECS[$i]:-0:0:0}"); i=$(( i + 1 )); done

# Evaluate the apply gate UP FRONT (so a dry-run reports it without committing, and
# the commit path knows whether to attempt apply). XO + state probes are bounded
# and never throw — a probe failure is just a 0 (→ plan-only), the safe direction.
eval_gate
log "apply-gate: FA_APPLY=$APPLY xo_reachable=$GATE_XO state_sane=$GATE_STATE golden_set=$GATE_GOLDEN → ${GATE%%:*}"
case "$GATE" in plan-only:*)
  warn "APPLY GATED — plan-only this tick (reason: ${GATE#plan-only:}). Last-good topology kept; no VM touched."
;; esac

if [ "$DRY_RUN" -eq 1 ]; then
  # Decide-only: farm-autoscale --dry-run mutates nothing (no tfvars, no state, no
  # tofu). This path MUST succeed even with XO down — it proves graceful degrade.
  log "--dry-run: decide-only (autoscaler --dry-run; no commit, no apply, no tofu contact)"
  FA_NOW="$NOW" "$AUTOSCALE" "${AS_ARGS[@]}" --dry-run || warn "autoscale --dry-run reported nonzero (degraded; loop continues)"
  log "--dry-run tick complete — nothing mutated."
  exit 0
fi

# COMMIT the decision: farm-autoscale writes the tfvars + state + runs `tofu plan`.
# The plan reads live XO; with XO DOWN it fails LOUDLY but the tfvars/state are
# already written (last-good advances locally) — so a plan failure DEGRADES the
# tick, it does NOT crash the loop. We capture the rc and carry on.
log "decide+commit: farm-autoscale.sh (writes tofu vars + state, runs tofu plan)"
AS_RC=0
FA_NOW="$NOW" "$AUTOSCALE" "${AS_ARGS[@]}" || AS_RC=$?
if [ "$AS_RC" -ne 0 ]; then
  warn "autoscaler returned rc=$AS_RC (likely XO-unreachable tofu plan) — DEGRADED."
  warn "  tofu vars/state are committed locally; last-good topology kept; loop NOT aborted."
fi

# --- 3) APPLY — GATED --------------------------------------------------------
if [ "$GATE" != "apply" ]; then
  log "apply skipped (${GATE}). Reconcile tick done (plan-only)."
  exit 0
fi

# Re-confirm the gate right before mutating (defence in depth — XO could have
# dropped between the probe and here; never apply against a stale green probe).
if ! xo_reachable; then
  warn "XO went unreachable since the gate check — ABORTING apply (degrade, keep last-good)."
  exit 0
fi
if ! tofu_state_sane; then
  warn "tofu state no longer sane — ABORTING apply (degrade, keep last-good)."
  exit 0
fi

TFVARS="$TOFU_DIR/farm-autoscale.auto.tfvars"
[ -f "$TFVARS" ] || { warn "no $TFVARS to apply (autoscaler did not commit) — skip apply."; exit 0; }

log "APPLY ENABLED — tofu apply the committed shapes ($TFVARS) at the fit-to-headroom burst size (build_memory_gib=$BUILD_MEM_GIB build_vcpus=$BUILD_VCPUS, max_small=$FA_MAX_SMALL/dom0)"
# OVERCOMMIT GUARD (safe hybrid mode): override tofu's default elastic VM size
# (build_memory_gib=16) with a fit-to-headroom size so an elastic burst VM coexists
# with the always-on baseline VM on the same dom0 instead of failing to boot. Paired
# with FA_MAX_SMALL=1 (capped above), a burst is one small fit-sized VM per dom0. The
# same size is already in effect for the autoscaler's `tofu plan` this tick (exported
# TF_VAR_build_memory_gib / TF_VAR_build_vcpus, above) so plan and apply agree; the
# explicit -var here is belt-and-suspenders at the mutation site.
# To go FULL-ELASTIC instead (operator-driven): decommission the always-on baseline
# VMs, then raise FA_BUILD_MEM_GIB/FA_BUILD_VCPUS/FA_MAX_SMALL back to big shapes —
# the always-on + bounded-burst hybrid is the default precisely because it can't
# overcommit a dom0 that still carries its baseline VM.
# -input=false: an unattended tick must never block on a prompt. -auto-approve is
# intentional ONLY behind the full gate (FA_APPLY + XO + state + golden). A failed
# apply degrades — log loudly, keep the loop alive, do NOT strand a running build.
APPLY_RC=0
( cd "$TOFU_DIR" && "$TOFU" apply -input=false -auto-approve \
    -var "build_memory_gib=$BUILD_MEM_GIB" -var "build_vcpus=$BUILD_VCPUS" \
    -var-file="$(basename "$TFVARS")" ) || APPLY_RC=$?
if [ "$APPLY_RC" -ne 0 ]; then
  warn "tofu apply rc=$APPLY_RC — DEGRADED. Last-good topology kept; running builds NOT stranded."
  exit 0
fi
log "tofu apply OK — farm converged to the committed shapes."

# for_each_kept_vm <callback> — iterate the build VMs the autoscaler KEPT this tick
# (from the committed shape vars), calling `<callback> <vm-name> <dom0-host> <vm-ip>`
# for each, IN ORDER. This is the SINGLE source of the dom0/shape + elastic-lane IP
# math: both the build-ready provision step (provision_build_ready) and the inter-job
# reset (reset_running_vms) drive off it, so they can NEVER disagree about which VMs
# exist or what their IPs are. Returns nonzero (caller skips its step) only if the
# shape vars can't be read (dom0_shape unavailable) — a per-VM problem is the
# callback's own concern (degrade per VM, never abort the iteration).
for_each_kept_vm() {
  local cb="$1"
  declare -f dom0_shape >/dev/null 2>&1 || { warn "dom0_shape unavailable — cannot enumerate kept VMs"; return 1; }
  local tfvars_text="" dk shape n i name host
  [ -f "$TFVARS" ] && tfvars_text="$(cat "$TFVARS")"
  for dk in "${ORDER[@]}"; do
    shape="$(dom0_shape "$tfvars_text" "$dk")"
    host="${DOM0_HOST[$dk]}"
    case "$shape" in
      big)
        "$cb" "${DOM0_BIG_NAME[$dk]}" "$host" "${DOM0_IPS[$dk]%% *}"
        ;;
      small)
        # small_count for this dom0 (default 1 if absent) → small-0..small-(n-1).
        n="$(printf '%s\n' "$tfvars_text" | sed -nE "/\"$dk\"[[:space:]]*=[[:space:]]*[0-9]/{s/.*\"$dk\"[[:space:]]*=[[:space:]]*([0-9]+).*/\\1/p;q}")"
        [ -n "$n" ] || n=1
        # The dom0's elastic lane IPs in small-index order (ip_base, +10, +20, +30);
        # the trailing legacy host (.52/.51) is NOT a small index, so read only the
        # first MAX_SMALL_INDEX lane tokens by position. Clamp n to that lane width so
        # a stale/hand-edited tfvars with small_count > 4 can't index past the elastic
        # IPs into the legacy host (the live autoscaler caps at 4, but be defensive).
        local -a lane; read -ra lane <<<"${DOM0_IPS[$dk]}"
        local max_small_idx=4
        [ "$n" -gt "$max_small_idx" ] && { warn "small_count=$n on $dk exceeds the $max_small_idx-wide lane — clamping to $max_small_idx"; n="$max_small_idx"; }
        i=0
        while [ "$i" -lt "$n" ]; do
          if [ "$i" -eq 0 ]; then name="${DOM0_SMALL_NAME[$dk]}"; else name="${DOM0_SMALL_NAME[$dk]}-$i"; fi
          "$cb" "$name" "$host" "${lane[$i]:-}"
          i=$(( i + 1 ))
        done
        ;;
      off) : ;; # nothing running on this dom0 — nothing to enumerate
    esac
  done
}

# --- 4) BUILD-READINESS: toolchain-on-first-provision + snapshot baseline ------
# MDE-VM-golden is a BASE template (no Rust toolchain) — a fresh clone CANNOT build.
# For each VM the autoscaler KEPT this tick, make it build-ready ONCE: if it already
# carries a `clean` baseline snapshot (taken only post-toolchain) it's ready → SKIP;
# else install the toolchain (infra/ansible/build-vm-toolchain.yml) and take the
# `clean` snapshot so every future tick's inter-job reset reverts to a TOOLCHAINED
# baseline. Per-VM isolated + degrade-don't-crash: ansible missing / unreachable VM
# / playbook or snapshot failure → WARN + skip that VM (next tick retries), NEVER
# abort the tick or strand a build. Runs ONLY on the gated apply path — provision_
# enabled "$GATE" makes plan-only/--dry-run a hard no-op (no ansible, no snapshot).
provision_build_ready() {
  local gate="$1"
  provision_enabled "$gate" || { log "provision_build_ready: skipped (${gate}) — no toolchain/snapshot on a non-apply tick"; return 0; }
  if ! command -v ansible-playbook >/dev/null 2>&1; then
    warn "ansible-playbook not found — cannot toolchain fresh build VMs this tick (they'll lack cargo/rustc); next tick retries"
    return 0
  fi
  [ -r "$TOOLCHAIN_PLAYBOOK" ] || { warn "toolchain playbook missing/unreadable ($TOOLCHAIN_PLAYBOOK) — skipping build-ready provision"; return 0; }
  [ -x "$SNAPSHOT" ] || { warn "farm-vm-snapshot.sh not found at $SNAPSHOT — cannot baseline a toolchained VM; skipping build-ready provision"; return 0; }
  for_each_kept_vm provision_one_vm || warn "could not enumerate kept VMs — build-ready provision skipped this tick"
}

# provision_one_vm <vm-name> <dom0-host> <vm-ip-or-empty> — make ONE kept VM build-
# ready. SKIP if it already has a `clean` snapshot (toolchain already baked, paid
# once). Else: toolchain via ansible (a temp inventory pinned to this VM's IP under
# the build_vms group the playbook targets), then take the `clean` baseline. Every
# failure WARNs + returns 0 — the next tick retries.
# SC2317: a for_each_kept_vm callback (invoked via "$cb") — indirect, so shellcheck
# can't see the call site.
# shellcheck disable=SC2317
provision_one_vm() {
  local name="$1" host="$2" ip="$3" probe_rc ready
  # READINESS — reuse farm-vm-snapshot.sh's `has-clean` (its resolve_vm + clean_snapshots,
  # which OWN the `clean` name-label) so this check can NEVER drift from what the
  # inter-job reset actually reverts to. Exit 0 = has a clean snapshot. Any dom0/ssh
  # failure → nonzero → treated as not-ready (the SAFE direction: re-toolchain, which
  # is idempotent, rather than skip a possibly-untoolchained VM). `|| probe_rc=$?`
  # keeps the nonzero out of `set -e`.
  probe_rc=0
  MCNF_XCP_HOST="$host" MCNF_FARM_KEY="$KEY" "$SNAPSHOT" has-clean "$name" --xcp-host "$host" >/dev/null 2>&1 || probe_rc=$?
  ready="$(vm_is_build_ready "$probe_rc")"
  if [ "$ready" = "ready" ]; then
    log "  $name already build-ready (has a clean snapshot) — skip toolchain (paid once)"
    return 0
  fi
  if [ -z "$ip" ]; then
    warn "  $name has no clean snapshot but no known IP — cannot toolchain; skipped (next tick retries)"
    return 0
  fi
  if ! timeout "$PROBE_TIMEOUT" bash -c "cat </dev/null >/dev/tcp/$ip/22" 2>/dev/null; then
    warn "  $name ($ip) unreachable on :22 — cannot toolchain; skipped (next tick retries)"
    return 0
  fi
  log "  $name ($ip) not build-ready → installing toolchain (ansible build-vm-toolchain.yml)"
  # The playbook is `hosts: build_vms`, so the target MUST be in that group — an
  # inline `-i host,` puts it in `ungrouped` (play matches nothing, exits 0, and we'd
  # snapshot an UN-toolchained VM). Write a one-host inventory FILE under [build_vms]
  # with the connection vars (inventory.ini doesn't list elastically-cloned IPs).
  local inv_file; inv_file="$(mktemp "${TMPDIR:-/tmp}/mcnf-fa-inv.XXXXXX")" || {
    warn "  could not create a temp inventory for $name — skipped (next tick retries)"; return 0; }
  { printf '[build_vms]\n'; toolchain_inventory_line "$ip" "$BUILD_USER" "$KEY"; } >"$inv_file"
  local pb_rc=0
  ANSIBLE_HOST_KEY_CHECKING=False ansible-playbook -i "$inv_file" "$TOOLCHAIN_PLAYBOOK" >/dev/null 2>&1 || pb_rc=$?
  rm -f "$inv_file"
  if [ "$pb_rc" -ne 0 ]; then
    warn "  toolchain playbook FAILED on $name ($ip) — skipped, NOT snapshotting a half-provisioned VM; next tick retries"
    return 0
  fi
  log "  toolchain installed on $name → taking the clean baseline snapshot"
  if ! MCNF_XCP_HOST="$host" MCNF_FARM_KEY="$KEY" "$SNAPSHOT" snapshot "$name" --xcp-host "$host" >/dev/null 2>&1; then
    warn "  baseline snapshot of $name FAILED — toolchain is installed but no clean point yet; next tick re-checks"
    return 0
  fi
  log "  $name is now build-ready (toolchained + clean baseline snapshot taken)"
}

# --- 5) inter-job snapshot-revert (L3) ---------------------------------------
# Between jobs, revert each freshly-converged build VM to its clean post-toolchain
# baseline (FA-2 / farm-vm-snapshot.sh reset) so job N+1 doesn't inherit job N's
# workspace/sccache state. We reset ONLY the VMs the autoscaler decided to KEEP this
# tick (for_each_kept_vm) — and skip any VM that is currently BUSY (a live build
# process), so a mid-flight job is never reverted out from under itself. Each reset
# is per-VM isolated + best effort: a missing snapshot / unreachable dom0 is warned
# and skipped, never failing the tick (the apply already converged; reset is the
# inter-job optimisation, and provision_build_ready already created the baseline).
reset_running_vms() {
  [ -x "$SNAPSHOT" ] || { warn "farm-vm-snapshot.sh not found at $SNAPSHOT — skipping inter-job reset"; return 0; }
  for_each_kept_vm reset_one_vm || warn "could not enumerate kept VMs — inter-job reset skipped this tick"
}

# reset_one_vm <vm-name> <dom0-host> <vm-ip-or-empty> — reset ONE VM to its clean
# snapshot, UNLESS it's mid-build (a live compiler on <vm-ip>, when known). Best
# effort: any failure is warned and swallowed (never aborts the tick).
# SC2317: a for_each_kept_vm callback (invoked via "$cb") — indirect call site.
# shellcheck disable=SC2317
reset_one_vm() {
  local name="$1" host="$2" ip="$3"
  if [ -n "$ip" ]; then
    if timeout "$PROBE_TIMEOUT" bash -c "cat </dev/null >/dev/tcp/$ip/22" 2>/dev/null \
       && "${SSH[@]}" -n "$BUILD_USER@$ip" \
            'pgrep -x cargo >/dev/null 2>&1 || pgrep -x rustc >/dev/null 2>&1 || pgrep -x cc1plus >/dev/null 2>&1' \
            >/dev/null 2>&1; then
      log "  skip reset of $name — a build is in flight (won't revert under a live job)"
      return 0
    fi
  fi
  log "  reset $name → clean baseline (dom0 $host)"
  if ! MCNF_XCP_HOST="$host" MCNF_FARM_KEY="$KEY" "$SNAPSHOT" reset "$name" --xcp-host "$host" >/dev/null 2>&1; then
    warn "  reset of $name failed/absent (no clean snapshot? mid-provision?) — skipped, tick continues"
  fi
}

log "build-readiness: toolchain-on-first-provision + clean-baseline snapshot for kept VMs"
# `|| warn` is belt-and-suspenders: provision_build_ready already returns 0 on every
# path, but the guard guarantees a future edit can't let a stray nonzero crash the
# tick out from under a running build (the apply already converged).
provision_build_ready "$GATE" || warn "build-ready provision step degraded (rc=$?) — tick continues; next tick retries"

log "inter-job snapshot-revert of the converged build VMs to their clean baseline (L3)"
reset_running_vms

log "reconcile tick complete (applied)."
exit 0
