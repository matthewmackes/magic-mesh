#!/usr/bin/env bash
# WL-ARCH-001 Phase B — offline self-test for the Ansible configure leg. Touches
# NO live mesh: the dynamic inventory reads a JSON fixture roster and the mde-seal
# lookup reads a stubbed store. Proves:
#   (1) mesh.py --list groups the fixture roster by role/scope + cloud_vm +
#       the WL-ARCH-006 delivery_<type> groups, and mesh.py --selftest passes,
#   (2) playbooks/site.yml passes ansible-playbook --syntax-check,
#   (3) the mde_seal lookup plugin resolves a fixture secret,
#   (4) python + bash syntax are clean (py_compile / bash -n) — the fallback when
#       ansible is absent on the builder.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT"

fail=0
pass() { printf '  PASS %s\n' "$1"; }
bad() { printf '  FAIL %s\n' "$1"; fail=1; }
have() { command -v "$1" >/dev/null 2>&1; }

echo "ansible configure-leg selftest (fixture roster + stubbed store — NO live mesh)"

# ── (4) always-on syntax checks (the ansible-absent fallback) ──
if have python3; then
  python3 -m py_compile inventory/mesh.py plugins/lookup/mde_seal.py \
    && pass "python syntax clean (mesh.py + mde_seal.py)" \
    || bad "python syntax error"
fi
bash -n "$HERE/selftest.sh" && pass "bash -n clean (selftest.sh)" || bad "bash -n failed"

# ── (1a) the self-contained --selftest (synthetic roster, no fixture) ──
if python3 inventory/mesh.py --selftest; then
  pass "mesh.py --selftest (delivery + role/scope/cloud_vm grouping)"
else
  bad "mesh.py --selftest failed"
fi

# ── (1b) the dynamic inventory groups the fixture roster ──
export MESH_INVENTORY_FIXTURE="$HERE/roster.fixture.json"
inv="$(python3 inventory/mesh.py --list)"
check_group() { # <group> <expected-json-array>
  local got
  got="$(MESH_INV="$inv" python3 -c '
import json, os, sys
inv = json.loads(os.environ["MESH_INV"])
sys.stdout.write(json.dumps(inv.get(sys.argv[1], {}).get("hosts", [])))
' "$1")"
  if [ "$got" = "$2" ]; then pass "inventory group $1 = $2"; else bad "inventory group $1: got $got want $2"; fi
}
check_group cloud_vm '["app-1", "ctr-1", "seat-1", "svc-1", "worker-a", "worker-b"]'
check_group role_lighthouse '["lh1"]'
check_group scope_media '["eagle"]'
check_group mesh '["app-1", "ctr-1", "droid-1", "eagle", "lh1", "seat-1", "svc-1", "worker-a", "worker-b"]'
# WL-ARCH-006 delivery groups → the per-type roles in site.yml.
check_group delivery_desktop_vm '["seat-1"]'
check_group delivery_service_vm '["svc-1"]'
check_group delivery_app_vm '["app-1"]'
check_group delivery_service_container '["ctr-1"]'
check_group delivery_android_vm '["droid-1"]'

# ── (2)+(3) ansible-driven checks (when ansible is installed) ──
if have ansible-playbook && have ansible; then
  export ANSIBLE_CONFIG="$ROOT/ansible.cfg"
  # (2) the playbook parses + resolves the inventory.
  if ansible-playbook --syntax-check playbooks/site.yml >/tmp/mde-cloud-synt.$$ 2>&1; then
    pass "playbooks/site.yml passes --syntax-check"
  else
    bad "site.yml syntax-check failed:"; sed 's/^/    /' /tmp/mde-cloud-synt.$$ >&2
  fi
  rm -f /tmp/mde-cloud-synt.$$

  # (3) the mde_seal lookup resolves a fixture secret via a stubbed helper.
  work="$(mktemp -d)"
  fixture="join-token-SELFTEST-$RANDOM-$$"
  stub="$work/mcnf-secret.sh"
  cat >"$stub" <<STUB
#!/usr/bin/env bash
[ "\$1" = get ] && [ "\$2" = nebula-join-token ] && { printf %s "$fixture"; exit 0; }
exit 3
STUB
  chmod +x "$stub"
  out="$(MDE_SEAL_HELPER="$stub" ANSIBLE_LOOKUP_PLUGINS="$ROOT/plugins/lookup" \
    ansible localhost -c local -m debug \
    -a "msg={{ lookup('mde_seal', 'nebula-join-token') }}" 2>/dev/null || true)"
  if printf '%s' "$out" | grep -q -- "$fixture"; then
    pass "mde_seal lookup resolves a fixture secret through ansible"
  else
    bad "mde_seal lookup did not resolve the fixture secret"
  fi
  rm -rf "$work"
else
  echo "  SKIP ansible not installed — inventory + python/bash checks above cover the fallback"
fi

[ "$fail" -eq 0 ] && echo "selftest: ALL PASS" || { echo "selftest: FAILURES" >&2; exit 1; }
