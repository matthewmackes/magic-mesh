#!/usr/bin/env bash
# WL-TEST-002 — crown-jewel LIVE etcd/Nebula harness runner.
#
# Runs the env-gated `#[ignore]` integration probe
# `live_fleet_etcd_quorum_and_nebula_overlay`
# (crates/mesh/mackesd/tests/live_fleet.rs) against the RUNNING fleet — the
# worklist's first harness target, the existing live lighthouses — and captures
# the surrounding substrate state to a timestamped artifact dir so a failure has
# forensic evidence (etcd member list + health, the Nebula overlay directory,
# the full test log).
#
# The offline docker-tests suites (tests/substrate_etcd.rs,
# tests/integration_testcontainers.rs) spin their OWN disposable containers. This
# lane is the LIVE half: it points the production mackesd_core substrate client
# at a real etcd + a real lighthouse. The live RUN is operator/infra-gated — this
# script REFUSES to run until the fleet coordinates are in the environment, so it
# can be wired into the farm now without touching anything.
#
# ── the false-green guard (why this does NOT use `xcp-build.sh cargo`) ────────
# `install-helpers/xcp-build.sh`'s remote() runs `cargo …` over SSH with no
# `SendEnv`/inline env, so MCNF_LIVE_ETCD / MCNF_LIVE_LH would be STRIPPED before
# the remote cargo process — the test would then self-skip and cargo would exit
# 0: a green run that never touched the fleet. (A farm build VM is not on the
# overlay anyway, so it could not reach etcd/LH even if the env survived.) This
# lane therefore runs cargo DIRECTLY (env in-process) and, belt-and-braces,
# asserts the test's `CROWN-JEWEL-LIVE:` sentinel proves it actually RAN: if the
# output shows `SKIP` (env didn't reach the process) or lacks `PASS`, the lane
# fails loud instead of reporting a false green.
set -euo pipefail

usage() {
  cat <<'USAGE'
crown-jewel-live.sh — run the live etcd/Nebula crown-jewel probe (WL-TEST-002)

Usage:
  MCNF_LIVE_ETCD=http://10.42.0.1:2379,http://10.42.0.2:2379 \
  MCNF_LIVE_LH=10.42.0.1:4242 \
    automation/testing/crown-jewel-live.sh

Required environment (the lane refuses to run without both):
  MCNF_LIVE_ETCD   Comma/space/newline list of etcd client endpoints on the
                   overlay (e.g. http://10.42.0.1:2379). Read-only probe.
  MCNF_LIVE_LH     Lighthouse Nebula underlay address host[:port] (port
                   defaults to 4242), e.g. 10.42.0.1:4242.

Optional environment:
  MCNF_CJ_ARTIFACT_DIR  Parent dir for the timestamped artifact folder
                        (default: $HOME/mcnf-crown-jewel-artifacts).
  MCNF_CJ_CARGO         cargo invocation to use (default: cargo). Must run
                        LOCALLY so the env reaches the test — do NOT set this to
                        an SSH/xcp-build wrapper (its remote() strips the env →
                        a false green the sentinel guard will reject anyway).
  ETCDCTL               etcdctl binary for the read-only member/health capture
                        (default: etcdctl; skipped with a note if absent).

Run host: a MESH MEMBER (lighthouse/seat/control node) with real cargo and
overlay reachability to the etcd endpoints + lighthouse. NOT a farm build VM.

Exit status: 0 only when the probe RAN and PASSed; non-zero on a missing env, a
self-skip (env didn't reach the test), or any probe failure. Artifacts are
captured on every path.
USAGE
}

case "${1:-}" in
  -h | --help)
    usage
    exit 0
    ;;
  "") ;;
  *)
    printf 'crown-jewel-live: unexpected argument: %s (see --help)\n' "$1" >&2
    exit 1
    ;;
esac

log() { printf 'crown-jewel-live: %s\n' "$*"; }
die() {
  printf 'crown-jewel-live: %s\n' "$*" >&2
  exit 1
}

# Locate the repo root from THIS script's own path so the WORKTREE copy of the
# repo is used (the xcp-build worktree lesson).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# ── the fleet gate — refuse to run until the coordinates are configured ───────
: "${MCNF_LIVE_ETCD:=}"
: "${MCNF_LIVE_LH:=}"
if [[ -z "$MCNF_LIVE_ETCD" ]]; then
  die "MCNF_LIVE_ETCD is unset — this lane needs the fleet's etcd endpoint(s).
       The live crown-jewel RUN is operator/infra-gated (see
       docs/ops/crown-jewel-live-test.md). Not running."
fi
if [[ -z "$MCNF_LIVE_LH" ]]; then
  die "MCNF_LIVE_LH is unset — this lane needs the lighthouse Nebula underlay
       address (host[:port]). See docs/ops/crown-jewel-live-test.md. Not running."
fi

# ── artifact dir (timestamped, UTC) ───────────────────────────────────────────
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
ART_PARENT="${MCNF_CJ_ARTIFACT_DIR:-$HOME/mcnf-crown-jewel-artifacts}"
ART_DIR="$ART_PARENT/crown-jewel-live-$STAMP"
mkdir -p "$ART_DIR"

CARGO="${MCNF_CJ_CARGO:-cargo}"
ETCDCTL="${ETCDCTL:-etcdctl}"
# etcdctl wants a single-value --endpoints; take the first of the list.
ETCD_FIRST="${MCNF_LIVE_ETCD%%[, ]*}"

log "artifact dir : $ART_DIR"
log "etcd targets : $MCNF_LIVE_ETCD"
log "lighthouse   : $MCNF_LIVE_LH"
log "cargo runner : $CARGO (LOCAL — env must reach the test in-process)"

# Record the run context up front (part of the forensic bundle).
{
  echo "crown-jewel-live artifact bundle"
  echo "stamp_utc    : $STAMP"
  echo "host         : $(hostname 2>/dev/null || echo unknown)"
  echo "repo_root    : $REPO_ROOT"
  echo "MCNF_LIVE_ETCD: $MCNF_LIVE_ETCD"
  echo "MCNF_LIVE_LH : $MCNF_LIVE_LH"
  echo "cargo        : $CARGO"
} >"$ART_DIR/context.txt"

# best-effort capture: run a command into a file; never abort the lane.
capture() {
  local out="$1"
  shift
  if ! command -v "$1" >/dev/null 2>&1; then
    printf '[skip] %s not found on PATH\n' "$1" >"$out"
    log "  capture: $1 not found — noted in $(basename "$out")"
    return 0
  fi
  if "$@" >"$out" 2>&1; then
    log "  capture: $(basename "$out") ok"
  else
    printf '\n[non-zero exit — captured anyway]\n' >>"$out"
    log "  capture: $(basename "$out") non-zero (captured)"
  fi
}

# ── pre-run substrate snapshot (read-only) ────────────────────────────────────
log "capturing substrate state …"
capture "$ART_DIR/etcd-member-list.txt" \
  "$ETCDCTL" --endpoints="$MCNF_LIVE_ETCD" member list -w table
capture "$ART_DIR/etcd-endpoint-health.txt" \
  "$ETCDCTL" --endpoints="$MCNF_LIVE_ETCD" endpoint health
capture "$ART_DIR/etcd-endpoint-status.txt" \
  "$ETCDCTL" --endpoints="$MCNF_LIVE_ETCD" endpoint status -w table
# the live peer/overlay directory straight from etcd (the Nebula membership view)
capture "$ART_DIR/mesh-peers-directory.txt" \
  "$ETCDCTL" --endpoints="$ETCD_FIRST" get --prefix /mesh/peers/
# local Nebula overlay interface, if this host is itself on the mesh
capture "$ART_DIR/nebula-iface.txt" \
  ip -4 addr show nebula1

# ── run the #[ignore] probe (env in-process) ──────────────────────────────────
log "running the live probe (cargo test --test live_fleet -- --ignored) …"
TEST_LOG="$ART_DIR/test-output.txt"
set +e
(
  cd "$REPO_ROOT"
  MCNF_LIVE_ETCD="$MCNF_LIVE_ETCD" MCNF_LIVE_LH="$MCNF_LIVE_LH" \
    $CARGO test -p mackesd --test live_fleet -- \
    --ignored --nocapture --test-threads=1
) 2>&1 | tee "$TEST_LOG"
TEST_RC="${PIPESTATUS[0]}"
set -e

# ── the false-green guard — the sentinel must prove it RAN ────────────────────
# The test prints CROWN-JEWEL-LIVE: SKIP only when the env did not reach it, and
# CROWN-JEWEL-LIVE: PASS only on a completed, green live run. If the env were
# stripped (the xcp-build remote() trap) we would see SKIP + a 0 exit — a false
# green. Reject that explicitly.
if grep -q "CROWN-JEWEL-LIVE: SKIP" "$TEST_LOG"; then
  die "FALSE-GREEN GUARD TRIPPED — the test SELF-SKIPPED: MCNF_LIVE_ETCD/MCNF_LIVE_LH
       did not reach the cargo test process even though this lane required them.
       This is the xcp-build.sh remote() SSH-env-not-forwarded trap — run cargo
       LOCALLY (do not set MCNF_CJ_CARGO to an SSH/xcp-build wrapper). Artifacts: $ART_DIR"
fi
if [[ "$TEST_RC" -ne 0 ]]; then
  die "live probe FAILED (cargo test exit $TEST_RC) — the fleet is set but a
       substrate assertion did not hold. See $TEST_LOG (and etcd-*/mesh-peers
       captures) in $ART_DIR"
fi
if ! grep -q "CROWN-JEWEL-LIVE: PASS" "$TEST_LOG"; then
  die "live probe produced exit 0 but NO 'CROWN-JEWEL-LIVE: PASS' line — the probe
       did not run to completion (filtered out? --ignored not honored?). Treating
       as a false green. See $TEST_LOG in $ART_DIR"
fi

log "PASS — live crown-jewel probe ran green. Artifacts: $ART_DIR"
