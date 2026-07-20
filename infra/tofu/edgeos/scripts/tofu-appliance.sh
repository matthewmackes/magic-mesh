#!/usr/bin/env bash
# WL-RUN-006 — per-appliance tofu wrapper. Selects the right VAR-FILE and
# BACKEND-CONFIG for one router appliance so N appliances share this single root
# without clobbering each other's state.
#
#   ./scripts/tofu-appliance.sh <appliance-id> <tofu-cmd> [args…]
#   ./scripts/tofu-appliance.sh --selftest        # offline self-check, no tofu
#
# <appliance-id> is `gateway` (the auto-loaded terraform.tfvars, grandfathered
# state at state/edgeos) OR a gateway MAC (`aa:bb:cc:dd:ee:ff` / `aa-bb-cc-dd-ee-ff`),
# which selects `appliances/<mac>.tfvars` + `appliances/<mac>.backend.hcl`
# (state/router/<mac>). The selection logic MIRRORS
# mackes_mesh_types::router_action::appliance_var_file (kept trivial + documented
# so the Rust fixture and this wrapper cannot drift).
set -euo pipefail

MODULE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

die() { echo "tofu-appliance: $*" >&2; exit 1; }

# Normalize a MAC to the lowercase dash form used for the per-appliance filenames.
norm_id() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | tr ':' '-'
}

# The var-file an appliance id selects (relative to the module dir). `gateway`
# (or empty) → the auto-loaded terraform.tfvars; else appliances/<mac>.tfvars.
var_file_for() {
  local id; id="$(norm_id "${1:-}")"
  if [ -z "$id" ] || [ "$id" = "gateway" ]; then
    printf 'terraform.tfvars'
  else
    printf 'appliances/%s.tfvars' "$id"
  fi
}

# The backend-config an appliance id selects. `gateway` → edgeos.backend.hcl
# (grandfathered state/edgeos); else appliances/<mac>.backend.hcl (state/router/<mac>).
backend_file_for() {
  local id; id="$(norm_id "${1:-}")"
  if [ -z "$id" ] || [ "$id" = "gateway" ]; then
    printf 'edgeos.backend.hcl'
  else
    printf 'appliances/%s.backend.hcl' "$id"
  fi
}

selftest() {
  local fails=0
  check() { # <expect> <actual> <label>
    if [ "$1" = "$2" ]; then echo "  [PASS] $3"; else echo "  [FAIL] $3: want '$1' got '$2'"; fails=1; fi
  }
  echo "tofu-appliance --selftest:"
  check "terraform.tfvars"                    "$(var_file_for gateway)"              "gateway → terraform.tfvars"
  check "terraform.tfvars"                    "$(var_file_for '')"                   "empty → terraform.tfvars (default instance)"
  check "appliances/aa-bb-cc-dd-ee-ff.tfvars" "$(var_file_for aa:bb:cc:dd:ee:ff)"    "MAC(:) → appliances/<mac>.tfvars"
  check "appliances/aa-bb-cc-dd-ee-ff.tfvars" "$(var_file_for AA-BB-CC-DD-EE-FF)"    "MAC(upper,-) → normalized appliances/<mac>.tfvars"
  check "edgeos.backend.hcl"                  "$(backend_file_for gateway)"          "gateway → edgeos.backend.hcl (state/edgeos)"
  check "appliances/aa-bb-cc-dd-ee-ff.backend.hcl" "$(backend_file_for aa:bb:cc:dd:ee:ff)" "MAC → appliances/<mac>.backend.hcl (state/router/<mac>)"
  # An unknown appliance's var-file must NOT exist (refuse a run without one).
  if [ -f "$MODULE_DIR/$(var_file_for 00:00:00:00:00:00)" ]; then
    echo "  [FAIL] unknown appliance unexpectedly has a var-file"; fails=1
  else
    echo "  [PASS] unknown appliance has no var-file (a run would be refused)"
  fi
  [ "$fails" -eq 0 ] && echo "tofu-appliance --selftest: OK" || { echo "tofu-appliance --selftest: FAIL"; return 1; }
}

main() {
  [ "${1:-}" = "--selftest" ] && { selftest; exit $?; }
  [ "$#" -ge 2 ] || die "usage: tofu-appliance.sh <appliance-id> <tofu-cmd> [args…]  (or --selftest)"

  local id="$1"; shift
  local cmd="$1"; shift
  local var_file backend_file
  var_file="$(var_file_for "$id")"
  backend_file="$(backend_file_for "$id")"

  [ -f "$MODULE_DIR/$var_file" ] || die "no var-file '$var_file' for appliance '$id' — copy appliances/example-router.tfvars.example first (see appliances/README.md)."

  cd "$MODULE_DIR"
  case "$cmd" in
    init)
      [ -f "$backend_file" ] || die "no backend-config '$backend_file' for appliance '$id' — copy appliances/example-router.backend.hcl.example first."
      exec tofu init -backend-config="$backend_file" "$@"
      ;;
    plan|apply|destroy|refresh|import|console|output)
      exec tofu "$cmd" -var-file="$var_file" "$@"
      ;;
    validate|fmt|providers|version|state|show)
      exec tofu "$cmd" "$@"
      ;;
    *)
      die "unsupported tofu command '$cmd' (init|plan|apply|destroy|refresh|import|console|output|validate|fmt|state|show)"
      ;;
  esac
}

main "$@"
