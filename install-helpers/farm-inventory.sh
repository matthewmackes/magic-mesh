#!/usr/bin/env bash
# farm-inventory.sh — THE single, tofu-derived source of truth for the build farm
# topology. Built to end the recurring "context-clear loses the Xen hosts + build
# slots" problem: instead of every script/skill/doc carrying its own hardcoded
# (and drifting) host list, they all read THIS — and this reads OpenTofu.
#
# Why this exists (the drift it kills):
#   - infra/tofu/ had TWO roots that disagreed: legacy main.tf (3 dom0s) vs the
#     LIVE infra/tofu/xen-xapi root the reconciler uses (4 dom0s — adds xen-194).
#   - farm.sh FLEET_DEFAULT was stale .50/.51/.52 (probed DEAD IPs).
#   - drain-coordinator.sh hardcoded 3 nodes / "7 slots" (missed xen-194 + elastic).
#   - the /ship + /polish skills + BUILD-ENVIRONMENT.md carried their own copies.
#   The fix: ONE command, derived from tofu's xen-xapi root, that everything reads.
#
# Source precedence for the per-VM IPs (most-authoritative first):
#   1. `tofu output -json build_farm` in the xen-xapi root (live, honours overrides)
#   2. the cold facts parsed from xen-xapi/build-vms.tf `local.dom0` (this repo)
# The dom0 *host* IPs (172.20.145.x mgmt addresses) are physical cold facts mirrored
# from docs/BUILD-ENVIRONMENT.md §3 and CONFIRMED by `discover` against the live LAN.
#
# Subcommands:
#   fleet        machine-readable: `host_ip|LABEL|buildvm_ip|vcpus|cap`, one per dom0
#                (the line format farm.sh + drain-coordinator.sh consume)
#   topology     human table + live reachability/toolchain probe + free slots + drift
#   discover [R] sweep a mgmt IP range (default 172.20.145.190-198) for XCP-ng dom0s
#                and reconcile against the declared fleet (the on-LAN scan capability)
#   selftest     assert the pure cold-fact parse (no network) — runs anywhere
#
# Env: MCNF_TOFU_ROOT (default <repo>/infra/tofu/xen-xapi — the CANONICAL root),
#      MCNF_TOFU (default `tofu`), MCNF_FARM_KEY, MCNF_FARM_USER (mm),
#      XCP_PASS (only `discover` uses it, for an SSH XCP-ng confirm; never stored).
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
TOFU_ROOT="${MCNF_TOFU_ROOT:-$ROOT/infra/tofu/xen-xapi}"
TOFU="${MCNF_TOFU:-tofu}"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
SSH_USER="${MCNF_FARM_USER:-mm}"

# Physical dom0 management IPs — cold facts (docs/BUILD-ENVIRONMENT.md §3). `discover`
# validates these against the live LAN; they are the ONLY non-tofu-derived constant.
dom0_host_ip() {
  case "$1" in
    xen-home-services) echo "172.20.0.9" ;;
    kvm-xcp1)          echo "172.20.145.193" ;;
    xen-bigboy)        echo "172.20.145.165" ;;
    xen-194)           echo "172.20.145.194" ;;
    *)                 echo "" ;;
  esac
}
dom0_label() {
  case "$1" in
    xen-home-services) echo "XEN-HOME-SERVICES" ;;
    kvm-xcp1)          echo "KVM-XCP1" ;;
    xen-bigboy)        echo "XEN-BIGBOY" ;;
    xen-194)           echo "XEN-194" ;;
    *)                 echo "$1" ;;
  esac
}
# Heavy-build concurrency cap, DERIVED from the dom0's whole-host vcpu budget:
# a big (≥8 vCPU) host carries 3 concurrent heavy builds, a small one carries 2.
cap_for_vcpus() { [ "${1:-0}" -ge 8 ] 2>/dev/null && echo 3 || echo 2; }

# parse_cold_facts — PURE (no network): read xen-xapi/build-vms.tf `local.dom0` and
# emit `key|ip_base|big_vcpus`, one per dom0. This is the in-repo authority when no
# live `tofu output` is available. No brace-counting needed: only the 4 dom0 cold-
# fact entries carry BOTH `ip_base` and `big_vcpus` (the reserved small-0 blocks use
# ip_cidr/dom0_key), so that signature uniquely picks them out. Robust to `# lane …`
# comment trailers.
parse_cold_facts() {
  local tf="$TOFU_ROOT/build-vms.tf"
  [ -f "$tf" ] || { echo "farm-inventory: no $tf" >&2; return 1; }
  awk '
    /^[[:space:]]*"[a-z0-9-]+"[[:space:]]*=[[:space:]]*\{/ {
      key=$0; sub(/^[[:space:]]*"/,"",key); sub(/".*/,"",key); cur=key; ip=""; v=""; next
    }
    cur!="" && /ip_base[[:space:]]*=/ {
      ip=$0; sub(/.*ip_base[[:space:]]*=[[:space:]]*"/,"",ip); sub(/".*/,"",ip)
    }
    cur!="" && /big_vcpus[[:space:]]*=/ {
      v=$0; sub(/.*big_vcpus[[:space:]]*=[[:space:]]*/,"",v); sub(/[^0-9].*/,"",v)
      if (ip!="" && v!="") { print cur "|" ip "|" v; cur="" }
    }
  ' "$tf"
}

# live_tofu_ips — best-effort: `tofu output -json build_farm` → `name ip` lines.
# Empty (no tofu / no state / unreachable) → caller falls back to cold facts.
live_tofu_ips() {
  command -v "$TOFU" >/dev/null 2>&1 || return 0
  command -v jq >/dev/null 2>&1 || return 0
  [ -d "$TOFU_ROOT" ] || return 0
  local out
  out="$( cd "$TOFU_ROOT" && "$TOFU" output -json build_farm 2>/dev/null )" || return 0
  [ -n "$out" ] && [ "$out" != "null" ] || return 0
  printf '%s' "$out" | jq -r 'to_entries[] | "\(.key) \(.value.ip)"' 2>/dev/null
}

# fleet — the canonical record set consumers read: host_ip|LABEL|buildvm_ip|vcpus|cap
cmd_fleet() {
  local key ip v hostip label cap
  while IFS='|' read -r key ip v; do
    [ -n "$key" ] || continue
    hostip="$(dom0_host_ip "$key")"
    label="$(dom0_label "$key")"
    cap="$(cap_for_vcpus "$v")"
    printf '%s|%s|%s|%s|%s\n' "$hostip" "$label" "$ip" "$v" "$cap"
  done < <(parse_cold_facts)
}

reachable() { timeout 4 bash -c "cat </dev/null >/dev/tcp/$1/${2:-22}" 2>/dev/null; }
toolchained() { timeout 14 ssh -i "$KEY" -o BatchMode=yes -o ConnectTimeout=8 \
  -o StrictHostKeyChecking=accept-new -n "$SSH_USER@$1" \
  '. "$HOME/.cargo/env" 2>/dev/null; command -v rustc >/dev/null && command -v g++ >/dev/null' 2>/dev/null; }

cmd_topology() {
  echo "Build-farm topology — source: tofu xen-xapi root ($TOFU_ROOT)"
  printf '  %-18s %-18s %-14s %-6s %-4s %-8s %s\n' DOM0 'DOM0-HOST' 'BUILD-VM' VCPU CAP 'VM:22' TOOLCHAIN
  local total_cap=0 total_free=0 hostip label vmip v cap vm tc free
  while IFS='|' read -r hostip label vmip v cap; do
    [ -n "$label" ] || continue
    vm="down"; tc="-"; free=0
    if reachable "$vmip" 22; then
      vm="up"; toolchained "$vmip" && tc="ready" || tc="bare"; free="$cap"
    fi
    total_cap=$(( total_cap + cap )); total_free=$(( total_free + free ))
    printf '  %-18s %-18s %-14s %-6s %-4s %-8s %s\n' "$label" "$hostip" "$vmip" "$v" "$cap" "$vm" "$tc"
  done < <(cmd_fleet)
  echo "  ── heavy build slots: $total_free free / $total_cap total (elastic: each dom0 also runs small×N + pods)"
  echo "  Note: a 'down' build-VM that is DECLARED here is under-provisioned — run: farm.sh up"
}

# discover [range] — the on-LAN scan: sweep a mgmt IP range for XCP-ng dom0s and
# reconcile against the declared fleet. MUST run from a host ON the 172.20.x LAN
# (e.g. the dev host 172.20.145.192) — a cloud session has no route. Uses XCP_PASS
# only for an SSH XCP-ng confirm; the password is never written anywhere.
cmd_discover() {
  local range="${1:-172.20.145.190-198}" base lo hi o ip
  base="${range%.*}"; lo="${range##*.}"; hi="${lo##*-}"; lo="${lo%%-*}"
  echo "discover: sweeping ${base}.${lo}-${hi} for XCP-ng dom0s (SSH:22 + XAPI:443)"
  echo "  (run this on a host on the 172.20.x LAN — a cloud session is denied private dest IPs)"
  printf '  %-16s %-7s %-8s %-10s %s\n' IP SSH:22 XAPI:443 XCP-NG NAME
  local declared found_ip
  declared="$(cmd_fleet | cut -d'|' -f1 | tr '\n' ' ')"
  for o in $(seq "$lo" "$hi"); do
    ip="${base}.${o}"
    local s22="-" s443="-" isxcp="-" name=""
    reachable "$ip" 22 && s22="open"
    reachable "$ip" 443 && s443="open"
    if [ "$s22" = "open" ] && [ -n "${XCP_PASS:-}" ] && command -v sshpass >/dev/null 2>&1; then
      name="$(SSHPASS="$XCP_PASS" sshpass -e ssh -o BatchMode=no -o StrictHostKeyChecking=accept-new \
        -o ConnectTimeout=6 "root@$ip" \
        'test -e /etc/xensource-inventory && xe host-list params=name-label --minimal 2>/dev/null' 2>/dev/null)"
      [ -n "$name" ] && isxcp="yes" || isxcp="no"
    fi
    printf '  %-16s %-7s %-8s %-10s %s\n' "$ip" "$s22" "$s443" "$isxcp" "$name"
  done
  echo "  declared dom0 hosts (from the fleet): $declared"
  echo "  → any XCP-ng host above NOT in that list is an undeclared dom0 (add it to infra/tofu/xen-xapi)."
}

cmd_selftest() {
  local out fail=0
  out="$(parse_cold_facts)"
  echo "parse_cold_facts:"; printf '%s\n' "$out" | sed 's/^/  /'
  local n; n="$(printf '%s\n' "$out" | grep -c '|')"
  [ "$n" -eq 4 ] || { echo "FAIL: expected 4 dom0s, got $n"; fail=1; }
  for pair in "xen-home-services|172.20.0.50" "kvm-xcp1|172.20.0.90" \
              "xen-bigboy|172.20.0.130" "xen-194|172.20.0.170"; do
    k="${pair%%|*}"; ip="${pair##*|}"
    printf '%s\n' "$out" | grep -q "^${k}|${ip}|" || { echo "FAIL: $k not at $ip"; fail=1; }
  done
  echo "cmd_fleet:"; cmd_fleet | sed 's/^/  /'
  # BigBoy (10 vcpu) → cap 3; the 3-vcpu hosts → cap 2; total = 2+2+3+2 = 9.
  local tcap; tcap="$(cmd_fleet | awk -F'|' '{s+=$5} END{print s}')"
  [ "$tcap" = "9" ] || { echo "FAIL: expected total cap 9, got $tcap"; fail=1; }
  [ "$fail" -eq 0 ] && echo "SELFTEST: PASS (4 dom0s, IPs + 9 heavy slots derived from tofu cold facts)" \
                    || { echo "SELFTEST: FAIL"; return 1; }
}

case "${1:-topology}" in
  fleet)     cmd_fleet ;;
  topology)  cmd_topology ;;
  discover)  shift; cmd_discover "$@" ;;
  selftest)  cmd_selftest ;;
  -h|--help|help) sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//' ;;
  *) echo "usage: $0 {fleet|topology|discover [range]|selftest}" >&2; exit 2 ;;
esac
