#!/usr/bin/env bash
set -euo pipefail

# WL-ARCH-010: the desktop shell may only emit typed Workload intent. The
# guard scans the production shell plus daemon registration/dispatch surfaces;
# test fixtures are excluded so they can pin fail-closed refusal behavior.
#
# Migration map (kept here beside the executable guard):
#   action/vm/lifecycle       -> action/workload/operation
#   action/container/lifecycle -> action/workload/operation
#   action/cloud/instance-*    -> action/workload/operation
#   VmPowerRequest            -> WorkloadOperationRequest
# The spawned production actuator is workload_compute, and its typed projection
# is state/workloads/<node>. Raw console endpoint publication is retired.
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
workspace_manifest="$repo_root/Cargo.toml"
mackesd_manifest="$repo_root/crates/mesh/mackesd/Cargo.toml"
shell_root="$repo_root/crates/desktop/mde-shell-egui/src"
phones_hub="$shell_root/phones_hub.rs"
spawn_file="$repo_root/crates/mesh/mackesd/src/bin/mackesd/spawn.rs"
workers_mod="$repo_root/crates/mesh/mackesd/src/workers/mod.rs"
worker_role="$repo_root/crates/mesh/mackesd/src/worker_role.rs"
cloud_verbs="$repo_root/crates/mesh/mackesd/src/workers/cloud/verbs.rs"
cloud_runner="$repo_root/crates/mesh/mackesd/src/workers/cloud/runner.rs"
cloud_android_lifecycle="$repo_root/crates/mesh/mackesd/src/workers/cloud/verbs/android_lifecycle.rs"
cloud_cuttlefish="$repo_root/crates/mesh/mackesd/src/workers/cloud/verbs/cuttlefish.rs"
kdc_cloud="$repo_root/crates/mesh/mackesd/src/workers/kdc_host/cloud.rs"
shared_cloud="$repo_root/crates/mesh/mackes-mesh-types/src/cloud.rs"
compute_migrate="$repo_root/crates/mesh/mackesd/src/workers/compute_migrate.rs"
workload_compute="$repo_root/crates/mesh/mackesd/src/workers/workload_compute.rs"
service_descriptors="$repo_root/crates/mesh/mackesd/src/descriptors.rs"
peer_contracts="$repo_root/crates/mesh/mackes-mesh-types/src/peers.rs"
desktop_sources="$repo_root/crates/mesh/mackesd/src/workers/desktop_sources.rs"
probe_nmap="$repo_root/crates/mesh/mackesd/src/probe_nmap.rs"
datacenter="$repo_root/crates/mesh/mackesd/src/ipc/datacenter.rs"
datacenter_orchestrator="$repo_root/crates/mesh/mackesd/src/workers/datacenter_orchestrator.rs"
inventory="$repo_root/docs/platform/workload-authority-inventory.md"
retired_console_worker="$repo_root/crates/mesh/mackesd/src/workers/console_broker.rs"
retired_cloud_console="$repo_root/crates/mesh/mackesd/src/workers/cloud/verbs/console.rs"
retired_attach_schema="$repo_root/packaging/browser-vm/browser-vm-transport-attach.schema.json"
retired_attach_example="$repo_root/packaging/browser-vm/browser-vm-transport-attach.example.json"
retired_attach_verifier="$repo_root/packaging/browser-vm/verify-transport-attach.sh"
retired_xcp_provision="$repo_root/crates/mesh/mackesd/src/workers/xcp_provision.rs"
retired_xcp_host="$repo_root/crates/mesh/mackesd/src/workers/xcp_host.rs"
retired_xcp_crate="$repo_root/crates/mesh/mackes-xcp/Cargo.toml"
retired_compute_provision="$repo_root/crates/mesh/mackesd/src/workers/compute_provision.rs"
retired_mesh_vm_provision="$repo_root/install-helpers/provision-mesh-vm.sh"
retired_cert_authority="$repo_root/crates/mesh/mackesd/src/workers/cert_authority.rs"
retired_vm_lifecycle="$repo_root/crates/mesh/mackesd/src/workers/vm_lifecycle.rs"
retired_container_lifecycle="$repo_root/crates/mesh/mackesd/src/workers/container.rs"
live_mirror_verifier="$repo_root/install-helpers/verify-live-mirrors.py"

scan_shell() {
  local source_root="$1"
  if command -v rg >/dev/null 2>&1; then
    rg -n --glob '*.rs' --glob '!**/tests.rs' \
      'action/(vm/lifecycle|container/lifecycle|cloud/(provision|container-deploy|instance-))|state/vdi/console|console-attach|console_broker|LIFECYCLE_TOPIC|VmPowerRequest|ArmAction::(Provision|Lifecycle)|ProvisionApply|arm_(provision|lifecycle)|lifecycle_request_body|issue\("(provision|container-deploy)"|is_nova_managed|CLOUD_MANAGED_TOOLTIP|Nova-managed' \
      "$source_root"
  else
    grep -RInE --include='*.rs' --exclude='tests.rs' \
      'action/(vm/lifecycle|container/lifecycle|cloud/(provision|container-deploy|instance-))|state/vdi/console|console-attach|console_broker|LIFECYCLE_TOPIC|VmPowerRequest|ArmAction::(Provision|Lifecycle)|ProvisionApply|arm_(provision|lifecycle)|lifecycle_request_body|issue\("(provision|container-deploy)"|is_nova_managed|CLOUD_MANAGED_TOOLTIP|Nova-managed' \
      "$source_root"
  fi
}

scan_shell_runtime_commands() {
  local source_root="$1"
  if command -v rg >/dev/null 2>&1; then
    rg -n --glob '*.rs' --glob '!**/tests.rs' \
      '"(sudo[[:space:]]+)?(virsh|podman)[[:space:]]' \
      "$source_root"
  else
    grep -RInE --include='*.rs' --exclude='tests.rs' \
      '"(sudo[[:space:]]+)?(virsh|podman)[[:space:]]' \
      "$source_root"
  fi
}

# Scan the production surfaces most likely to grow a second runtime inventory.
# `workload_compute.rs` is deliberately outside this set: it is the sole
# libvirt/Quadlet adapter and must retain its bounded lifecycle/readiness calls.
# Test modules are excluded because their fake command strings are negative
# fixtures, not reachable production inventory.
scan_runtime_inventory_candidates() {
  local source_root="$1" file relative
  local -a sources=(
    "crates/mesh/mackesd/src"
    "crates/desktop/mde-shell-egui/src"
  )
  local -a candidate_files=()

  for relative in "${sources[@]}"; do
    [ -e "$source_root/$relative" ] || continue
    if [ -d "$source_root/$relative" ]; then
      if command -v rg >/dev/null 2>&1; then
        mapfile -t candidate_files < <(rg --files "$source_root/$relative" | rg '\.(rs|sh|py)$')
      else
        mapfile -t candidate_files < <(find "$source_root/$relative" -type f \
          \( -name '*.rs' -o -name '*.sh' -o -name '*.py' \) -print)
      fi
    else
      candidate_files=("$source_root/$relative")
    fi

    for file in "${candidate_files[@]}"; do
      relative=${file#"$source_root"/}
      [ "$relative" = 'crates/mesh/mackesd/src/workers/workload_compute.rs' ] && continue
      # Skip only the item carrying cfg(test), not the rest of its file: inline
      # test helpers must not hide production declarations that follow them.
      awk -v relative="$relative" '
        function brace_delta(line, copy, opens, closes) {
          copy=line; opens=gsub(/\{/, "", copy)
          copy=line; closes=gsub(/\}/, "", copy)
          return opens-closes
        }
        skip_test {
          test_depth += brace_delta($0)
          if (test_depth <= 0) { skip_test=0; test_depth=0 }
          next
        }
        pending_test {
          delta=brace_delta($0)
          if (delta > 0) { pending_test=0; skip_test=1; test_depth=delta }
          if ($0 ~ /;/) { pending_test=0 }
          next
        }
        /^#\[cfg\(test\)\]/{pending_test=1; next}
        /"(\/usr\/bin\/)?(virsh|podman)"|\.list_instances[[:space:]]*\(|(virsh_output|podman_output)[[:space:]]*\(/ {print relative ":" FNR ":" $0}
      ' "$file"
    done
  done
}

# S1 still records bounded read-only inventory debt in Cloud/Cuttlefish and
# storage. Keep that debt count-limited and line-exact: removal is accepted,
# while a duplicate or differently-shaped command is a new authority bypass.
# The libvirt `version` health probe and the Podman volume runner are not
# inventory, but are pinned here because the deliberately broad candidate scan
# makes future argument expansion reviewable.
is_reviewed_runtime_inventory_candidate() {
  local relative="$1" code="$2" ordinal="$3"
  case "$relative|$code" in
    'crates/mesh/mackesd/src/probe_nmap.rs|"podman",')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/kvm.rs|"podman",')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/image_build.rs|run("podman", &["build", "-t", &tag, "-f", &cf, &ctx])?;')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/image_build.rs|run("podman", &["save", "-o", &art.to_string_lossy(), &tag])?;')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/cloud/verbs/cuttlefish.rs|.list_instances()')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/cloud/runner.rs|TOOL_LIBVIRT => ("virsh", vec!["-c", uri.as_str(), "version"], uri.as_str()),')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/cloud/runner.rs|"virsh",')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/cloud/runner.rs|let status = match Self::run("virsh", &["-c", &self.libvirt_uri, "domstate", name]) {')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/storage.rs|if let Some(uuids) = virsh_output(&["list", "--state-running", "--uuid"]) {')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/storage.rs|if let Some(blk) = virsh_output(&["domblklist", "--details", &uuid]) {')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/storage.rs|let name = virsh_output(&["domname", &uuid])')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/storage.rs|if let Some(json) = podman_output(&["ps", "--format", "json", "--filter", "status=running"])')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/storage.rs|if let Some(mounts) = podman_output(&[')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/storage.rs|bounded_stdout("virsh", args)')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/storage.rs|bounded_stdout("podman", args)')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/storage.rs|fn virsh_output(args: &[&str]) -> Option<String> {')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/storage.rs|fn podman_output(args: &[&str]) -> Option<String> {')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/virtual_storage.rs|if let Some(uuids) = bounded_stdout("virsh", &["list", "--state-running", "--uuid"]) {')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/virtual_storage.rs|if let Some(blk) = bounded_stdout("virsh", &["domblklist", "--details", &uuid]) {')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/virtual_storage.rs|let name = bounded_stdout("virsh", &["domname", &uuid])')
      [ "$ordinal" -le 1 ] ;;
    'crates/mesh/mackesd/src/workers/virtual_storage.rs|"podman",')
      [ "$ordinal" -le 2 ] ;;
    'crates/mesh/mackesd/src/workers/virtual_storage.rs|let mut cmd = Command::new("podman");')
      [ "$ordinal" -le 1 ] ;;
    *) return 1 ;;
  esac
}

audit_runtime_inventory_authority() {
  local source_root="$1" permit_reviewed="${2:-1}" finding relative numbered code key
  local -A seen=()
  local failed=0

  while IFS= read -r finding; do
    [ -n "$finding" ] || continue
    relative=${finding%%:*}
    numbered=${finding#*:}
    code=${numbered#*:}
    code=${code#"${code%%[![:space:]]*}"}
    code=${code%"${code##*[![:space:]]}"}
    key="$relative|$code"
    seen["$key"]=$(( ${seen["$key"]:-0} + 1 ))
    if [ "$permit_reviewed" -eq 1 ] \
      && is_reviewed_runtime_inventory_candidate "$relative" "$code" "${seen["$key"]}"; then
      continue
    fi
    printf '%s\n' "$finding"
    failed=1
  done < <(scan_runtime_inventory_candidates "$source_root")

  return "$failed"
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
  awk -v needle="$needle" '
    /#\[cfg\(test\)\]/ { exit }
    index($0, needle) { found=1 }
    END { exit(found ? 0 : 1) }
  ' "$file"
}

has_durable_migration_boundary() {
  local file="$1"
  contains_literal 'drain_migration_commands' "$file" \
    && contains_literal 'WorkloadMigrationJournal' "$file" \
    && contains_literal 'WorkloadMigrationJournalPhase::Pending' "$file" \
    && contains_literal 'replay_migration_commands' "$file"
}

has_durable_distributed_migration() {
  local file="$1"
  contains_literal 'struct MigrationLedger' "$file" \
    && contains_literal 'source_cursor' "$file" \
    && contains_literal 'ack_jobs' "$file" \
    && contains_literal 'PendingPhase::Relinquish' "$file" \
    && contains_literal 'PendingPhase::Rollback' "$file"
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
  printf '%s\n' 'EntryKind::Tab("virsh list --all")' >"$fixture/src/raw-runtime.rs"
  if ! scan_shell_runtime_commands "$fixture/src" >/dev/null 2>&1; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — raw shell runtime command was not detected' >&2
    return 1
  fi
  printf '%s\n' 'EntryKind::Link(Surface::InfraCode)' >"$fixture/src/raw-runtime.rs"
  if scan_shell_runtime_commands "$fixture/src" >/dev/null 2>&1; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — typed Workloads link was rejected' >&2
    return 1
  fi
  local regression label fixture_path fixture_body
  for regression in \
    'Cloud|crates/mesh/mackesd/src/workers/cloud/runner.rs|let mut command = Command::new("virsh"); command.arg("list");' \
    'Cuttlefish|crates/mesh/mackesd/src/workers/cloud/verbs/cuttlefish.rs|runner.list_instances();' \
    'storage|crates/mesh/mackesd/src/workers/storage.rs|let mut command = Command::new("virsh"); command.arg("dominfo");' \
    'shell|crates/desktop/mde-shell-egui/src/runtime.rs|let mut command = Command::new("podman"); command.arg("ps");'; do
    IFS='|' read -r label fixture_path fixture_body <<<"$regression"
    mkdir -p "$(dirname "$fixture/$fixture_path")"
    printf '%s\n' "$fixture_body" >"$fixture/$fixture_path"
    if audit_runtime_inventory_authority "$fixture" 0 >/dev/null; then
      printf 'lint-workload-authority.sh: self-test failed — %s runtime inventory fixture was accepted\n' "$label" >&2
      return 1
    fi
    rm -f "$fixture/$fixture_path"
  done
  printf '%s\n' 'let mut command = Command::new("virsh"); command.arg("domstate");' \
    >"$fixture/crates/mesh/mackesd/src/workers/workload_compute.rs"
  if ! audit_runtime_inventory_authority "$fixture" 0 >/dev/null; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — sole workload_compute adapter was rejected' >&2
    return 1
  fi
  cat >"$fixture/crates/desktop/mde-shell-egui/src/runtime.rs" <<'EOF'
#[cfg(test)]
mod tests {
    const HOSTILE_FIXTURE: &str = "virsh";
}
fn typed_projection_only() {}
EOF
  if ! audit_runtime_inventory_authority "$fixture" 0 >/dev/null; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — cfg(test) command fixture leaked into production findings' >&2
    return 1
  fi
  for retired in 'action/container/lifecycle' 'action/cloud/provision' 'action/cloud/container-deploy' 'action/cloud/instance-start' 'state/vdi/console' 'console-attach' 'console_broker' 'VmPowerRequest' 'ArmAction::Provision' 'ArmAction::Lifecycle' 'ProvisionApply' 'arm_provision' 'arm_lifecycle' 'lifecycle_request_body' 'issue("provision"' 'issue("container-deploy"' 'is_nova_managed' 'CLOUD_MANAGED_TOOLTIP' 'Nova-managed'; do
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
  printf '%s\n' 'let mut source_cursor = None; let mut pending_commits = Vec::new();' >"$fixture/compute_migrate.rs"
  if has_durable_distributed_migration "$fixture/compute_migrate.rs"; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — volatile distributed migration fixture was accepted' >&2
    return 1
  fi
  cat >"$fixture/compute_migrate.rs" <<'EOF'
struct MigrationLedger { source_cursor: Option<String>, ack_jobs: Vec<String> }
fn reconcile() { let _ = PendingPhase::Relinquish; let _ = PendingPhase::Rollback; }
EOF
  if ! has_durable_distributed_migration "$fixture/compute_migrate.rs"; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — durable distributed migration fixture was rejected' >&2
    return 1
  fi
  printf '%s\n' 'fn drain_migration_commands() {}' >"$fixture/workload_compute.rs"
  if has_durable_migration_boundary "$fixture/workload_compute.rs"; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — volatile migration fixture was accepted' >&2
    return 1
  fi
  cat >"$fixture/workload_compute.rs" <<'EOF'
struct WorkloadMigrationJournal;
fn drain_migration_commands() { let _ = WorkloadMigrationJournalPhase::Pending; }
fn replay_migration_commands() {}
EOF
  if ! has_durable_migration_boundary "$fixture/workload_compute.rs"; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — durable migration fixture was rejected' >&2
    return 1
  fi
  printf '%s\n' 'let _ = Command::new("virsh");' >"$fixture/descriptors.rs"
  if ! production_contains_literal 'Command::new("virsh")' "$fixture/descriptors.rs"; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — raw descriptor runtime probe was not detected' >&2
    return 1
  fi
  printf '%s\n' 'const RETIRED: &str = "compute-inventory.json";' >"$fixture/probe_nmap.rs"
  if ! production_contains_literal 'compute-inventory.json' "$fixture/probe_nmap.rs"; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — retired compute inventory reader was not detected' >&2
    return 1
  fi
  printf '%s\n' 'const RETIRED: &str = "action/dc/vm-power";' >"$fixture/datacenter.rs"
  if ! production_contains_literal 'action/dc/vm-' "$fixture/datacenter.rs"; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — Datacenter VM action fixture was not detected' >&2
    return 1
  fi
  printf '%s\n' 'let _ = Command::new("xe").args(["vm-list"]);' >"$fixture/orchestrator.rs"
  if ! production_contains_literal '"vm-list"' "$fixture/orchestrator.rs"; then
    printf '%s\n' 'lint-workload-authority.sh: self-test failed — Datacenter VM roster fixture was not detected' >&2
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
  && [ -f "$workload_compute" ] && [ -f "$service_descriptors" ] \
  && [ -f "$peer_contracts" ] && [ -f "$desktop_sources" ] \
  && [ -f "$probe_nmap" ] && [ -f "$datacenter" ] \
  && [ -f "$kdc_cloud" ] && [ -f "$shared_cloud" ] && [ -f "$phones_hub" ] \
  && [ -f "$datacenter_orchestrator" ] && [ -f "$inventory" ] || {
  printf '%s\n' 'lint-workload-authority.sh: authority inventory or daemon registration surfaces are missing' >&2
  exit 1
}

if scan_shell "$shell_root"; then
  printf '%s\n' 'lint-workload-authority.sh: legacy lifecycle publisher found in shell' >&2
  exit 1
fi

if scan_shell_runtime_commands "$shell_root"; then
  printf '%s\n' 'lint-workload-authority.sh: shell bypasses the typed Workload projection with a raw runtime command' >&2
  exit 1
fi

if ! runtime_inventory_findings=$(audit_runtime_inventory_authority "$repo_root" 1); then
  printf '%s\n' "$runtime_inventory_findings" >&2
  printf '%s\n' 'lint-workload-authority.sh: direct runtime inventory exists outside the sole Workload adapter' >&2
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
  "$retired_attach_verifier" \
  "$retired_xcp_provision" \
  "$retired_xcp_host" \
  "$retired_xcp_crate" \
  "$retired_compute_provision" \
  "$retired_mesh_vm_provision" \
  "$retired_cert_authority" \
  "$retired_vm_lifecycle" \
  "$retired_container_lifecycle"; do
  if [ -e "$retired_file" ]; then
    printf 'lint-workload-authority.sh: retired authority artifact is reachable: %s\n' "$retired_file" >&2
    exit 1
  fi
done

if contains_literal 'xcp_provision' "$spawn_file" \
  || contains_literal 'xcp_host' "$spawn_file" \
  || contains_literal 'action/provision/' "$spawn_file" \
  || contains_literal 'pub mod xcp_provision' "$workers_mod" \
  || contains_literal 'pub mod xcp_host' "$workers_mod" \
  || contains_literal 'crates/mesh/mackes-xcp' "$workspace_manifest" \
  || contains_literal 'mackes-xcp' "$mackesd_manifest" \
  || production_contains_literal 'action/dc/vm-' "$datacenter" \
  || production_contains_literal '"vm-list"' "$datacenter_orchestrator" \
  || production_contains_literal 'DcResource::new("vm"' "$datacenter_orchestrator"; then
  printf '%s\n' 'lint-workload-authority.sh: retired Datacenter/XCP VM authority is registered or sampled' >&2
  exit 1
fi

if contains_literal 'compute_provision' "$spawn_file" \
  || contains_literal 'pub mod compute_provision' "$workers_mod" \
  || contains_literal '"compute_provision"' "$worker_role" \
  || contains_literal 'compute/create/' "$spawn_file"; then
  printf '%s\n' 'lint-workload-authority.sh: retired compute-provision VM authority is registered' >&2
  exit 1
fi

if contains_literal 'cert_authority' "$spawn_file" \
  || contains_literal 'pub mod cert_authority' "$workers_mod" \
  || contains_literal '"cert_authority"' "$worker_role" \
  || contains_literal 'action/compute/cert-sign-request' "$spawn_file"; then
  printf '%s\n' 'lint-workload-authority.sh: retired producerless certificate responder is registered' >&2
  exit 1
fi

if production_contains_literal 'fn provision(&self' "$cloud_runner" \
  || production_contains_literal 'fn lifecycle(&self' "$cloud_runner" \
  || production_contains_literal '"apply"' "$cloud_runner" \
  || production_contains_literal '.runner.lifecycle' "$cloud_android_lifecycle" \
  || production_contains_literal 'fn lifecycle(' "$cloud_cuttlefish"; then
  printf '%s\n' 'lint-workload-authority.sh: cloud retains a direct VM provision/lifecycle actuator' >&2
  exit 1
fi

if production_contains_literal 'cloud-start-all' "$kdc_cloud" \
  || production_contains_literal 'cloud-stop-all' "$kdc_cloud" \
  || production_contains_literal 'cloud-reboot-all' "$kdc_cloud" \
  || production_contains_literal 'instance-start-all' "$kdc_cloud" \
  || production_contains_literal 'instance-stop-all' "$kdc_cloud" \
  || production_contains_literal 'instance-reboot-all' "$kdc_cloud" \
  || production_contains_literal 'cloud-start-all' "$phones_hub" \
  || production_contains_literal 'cloud-stop-all' "$phones_hub" \
  || production_contains_literal 'cloud-reboot-all' "$phones_hub" \
  || production_contains_literal 'pub enum LifecycleAction' "$shared_cloud"; then
  printf '%s\n' 'lint-workload-authority.sh: phone/shared contracts retain a retired cloud lifecycle publisher' >&2
  exit 1
fi

if ! production_contains_literal 'cloud provision is retired; use `action/workload/operation`' "$cloud_verbs"; then
  printf '%s\n' 'lint-workload-authority.sh: retained cloud provision requests no longer fail closed' >&2
  exit 1
fi

if ! production_contains_literal 'container-deploy is retired; submit a typed Workload operation' "$repo_root/crates/mesh/mackesd/src/workers/cloud/verbs/container.rs"; then
  printf '%s\n' 'lint-workload-authority.sh: retained cloud container-deploy requests no longer fail closed' >&2
  exit 1
fi

if ! production_contains_literal 'if topic.starts_with("event/dc/vm/")' "$datacenter_orchestrator"; then
  printf '%s\n' 'lint-workload-authority.sh: Datacenter projection no longer refuses retained VM topics' >&2
  exit 1
fi

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
  || ! has_durable_migration_boundary "$workload_compute" \
  || ! has_durable_distributed_migration "$compute_migrate"; then
  printf '%s\n' 'lint-workload-authority.sh: migration commands are not durably reconciler-owned' >&2
  exit 1
fi

if production_contains_literal 'Command::new("virsh")' "$service_descriptors" \
  || production_contains_literal 'Command::new("podman")' "$service_descriptors" \
  || contains_literal 'pub vms: Vec<VmInfo>' "$peer_contracts" \
  || contains_literal 'pub containers: Vec<ContainerInfo>' "$peer_contracts" \
  || contains_literal 'descriptors.vms' "$desktop_sources" \
  || contains_literal 'desc.vms' "$desktop_sources"; then
  printf '%s\n' 'lint-workload-authority.sh: heartbeat descriptors retain a competing VM/container runtime projection' >&2
  exit 1
fi

if production_contains_literal 'compute-inventory.json' "$probe_nmap" \
  || production_contains_literal 'vm_overlay_targets' "$probe_nmap"; then
  printf '%s\n' 'lint-workload-authority.sh: network discovery still trusts the retired compute runtime projection' >&2
  exit 1
fi

printf '%s\n' 'lint-workload-authority.sh: clean — one typed Workload actuator/projection; retired lifecycle and console paths absent'
