#!/bin/sh
# install-helpers/lint-carbon-tokens.sh — §4 Carbon single-source gate (AUD-21).
#
# §4 locks the IBM Carbon palette as single-sourced in `crates/shared/mde-theme`
# (the `carbon` ramp + `Palette`). Render code elsewhere must read those tokens,
# never mint raw colours. This gate fails if any crate OUTSIDE mde-theme builds a
# colour from a raw literal — `Color::from_rgb(` / `Color::from_rgb8(` — on a
# line not marked `// carbon-ok`.
#
# The `// carbon-ok` escape is for the two legitimate non-token cases: test
# fixtures, and genuinely *dynamic* colours derived from data at runtime (e.g.
# album-art extraction). Each must carry a one-line justification.
#
# Run with `--self-test` to verify the gate (clean tree passes; a synthetic
# unmarked literal is caught). Exit 0 = clean, 1 = a violation.

set -eu
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

scan() {
    _root="$1"
    # Color::from_rgb( / from_rgb8( outside mde-theme, sans a carbon-ok marker.
    grep -rnE 'Color::from_rgb8?\(' "$_root"/crates --include='*.rs' 2>/dev/null \
        | grep -v '/mde-theme/' \
        | grep -v 'carbon-ok' \
        || true
}

if [ "${1:-}" = "--self-test" ]; then
    _tmp="$(mktemp -d)"
    mkdir -p "$_tmp/crates/x/src"
    printf 'let c = Color::from_rgb(0.1,0.2,0.3);\n' > "$_tmp/crates/x/src/a.rs"
    if [ -z "$(scan "$_tmp")" ]; then
        echo "lint-carbon-tokens.sh: SELF-TEST FAILED — synthetic violation not caught" >&2
        rm -rf "$_tmp"; exit 1
    fi
    printf 'let c = Color::from_rgb(0.1,0.2,0.3); // carbon-ok: test\n' > "$_tmp/crates/x/src/a.rs"
    if [ -n "$(scan "$_tmp")" ]; then
        echo "lint-carbon-tokens.sh: SELF-TEST FAILED — carbon-ok marker not honoured" >&2
        rm -rf "$_tmp"; exit 1
    fi
    rm -rf "$_tmp"
    echo "lint-carbon-tokens.sh: self-test passed"
    exit 0
fi

HITS="$(scan "$REPO_ROOT")"
if [ -n "$HITS" ]; then
    echo "lint-carbon-tokens.sh: §4 violation — raw colour literal(s) outside mde-theme:" >&2
    echo "$HITS" | sed 's/^/  /' >&2
    echo "  → read an mde_theme::carbon / Palette token, or mark the line // carbon-ok with a reason." >&2
    exit 1
fi
echo "lint-carbon-tokens.sh: clean — no raw colour literals outside mde-theme (§4)"
