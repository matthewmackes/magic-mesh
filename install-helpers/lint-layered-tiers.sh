#!/bin/sh
# install-helpers/lint-layered-tiers.sh — E12 layered-tiers boundary gate.
#
# Replaces the old two-bucket `lint-mesh-boundary.sh` (mesh↔shell). Under E12
# "Quasar" (AI_GOVERNANCE §6 / docs/design/quasar-vdi-desktop.md) the workspace
# is THREE NESTED tiers and dependencies may point only INWARD:
#
#     mesh-substrate  ⊂  platform-services  ⊂  desktop-shell
#     (Nebula, etcd,     (mackesd: session-     (the one egui shell on the DRM
#      Syncthing,         broker + vm-lifecycle,  seat + mde-vdi; the in-shell
#      CA/KDC)            mde-bus, fleet, the     mesh panels)
#                         mesh services)
#
# A dependency edge that points OUTWARD — substrate→services, substrate→shell,
# or services→shell — is a CI FAILURE: it would drag a desktop dependency into
# the headless substrate and break "the mesh stays headless-capable". Inward
# edges (shell → services → substrate) are fine.
#
# `crates/shared/*` is the cross-cutting base BENEATH every tier: any tier may
# depend on it; it may itself depend only on other shared crates.
#
# Tier is a crate's directory, with ONE curated exception: mackesd, magic-fleet
# and mde-enroll physically live under crates/mesh/ but ARE the platform-services
# daemon / fleet / enrollment (§6), so they rank as services — which is exactly
# why a pure-directory split would wrongly red-flag e.g. `mackesd → mde-bus`.
#
# Run `lint-layered-tiers.sh --self-test` to verify the gate logic (the current
# tree passes; a planted outward edge is caught; an inward edge is NOT flagged).
# Exit 0 = clean, 1 = an outward edge.

set -eu
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# tier_of <repo-relative-path> -> "<rank>|<tier>" ("" = outside the tiered tree).
# Ranks increase OUTWARD: shared(0) < mesh-substrate(1) < platform-services(2)
# < desktop-shell(3). An edge whose TARGET rank exceeds its SOURCE rank points
# outward and fails. The three platform-services crates that live under
# crates/mesh/ are matched FIRST so the generic crates/mesh/* rule can't claim
# them.
tier_of() {
    case "$1" in
        crates/shared/*)
            echo "0|shared" ;;
        crates/mesh/mackesd|crates/mesh/mackesd/*|\
        crates/mesh/magic-fleet|crates/mesh/magic-fleet/*|\
        crates/mesh/mde-enroll|crates/mesh/mde-enroll/*)
            echo "2|platform-services" ;;
        crates/mesh/*|crates/kdc/*)
            echo "1|mesh-substrate" ;;
        crates/platform/*|crates/services/*)
            echo "2|platform-services" ;;
        crates/desktop/*|crates/workbench/*)
            echo "3|desktop-shell" ;;
        *)
            echo "" ;;
    esac
}

# dep_paths <Cargo.toml> -> the `path = "..."` value of every DEPENDENCY entry,
# one per line. TOML-section-aware: only [dependencies] / [dev-dependencies] /
# [build-dependencies] (incl. their dotted `[dependencies.NAME]` and
# `[target.'cfg(...)'.dependencies]` forms) are scanned, so the [lib]/[[bin]]/
# [[example]] `path = "src/..."` build-target keys are correctly ignored.
dep_paths() {
    awk '
        function is_dep(h) {
            return (h ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)(\.|$)/)
        }
        {
            line = $0
            sub(/#.*/, "", line)                 # strip comments (paths carry no #)
            if (line ~ /^[[:space:]]*\[/) {       # a [section] / [[array]] header
                h = line
                gsub(/[][[:space:]]/, "", h)      # drop [ ] and whitespace
                in_deps = is_dep(h)
                next
            }
            if (in_deps && match(line, /path[[:space:]]*=[[:space:]]*"[^"]*"/)) {
                seg = substr(line, RSTART, RLENGTH)
                sub(/^path[[:space:]]*=[[:space:]]*"/, "", seg)
                sub(/"$/, "", seg)
                print seg
            }
        }
    ' "$1"
}

# scan <root>: echo one line per OUTWARD dependency edge under <root>/crates.
# Output empty ⇒ clean. Always returns 0 (the caller decides on emptiness).
scan() {
    _root="$1"
    for _tc in "$_root"/crates/*/*/Cargo.toml; do
        [ -f "$_tc" ] || continue
        _cdir="$(dirname "$_tc")"
        _src="${_cdir#"$_root"/}"
        _si="$(tier_of "$_src")"
        [ -n "$_si" ] || continue
        _srank="${_si%%|*}"; _stier="${_si#*|}"
        dep_paths "$_tc" | while IFS= read -r _dp; do
            [ -n "$_dp" ] || continue
            _tgt="$(realpath -m --relative-to="$_root" "$_cdir/$_dp" 2>/dev/null || true)"
            [ -n "$_tgt" ] || continue
            _ti="$(tier_of "$_tgt")"
            [ -n "$_ti" ] || continue
            _trank="${_ti%%|*}"; _ttier="${_ti#*|}"
            if [ "$_trank" -gt "$_srank" ]; then
                echo "  $_src ($_stier) -> $_tgt ($_ttier)"
            fi
        done
    done
    return 0
}

if [ "${1:-}" = "--self-test" ]; then
    rc=0

    # (a) the current tree must be clean.
    _live="$(scan "$REPO_ROOT")"
    if [ -n "$_live" ]; then
        echo "lint-layered-tiers.sh: SELF-TEST FAIL — current tree has an outward edge:" >&2
        echo "$_live" >&2
        rc=1
    fi

    # (b) the planted OUTWARD edges §6 names — substrate→services and
    #     services→shell — plus substrate→shell (the headless invariant) must
    #     each be caught.
    _t="$(mktemp -d)"
    mkdir -p "$_t/crates/mesh/badsub" "$_t/crates/services/badsvc" \
             "$_t/crates/desktop/victim"
    printf '[package]\nname = "victim"\n' > "$_t/crates/desktop/victim/Cargo.toml"
    # substrate crate reaching OUT to both a service and the shell.
    printf '[package]\nname = "badsub"\n[dependencies]\nbadsvc = { path = "../../services/badsvc" }\nvictim = { path = "../../desktop/victim" }\n' \
        > "$_t/crates/mesh/badsub/Cargo.toml"
    # service crate reaching OUT to the shell.
    printf '[package]\nname = "badsvc"\n[dependencies]\nvictim = { path = "../../desktop/victim" }\n' \
        > "$_t/crates/services/badsvc/Cargo.toml"
    _caught="$(scan "$_t")"
    if ! printf '%s\n' "$_caught" | grep -q 'badsub (mesh-substrate).*badsvc (platform-services)'; then
        echo "lint-layered-tiers.sh: SELF-TEST FAIL — planted substrate→services edge NOT caught" >&2
        rc=1
    fi
    if ! printf '%s\n' "$_caught" | grep -q 'badsvc (platform-services).*victim (desktop-shell)'; then
        echo "lint-layered-tiers.sh: SELF-TEST FAIL — planted services→shell edge NOT caught" >&2
        rc=1
    fi
    if ! printf '%s\n' "$_caught" | grep -q 'badsub (mesh-substrate).*victim (desktop-shell)'; then
        echo "lint-layered-tiers.sh: SELF-TEST FAIL — planted substrate→shell edge NOT caught" >&2
        rc=1
    fi

    # (c) an INWARD edge (shell→substrate) must NOT be flagged.
    _t2="$(mktemp -d)"
    mkdir -p "$_t2/crates/desktop/shellish" "$_t2/crates/mesh/realsub"
    printf '[package]\nname = "realsub"\n' > "$_t2/crates/mesh/realsub/Cargo.toml"
    printf '[package]\nname = "shellish"\n[dependencies]\nrealsub = { path = "../../mesh/realsub" }\n' \
        > "$_t2/crates/desktop/shellish/Cargo.toml"
    if [ -n "$(scan "$_t2")" ]; then
        echo "lint-layered-tiers.sh: SELF-TEST FAIL — inward shell→substrate edge wrongly flagged" >&2
        rc=1
    fi

    rm -rf "$_t" "$_t2"
    if [ "$rc" -eq 0 ]; then
        echo "lint-layered-tiers.sh: self-test PASS (tree clean; planted outward edges caught; inward edge allowed)"
    fi
    exit "$rc"
fi

HITS="$(scan "$REPO_ROOT")" || true
if [ -z "$HITS" ]; then
    echo "lint-layered-tiers.sh: clean — every dependency edge points inward across the three tiers (§6)"
    exit 0
else
    echo "lint-layered-tiers.sh: VIOLATION — outward dependency edge(s) across the layered tiers (§6):" >&2
    echo "$HITS" >&2
    echo "  mesh-substrate ⊂ platform-services ⊂ desktop-shell — dependencies must point INWARD." >&2
    echo "  Move the shared code into crates/shared/*, or invert the edge (have the inner tier" >&2
    echo "  publish a handle/event the outer tier consumes) so the headless substrate never" >&2
    echo "  pulls a desktop-shell dependency." >&2
    exit 1
fi
