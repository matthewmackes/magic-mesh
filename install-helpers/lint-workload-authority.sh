#!/usr/bin/env bash
set -euo pipefail

# WL-ARCH-010: the desktop shell may only emit typed Workload intent. The
# daemon keeps retired topic readers temporarily for migration evidence, so the
# guard is intentionally scoped to production shell sources and excludes tests.
#
# Migration map (kept here beside the executable guard):
#   action/vm/lifecycle       -> action/workload/operation
#   action/container/lifecycle -> action/workload/operation
#   VmPowerRequest            -> WorkloadOperationRequest
# Retained daemon readers and the live-proof verifier are evidence-only during
# migration; they are not allowed to become new shell publishers. The spawned
# production actuator is workload_compute, and its typed projection is
# state/workloads/<node>.
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
shell_root="$repo_root/crates/desktop/mde-shell-egui/src"
spawn_file="$repo_root/crates/mesh/mackesd/src/bin/mackesd/spawn.rs"

scan_shell() {
  local source_root="$1"
  if command -v rg >/dev/null 2>&1; then
    rg -n --glob '*.rs' --glob '!**/tests.rs' \
      'action/(vm/lifecycle|container/lifecycle)|LIFECYCLE_TOPIC|VmPowerRequest' \
      "$source_root"
  else
    grep -RInE --include='*.rs' --exclude='tests.rs' \
      'action/(vm/lifecycle|container/lifecycle)|LIFECYCLE_TOPIC|VmPowerRequest' \
      "$source_root"
  fi
}

contains_literal() {
  local needle="$1" file="$2"
  if command -v rg >/dev/null 2>&1; then
    rg -Fq -- "$needle" "$file"
  else
    grep -Fq -- "$needle" "$file"
  fi
}

has_retired_spawn() {
  # Keep this check multiline-aware without making the farm image depend on
  # ripgrep or PCRE: an actuator call ends at the first `});` after its start.
  awk '
    /^[[:space:]]*spawn_tiered\(/ { in_call=1; call=$0; next }
    in_call {
      call = call "\n" $0
      if ($0 ~ /\}\);[[:space:]]*$/) {
        if (call ~ /"(vm_lifecycle|container)"/) { print call; found=1 }
        in_call=0
        call=""
      }
    }
    END { exit(found ? 0 : 1) }
  ' "$1"
}

run_self_test() {
  local fixture
  fixture="$(mktemp -d "${TMPDIR:-/tmp}/lint-workload-authority.XXXXXX")"
  trap 'rm -rf -- "$fixture"' RETURN
  mkdir -p "$fixture/src"
  printf '%s\n' 'const RETIRED = "action/vm/lifecycle";' >"$fixture/src/legacy.rs"
  if ! scan_shell "$fixture/src" >/dev/null 2>&1; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — legacy publisher fixture was not detected' >&2
    return 1
  fi
  mkdir -p "$fixture/clean"
  printf '%s\n' 'publish(WorkloadOperationRequest::default());' >"$fixture/clean/typed.rs"
  if scan_shell "$fixture/clean" >/dev/null 2>&1; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — typed fixture was rejected' >&2
    return 1
  fi
  printf '%s\n' 'lint-workload-authority.sh: self-test passed — legacy publisher guard is fail-closed'
}

if [[ "${1:-}" == "--self-test" ]]; then
  run_self_test
  exit $?
fi

[ -d "$shell_root" ] || {
  printf 'lint-workload-authority.sh: missing shell source root: %s\n' "$shell_root" >&2
  exit 1
}
[ -f "$spawn_file" ] || {
  printf 'lint-workload-authority.sh: missing canonical spawn file: %s\n' "$spawn_file" >&2
  exit 1
}

if scan_shell "$shell_root"; then
  printf '%s\n' 'lint-workload-authority.sh: legacy lifecycle publisher found in shell' >&2
  exit 1
fi

if ! contains_literal 'spawn_tiered(sup, worker_names, role_rank, "workload_compute", ||' "$spawn_file" \
  || ! contains_literal 'workload_compute::WorkloadComputeWorker::new' "$spawn_file"; then
  printf '%s\n' 'lint-workload-authority.sh: canonical workload_compute actuator is not spawned' >&2
  exit 1
fi

if has_retired_spawn "$spawn_file"; then
  printf '%s\n' 'lint-workload-authority.sh: retired VM/container actuator is still spawned' >&2
  exit 1
fi

printf '%s\n' 'lint-workload-authority.sh: clean — shell has one typed Workload authority and mackesd has one spawned actuator'
