#!/usr/bin/env bash
# WL-FUNC-022 S6: fail closed if the retired Timers surface, shell scheduling
# authority, stale clock route, or separately-installed Timers payload returns.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHELL_ROOT="$ROOT/crates/desktop/mde-shell-egui/src"
SURFACES="$SHELL_ROOT/surfaces.rs"
CLOCK_UI="$SHELL_ROOT/timers.rs"
PACKAGE_MANIFEST="$ROOT/crates/mesh/mackesd/Cargo.toml"

usage() {
  printf '%s\n' 'usage: install-helpers/lint-clock-cutover.sh [--self-test] [--rpm RPM]'
}

production_rust() {
  awk '/^[[:space:]]*#\[cfg\(test\)\]/{exit} {print}' "$1"
}

scan_source() {
  local root="$1" findings="$2" file rel
  : >"$findings"
  for file in "$root/crates/desktop/mde-shell-egui/src/surfaces.rs" \
              "$root/crates/desktop/mde-shell-egui/src/timers.rs" \
              "$root/crates/mesh/mackesd/Cargo.toml"; do
    [[ -f "$file" ]] || printf 'missing required cutover input: %s\n' "${file#"$root/"}" >>"$findings"
  done
  [[ -s "$findings" ]] && return 0

  while IFS= read -r file; do
    rel="${file#"$root/"}"
    production_rust "$file" | grep -InE \
      'Surface::Timers|(^|[^[:alnum:]_])(TimerStore|AlarmStore|TimersState|AlarmScheduler)([^[:alnum:]_]|$)|timers-alarms\.json|schedule_(alarm|timer)|fire_due_(alarm|timer)|tick_(alarm|timer)' \
      | sed "s#^#retired shell Clock authority in $rel:#" >>"$findings" || true
  done < <(find "$root/crates/desktop/mde-shell-egui/src" -type f -name '*.rs' -print | sort)

  grep -Fq 'Clock,' "$root/crates/desktop/mde-shell-egui/src/surfaces.rs" \
    || printf '%s\n' 'missing canonical Surface::Clock declaration' >>"$findings"
  grep -Fq 'The shell owns presentation state only.' "$root/crates/desktop/mde-shell-egui/src/timers.rs" \
    || printf '%s\n' 'Clock UI lost its presentation-only authority declaration' >>"$findings"

  grep -InE \
    '(source|dest)[[:space:]]*=[[:space:]]*"[^"]*(mde-(timers|alarms)|org\.magicmesh\.(Timers|Alarms))' \
    "$root/crates/mesh/mackesd/Cargo.toml" \
    | sed 's/^/retired Clock package asset:/' >>"$findings" || true
}

scan_docs() {
  local root="$1" findings="$2" file rel
  while IFS= read -r file; do
    rel="${file#"$root/"}"
    grep -InEi \
      'click (the )?status-bar clock|clock([^.!|]{0,60})open(s|ing)? (the )?Notification Center' \
      "$file" | grep -Eiv 'must not|does not|never|cannot' \
      | sed "s#^#stale Clock route in $rel:#" >>"$findings" || true
  done < <(find "$root/docs" -type f -name '*.md' \
    -not -path '*/design-archive/*' -not -path '*/worklist-archive/*' \
    -not -path '*/platform/evidence/*' -not -path '*/platform/WORKLIST.md' -print | sort)
}

scan_package_list() {
  local list="$1" findings="$2"
  grep -InE \
    '/(usr/)?(bin|libexec)/mde-(timers|alarms)(/|$)|/usr/share/applications/(org\.magicmesh\.)?(Timers|Alarms)\.desktop$|/usr/lib/systemd/system/mde-(timers|alarms)\.(service|timer)$' \
    "$list" | sed 's/^/retired installed Clock payload:/' >>"$findings" || true
}

run_lint() {
  local rpm_path="${1:-}" findings package_list
  findings="$(mktemp)"
  package_list="$(mktemp)"
  trap 'rm -f -- "${findings:-}" "${package_list:-}"' RETURN
  scan_source "$ROOT" "$findings"
  scan_docs "$ROOT" "$findings"
  if [[ -n "$rpm_path" ]]; then
    command -v rpm >/dev/null 2>&1 || { printf '%s\n' 'lint-clock-cutover.sh: rpm is required for --rpm' >&2; return 1; }
    rpm -qlp -- "$rpm_path" >"$package_list" || return 1
    scan_package_list "$package_list" "$findings"
  fi
  sort -u "$findings" -o "$findings"
  if [[ -s "$findings" ]]; then
    printf '%s\n' 'lint-clock-cutover.sh: Clock hard-cut violation(s):' >&2
    sed 's/^/  /' "$findings" >&2
    return 1
  fi
  printf '%s\n' 'lint-clock-cutover.sh: clean — Clock and Notification Center remain distinct; retired Timers authority/payload absent'
}

self_test() {
  local fixture findings list failures=0
  fixture="$(mktemp -d)"; findings="$(mktemp)"; list="$(mktemp)"
  trap 'rm -rf -- "${fixture:-}"; rm -f -- "${findings:-}" "${list:-}"' RETURN
  mkdir -p "$fixture/crates/desktop/mde-shell-egui/src" "$fixture/crates/mesh/mackesd" "$fixture/docs/design"
  printf '%s\n' 'pub enum Surface { Clock, }' >"$fixture/crates/desktop/mde-shell-egui/src/surfaces.rs"
  printf '%s\n' '//! The shell owns presentation state only.' >"$fixture/crates/desktop/mde-shell-egui/src/timers.rs"
  printf '%s\n' 'assets = [{ source = "target/release/mde-shell-egui", dest = "/usr/bin/mde-shell-egui" }]' >"$fixture/crates/mesh/mackesd/Cargo.toml"
  printf '%s\n' 'The visible clock opens Clock. The bell opens Notification Center.' >"$fixture/docs/design/live.md"
  scan_source "$fixture" "$findings"; scan_docs "$fixture" "$findings"
  [[ ! -s "$findings" ]] || { printf '%s\n' 'lint-clock-cutover.sh: SELF-TEST FAILED — clean cutover rejected' >&2; failures=$((failures + 1)); }

  printf '%s\n' 'route(Surface::Timers);' >"$fixture/crates/desktop/mde-shell-egui/src/legacy.rs"
  printf '%s\n' 'Pointer parity: click status-bar clock.' >"$fixture/docs/design/live.md"
  printf '%s\n' '/usr/bin/mde-timers' >"$list"
  scan_source "$fixture" "$findings"; scan_docs "$fixture" "$findings"; scan_package_list "$list" "$findings"
  grep -Fq 'Surface::Timers' "$findings" || { printf '%s\n' 'lint-clock-cutover.sh: SELF-TEST FAILED — retired surface accepted' >&2; failures=$((failures + 1)); }
  grep -Fq 'stale Clock route' "$findings" || { printf '%s\n' 'lint-clock-cutover.sh: SELF-TEST FAILED — stale route prose accepted' >&2; failures=$((failures + 1)); }
  grep -Fq 'retired installed Clock payload' "$findings" || { printf '%s\n' 'lint-clock-cutover.sh: SELF-TEST FAILED — retired installed payload accepted' >&2; failures=$((failures + 1)); }

  rm -f "$fixture/crates/desktop/mde-shell-egui/src/legacy.rs"
  printf '%s\n' 'struct AlarmStore;' >"$fixture/crates/desktop/mde-shell-egui/src/legacy.rs"
  scan_source "$fixture" "$findings"
  grep -Fq 'AlarmStore' "$findings" || { printf '%s\n' 'lint-clock-cutover.sh: SELF-TEST FAILED — shell alarm store accepted' >&2; failures=$((failures + 1)); }

  printf '%s\n' 'assets = [{ source = "target/release/mde-timers", dest = "/usr/bin/mde-timers" }]' >"$fixture/crates/mesh/mackesd/Cargo.toml"
  scan_source "$fixture" "$findings"
  grep -Fq 'retired Clock package asset' "$findings" || { printf '%s\n' 'lint-clock-cutover.sh: SELF-TEST FAILED — retired package manifest asset accepted' >&2; failures=$((failures + 1)); }

  [[ "$failures" -eq 0 ]] || return 1
  printf '%s\n' 'lint-clock-cutover.sh: self-test passed'
}

case "${1:-}" in
  '') run_lint ;;
  --self-test) self_test ;;
  --rpm) [[ $# -eq 2 ]] || { usage >&2; exit 2; }; run_lint "$2" ;;
  -h|--help) usage ;;
  *) usage >&2; exit 2 ;;
esac
