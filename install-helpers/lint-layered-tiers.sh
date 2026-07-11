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
# depend on it; it may itself depend only on other shared crates. The ONE
# carve-out is the egui/eframe/DRM GUI harness `crates/shared/mde-egui`: although
# it lives under crates/shared/ it is NOT tier-0 (a headless substrate/services
# crate must not silently pull the whole GUI stack), so it ranks at the
# desktop-shell tier ("gui") — see tier_of below.
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
# < desktop-shell(3). The GUI harness shares the outermost rank (3, label "gui")
# even though it lives under crates/shared/, so a headless tier that pulls it
# points outward and fails. An edge whose TARGET rank exceeds its SOURCE rank
# points outward and fails. The three platform-services crates that live under
# crates/mesh/, and the GUI harness under crates/shared/, are matched FIRST so
# the generic crates/mesh/* and crates/shared/* rules can't claim them.
tier_of() {
    case "$1" in
        # The heavy egui/eframe/DRM GUI harness. NOT blanket tier-0-shared:
        # matched before crates/shared/* and ranked at the desktop-shell tier
        # (rank 3, label "gui"). A headless substrate(1)/platform-services(2)
        # crate that pulls the GUI stack then points OUTWARD and trips the gate
        # — the whole point, so "the mesh stays headless-capable". Desktop-shell
        # crates depend on it INWARD (3 -> 3, allowed). Any new GUI-harness crate
        # added under crates/shared/ belongs on this line.
        crates/shared/mde-egui|crates/shared/mde-egui/*)
            echo "3|gui" ;;
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

# is_excepted_edge <src-dir> <tgt-dir> -> success (0) if this exact source→target
# pair is a CURATED, documented exception that is allowed to point outward.
# Keep this list TINY — every entry is a deliberate hole in the gate, and it is
# matched on the exact pair (not a wildcard), so it closes NOTHING else.
# The src/tgt are joined with a space (never a case-glob metacharacter, and no
# crate path contains one) and the arm is quoted, so it is an EXACT literal match
# — not `|`-alternation, which would silently never match.
is_excepted_edge() {
    case "$1 $2" in
        # mde-role-chooser IS the first-run onboarding GUI (ONBOARD-WIZARD OW-1):
        # a four-step egui surface (disclaimer → role → intent → confirm) rendered
        # on the seat via `mde_egui::run_client`, whose whole body is an
        # `eframe::App`. It has NO headless render path, so the GUI-harness edge is
        # load-bearing in every config. It is filed under crates/platform/ (a
        # platform-services binary that pins the role via `mackesd role-pin`) but is
        # a genuine GUI surface, so this ONE edge into the gui tier is allowed.
        # Narrow by construction: any OTHER platform/substrate crate reaching
        # mde-egui, or the role-chooser reaching a desktop-shell crate, still fails.
        "crates/platform/mde-role-chooser crates/shared/mde-egui") return 0 ;;
        *) return 1 ;;
    esac
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
                if is_excepted_edge "$_src" "$_tgt"; then
                    continue
                fi
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
    #     services→shell — plus substrate→shell (the headless invariant) and
    #     platform→gui (the GUI-harness carve-out) must each be caught.
    _t="$(mktemp -d)"
    mkdir -p "$_t/crates/mesh/badsub" "$_t/crates/services/badsvc" \
             "$_t/crates/desktop/victim" "$_t/crates/shared/mde-egui" \
             "$_t/crates/platform/badplat"
    printf '[package]\nname = "victim"\n' > "$_t/crates/desktop/victim/Cargo.toml"
    printf '[package]\nname = "mde-egui"\n' > "$_t/crates/shared/mde-egui/Cargo.toml"
    # substrate crate reaching OUT to both a service and the shell.
    printf '[package]\nname = "badsub"\n[dependencies]\nbadsvc = { path = "../../services/badsvc" }\nvictim = { path = "../../desktop/victim" }\n' \
        > "$_t/crates/mesh/badsub/Cargo.toml"
    # service crate reaching OUT to the shell.
    printf '[package]\nname = "badsvc"\n[dependencies]\nvictim = { path = "../../desktop/victim" }\n' \
        > "$_t/crates/services/badsvc/Cargo.toml"
    # a headless platform-services crate reaching OUT into the GUI harness — the
    # arch-4 loophole this carve-out closes.
    printf '[package]\nname = "badplat"\n[dependencies]\nmde-egui = { path = "../../shared/mde-egui" }\n' \
        > "$_t/crates/platform/badplat/Cargo.toml"
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
    if ! printf '%s\n' "$_caught" | grep -q 'badplat (platform-services).*mde-egui (gui)'; then
        echo "lint-layered-tiers.sh: SELF-TEST FAIL — planted platform→gui edge NOT caught" >&2
        rc=1
    fi

    # (c) edges that must NOT be flagged: an INWARD edge (shell→substrate), a
    #     desktop-shell→gui edge (inward, 3->3), and the curated
    #     role-chooser→gui exception.
    _t2="$(mktemp -d)"
    mkdir -p "$_t2/crates/desktop/shellish" "$_t2/crates/mesh/realsub" \
             "$_t2/crates/shared/mde-egui" "$_t2/crates/desktop/guiuser" \
             "$_t2/crates/platform/mde-role-chooser"
    printf '[package]\nname = "realsub"\n' > "$_t2/crates/mesh/realsub/Cargo.toml"
    printf '[package]\nname = "mde-egui"\n' > "$_t2/crates/shared/mde-egui/Cargo.toml"
    printf '[package]\nname = "shellish"\n[dependencies]\nrealsub = { path = "../../mesh/realsub" }\n' \
        > "$_t2/crates/desktop/shellish/Cargo.toml"
    # a desktop-shell crate depending on the GUI harness is inward (3 -> 3) — fine.
    printf '[package]\nname = "guiuser"\n[dependencies]\nmde-egui = { path = "../../shared/mde-egui" }\n' \
        > "$_t2/crates/desktop/guiuser/Cargo.toml"
    # the curated role-chooser→gui exception must NOT be flagged.
    printf '[package]\nname = "mde-role-chooser"\n[dependencies]\nmde-egui = { path = "../../shared/mde-egui" }\n' \
        > "$_t2/crates/platform/mde-role-chooser/Cargo.toml"
    if [ -n "$(scan "$_t2")" ]; then
        echo "lint-layered-tiers.sh: SELF-TEST FAIL — an inward / excepted edge (shell→substrate, desktop→gui, or the role-chooser→gui exception) was wrongly flagged" >&2
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
