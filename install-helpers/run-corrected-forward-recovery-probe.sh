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
SELF_TEST=0
DOWN_ATTEMPTS=20
UP_ATTEMPTS=60

usage() {
    cat <<'EOF'
usage: run-corrected-forward-recovery-probe.sh --target HOST \
  --expect-host NAME --expect-role ROLE --expect-overlay CIDR \
  --session-user USER [--ssh-user USER] [--ssh-key PATH] [--reboot]
       run-corrected-forward-recovery-probe.sh --self-test

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
        --self-test) SELF_TEST=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'recovery-probe: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

validate_return_generation() {
    local before=$1 after=$2
    /usr/bin/python3 - "$before" "$after" <<'PY'
import json
import re
import sys

identifier = re.compile(r"^[A-Za-z0-9._:-]{1,128}$")
package = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+~:-]{0,255}$")
overlay = re.compile(r"^(?:[0-9]{1,3}\.){3}[0-9]{1,3}/(?:[0-9]|[12][0-9]|3[0-2])$")
payload = re.compile(r"^sha256:[0-9a-f]{64}$")

try:
    before = json.loads(sys.argv[1])
    after = json.loads(sys.argv[2])
except (IndexError, json.JSONDecodeError) as error:
    raise SystemExit(f"recovery-probe: REFUSED: malformed return evidence: {error}")

authority = ("target", "role", "session_user")
for field in authority:
    value = before.get(field)
    if not isinstance(value, str) or not identifier.fullmatch(value):
        raise SystemExit(f"recovery-probe: REFUSED: invalid preflight authority: {field}")
    if after.get(field) != value:
        raise SystemExit(f"recovery-probe: REFUSED: returning boot changed authority: {field}")
before_package = before.get("package")
if not isinstance(before_package, str) or not package.fullmatch(before_package):
    raise SystemExit("recovery-probe: REFUSED: invalid preflight authority: package")
if after.get("package") != before_package:
    raise SystemExit("recovery-probe: REFUSED: returning boot changed authority: package")
before_overlay = before.get("overlay")
if not isinstance(before_overlay, str) or not overlay.fullmatch(before_overlay):
    raise SystemExit("recovery-probe: REFUSED: invalid preflight authority: overlay")
if after.get("overlay") != before_overlay:
    raise SystemExit("recovery-probe: REFUSED: returning boot changed authority: overlay")

before_payload = before.get("package_payload_digest")
after_payload = after.get("package_payload_digest")
if not isinstance(before_payload, str) or not payload.fullmatch(before_payload) \
        or before_payload == "sha256:" + "0" * 64:
    raise SystemExit("recovery-probe: REFUSED: invalid preflight package payload")
if after_payload != before_payload:
    raise SystemExit("recovery-probe: REFUSED: returning boot changed package payload")

before_boot = before.get("boot_id")
after_boot = after.get("boot_id")
if not isinstance(before_boot, str) or not identifier.fullmatch(before_boot):
    raise SystemExit("recovery-probe: REFUSED: invalid preflight boot generation")
if not isinstance(after_boot, str) or not identifier.fullmatch(after_boot):
    raise SystemExit("recovery-probe: REFUSED: invalid returning boot generation")
if after_boot == before_boot:
    raise SystemExit("recovery-probe: REFUSED: returning boot did not advance generation")
PY
}

run_self_test() {
    local before good hostile
    before='{"target":"seat-15","role":"workstation","overlay":"172.20.0.15/24","session_user":"mm","package":"magic-mesh-33.0.0-1.x86_64","package_payload_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","boot_id":"boot-before"}'
    good='{"target":"seat-15","role":"workstation","overlay":"172.20.0.15/24","session_user":"mm","package":"magic-mesh-33.0.0-1.x86_64","package_payload_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","boot_id":"boot-after"}'
    validate_return_generation "$before" "$good"
    for hostile in \
        "${good/boot-after/boot-before}" \
        "${good/seat-15/seat-16}" \
        "${good/magic-mesh-33.0.0-1.x86_64/magic-mesh-32.0.0-1.x86_64}" \
        "${good/sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb}"; do
        if validate_return_generation "$before" "$hostile" >/dev/null 2>&1; then
            printf '%s\n' 'recovery-probe: self-test accepted hostile return evidence' >&2
            return 1
        fi
    done
    printf '%s\n' 'recovery-probe: self-test passed 5/5 (boot generation + target/package/payload binding)'
}

if [ "$SELF_TEST" -eq 1 ]; then
    [ "$REBOOT" -eq 0 ] && [ -z "$TARGET$EXPECT_HOST$EXPECT_ROLE$EXPECT_OVERLAY$SESSION_USER" ] \
        || { printf '%s\n' 'recovery-probe: --self-test cannot be combined with live arguments' >&2; exit 2; }
    run_self_test
    exit $?
fi

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

printf 'recovery-probe: target returned; binding recovery to exact boot generation and package payload\n'
return_preflight="$(remote_verify preflight)" || {
    printf 'recovery-probe: REFUSED: returning boot failed read-only package/identity admission\n' >&2
    exit 1
}
validate_return_generation "$preflight" "$return_preflight" || {
    printf 'recovery-probe: REFUSED: returning boot differs from authorized preflight; recovery was not started\n' >&2
    exit 1
}

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
validate_return_generation "$preflight" "$post" || {
    printf 'recovery-probe: FAIL: post-recovery authority differs from authorized return generation\n' >&2
    exit 1
}
printf '%s\n' "$post"
printf 'recovery-probe: PASS target=%s path=reboot+network-return\n' "$EXPECT_HOST"
