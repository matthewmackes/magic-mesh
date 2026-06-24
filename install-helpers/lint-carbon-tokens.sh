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

# Multiline `Color { r: <float>, g: <float>, b: <float>, .. }` struct literals.
# The single-line `Color::from_rgb(` grep can't see these, so a render-path token
# minted field-by-field (`Color { r: 0.9, g: 0.2, b: 0.2, a: 1.0 }`) would slip
# the gate. This awk walks each `*.rs` file, and inside any `Color { ... }` block
# flags it ONLY when ALL THREE of `r:`/`g:`/`b:` are *bare numeric* floats — i.e.
# a genuine raw literal. Token-derived fills (`r: accent.r * 1.1`, alpha-only
# spreads like `Color { a: 0.12, ..danger }`) are NOT raw colour, so they pass.
# A `// carbon-ok` anywhere in the block (or on the opening line) waives it, same
# as the single-line grep.
scan_multiline() {
    _root="$1"
    # shellcheck disable=SC2038
    find "$_root"/crates -name '*.rs' 2>/dev/null \
        | grep -v '/mde-theme/' \
        | while IFS= read -r _f; do
            awk '
                { line = $0 }
                # Entering a Color { ... } literal (opening brace on this line).
                /Color[[:space:]]*\{/ && depth == 0 {
                    depth = 1
                    start_no = NR
                    start_line = line
                    ok = (line ~ /carbon-ok/) ? 1 : 0
                    rnum = 0; gnum = 0; bnum = 0
                }
                depth > 0 {
                    if (line ~ /carbon-ok/) ok = 1
                    # A field is a RAW literal only if its value starts with a digit
                    # (optionally a leading -). `r: accent.r` / `..danger` never match.
                    if (line ~ /[^_.a-zA-Z]r[[:space:]]*:[[:space:]]*-?[0-9]/ || line ~ /^[[:space:]]*r[[:space:]]*:[[:space:]]*-?[0-9]/) rnum = 1
                    if (line ~ /[^_.a-zA-Z]g[[:space:]]*:[[:space:]]*-?[0-9]/ || line ~ /^[[:space:]]*g[[:space:]]*:[[:space:]]*-?[0-9]/) gnum = 1
                    if (line ~ /[^_.a-zA-Z]b[[:space:]]*:[[:space:]]*-?[0-9]/ || line ~ /^[[:space:]]*b[[:space:]]*:[[:space:]]*-?[0-9]/) bnum = 1
                    # Close the block on the first closing brace (single- or multi-line).
                    if ((line ~ /\}/ && NR > start_no) || (line ~ /Color[[:space:]]*\{.*\}/)) {
                        if (rnum && gnum && bnum && !ok)
                            printf "%s:%d:%s\n", FILENAME, start_no, start_line
                        depth = 0
                    }
                }
            ' "$_f"
        done
}

scan() {
    _root="$1"
    # (a) Color::from_rgb( / from_rgb8( outside mde-theme, sans a carbon-ok marker.
    grep -rnE 'Color::from_rgb8?\(' "$_root"/crates --include='*.rs' 2>/dev/null \
        | grep -v '/mde-theme/' \
        | grep -v 'carbon-ok' \
        || true
    # (b) Multiline `Color { r/g/b: <float> }` struct literals (same exemptions).
    scan_multiline "$_root"
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

    # Multiline struct-literal cases.
    printf 'let c = Color {\n    r: 0.9,\n    g: 0.2,\n    b: 0.2,\n    a: 1.0,\n};\n' \
        > "$_tmp/crates/x/src/a.rs"
    if [ -z "$(scan "$_tmp")" ]; then
        echo "lint-carbon-tokens.sh: SELF-TEST FAILED — multiline Color { r/g/b: float } not caught" >&2
        rm -rf "$_tmp"; exit 1
    fi
    # Token-derived fills + alpha-only spreads are NOT raw colour: must pass clean.
    printf 'let c = Color {\n    r: accent.r * 1.1,\n    g: accent.g,\n    b: accent.b,\n    a: 0.08,\n};\nlet d = Color { a: 0.12, ..palette.danger.into_cosmic_color() };\n' \
        > "$_tmp/crates/x/src/a.rs"
    if [ -n "$(scan "$_tmp")" ]; then
        echo "lint-carbon-tokens.sh: SELF-TEST FAILED — token-derived/alpha-spread Color flagged" >&2
        rm -rf "$_tmp"; exit 1
    fi
    # A carbon-ok marker inside the block waives it.
    printf 'let c = Color {\n    r: 0.5, // carbon-ok: test fixture\n    g: 0.5,\n    b: 0.5,\n    a: 1.0,\n};\n' \
        > "$_tmp/crates/x/src/a.rs"
    if [ -n "$(scan "$_tmp")" ]; then
        echo "lint-carbon-tokens.sh: SELF-TEST FAILED — carbon-ok in multiline block not honoured" >&2
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
