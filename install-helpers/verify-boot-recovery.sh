#!/bin/bash
# verify-boot-recovery.sh — BOOT-REC-4: the reboot-recovery RELEASE GATE.
#
# Operator hard requirement (2026-06-16): every node MUST fully recover from a
# power outage / reboot / shutdown with ZERO manual steps. This script asserts a
# node is a healthy mesh member; run it AFTER a clean reboot (give the node a
# minute to settle) — a non-zero exit means recovery is incomplete and a release
# is gated.
#
# Run locally on the node:   verify-boot-recovery.sh
# Or remotely:               ssh <node> 'bash -s' < verify-boot-recovery.sh
#
# Modes:
#   --identity-guard  fail closed before Nebula starts if the enrolled local
#                     identity is stale, malformed, or has duplicate active
#                     flat/generation state.
#
# Checks (each is a recovery invariant):
#   1. one admitted Nebula identity and the grouped mackesd target are active.
#   2. /mnt/mesh-storage exists + the Syncthing file plane is active (SUBSTRATE-V2:
#      a plain replicated dir, no FUSE).
#   3. a strict majority of the configured etcd coordination plane can commit
#      proposals (leadership survives one member loss in a three-node quorum).
#   4. the bus answers action/shell/healthz (no readonly-DB latch — BOOT-REC-3).
#   5. on a Workstation (desktop user present): ~/Documents is a bind mountpoint
#      (FPG-7 communal sync — AUDIT-MESH-15).
set -u
QNM="${MDE_WORKGROUP_ROOT:-/mnt/mesh-storage}"
ETCD_ENDPOINTS_FILE=/etc/mackesd/etcd-endpoints
fail=0
ok()   { printf '  \033[32mok\033[0m   %s\n' "$1"; }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=1; }

identity_guard_fail() {
    printf 'identity-guard: REFUSED: %s\n' "$1" >&2
    return 1
}

trusted_regular_file() {
    local path="$1" mode
    [ -f "$path" ] && [ ! -L "$path" ] || return 1
    [ "$(stat -Lc '%u' -- "$path" 2>/dev/null)" = 0 ] || return 1
    mode="$(stat -Lc '%a' -- "$path" 2>/dev/null)" || return 1
    case "$mode" in
        *[2367][0-9]|*[0-9][2367]) return 1 ;;
    esac
}

run_identity_guard() {
    local nebula_dir="${MCNF_NEBULA_DIR:-/etc/nebula}"
    local cert_tool="${MCNF_NEBULA_CERT_BIN:-/usr/bin/nebula-cert}"
    local config cert key ca target generation

    [ "$(id -u)" -eq 0 ] || { identity_guard_fail "must-run-as-root"; return; }
    if [ -f "$nebula_dir/config.yaml" ]; then
        config="$nebula_dir/config.yaml"
    elif [ -f "$nebula_dir/config.yml" ]; then
        config="$nebula_dir/config.yml"
    else
        identity_guard_fail "missing-config"
        return
    fi
    trusted_regular_file "$config" || { identity_guard_fail "untrusted-config"; return; }

    ca="$nebula_dir/ca.crt"
    trusted_regular_file "$ca" || { identity_guard_fail "untrusted-ca"; return; }

    if [ -e "$nebula_dir/identity/current" ] || [ -L "$nebula_dir/identity/current" ]; then
        [ -d "$nebula_dir/identity" ] && [ ! -L "$nebula_dir/identity" ] \
            || { identity_guard_fail "untrusted-identity-root"; return; }
        [ "$(stat -Lc '%u:%a' -- "$nebula_dir/identity" 2>/dev/null)" = "0:700" ] \
            || { identity_guard_fail "untrusted-identity-root"; return; }
        [ -L "$nebula_dir/identity/current" ] \
            || { identity_guard_fail "current-not-symlink"; return; }
        target="$(readlink -- "$nebula_dir/identity/current")" \
            || { identity_guard_fail "current-unreadable"; return; }
        case "$target" in
            ''|.|..|/*|*/*) identity_guard_fail "current-target-unsafe"; return ;;
        esac
        generation="$nebula_dir/identity/$target"
        [ -d "$generation" ] && [ ! -L "$generation" ] \
            || { identity_guard_fail "generation-untrusted"; return; }
        [ "$(stat -Lc '%u:%a' -- "$generation" 2>/dev/null)" = "0:700" ] \
            || { identity_guard_fail "generation-untrusted"; return; }
        [ "$(readlink -- "$nebula_dir/host.crt" 2>/dev/null)" = "identity/current/host.crt" ] \
            || { identity_guard_fail "duplicate-or-stale-flat-certificate"; return; }
        [ "$(readlink -- "$nebula_dir/host.key" 2>/dev/null)" = "identity/current/host.key" ] \
            || { identity_guard_fail "duplicate-or-stale-flat-key"; return; }
        cert="$generation/host.crt"
        key="$generation/host.key"
    else
        cert="$nebula_dir/host.crt"
        key="$nebula_dir/host.key"
    fi

    trusted_regular_file "$cert" || { identity_guard_fail "untrusted-certificate"; return; }
    trusted_regular_file "$key" || { identity_guard_fail "untrusted-private-key"; return; }
    [ "$(stat -Lc '%a' -- "$key" 2>/dev/null)" = 600 ] \
        || { identity_guard_fail "private-key-mode"; return; }
    [ "$(stat -Lc '%d:%i' -- "$cert" 2>/dev/null)" != "$(stat -Lc '%d:%i' -- "$key" 2>/dev/null)" ] \
        || { identity_guard_fail "certificate-key-alias"; return; }
    [ -x "$cert_tool" ] && trusted_regular_file "$cert_tool" \
        || { identity_guard_fail "nebula-cert-untrusted"; return; }
    if ! timeout 8 "$cert_tool" verify -ca "$ca" -crt "$cert" >/dev/null 2>&1; then
        identity_guard_fail "certificate-stale-or-untrusted"
        return
    fi
    printf 'identity-guard: PASS: one current, trusted Nebula identity\n'
}

if [ "${1:-}" = "--identity-guard" ]; then
    [ "$#" -eq 1 ] || { printf '%s\n' '--identity-guard takes no arguments' >&2; exit 2; }
    run_identity_guard
    exit $?
elif [ "$#" -ne 0 ]; then
    printf 'usage: %s [--identity-guard]\n' "$0" >&2
    exit 2
fi

echo "== BOOT-REC-4 recovery gate =="

systemctl is-active --quiet nebula.service && ok "Nebula identity active" || bad "Nebula identity not active"
if run_identity_guard >/dev/null; then ok "one current, trusted Nebula identity"; else bad "Nebula identity guard refused current state"; fi
systemctl is-active --quiet mackesd.target && ok "mackesd target active" || bad "mackesd target not active"

if [ -d "$QNM" ]; then ok "$QNM present (shared dir)"; else bad "$QNM missing (file plane down)"; fi
if systemctl is-active --quiet syncthing 2>/dev/null; then ok "syncthing active (file plane)"; else bad "syncthing not active (file plane down)"; fi

if [ -s "$ETCD_ENDPOINTS_FILE" ] && command -v etcdctl >/dev/null 2>&1; then
    EPS="$(tr '\n' ',' < "$ETCD_ENDPOINTS_FILE" | sed 's/,$//')"
    IFS=',' read -r -a etcd_endpoints <<< "$EPS"
    etcd_total=0
    etcd_healthy=0
    for endpoint in "${etcd_endpoints[@]}"; do
        [ -n "$endpoint" ] || continue
        etcd_total=$((etcd_total + 1))
        if ETCDCTL_API=3 timeout 8 etcdctl --endpoints="$endpoint" endpoint health >/dev/null 2>&1; then
            etcd_healthy=$((etcd_healthy + 1))
        fi
    done
    etcd_required=$((etcd_total / 2 + 1))
    if [ "$etcd_total" -gt 0 ] && [ "$etcd_healthy" -ge "$etcd_required" ]; then
        ok "etcd strict quorum healthy ($etcd_healthy/$etcd_total; require $etcd_required)"
    else
        bad "etcd strict quorum unavailable ($etcd_healthy/$etcd_total; require $etcd_required)"
    fi
else
    ok "etcd not configured here (single-node / pre-cluster) — coordination check skipped"
fi

hz="$(MDE_BUS_ROOT=/run/mde-bus timeout 8 mde-bus request action/shell/healthz --timeout-secs 6 2>&1)"
if printf '%s' "$hz" | grep -qiE '"?(ok|ready|healthy)"?|node_count'; then
    ok "bus healthz answers"
else
    bad "bus healthz no reply (readonly-DB latch? BOOT-REC-3) — $(printf '%s' "$hz" | head -c 80)"
fi

# Workstation-only: a desktop user (uid 1000-60000 under /home) → expect the binds.
duser_home="$(awk -F: '$3>=1000 && $3<60000 && $6 ~ /^\/home/ {print $6; exit}' /etc/passwd)"
if [ -n "$duser_home" ]; then
    if mountpoint -q "$duser_home/Documents"; then
        ok "~/Documents bind-mounted (FPG-7 sync)"
    else
        bad "$duser_home/Documents not bind-mounted (AUDIT-MESH-15)"
    fi
else
    ok "no desktop user — XDG bind not expected (headless role)"
fi

echo
if [ "$fail" = 0 ]; then
    echo "BOOT-REC-4: PASS — node fully recovered."
else
    echo "BOOT-REC-4: FAIL — recovery incomplete; release gated."
fi
exit "$fail"
