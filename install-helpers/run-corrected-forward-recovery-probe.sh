#!/usr/bin/env bash
# Bounded outer controller for WL-CRIT-007/S4. A reboot is issued only after
# the streamed read-only verifier proves the exact enrolled target and the
# package-owned S2 helper/unit path. The controller never enrolls or rolls back.
set -euo pipefail

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
VERIFIER="$REPO/install-helpers/verify-corrected-forward-recovery.py"
SSH_USER=mm
SSH_KEY="${HOME}/.ssh/mackes_mesh_ed25519"
TARGET=
EXPECT_HOST=
EXPECT_ROLE=
EXPECT_OVERLAY=
SESSION_USER=
REBOOT=0
DOWN_ATTEMPTS=20
UP_ATTEMPTS=60

usage() {
    cat <<'EOF'
usage: run-corrected-forward-recovery-probe.sh --target HOST \
  --expect-host NAME --expect-role ROLE --expect-overlay CIDR \
  --session-user USER [--ssh-user USER] [--ssh-key PATH] [--reboot]

Without --reboot, performs the exact read-only destructive-action preflight.
With --reboot, waits at most 60 seconds for disconnect and 180 seconds for SSH
return, triggers the installed bounded recovery unit, then runs the post gate.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target) TARGET="${2:-}"; shift 2 ;;
        --expect-host) EXPECT_HOST="${2:-}"; shift 2 ;;
        --expect-role) EXPECT_ROLE="${2:-}"; shift 2 ;;
        --expect-overlay) EXPECT_OVERLAY="${2:-}"; shift 2 ;;
        --session-user) SESSION_USER="${2:-}"; shift 2 ;;
        --ssh-user) SSH_USER="${2:-}"; shift 2 ;;
        --ssh-key) SSH_KEY="${2:-}"; shift 2 ;;
        --reboot) REBOOT=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'recovery-probe: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

for value in "$TARGET" "$EXPECT_HOST" "$EXPECT_ROLE" "$EXPECT_OVERLAY" "$SESSION_USER" "$SSH_USER"; do
    case "$value" in
        ''|*[!A-Za-z0-9._:/-]*) printf 'recovery-probe: malformed or missing identity argument\n' >&2; exit 2 ;;
    esac
done
[ -f "$VERIFIER" ] && [ ! -L "$VERIFIER" ] || { printf 'recovery-probe: verifier unavailable\n' >&2; exit 2; }
[ -f "$SSH_KEY" ] && [ ! -L "$SSH_KEY" ] || { printf 'recovery-probe: SSH key unavailable\n' >&2; exit 2; }

SSH=(/usr/bin/ssh -i "$SSH_KEY" -o BatchMode=yes -o ConnectTimeout=5 -o StrictHostKeyChecking=accept-new "$SSH_USER@$TARGET")
VERIFY_ARGS=(--expect-host "$EXPECT_HOST" --expect-role "$EXPECT_ROLE" --expect-overlay "$EXPECT_OVERLAY" --session-user "$SESSION_USER")

remote_verify() {
    local mode=$1
    shift
    "${SSH[@]}" sudo -n /usr/bin/python3 - "$mode" "${VERIFY_ARGS[@]}" "$@" <"$VERIFIER"
}

printf 'recovery-probe: read-only preflight target=%s expected_host=%s\n' "$TARGET" "$EXPECT_HOST"
preflight="$(remote_verify preflight)" || {
    printf 'recovery-probe: REFUSED before reboot; target was not mutated\n' >&2
    exit 1
}
boot_id="$(/usr/bin/python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["boot_id"])' <<<"$preflight")"
printf 'recovery-probe: preflight PASS package-bound boot_id=%s\n' "$boot_id"
if [ "$REBOOT" -eq 0 ]; then
    printf 'recovery-probe: preflight-only PASS; --reboot was not supplied\n'
    exit 0
fi

printf 'recovery-probe: issuing one bounded reboot to exact target %s\n' "$EXPECT_HOST"
"${SSH[@]}" sudo -n /usr/libexec/mackesd/seat-update-warning || {
    printf 'recovery-probe: REFUSED: mandatory visible five-second warning failed; reboot was not issued\n' >&2
    exit 1
}
"${SSH[@]}" sudo -n /usr/bin/systemctl reboot >/dev/null 2>&1 || true

went_down=0
for ((attempt=1; attempt<=DOWN_ATTEMPTS; attempt++)); do
    if ! "${SSH[@]}" /usr/bin/true >/dev/null 2>&1; then
        went_down=1
        break
    fi
    /usr/bin/sleep 3
done
[ "$went_down" -eq 1 ] || { printf 'recovery-probe: REFUSED: target never disconnected after reboot request\n' >&2; exit 1; }

came_back=0
for ((attempt=1; attempt<=UP_ATTEMPTS; attempt++)); do
    if "${SSH[@]}" /usr/bin/true >/dev/null 2>&1; then
        came_back=1
        break
    fi
    /usr/bin/sleep 3
done
[ "$came_back" -eq 1 ] || { printf 'recovery-probe: FAIL: target did not return over SSH within 180 seconds\n' >&2; exit 1; }

printf 'recovery-probe: target returned; exercising installed network-return recovery unit\n'
"${SSH[@]}" sudo -n /usr/bin/timeout 100 /usr/bin/systemctl start mcnf-peer-recovery.service
# Type=notify deliberately reports READY before the bounded orchestration has
# finished.  Do not race the read-only post gate against Syncthing/XDG/worker
# recovery: wait for the transient service to settle and require its terminal
# Result to be success.  The fixed remote program has no caller-controlled
# shell fragments and the outer timeout remains below the unit's own 90s cap.
"${SSH[@]}" sudo -n /usr/bin/timeout 95 /usr/bin/bash -s <<'RECOVERY_WAIT' || {
  for attempt in $(seq 1 90); do
    state=$(/usr/bin/systemctl show mcnf-peer-recovery.service -p ActiveState --value)
    case "$state" in
      inactive)
        result=$(/usr/bin/systemctl show mcnf-peer-recovery.service -p Result --value)
        [ "$result" = success ]
        exit
        ;;
      failed) exit 1 ;;
      active|activating|deactivating) /usr/bin/sleep 1 ;;
      *) exit 1 ;;
    esac
  done
  exit 1
RECOVERY_WAIT
    printf 'recovery-probe: FAIL: installed recovery unit did not settle successfully\n' >&2
    exit 1
}
post="$(remote_verify post-reboot --before-boot-id "$boot_id")" || {
    printf 'recovery-probe: FAIL: corrected-forward post-reboot verification refused\n' >&2
    exit 1
}
printf '%s\n' "$post"
printf 'recovery-probe: PASS target=%s path=reboot+network-return\n' "$EXPECT_HOST"
