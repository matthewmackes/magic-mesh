#!/bin/sh
# install-helpers/lint-mesh-boundary.sh — E11.2 mesh↔shell boundary gate.
#
# The Magic Mesh decoupling (docs/design/mesh-decoupling.md, Q6/Q49) requires the
# mesh side to NEVER depend on the EOL-bound desktop shell. This gate fails if any
# mesh-side crate declares a Cargo path-dependency into crates/shell/*.
#
#   mesh-side : crates/{mesh,platform,workbench,services,kdc,applets}/*
#   shell     : crates/shell/*  (mde, mde-ui, mde-popover, …) — being deleted (E11.12)
#   allowed   : crates/shared/* (mde-theme, mde-iced-components) — cross-cutting carry-forward
#
# Run `lint-mesh-boundary.sh --self-test` to verify the gate logic (clean tree
# passes; a synthetic violation is caught). Exit 0 = clean, 1 = a violation.

set -eu
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

MESH_DIRS="mesh platform workbench services kdc applets"

# scan <root>: echo any mesh-side → crates/shell/ path-deps; return 1 if any found.
scan() {
    _root="$1"
    _found=0
    for _d in $MESH_DIRS; do
        for _tc in "$_root"/crates/"$_d"/*/Cargo.toml; do
            [ -f "$_tc" ] || continue
            # A Cargo path dependency whose path traverses into the shell dir.
            _hits="$(grep -nE 'path[[:space:]]*=[[:space:]]*"[^"]*/shell/' "$_tc" 2>/dev/null || true)"
            if [ -n "$_hits" ]; then
                echo "  ${_tc#"$_root"/}:"
                echo "$_hits" | sed 's/^/    /'
                _found=1
            fi
        done
    done
    return "$_found"
}

if [ "${1:-}" = "--self-test" ]; then
    # (a) the current tree must be clean.
    if scan "$REPO_ROOT" >/dev/null; then
        : # clean
    else
        echo "lint-mesh-boundary.sh: SELF-TEST FAIL — current tree has a mesh→shell violation"
        exit 1
    fi
    # (b) a synthetic violation must be caught.
    _tmp="$(mktemp -d)"
    mkdir -p "$_tmp/crates/mesh/fixturecrate"
    printf '[package]\nname = "fixturecrate"\n[dependencies]\nmde-ui = { path = "../../shell/mde-ui" }\n' \
        > "$_tmp/crates/mesh/fixturecrate/Cargo.toml"
    if scan "$_tmp" >/dev/null; then
        echo "lint-mesh-boundary.sh: SELF-TEST FAIL — synthetic mesh→shell violation NOT caught"
        rm -rf "$_tmp"
        exit 1
    fi
    rm -rf "$_tmp"
    echo "lint-mesh-boundary.sh: self-test PASS (clean tree passes; synthetic violation caught)"
    exit 0
fi

if out="$(scan "$REPO_ROOT")"; then
    echo "lint-mesh-boundary.sh: clean — no mesh-side crate depends on the EOL-bound shell (E11)"
    exit 0
else
    echo "lint-mesh-boundary.sh: VIOLATION — a mesh-side crate depends on crates/shell/* (the EOL'd desktop shell):"
    echo "$out"
    echo "The mesh must not depend on the desktop shell (E11 decoupling, Q6/Q49). Depend on a"
    echo "crates/shared/* crate instead, or move the shared code there."
    exit 1
fi
