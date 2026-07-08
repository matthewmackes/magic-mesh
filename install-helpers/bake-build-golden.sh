#!/usr/bin/env bash
# bake-build-golden.sh — DAR-34: bake the Rust+sccache toolchain into the one
# canonical XCP build template, MDE-VM-golden.
#
# This script is intentionally live-gated: it operates on an already-created
# throwaway clone of MDE-VM-golden, never on the template object directly. The
# operator can inspect the clone, then use the printed XCP commands to halt it
# and mark it as the replacement template.
#
# Usage:
#   install-helpers/bake-build-golden.sh --host 172.20.0.52 [--user mm]
#   install-helpers/bake-build-golden.sh --host 172.20.0.52 --inventory /tmp/inv.ini \
#     --minio-endpoint http://10.42.0.1:9000
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"

HOST=""
USER_="${MCNF_BUILD_USER:-mm}"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
INVENTORY=""
LIMIT="bake_golden"
TEMPLATE_NAME="${MCNF_GOLDEN_TEMPLATE:-MDE-VM-golden}"
TOOLCHAIN_PLAYBOOK="$REPO/infra/ansible/build-vm-toolchain.yml"
SCCACHE_PLAYBOOK="$REPO/infra/ansible/sccache.yml"
GENERALIZE="$HERE/build-mde-vm-golden.sh"
MINIO_ENDPOINT="${MCNF_MINIO_ENDPOINT:-}"
EXTRA_ANSIBLE_ARGS=()

usage() {
  sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
  cat <<EOF

Options:
  --host <ip-or-name>        Required. The prepared clone to bake.
  --user <user>              SSH user on the clone (default: $USER_).
  --key <path>               SSH key (default: $KEY).
  --inventory <path>         Existing Ansible inventory. If omitted, a temporary
                             one-host inventory is generated.
  --limit <name>             Ansible host limit/name (default: $LIMIT).
  --template-name <name>     Template name to print in handoff (default: $TEMPLATE_NAME).
  --minio-endpoint <url>     Pass through to sccache.yml as minio_endpoint.
  --ansible-arg <arg>        Extra argument forwarded to ansible-playbook.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --host) HOST="$2"; shift 2 ;;
    --user) USER_="$2"; shift 2 ;;
    --key) KEY="$2"; shift 2 ;;
    --inventory) INVENTORY="$2"; shift 2 ;;
    --limit) LIMIT="$2"; shift 2 ;;
    --template-name) TEMPLATE_NAME="$2"; shift 2 ;;
    --minio-endpoint) MINIO_ENDPOINT="$2"; shift 2 ;;
    --ansible-arg) EXTRA_ANSIBLE_ARGS+=("$2"); shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "bake-build-golden: unknown arg: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[ -n "$HOST" ] || { echo "bake-build-golden: --host is required" >&2; usage >&2; exit 2; }
[ -r "$KEY" ] || { echo "bake-build-golden: SSH key not readable: $KEY" >&2; exit 1; }
[ -r "$TOOLCHAIN_PLAYBOOK" ] || { echo "bake-build-golden: missing $TOOLCHAIN_PLAYBOOK" >&2; exit 1; }
[ -r "$SCCACHE_PLAYBOOK" ] || { echo "bake-build-golden: missing $SCCACHE_PLAYBOOK" >&2; exit 1; }
[ -x "$GENERALIZE" ] || { echo "bake-build-golden: missing executable $GENERALIZE" >&2; exit 1; }
command -v ansible-playbook >/dev/null 2>&1 || { echo "bake-build-golden: ansible-playbook not found" >&2; exit 1; }

tmp_inventory=""
cleanup() {
  [ -n "$tmp_inventory" ] && rm -f "$tmp_inventory"
}
trap cleanup EXIT

if [ -z "$INVENTORY" ]; then
  tmp_inventory="$(mktemp "${TMPDIR:-/tmp}/mcnf-bake-golden.XXXXXX.ini")"
  INVENTORY="$tmp_inventory"
  cat >"$INVENTORY" <<EOF
[build_vms]
$LIMIT ansible_host=$HOST ansible_user=$USER_ ansible_ssh_private_key_file=$KEY ansible_ssh_common_args='-o StrictHostKeyChecking=accept-new'
EOF
fi

run_playbook() {
  local playbook="$1"; shift
  ansible-playbook -i "$INVENTORY" "$playbook" -l "$LIMIT" "${EXTRA_ANSIBLE_ARGS[@]}" "$@"
}

echo "==> baking $HOST as $TEMPLATE_NAME"
echo "==> install Rust/dev toolchain"
run_playbook "$TOOLCHAIN_PLAYBOOK"

echo "==> install/configure shared sccache"
sccache_args=()
[ -n "$MINIO_ENDPOINT" ] && sccache_args+=("-e" "minio_endpoint=$MINIO_ENDPOINT")
run_playbook "$SCCACHE_PLAYBOOK" "${sccache_args[@]}"

echo "==> verify baked tools on $HOST"
ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes "$USER_@$HOST" \
  '. "$HOME/.cargo/env"; . "$HOME/.sccache.env"; rustc --version && sccache --version && mold --version'

echo "==> generalize the baked clone"
"$GENERALIZE" "$USER_@$HOST"

cat <<EOF
==> baked clone is generalized.

Next live XCP steps, run after confirming the clone UUID:
  xe vm-shutdown uuid=<baked-clone-uuid> --force
  xe vm-param-set uuid=<baked-clone-uuid> name-label=$TEMPLATE_NAME is-a-template=true
  xe template-list name-label=$TEMPLATE_NAME

After replacing the template, prove DAR-34 on a fresh clone:
  ssh -i "$KEY" $USER_@<fresh-clone-ip> '. ~/.cargo/env; . ~/.sccache.env; rustc --version && sccache --version && mold --version'
EOF
