#!/bin/sh
# install-helpers/lint-motion-tokens.sh — MOTION-AUDIT-2 single-source motion gate.
#
# The MOTION system (docs/design/motion-system.md, motion-language.md) locks every
# animation *timing* into the Carbon duration grid, single-sourced in
# `crates/shared/mde-theme` (`motion::DURATION_*`, the `Motion::*()` presets, and
# the `Tween`/`LoopingTween`/`Animator` primitives). §4's "no raw literals outside
# mde-theme" rule extends to motion: a GUI must drive its tweens from those tokens,
# never mint a bespoke animation duration inline.
#
# HARD gate (exit 1): a `Tween`/`LoopingTween` constructed from a *raw numeric*
# `Duration::from_millis(N)` / `Duration::from_secs(N)` literal, in any crate
# OUTSIDE mde-theme, on a line not marked `// motion-ok`. Such a tween is a bespoke
# animation timing that must route through MOTION-INFRA (a `Motion::*()` preset or
# a `DURATION_*` token). The `// motion-ok` escape is for a genuinely dynamic,
# data-derived duration (rare) with a one-line justification.
#
# SOFT report (exit 0): per-frame animation ticks — `time::every(...)` faster than
# 100 ms that are NOT the canonical 16 ms frame cadence — are listed as candidates
# to consolidate onto the shared frame cadence / MOTION-INFRA. Informational: these
# are existing, view-gated polls (e.g. the live-map flow animation); the
# MOTION-AUDIT-1 inventory lifts any genuinely-bespoke one to a follow-up.
#
# Run with `--self-test` to verify the gate (clean tree passes; a synthetic
# raw-literal tween is caught). Exit 0 = clean, 1 = a violation.

set -eu
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# The canonical per-frame animation cadence. iced's vendored fork ships no
# animation runtime, so a 16 ms `time::every` IS the shared frame clock the whole
# shell drives tweens from — not a bespoke literal.
FRAME_CADENCE_MS=16

# ── HARD gate: tweens minted from a raw Duration literal outside mde-theme ──────
scan_tween_literals() {
    _root="$1"
    # Lines that BOTH construct a tween AND carry a raw numeric Duration literal.
    # A token-driven tween reads `Motion::foo().duration` / a `DURATION_*` const /
    # a local variable, so it carries no `Duration::from_*(<digits>)` on the line.
    find "$_root"/crates -name '*.rs' 2>/dev/null \
        | grep -v '/mde-theme/' \
        | while IFS= read -r _f; do
            grep -nE '(Tween|LoopingTween)::(starting_at|resolved|pulse)\(' "$_f" 2>/dev/null \
                | grep -E 'Duration::from_(millis|secs)\([0-9_]+\)' \
                | grep -v '// motion-ok' \
                | while IFS= read -r _hit; do
                    printf '%s:%s\n' "$_f" "$_hit"
                done
        done
}

# ── SOFT report: animation-cadence ticks that aren't the 16 ms frame clock ──────
report_fast_ticks() {
    _root="$1"
    find "$_root"/crates -name '*.rs' 2>/dev/null \
        | grep -v '/mde-theme/' \
        | while IFS= read -r _f; do
            grep -nE 'time::every\(\s*Duration::from_millis\(([0-9]|[1-9][0-9])\)' "$_f" 2>/dev/null \
                | grep -vE "Duration::from_millis\($FRAME_CADENCE_MS\)" \
                | while IFS= read -r _hit; do
                    printf '%s:%s\n' "$_f" "$_hit"
                done
        done
}

run_gate() {
    _root="$1"
    _violations="$(scan_tween_literals "$_root" || true)"

    _ticks="$(report_fast_ticks "$_root" || true)"
    if [ -n "$_ticks" ]; then
        echo "motion-tokens: NOTE — animation-cadence ticks not on the ${FRAME_CADENCE_MS}ms frame clock"
        echo "  (informational; verify each is needs_tick/in-flight-gated, MOTION-PERF-1):"
        echo "$_ticks" | sed 's/^/    /'
        echo
    fi

    if [ -n "$_violations" ]; then
        echo "motion-tokens: FAIL — bespoke animation timing literal(s) outside mde-theme:"
        echo "$_violations" | sed 's/^/    /'
        echo
        echo "  A Tween/LoopingTween must take its duration from mde_theme::motion"
        echo "  (a Motion::*() preset or a DURATION_* token), not a raw Duration literal."
        echo "  Route it through MOTION-INFRA, or mark a genuinely-dynamic duration"
        echo "  '// motion-ok' with a one-line reason."
        return 1
    fi
    echo "motion-tokens: OK — every animation tween sources its timing from mde-theme."
    return 0
}

self_test() {
    _tmp="$(mktemp -d)"
    trap 'rm -rf "$_tmp"' EXIT
    mkdir -p "$_tmp/crates/workbench/mde-workbench/src"
    cat > "$_tmp/crates/workbench/mde-workbench/src/synthetic.rs" <<'EOF'
fn bad() {
    let _ = Tween::starting_at(now, Duration::from_millis(250));
}
EOF
    if run_gate "$_tmp" >/dev/null 2>&1; then
        echo "self-test FAILED: synthetic raw-literal tween was not caught"
        return 1
    fi
    # And a token-driven tween must pass.
    cat > "$_tmp/crates/workbench/mde-workbench/src/synthetic.rs" <<'EOF'
fn good() {
    let _ = Tween::resolved(now, Motion::panel_mount().duration, rm);
}
EOF
    if ! run_gate "$_tmp" >/dev/null 2>&1; then
        echo "self-test FAILED: a token-driven tween was wrongly flagged"
        return 1
    fi
    echo "motion-tokens: self-test OK (catches raw-literal tweens, passes token tweens)."
    return 0
}

if [ "${1:-}" = "--self-test" ]; then
    self_test
else
    run_gate "$REPO_ROOT"
fi
