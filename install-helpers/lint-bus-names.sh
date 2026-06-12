#!/bin/sh
# install-helpers/lint-bus-names.sh — §2 no-private-D-Bus-name gate (AUD-21).
#
# §2: the platform's IPC is the mesh Bus, not D-Bus. The only D-Bus allowed is
# interop with EXISTING freedesktop interfaces (`org.freedesktop.*`, and the FDO
# MPRIS standard `org.mpris.*`). Registering a NEW MDE-private well-known name
# (dev/org/com.mackes.*) as a server is forbidden — single-instance, focus
# hand-off, etc. ride the Bus instead.
#
# This gate fails if any crate calls `request_name` / `request_name_with_flags`
# (the D-Bus name-claim) while a private `*.mackes.*` name literal appears on the
# same line or its neighbours. Pure interop (`org.freedesktop.*` / `org.mpris.*`)
# passes; a `const APP_ID = "com.mackes.…"` that is NOT a D-Bus name-claim passes.
#
# Run with `--self-test` to verify. Exit 0 = clean, 1 = a violation.

set -eu
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

scan() {
    _root="$1"
    # Any request_name call with a private mackes name within 1 line of context.
    grep -rnE -A1 -B1 'request_name' "$_root"/crates --include='*.rs' 2>/dev/null \
        | grep -E '"(dev|org|com)\.mackes\.' \
        || true
}

if [ "${1:-}" = "--self-test" ]; then
    _tmp="$(mktemp -d)"
    mkdir -p "$_tmp/crates/x/src"
    printf 'let wk = WellKnownName::try_from("dev.mackes.MDE.Foo")?;\nconn.request_name(wk)?;\n' \
        > "$_tmp/crates/x/src/a.rs"
    if [ -z "$(scan "$_tmp")" ]; then
        echo "lint-bus-names.sh: SELF-TEST FAILED — synthetic violation not caught" >&2
        rm -rf "$_tmp"; exit 1
    fi
    # FDO interop must pass.
    printf 'conn.request_name("org.mpris.MediaPlayer2.mde-music")?;\n' > "$_tmp/crates/x/src/a.rs"
    if [ -n "$(scan "$_tmp")" ]; then
        echo "lint-bus-names.sh: SELF-TEST FAILED — FDO interop wrongly flagged" >&2
        rm -rf "$_tmp"; exit 1
    fi
    rm -rf "$_tmp"
    echo "lint-bus-names.sh: self-test passed"
    exit 0
fi

HITS="$(scan "$REPO_ROOT")"
if [ -n "$HITS" ]; then
    echo "lint-bus-names.sh: §2 violation — a private D-Bus name is being registered:" >&2
    echo "$HITS" | sed 's/^/  /' >&2
    echo "  → move the surface onto the mesh Bus; only org.freedesktop.*/org.mpris.* interop is allowed." >&2
    exit 1
fi
echo "lint-bus-names.sh: clean — no private D-Bus well-known names registered (§2)"
