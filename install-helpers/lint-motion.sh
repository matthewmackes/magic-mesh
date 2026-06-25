#!/bin/sh
# install-helpers/lint-motion.sh — MOTION-AUDIT-2: bespoke-animation gate (§4).
#
# The MOTION epic single-sources every "how long does this animation take" in the
# shared `crates/shared/mde-theme` motion modules — `motion.rs` (the Carbon
# duration/easing grid + `Motion` presets + the `list`/`context_menu`/`icon`
# stagger tokens), `animation.rs` (`Tween`/`ease`/`Transition`/the helpers), and
# `frame_timer.rs`. Render code elsewhere must drive its tweens/staggers/easings
# from those tokens, never hand-roll a beat. (AI_GOVERNANCE §4 — no scattered
# metric literals outside the token modules; MOTION-AUDIT-2 — no isolated
# one-screen effect.)
#
# This gate fails when a binding that is clearly *animation timing* — its name
# carries a motion keyword (stagger / tween / ease / fade / slide / reveal /
# pulse / shimmer / blink / a `*_anim*`/`*_motion*` duration) — is assigned a
# **bare `Duration::from_millis(<int>)` / `from_secs*(<int>)` literal** instead of
# a `mde_theme::motion` token / a `Motion::*` preset. It is deliberately precise:
#
#   * It only inspects a binding's NAME for the motion intent, so a legitimate
#     non-animation Duration — a `time::every` subscription cadence, a network
#     `*_TIMEOUT`, a `*_POLL`, an input `DEBOUNCE`/double-click window, an RTP
#     `*_INTERVAL` — is never flagged (those names carry no animation keyword).
#   * A **frame-tick cadence** is exempt even with a motion-ish name: a
#     `*_TICK` / `*TICK_MS` / `*_FPS` binding is the `time::every(..)` repaint
#     clock, not a tween duration, so `const ANIM_TICK = from_millis(16)` passes.
#   * A literal that is actually a shared token (`from_millis(REDUCE_MOTION_CAP_MS)`,
#     `from_millis(list::STAGGER_STEP_MS as u64)`) passes — the value is not bare.
#   * `mde-theme` itself is skipped (it IS the single source).
#   * A genuine non-token animation literal can be waived with a `// motion-ok:
#     <reason>` marker on the line (e.g. an off-grid one-shot the design doc
#     explicitly blesses), mirroring the `// carbon-ok` escape in
#     lint-carbon-tokens.sh.
#
# Wired into the lint suite the same way `lint-mesh-boundary.sh` /
# `lint-carbon-tokens.sh` are (CONTRIBUTING.md + .github/workflows/ci.yml).
#
# Run with `--self-test` to verify the gate (a clean tree passes; a synthetic
# bespoke stagger literal is caught; the cadence/token/marker exemptions hold).
# Exit 0 = clean, 1 = a violation.

set -eu
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Identifier segments (split on `_`) that mean "animation timing". Matched as a
# WHOLE `_`-delimited word of the binding name, so `LEASE_DURATION` (Raft lease)
# does NOT match `EASE`, `RELEASE`/`INCREASE` don't match, etc. — only a real
# `*_STAGGER`/`*_TWEEN`/`*_FADE`/… segment does.
MOTION_WORDS=" STAGGER TWEEN EASE EASING FADE CROSSFADE SLIDE REVEAL PULSE SHIMMER BLINK DEPRESS LIFT WIGGLE BOUNCE "

# A frame-tick / cadence segment is the repaint clock (`time::every(..)`), not a
# tween duration — exempt even with a motion-ish name.
CADENCE_WORDS=" TICK FPS CADENCE INTERVAL SAMPLE POLL TIMEOUT "

# scan <root>: echo `file:line:text` for every bespoke animation-duration literal.
# Strategy: find `<NAME>... = ... Duration::from_millis/from_secs*( <bare int> )`
# on one line, keep only NAMEs whose `_`-segments name animation timing, then drop
# the cadence names, the trivial 0/1 epsilon, token-sourced values, and
# `// motion-ok`-waived lines.
scan() {
    _root="$1"
    # (a) candidate lines: a binding assigned a from_millis/from_secs literal.
    #     Exclude the trivial `from_millis(0|1)` test/arithmetic epsilon — never a
    #     real animation duration — by requiring the literal to be ≥2 chars or ≥2.
    grep -rnE \
        '(const|let|static)[[:space:]]+[A-Za-z_][A-Za-z0-9_]*[^=]*=[^=].*Duration::from_(millis|secs|secs_f32|secs_f64)[[:space:]]*\([[:space:]]*-?[0-9_]+' \
        "$_root"/crates --include='*.rs' 2>/dev/null \
        | grep -v '/mde-theme/' \
        | grep -vi 'motion-ok' \
        | awk -v MW="$MOTION_WORDS" -v CW="$CADENCE_WORDS" '
            {
                line = $0
                # Strip the "file:line:" prefix to read the binding name safely.
                body = line
                sub(/^[^:]+:[0-9]+:/, "", body)
                # Capture the bound identifier (first token after the keyword).
                if (match(body, /(const|let|static)[[:space:]]+[A-Za-z_][A-Za-z0-9_]*/)) {
                    decl = substr(body, RSTART, RLENGTH)
                    sub(/^(const|let|static)[[:space:]]+/, "", decl)
                    name = toupper(decl)
                } else {
                    next
                }
                # Skip the trivial 0/1 epsilon literal (a `+ from_millis(1)` settle
                # nudge in a test, never a tween duration).
                if (body ~ /from_(millis|secs|secs_f32|secs_f64)[[:space:]]*\([[:space:]]*-?[01][[:space:]]*\)/) next
                # Split the name on `_` and check each whole segment.
                n = split(name, seg, "_")
                is_motion = 0; is_cadence = 0
                for (i = 1; i <= n; i++) {
                    if (index(MW, " " seg[i] " ") > 0) is_motion = 1
                    if (index(CW, " " seg[i] " ") > 0) is_cadence = 1
                }
                if (!is_motion) next            # not animation timing by name
                if (is_cadence) next            # a repaint clock / timeout, not a tween
                print line
            }
        ' \
        || true
}

if [ "${1:-}" = "--self-test" ]; then
    _fail=0
    _tmp="$(mktemp -d)"
    mkdir -p "$_tmp/crates/x/src"

    # (1) The current tree must be clean (the real fixes already landed).
    if [ -n "$(scan "$REPO_ROOT")" ]; then
        echo "lint-motion.sh: SELF-TEST FAIL — current tree has a bespoke animation literal:" >&2
        scan "$REPO_ROOT" | sed 's/^/    /' >&2
        _fail=1
    fi

    # (2) A bespoke per-row stagger literal MUST be caught.
    printf 'const ROW_STAGGER: Duration = Duration::from_millis(28);\n' \
        > "$_tmp/crates/x/src/a.rs"
    if [ -z "$(scan "$_tmp")" ]; then
        echo "lint-motion.sh: SELF-TEST FAIL — bespoke stagger literal not caught" >&2
        _fail=1
    fi

    # (3) A token-sourced stagger MUST pass (value is not a bare literal).
    printf 'const ROW_STAGGER: Duration = Duration::from_millis(list::STAGGER_STEP_MS as u64);\n' \
        > "$_tmp/crates/x/src/a.rs"
    if [ -n "$(scan "$_tmp")" ]; then
        echo "lint-motion.sh: SELF-TEST FAIL — token-sourced stagger wrongly flagged" >&2
        _fail=1
    fi

    # (4) A frame-tick cadence MUST pass even with a motion-ish name.
    printf 'const ANIM_TICK: Duration = Duration::from_millis(16);\nconst SLIDE_TICK: Duration = Duration::from_millis(16);\nconst SHIMMER_TICK: Duration = Duration::from_millis(33);\n' \
        > "$_tmp/crates/x/src/a.rs"
    if [ -n "$(scan "$_tmp")" ]; then
        echo "lint-motion.sh: SELF-TEST FAIL — frame-tick cadence wrongly flagged" >&2
        _fail=1
    fi

    # (5) Legitimate non-animation Durations MUST pass (no motion keyword).
    printf 'const NEBULA_PROBE_TIMEOUT: Duration = Duration::from_secs(2);\nconst POLL_DELAY: Duration = Duration::from_secs(3);\nconst DEBOUNCE: Duration = Duration::from_millis(250);\nconst DTMF_PACKET_INTERVAL: Duration = Duration::from_millis(20);\nlet at = Duration::from_millis(700);\n' \
        > "$_tmp/crates/x/src/a.rs"
    if [ -n "$(scan "$_tmp")" ]; then
        echo "lint-motion.sh: SELF-TEST FAIL — legitimate non-animation Duration wrongly flagged" >&2
        _fail=1
    fi

    # (6) The // motion-ok marker MUST waive a flagged line.
    printf 'const REVEAL_STAGGER_STEP: Duration = Duration::from_millis(40); // motion-ok: off-grid one-shot blessed by motion-system.md\n' \
        > "$_tmp/crates/x/src/a.rs"
    if [ -n "$(scan "$_tmp")" ]; then
        echo "lint-motion.sh: SELF-TEST FAIL — motion-ok marker not honoured" >&2
        _fail=1
    fi

    rm -rf "$_tmp"
    if [ "$_fail" -ne 0 ]; then
        echo "lint-motion.sh: SELF-TEST FAILED" >&2
        exit 1
    fi
    echo "lint-motion.sh: self-test passed (clean tree passes; bespoke stagger caught; cadence/token/marker exemptions hold)"
    exit 0
fi

HITS="$(scan "$REPO_ROOT")"
if [ -n "$HITS" ]; then
    echo "lint-motion.sh: §4 violation — bespoke animation-duration literal(s) outside mde-theme:" >&2
    echo "$HITS" | sed 's/^/  /' >&2
    echo "  → source the duration from an mde_theme::motion token (a Motion::* preset, or a" >&2
    echo "    list::/icon:: stagger token), route the tween through Animator/Tween::resolved, or" >&2
    echo "    mark the line // motion-ok with a reason if it's a design-blessed off-grid one-shot." >&2
    exit 1
fi
echo "lint-motion.sh: clean — no bespoke animation-duration literals outside mde-theme (§4 / MOTION-AUDIT-2)"
