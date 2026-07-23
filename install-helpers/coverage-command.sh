#!/usr/bin/env bash
# coverage-command.sh — the canonical current-workspace coverage invocation.
#
# Keep this command in one checked-in file so the farm gate and any hosted CI
# runner measure the same denominator. Binary entrypoints are excluded because
# they are daemon/CLI bootstrap glue; all workspace libraries remain included.
# The farm wrapper (xcp-build.sh coverage) provisions cargo-llvm-cov and the
# llvm-tools component before executing this file.
set -euo pipefail

FLOOR="${MCNF_COVERAGE_FLOOR:-80}"
case "$FLOOR" in
  ''|*[!0-9]*)
    echo "coverage-command.sh: MCNF_COVERAGE_FLOOR must be an integer (got '$FLOOR')" >&2
    exit 2
    ;;
esac

exec cargo llvm-cov --workspace --locked \
  --features mackesd/async-services \
  --ignore-filename-regex '(/bin/[^/]+\.rs|/main\.rs)$' \
  --fail-under-lines "$FLOOR" \
  --summary-only \
  -- --test-threads=1
