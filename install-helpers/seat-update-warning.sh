#!/usr/bin/env bash
# Publish the mandatory visible seat-update notice and hold the five-second
# operator window before the caller mutates or restarts the seat.
set -euo pipefail

readonly WAIT_SECONDS=5
readonly TOAST_TOPIC="event/toast/show"
readonly TOAST_BODY='{"severity":"warning","source_host":"deployment-controller","flag":"AI-GENERATED-ALERT","headline":"This seat will update in 5 seconds."}'

if [[ "${1:-}" == "--self-test" ]]; then
  [[ "$WAIT_SECONDS" -eq 5 ]]
  [[ "$TOAST_TOPIC" == "event/toast/show" ]]
  [[ "$TOAST_BODY" == *'"flag":"AI-GENERATED-ALERT"'* ]]
  [[ "$TOAST_BODY" == *'"severity":"warning"'* ]]
  echo "seat-update-warning: self-test passed"
  exit 0
fi

if (($#)); then
  echo "usage: $0 [--self-test]" >&2
  exit 2
fi

command -v mde-bus >/dev/null 2>&1 || {
  echo "seat-update-warning: mde-bus is required; update not started" >&2
  exit 1
}

export MDE_BUS_ROOT="${MDE_BUS_ROOT:-/run/mde-bus}"
mde-bus publish "$TOAST_TOPIC" --body-flag "$TOAST_BODY" >/dev/null
echo "seat-update-warning: visible warning published; waiting ${WAIT_SECONDS}s"
sleep "$WAIT_SECONDS"
