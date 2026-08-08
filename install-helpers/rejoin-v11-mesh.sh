#!/usr/bin/env bash
# rejoin-v11-mesh.sh — bring an old-mesh node onto the new v11 (SUBSTRATE-V2-bound)
# mesh in one shot. Run ON the node being rejoined (e.g. .13/eagle).
#
#   sudo ./rejoin-v11-mesh.sh [lighthouse-public-ip] [role] ['<join-token>']
#     lighthouse-public-ip  default 174.138.68.216 (lighthouse-01)
#     role                  default workstation
#     join-token            optional; if omitted, minted via ssh to the lighthouse
#
# One-liner:
#   curl -sL https://raw.githubusercontent.com/matthewmackes/magic-mesh/master/install-helpers/rejoin-v11-mesh.sh | sudo bash -s -- 174.138.68.216
set -euo pipefail

# WL-CRIT-007/S1 — a rejoin must never carry a previous node identity into a
# new enrollment.  `mackesd leave` is the single owner of this teardown; this
# helper only verifies its postcondition before it consumes the fresh bearer.
identity_is_absent() {
  local root="$1"
  local path
  for path in \
    "$root/etc/nebula/host.crt" \
    "$root/etc/nebula/host.key" \
    "$root/var/lib/mde/role.toml"; do
    # Check -L as well as -e: a broken symlink is still stale identity state
    # and must not satisfy the guard.
    if [[ -e "$path" || -L "$path" ]]; then
      return 1
    fi
  done
  return 0
}

valid_role() {
  case "$1" in
    lighthouse|workstation) return 0 ;;
    *) return 1 ;;
  esac
}

run_self_test() {
  local test_root
  test_root="$(mktemp -d "${TMPDIR:-/tmp}/mcnf-rejoin-v11-self-test.XXXXXX")"
  trap 'if [[ -n "${test_root:-}" ]]; then rm -rf -- "$test_root"; fi' EXIT
  mkdir -p "$test_root/etc/nebula" "$test_root/var/lib/mde"

  if ! identity_is_absent "$test_root"; then
    echo "self-test: empty identity state was rejected" >&2
    return 1
  fi

  : >"$test_root/etc/nebula/host.crt"
  if identity_is_absent "$test_root"; then
    echo "self-test: stale host certificate was accepted" >&2
    return 1
  fi
  rm -f -- "$test_root/etc/nebula/host.crt"

  ln -s /does-not-exist "$test_root/etc/nebula/host.key"
  if identity_is_absent "$test_root"; then
    echo "self-test: broken identity symlink was accepted" >&2
    return 1
  fi
  rm -f -- "$test_root/etc/nebula/host.key"

  : >"$test_root/var/lib/mde/role.toml"
  if identity_is_absent "$test_root"; then
    echo "self-test: stale role pin was accepted" >&2
    return 1
  fi
  rm -f -- "$test_root/var/lib/mde/role.toml"

  valid_role workstation
  valid_role lighthouse
  if valid_role 'workstation;touch /tmp/mcnf-rejoin-self-test-escaped'; then
    echo "self-test: hostile role was accepted" >&2
    return 1
  fi

  echo "rejoin-v11-mesh: self-test passed (identity teardown gate)"
}

if [[ "${1:-}" == "--self-test" ]]; then
  [[ "$#" -eq 1 ]] || { echo "--self-test takes no additional arguments" >&2; exit 2; }
  run_self_test
  exit 0
fi

[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo)"; exit 1; }
LH="${1:-174.138.68.216}"; ROLE="${2:-workstation}"; TOKEN="${3:-}"
valid_role "$ROLE" || {
  echo "unsupported role '$ROLE' (expected lighthouse or workstation)" >&2
  exit 2
}

echo "==> [1/4] upgrade to 11.0.1 (FOUND-NEBULA fix)"
rm -f /etc/yum.repos.d/mackes-mirror-magic-mesh.repo             # dead file:// mirror → dnf error 37
. /etc/os-release
URL="https://github.com/matthewmackes/magic-mesh/releases/download/magic-mesh-v11.0.1/magic-mesh-11.0.1-1.fc${VERSION_ID}.x86_64.rpm"
dnf install -y --refresh "$URL" >/tmp/rejoin-dnf.log 2>&1 \
  && echo "    $(rpm -q magic-mesh)" \
  || { echo "    UPGRADE FAILED:"; tail -8 /tmp/rejoin-dnf.log; exit 1; }

echo "==> [2/4] obtain a single-use join token"
if [ -z "$TOKEN" ]; then
  if ! TOKEN="$(ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 "root@${LH}" \
              "mackesd add-peer --role ${ROLE}" 2>/dev/null | grep -m1 '^mesh:')"; then
    TOKEN=""
  fi
fi
[ -n "$TOKEN" ] || { echo "    NO TOKEN — mint one on the lighthouse and pass it:"; \
  echo "      ssh root@${LH} 'mackesd add-peer --role ${ROLE}'"; \
  echo "      sudo $0 ${LH} ${ROLE} '<token>'"; exit 1; }
echo "    token: ${TOKEN:0:48}..."

echo "==> [3/4] leave the dead old mesh + join the new one"
systemctl stop mackesd 2>/dev/null || true
if ! timeout 45 mackesd leave --yes 2>/dev/null; then
  echo "    REFUSING TO JOIN: old-mesh leave did not complete; stale identity may remain" >&2
  exit 1
fi
if ! identity_is_absent /; then
  echo "    REFUSING TO JOIN: old certificate/key or role pin remains after leave" >&2
  exit 1
fi
timeout 90 mackesd join "$TOKEN" --role "$ROLE" 2>&1 | tail -6

echo "==> [4/4] verify"
sleep 3
echo "    overlay: $(ip -4 -o addr show nebula1 2>/dev/null | awk '{print $4}')"
echo "    mackesd: $(systemctl is-active mackesd 2>/dev/null)  nebula: $(systemctl is-active nebula 2>/dev/null)"
ping -c2 -W2 10.42.0.1 >/dev/null 2>&1 && echo "    lighthouse 10.42.0.1: REACHABLE ✓" || echo "    lighthouse 10.42.0.1: not reachable yet (give nebula ~10s)"
