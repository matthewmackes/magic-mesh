#!/usr/bin/env bash
# Bounded, observation-only live provider-loss probe for WL-FUNC-021.
#
# This helper never interrupts a provider, playback, network interface, or
# service.  It can pass only when the approved seat naturally reports a
# healthy provider, then a provider-only loss while the daemon stays healthy,
# then provider recovery during the bounded observation window.  A healthy
# seat with no naturally occurring outage is an honest refusal, not a pass.
set -Eeuo pipefail

HOST="${MUSIC_LIVE_HOST:-172.20.0.15}"
USER_="${MUSIC_LIVE_USER:-mm}"
SSH_KEY="${MUSIC_LIVE_SSH_KEY:-${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}}"
BUS_ROOT="${MUSIC_LIVE_BUS_ROOT:-/run/mde-bus}"
SSH_TIMEOUT="${MUSIC_LIVE_PROVIDER_LOSS_SSH_TIMEOUT_SECONDS:-75}"
COMMAND_TIMEOUT="${MUSIC_LIVE_PROVIDER_LOSS_COMMAND_TIMEOUT_SECONDS:-6}"
OBSERVE_SECONDS="${MUSIC_LIVE_PROVIDER_LOSS_OBSERVE_SECONDS:-45}"
SAMPLE_INTERVAL="${MUSIC_LIVE_PROVIDER_LOSS_SAMPLE_INTERVAL_SECONDS:-2}"

RUN_DIR=""
SSH_PID=""

usage() {
    cat >&2 <<'EOF'
usage: verify-music-live-provider-loss.sh [--self-test]

Runs a bounded, read-only observation against the approved Music seat.  The
probe samples mde-musicd ping, action/music/list-albums,
action/music/get-state, and the user service state.  It passes only after a
natural healthy -> provider-unavailable -> healthy transition while the
daemon remains active.  It never induces provider loss and refuses honestly
when no such transition is observed.

Environment: MUSIC_LIVE_HOST, MUSIC_LIVE_USER, MUSIC_LIVE_SSH_KEY,
MUSIC_LIVE_BUS_ROOT, MUSIC_LIVE_PROVIDER_LOSS_SSH_TIMEOUT_SECONDS (10..120),
MUSIC_LIVE_PROVIDER_LOSS_COMMAND_TIMEOUT_SECONDS (1..20),
MUSIC_LIVE_PROVIDER_LOSS_OBSERVE_SECONDS (6..110), and
MUSIC_LIVE_PROVIDER_LOSS_SAMPLE_INTERVAL_SECONDS (1..10).
EOF
}

fail() {
    printf '[FAIL] %s\n' "$1" >&2
    exit 2
}

refuse() {
    printf '[REFUSAL] %s\n' "$1" >&2
    exit 3
}

bounded_integer() {
    local value=$1 minimum=$2 maximum=$3
    [[ "$value" =~ ^[0-9]+$ ]] && (( value >= minimum && value <= maximum ))
}

classify_sample() {
    local service=$1 provider=$2 catalog=$3 state=$4
    if [[ "$service" != active || "$state" != ok ]]; then
        printf 'infrastructure'
    elif [[ "$provider" == ok && "$catalog" == ok ]]; then
        printf 'healthy'
    elif [[ "$provider" == unavailable && "$catalog" == ok ]]; then
        printf 'provider_loss'
    else
        printf 'ambiguous'
    fi
}

validate_config() {
    [[ "$HOST" =~ ^[A-Za-z0-9._:-]+$ ]] ||
        fail 'live host contains unsupported characters'
    [[ "$USER_" =~ ^[A-Za-z0-9._-]+$ ]] ||
        fail 'live user contains unsupported characters'
    [[ "$BUS_ROOT" == /* && "$BUS_ROOT" != *[[:space:]]* ]] ||
        fail 'Bus root must be an absolute path without whitespace'
    [[ -n "$SSH_KEY" ]] || fail 'SSH key path must be non-empty'
    bounded_integer "$SSH_TIMEOUT" 10 120 ||
        fail 'SSH timeout must be 10..120 seconds'
    bounded_integer "$COMMAND_TIMEOUT" 1 20 ||
        fail 'command timeout must be 1..20 seconds'
    bounded_integer "$OBSERVE_SECONDS" 6 110 ||
        fail 'observation window must be 6..110 seconds'
    bounded_integer "$SAMPLE_INTERVAL" 1 10 ||
        fail 'sample interval must be 1..10 seconds'
    (( SSH_TIMEOUT > OBSERVE_SECONDS )) ||
        fail 'SSH timeout must exceed the observation window'
}

self_test() {
    [[ "$(classify_sample active ok ok ok)" == healthy ]]
    [[ "$(classify_sample active unavailable unavailable ok)" == ambiguous ]]
    [[ "$(classify_sample inactive ok ok ok)" == infrastructure ]]
    [[ "$(classify_sample active unavailable ok ok)" == provider_loss ]]
    [[ "$(classify_sample active ok unavailable ok)" == ambiguous ]]
    ! bounded_integer 0 1 10
    bounded_integer 10 1 10
    ! bounded_integer 11 1 10
    printf 'verify-music-live-provider-loss: self-test passed (no SSH attempted)\n'
}

cleanup() {
    local status=$?
    trap - EXIT INT TERM
    if [[ -n "$SSH_PID" ]] && kill -0 "$SSH_PID" 2>/dev/null; then
        kill -TERM "$SSH_PID" 2>/dev/null || true
        wait "$SSH_PID" 2>/dev/null || true
    fi
    if [[ -n "$RUN_DIR" && -d "$RUN_DIR" ]]; then
        rm -rf -- "$RUN_DIR"
    fi
    exit "$status"
}

if [[ "${1:-}" == '--self-test' ]]; then
    [[ "$#" == 1 ]] || fail '--self-test takes no additional arguments'
    self_test
    exit 0
fi
if [[ "${1:-}" == '--help' || "${1:-}" == '-h' ]]; then
    usage
    exit 0
fi
[[ "$#" == 0 ]] || { usage; fail "unknown argument: $1"; }

validate_config
command -v ssh >/dev/null 2>&1 || fail 'ssh is required'
command -v timeout >/dev/null 2>&1 || fail 'timeout is required'
[[ -f "$SSH_KEY" && -r "$SSH_KEY" ]] ||
    refuse 'configured SSH key is unavailable; no live proof was attempted'

RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mcnf-music-live-loss.XXXXXX")"
trap cleanup EXIT INT TERM

printf '== Music live provider-loss observation (%s@%s) ==\n' "$USER_" "$HOST"
printf '[INFO] observation-only; no provider interruption or playback is requested\n'

SSH_PID=''
timeout --foreground --signal=TERM --kill-after=3s "${SSH_TIMEOUT}s" \
    ssh -i "$SSH_KEY" \
        -o BatchMode=yes \
        -o ConnectTimeout=10 \
        -o ServerAliveInterval=5 \
        -o ServerAliveCountMax=1 \
        -o StrictHostKeyChecking=accept-new \
        -- "$USER_@$HOST" bash -s -- \
        "$BUS_ROOT" "$COMMAND_TIMEOUT" "$OBSERVE_SECONDS" "$SAMPLE_INTERVAL" \
        >"$RUN_DIR/output" 2>"$RUN_DIR/ssh-error" <<'REMOTE_SCRIPT' &
set -Eeuo pipefail

bus_root=$1
command_timeout=$2
observe_seconds=$3
sample_interval=$4

if [[ "$bus_root" != /* || "$bus_root" == *[[:space:]]* ]]; then
    printf '[REFUSAL] remote Bus root is not a safe absolute path\n'
    exit 3
fi
for required in bash id mde-musicd mde-bus systemctl timeout sleep; do
    command -v "$required" >/dev/null 2>&1 || {
        printf '[REFUSAL] remote prerequisite unavailable: %s\n' "$required"
        exit 3
    }
done

uid="$(id -u)"

sample() {
    local service=inactive provider=unavailable catalog=unavailable state=failed

    if timeout --signal=TERM --kill-after=1s "${command_timeout}s" \
        systemctl --user is-active --quiet mde-musicd.service \
        >/dev/null 2>&1; then
        service=active
    fi

    # All command bodies are discarded.  In particular, no provider URL,
    # catalog row, Bus reply, or daemon error can become probe output.
    if timeout --signal=TERM --kill-after=1s "${command_timeout}s" \
        mde-musicd ping --retry 0 >/dev/null 2>&1; then
        provider=ok
    fi
    if timeout --signal=TERM --kill-after=1s "${command_timeout}s" \
        env XDG_RUNTIME_DIR="/run/user/$uid" mde-bus request action/music/list-albums \
        --bus-root "$bus_root" --timeout-secs "$command_timeout" --json \
        >/dev/null 2>&1; then
        catalog=ok
    fi
    if timeout --signal=TERM --kill-after=1s "${command_timeout}s" \
        env XDG_RUNTIME_DIR="/run/user/$uid" mde-bus request action/music/get-state \
        --bus-root "$bus_root" --timeout-secs "$command_timeout" --json \
        >/dev/null 2>&1; then
        state=ok
    fi
    printf '%s %s %s %s\n' "$service" "$provider" "$catalog" "$state"
}

classify() {
    local service=$1 provider=$2 catalog=$3 state=$4
    if [[ "$service" != active || "$state" != ok ]]; then
        printf 'infrastructure'
    elif [[ "$provider" == ok && "$catalog" == ok ]]; then
        printf 'healthy'
    elif [[ "$provider" == unavailable && "$catalog" == ok ]]; then
        printf 'provider_loss'
    else
        printf 'ambiguous'
    fi
}

healthy_seen=0
loss_seen=0
sample_count=0
deadline=$((SECONDS + observe_seconds))

while (( SECONDS <= deadline )); do
    IFS=' ' read -r service provider catalog state < <(sample)
    class="$(classify "$service" "$provider" "$catalog" "$state")"
    sample_count=$((sample_count + 1))
    printf '[sample %d] service=%s provider=%s catalog=%s state=%s class=%s\n' \
        "$sample_count" "$service" "$provider" "$catalog" "$state" "$class"

    case "$class" in
        healthy)
            if (( loss_seen )); then
                printf '[PASS] natural provider recovery observed after %d samples\n' \
                    "$sample_count"
                exit 0
            fi
            healthy_seen=1
            ;;
        provider_loss)
            if (( healthy_seen )); then
                loss_seen=1
                printf '[INFO] provider loss observed while daemon remained healthy\n'
            fi
            ;;
        infrastructure)
            if (( loss_seen )); then
                printf '[REFUSAL] daemon infrastructure was not continuously healthy\n'
                exit 3
            fi
            ;;
        ambiguous)
            if (( loss_seen )); then
                printf '[REFUSAL] provider sample was ambiguous during recovery\n'
                exit 3
            fi
            ;;
    esac

    (( SECONDS >= deadline )) && break
    sleep "$sample_interval"
done

if (( loss_seen )); then
    printf '[REFUSAL] provider loss was observed, but recovery was not observed before the deadline\n'
elif (( healthy_seen )); then
    printf '[REFUSAL] no natural provider loss was observed before the deadline\n'
else
    printf '[REFUSAL] no healthy baseline was established; live loss is unverifiable\n'
fi
exit 3
REMOTE_SCRIPT
SSH_PID=$!

ssh_rc=0
wait "$SSH_PID" || ssh_rc=$?
SSH_PID=''

if (( ssh_rc == 0 )); then
    cat "$RUN_DIR/output"
    printf 'verify-music-live-provider-loss: PASS\n'
    exit 0
fi

cat "$RUN_DIR/output"
if (( ssh_rc == 124 )); then
    refuse 'bounded SSH observation timed out; no complete live transition proof exists'
elif (( ssh_rc == 3 )); then
    # The remote observer already emitted the specific refusal (for example,
    # a healthy window with no natural outage). Preserve that evidence without
    # adding a contradictory duplicate line. A prerequisite refusal that did
    # not reach the observer still gets a concise wrapper-level reason.
    if grep -q '^\[REFUSAL\]' "$RUN_DIR/output"; then
        exit 3
    fi
    refuse 'live provider-loss transition was not fully observed'
else
    refuse "live seat could not be observed (bounded SSH rc=$ssh_rc)"
fi
