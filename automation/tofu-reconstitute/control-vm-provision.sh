#!/usr/bin/env bash
# control-vm-provision.sh — DAR-49: operator-run live provisioning and proof for
# the dedicated MCNF backoffice control VM.
#
# Default mode is check/plan only. Pass --live to apply. Post-apply proof checks:
#   - produced state has no age private key / unseal passphrase / provider token
#   - mackesd peers shows the control VM as an enrolled overlay peer
#   - /mcnf/age-recipients/<control-node-id> exists
#   - optional SSH health probes against the overlay IP
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
ROOT="$REPO/infra/tofu/control-vm"
GEN_BACKEND="$REPO/automation/state-backend/gen-backend-config.sh"
TOFU_ENV="$REPO/automation/lib/tofu-env.sh"
SECRET="$REPO/automation/secrets/mcnf-secret.sh"
STATUS="$REPO/automation/backoffice/backoffice-status.sh"
ETCD_LIB="$REPO/automation/lib/etcd-endpoints.sh"

BACKEND_IP="${MCNF_STATE_BACKEND_IP:-${MCNF_CONTROL_IP:-}}"
TIER="${MCNF_BACKOFFICE_TIER:-minimal}"
CONTROL_NAME="${MCNF_CONTROL_VM_NAME:-mcnf-control}"
LIVE=0
WAIT=1
TIMEOUT="${MCNF_CONTROL_VM_TIMEOUT:-900}"
TOFU="${MCNF_TOFU:-$(command -v tofu || command -v terraform || true)}"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"

usage() {
  sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
  cat <<EOF

Options:
  --backend-ip <ip>      Required. Current state-backend host for control-vm init.
  --tier minimal|full    Backoffice tier var override (default: $TIER).
  --name <label>         Control VM name_label / peer name (default: $CONTROL_NAME).
  --live                 Run tofu apply and post-apply proofs. Default is plan only.
  --no-wait              Do not wait for peer enrollment after apply.
  --timeout <seconds>    Enrollment wait timeout (default: $TIMEOUT).
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --backend-ip) BACKEND_IP="$2"; shift 2 ;;
    --tier) TIER="$2"; shift 2 ;;
    --name) CONTROL_NAME="$2"; shift 2 ;;
    --live) LIVE=1; shift ;;
    --no-wait) WAIT=0; shift ;;
    --timeout) TIMEOUT="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "control-vm-provision: unknown arg: $1" >&2; usage >&2; exit 2 ;;
  esac
done

log() { echo "==> control-vm-provision: $*"; }
die() { echo "control-vm-provision: $*" >&2; exit 1; }

[ -n "$BACKEND_IP" ] || die "--backend-ip <state-backend-host> is required"
case "$TIER" in minimal|full) ;; *) die "--tier must be minimal or full" ;; esac
[ -n "$TOFU" ] || die "neither tofu nor terraform found on PATH"
[ -x "$GEN_BACKEND" ] || die "missing executable $GEN_BACKEND"
[ -r "$TOFU_ENV" ] || die "missing $TOFU_ENV"
[ -r "$ETCD_LIB" ] || die "missing $ETCD_LIB"

# shellcheck source=../lib/etcd-endpoints.sh
. "$ETCD_LIB"

peer_json_field() {
  python3 - "$CONTROL_NAME" <<'PY'
import json, re, sys
needle = sys.argv[1].lower()
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(1)
items = data if isinstance(data, list) else data.get("peers") or data.get("nodes") or []
for p in items:
    text = " ".join(str(p.get(k, "")) for k in ("name", "hostname", "node_id", "id", "label", "role", "notes")).lower()
    if needle not in text:
        continue
    for k in ("overlay_ip", "nebula_ip", "vpn_ip", "ip", "addr"):
        v = str(p.get(k, ""))
        if re.match(r"^10\.", v):
            print(v); sys.exit(0)
    for v in p.values():
        if isinstance(v, str) and re.match(r"^10\.", v):
            print(v); sys.exit(0)
sys.exit(2)
PY
}

control_node_id() {
  python3 - "$CONTROL_NAME" <<'PY'
import json, sys
needle = sys.argv[1].lower()
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(1)
items = data if isinstance(data, list) else data.get("peers") or data.get("nodes") or []
for p in items:
    text = " ".join(str(p.get(k, "")) for k in ("name", "hostname", "node_id", "id", "label", "role", "notes")).lower()
    if needle in text:
        print(p.get("node_id") or p.get("id") or p.get("name") or p.get("hostname") or "")
        sys.exit(0)
sys.exit(2)
PY
}

assert_state_has_no_plaintext_secrets() {
  local state="$1"
  # Look for private age keys, unseal passphrases, or obvious provider-token names.
  # Sensitive values should be absent/redacted; this intentionally errs on the side
  # of stopping the cutover if state contains a suspicious secret marker.
  if rg -n "AGE-SECRET-KEY|unseal[_-]?pass|passphrase|DIGITALOCEAN_TOKEN|XOA_TOKEN|AWS_SECRET_ACCESS_KEY|xapi_password|join_token" "$state" >/dev/null; then
    rg -n "AGE-SECRET-KEY|unseal[_-]?pass|passphrase|DIGITALOCEAN_TOKEN|XOA_TOKEN|AWS_SECRET_ACCESS_KEY|xapi_password|join_token" "$state" >&2 || true
    die "state contains a forbidden plaintext secret marker"
  fi
}

etcd_key_present() { # <key>
  local endpoints ep k resp
  endpoints="$(mcnf_resolve_etcd)" || return 1
  ep="${endpoints%%,*}"
  k="$(printf '%s' "$1" | base64 -w0)"
  resp="$(curl -fsS --max-time 5 -X POST "$ep/v3/kv/range" -d "{\"key\":\"$k\"}" 2>/dev/null || true)"
  printf '%s' "$resp" | grep -q '"kvs"'
}

log "generate backend config for control-vm at $BACKEND_IP"
"$GEN_BACKEND" --control-ip "$BACKEND_IP" --roots control-vm
CFG="$ROOT/control-vm.backend.hcl"

export TF_VAR_backoffice_tier="$TIER"
export TF_VAR_control_vm_name="$CONTROL_NAME"

log "init/validate/plan"
bash -lc ". '$TOFU_ENV'; tofu_env_load control-vm >/dev/null; '$TOFU' -chdir='$ROOT' init -input=false -backend-config='$CFG'"
bash -lc ". '$TOFU_ENV'; tofu_env_load control-vm >/dev/null; '$TOFU' -chdir='$ROOT' validate"
bash -lc ". '$TOFU_ENV'; tofu_env_load control-vm >/dev/null; '$TOFU' -chdir='$ROOT' plan -input=false -out=control-vm.plan"

if [ "$LIVE" -ne 1 ]; then
  log "CHECK mode complete. Re-run with --live to apply and prove enrollment."
  exit 0
fi

log "LIVE apply"
bash -lc ". '$TOFU_ENV'; tofu_env_load control-vm >/dev/null; '$TOFU' -chdir='$ROOT' apply -input=false control-vm.plan"

state_file="$(mktemp "${TMPDIR:-/tmp}/control-vm-state.XXXXXX.tfstate")"
trap 'rm -f "$state_file"' EXIT
bash -lc ". '$TOFU_ENV'; tofu_env_load control-vm >/dev/null; '$TOFU' -chdir='$ROOT' state pull" >"$state_file"
assert_state_has_no_plaintext_secrets "$state_file"
log "state plaintext-secret grep passed"

if [ "$WAIT" -eq 1 ]; then
  log "wait for control VM peer enrollment ($CONTROL_NAME)"
  deadline=$(( $(date +%s) + TIMEOUT ))
  overlay=""
  node_id=""
  while [ "$(date +%s)" -lt "$deadline" ]; do
    peers="$(mackesd peers --json 2>/dev/null || true)"
    if [ -n "$peers" ]; then
      overlay="$(printf '%s' "$peers" | peer_json_field || true)"
      node_id="$(printf '%s' "$peers" | control_node_id || true)"
      [ -n "$overlay" ] && [ -n "$node_id" ] && break
    fi
    sleep 10
  done
  [ -n "$overlay" ] || die "control VM did not appear in mackesd peers --json within ${TIMEOUT}s"
  log "control VM enrolled: node=$node_id overlay=$overlay"

  recip_key="/mcnf/age-recipients/$node_id"
  etcd_key_present "$recip_key" || die "missing expected VM recipient key: $recip_key"
  log "VM recipient key exists: $recip_key"

  if ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=10 "mm@$overlay" true 2>/dev/null; then
    log "overlay SSH reachable; probing tier services"
    ssh -i "$KEY" -o StrictHostKeyChecking=accept-new "mm@$overlay" \
      "systemctl is-active mcnf-state-backend.service mcnf-backoffice-up.service 2>/dev/null || true; curl -fsS --max-time 5 http://$overlay:8390/state/__probe__ >/dev/null || test \"\$?\" = 22" || true
    if [ -x "$STATUS" ]; then
      MCNF_CONTROL_IP="$overlay" "$STATUS" --json || true
    fi
  else
    log "overlay SSH not reachable yet; enrollment proof captured, service probes deferred"
  fi
fi

log "control VM provision proof complete"
