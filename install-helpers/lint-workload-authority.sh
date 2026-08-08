#!/usr/bin/env bash
set -euo pipefail

# WL-ARCH-010: the desktop shell may only emit typed Workload intent. The
# guard scans the production shell plus daemon registration/dispatch surfaces;
# test fixtures are excluded so they can pin fail-closed refusal behavior.
#
# Migration map (kept here beside the executable guard):
#   action/vm/lifecycle       -> action/workload/operation
#   action/container/lifecycle -> action/workload/operation
#   VmPowerRequest            -> WorkloadOperationRequest
# The spawned production actuator is workload_compute, and its typed projection
# is state/workloads/<node>. Raw console endpoint publication is retired.
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
shell_root="$repo_root/crates/desktop/mde-shell-egui/src"
spawn_file="$repo_root/crates/mesh/mackesd/src/bin/mackesd/spawn.rs"
workers_mod="$repo_root/crates/mesh/mackesd/src/workers/mod.rs"
cloud_verbs="$repo_root/crates/mesh/mackesd/src/workers/cloud/verbs.rs"
compute_migrate="$repo_root/crates/mesh/mackesd/src/workers/compute_migrate.rs"
workload_compute="$repo_root/crates/mesh/mackesd/src/workers/workload_compute.rs"
inventory="$repo_root/docs/platform/workload-authority-inventory.md"
retired_console_worker="$repo_root/crates/mesh/mackesd/src/workers/console_broker.rs"
retired_cloud_console="$repo_root/crates/mesh/mackesd/src/workers/cloud/verbs/console.rs"
retired_attach_schema="$repo_root/packaging/browser-vm/browser-vm-transport-attach.schema.json"
retired_attach_example="$repo_root/packaging/browser-vm/browser-vm-transport-attach.example.json"
retired_attach_verifier="$repo_root/packaging/browser-vm/verify-transport-attach.sh"
live_mirror_verifier="$repo_root/install-helpers/verify-live-mirrors.py"

scan_shell() {
  local source_root="$1"
  if command -v rg >/dev/null 2>&1; then
    rg -n --glob '*.rs' --glob '!**/tests.rs' \
      'action/(vm/lifecycle|container/lifecycle)|state/vdi/console|console-attach|console_broker|LIFECYCLE_TOPIC|VmPowerRequest' \
      "$source_root"
  else
    grep -RInE --include='*.rs' --exclude='tests.rs' \
      'action/(vm/lifecycle|container/lifecycle)|state/vdi/console|console-attach|console_broker|LIFECYCLE_TOPIC|VmPowerRequest' \
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

production_contains_literal() {
  local needle="$1" file="$2"
  awk '/#\[cfg\(test\)\]/{exit} {print}' "$file" | grep -Fq -- "$needle"
}

has_retired_spawn() {
  # Keep this check multiline-aware without making the farm image depend on
  # ripgrep or PCRE: an actuator call ends at the first `});` after its start.
  awk '
    /^[[:space:]]*spawn_tiered\(/ { in_call=1; call=$0; next }
    in_call {
      call = call "\n" $0
      if ($0 ~ /\}\);[[:space:]]*$/) {
        if (call ~ /"(vm_lifecycle|container|console_broker)"/) { print call; found=1 }
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
  for retired in 'action/container/lifecycle' 'state/vdi/console' 'console-attach' 'console_broker' 'VmPowerRequest'; do
    printf 'const RETIRED: &str = "%s";\n' "$retired" >"$fixture/src/legacy.rs"
    if ! scan_shell "$fixture/src" >/dev/null 2>&1; then
      printf 'lint-workload-authority.sh: self-test failed — retired fixture was not detected: %s\n' "$retired" >&2
      return 1
    fi
  done
  cat >"$fixture/spawn.rs" <<'EOF'
spawn_tiered(
    sup,
    names,
    rank,
    "console_broker",
    || Retired::new(),
});
EOF
  if ! has_retired_spawn "$fixture/spawn.rs" >/dev/null; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — retired spawn fixture was not detected' >&2
    return 1
  fi
  printf '%s\n' 'let mut command = Command::new("virsh");' >"$fixture/compute_migrate.rs"
  if ! production_contains_literal 'Command::new("virsh")' "$fixture/compute_migrate.rs"; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — direct migration actuator fixture was not detected' >&2
    return 1
  fi
  printf '%s\n' 'lint-workload-authority.sh: self-test passed — lifecycle and presentation guards are fail-closed'
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
[ -f "$workers_mod" ] && [ -f "$cloud_verbs" ] && [ -f "$compute_migrate" ] \
  && [ -f "$workload_compute" ] && [ -f "$inventory" ] || {
  printf '%s\n' 'lint-workload-authority.sh: authority inventory or daemon registration surfaces are missing' >&2
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

if contains_literal 'pub mod console_broker' "$workers_mod" \
  || contains_literal 'mod console;' "$cloud_verbs" \
  || contains_literal 'ConsoleAttach' "$cloud_verbs"; then
  printf '%s\n' 'lint-workload-authority.sh: retired console authority is registered or dispatched' >&2
  exit 1
fi

if contains_literal 'state/vdi/console' "$live_mirror_verifier"; then
  printf '%s\n' 'lint-workload-authority.sh: live verifier still reads the retired console projection' >&2
  exit 1
fi

for retired_file in \
  "$retired_console_worker" \
  "$retired_cloud_console" \
  "$retired_attach_schema" \
  "$retired_attach_example" \
  "$retired_attach_verifier"; do
  if [ -e "$retired_file" ]; then
    printf 'lint-workload-authority.sh: retired authority artifact is reachable: %s\n' "$retired_file" >&2
    exit 1
  fi
done

if has_retired_spawn "$spawn_file"; then
  printf '%s\n' 'lint-workload-authority.sh: retired VM/container actuator is still spawned' >&2
  exit 1
fi

if production_contains_literal 'Command::new("virsh")' "$compute_migrate" \
  || production_contains_literal 'SystemWorkloadActuator' "$compute_migrate"; then
  printf '%s\n' 'lint-workload-authority.sh: compute_migrate directly owns a libvirt actuator' >&2
  exit 1
fi
if ! contains_literal 'WorkloadMigrationClient' "$compute_migrate" \
  || ! contains_literal 'drain_migration_commands' "$workload_compute"; then
  printf '%s\n' 'lint-workload-authority.sh: migration commands do not terminate at the Workload reconciler' >&2
  exit 1
fi

printf '%s\n' 'lint-workload-authority.sh: clean — one typed Workload actuator/projection; retired lifecycle and console paths absent'
