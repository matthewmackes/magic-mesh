#!/usr/bin/env bash
# OpenStack live create→verify→delete lane (WL-TEST-001).
#
# Runs the resource-MUTATING `#[ignore]` integration test
# `openstack::client::contract::live_openstack_create_verify_delete` against a
# real OpenStack cloud. The test authenticates via a clouds.yaml, creates a tiny
# throwaway Heat stack (a single in-Heat `OS::Heat::RandomString` — no Nova/
# Neutron/Cinder infrastructure), polls it to `CREATE_COMPLETE`, then deletes it.
# Cleanup is guaranteed even on an assertion failure by a Drop-guard in the test,
# so a panic mid-verify still deletes the stack.
#
# The test is `#[ignore]` and stays that way until a farm OpenStack endpoint +
# throwaway-project quota exists (see docs/ops/openstack-live-test.md). This lane
# is the runner for that day; it REFUSES to run until the endpoint env is set, so
# it can be wired into the farm now without mutating anything.
set -euo pipefail

usage() {
  cat <<'USAGE'
openstack-live-test.sh — run the OpenStack create→verify→delete live test (WL-TEST-001)

Usage:
  MDE_OPENSTACK_LIVE_TARGET=/etc/openstack/clouds.yaml \
  MDE_OPENSTACK_LIVE_MUTATE=1 \
    install-helpers/openstack-live-test.sh

Required environment (both — the lane refuses to run without them):
  MDE_OPENSTACK_LIVE_TARGET   Path to the clouds.yaml to authenticate with. This
                              is exported to the test as OS_CLIENT_CONFIG_FILE.
                              Point it at a THROWAWAY project — the test creates
                              and deletes a real (if trivial) Heat stack.
  MDE_OPENSTACK_LIVE_MUTATE   Must be exactly "1". The explicit opt-in that this
                              run may create+delete real cloud resources. Without
                              it the test SKIPs even when the target is set — so a
                              stray `--ignored` run can never mutate a cloud.

Optional environment:
  OS_CLOUD                    Selects the clouds.yaml context when the file holds
                              more than one (passed through to the test).
  MCNF_BUILD_HOST             Farm build node IP for xcp-build.sh (default: the
                              slot xcp-build.sh resolves). The workspace build is
                              farm-only; local cargo is a disabled shim.

What it creates / deletes:
  A single Heat stack named `mde-livetest-<nanos>` containing one
  `OS::Heat::RandomString` resource. No compute/network/volume resources are
  allocated. The stack is deleted before the test returns; cleanup also runs on
  an assertion failure (Drop-guard), and a leaked RandomString stack costs
  nothing even in the worst case.

Exit status: the underlying `cargo test` exit status (0 = pass/skip).
USAGE
}

log() { printf 'openstack-live-test: %s\n' "$*"; }
die() {
  printf 'openstack-live-test: %s\n' "$*" >&2
  exit 1
}

case "${1:-}" in
  -h | --help)
    usage
    exit 0
    ;;
  "") ;;
  *) die "unexpected argument: $1 (see --help)" ;;
esac

# Locate the repo root from this script's own path so the WORKTREE copy of
# xcp-build.sh (not the main repo's) is used — see the xcp-build worktree note.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── the endpoint gate — refuse to run until a real cloud is configured ────────
: "${MDE_OPENSTACK_LIVE_TARGET:=}"
if [[ -z "$MDE_OPENSTACK_LIVE_TARGET" ]]; then
  die "MDE_OPENSTACK_LIVE_TARGET is unset — this lane needs a clouds.yaml path.
       Live OpenStack execution is GATED on a farm endpoint + throwaway quota
       that does not yet exist; see docs/ops/openstack-live-test.md. Not running."
fi
if [[ ! -f "$MDE_OPENSTACK_LIVE_TARGET" ]]; then
  die "MDE_OPENSTACK_LIVE_TARGET=$MDE_OPENSTACK_LIVE_TARGET is not a file."
fi
if [[ "${MDE_OPENSTACK_LIVE_MUTATE:-}" != "1" ]]; then
  die "MDE_OPENSTACK_LIVE_MUTATE must be exactly 1 (explicit create+delete opt-in).
       Refusing to mutate a live cloud without it."
fi

# The test resolves clouds.yaml via OS_CLIENT_CONFIG_FILE (openstacksdk standard).
export OS_CLIENT_CONFIG_FILE="$MDE_OPENSTACK_LIVE_TARGET"

log "target clouds.yaml : $MDE_OPENSTACK_LIVE_TARGET"
log "OS_CLOUD           : ${OS_CLOUD:-<single default context>}"
log "mutate opt-in      : yes (MDE_OPENSTACK_LIVE_MUTATE=1)"
log "running create→verify→delete live test (cleanup guaranteed on failure)…"

# Farm-only build+run: the workspace build is a disabled shim locally, so the
# ignored test must run through the farm builder. --ignored promotes it; a single
# test thread keeps the live round-trip serial and its --nocapture log readable.
cd "$REPO_ROOT"
exec ./install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  openstack::client::contract::live_openstack_create_verify_delete -- \
  --ignored --nocapture --test-threads=1
