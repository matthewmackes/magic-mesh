#!/usr/bin/env bash
# score-surface.sh — deterministic critic for refining-carbon-gui. Computes an
# objective score + findings JSON for a surface from MACHINE checks (the Carbon
# token lint + source anti-pattern greps), so each round's accept/reject is
# grounded in external signal, not the model judging its own prose. The PNG arg
# is recorded for provenance; the model reads the PNG separately for the visual
# half (contrast/layout it can see but a grep can't).
#
# Usage: score-surface.sh <render.png> <crate>
# Stdout: {score, crate, png, checks:{...}, findings:[...]}  (higher score = cleaner)
set -uo pipefail
PNG="${1:-}"; CRATE="${2:-}"
REPO="$(cd "$(dirname "$0")/../../../.." && pwd)"
[ -n "$CRATE" ] || { echo '{"error":"usage: score-surface.sh <png> <crate>"}'; exit 2; }
SRC="$(find "$REPO/crates" -type d -path "*/$CRATE/src" 2>/dev/null | head -1)"
[ -n "$SRC" ] || { echo "{\"error\":\"crate src not found for $CRATE\"}"; exit 2; }

# 1. Carbon token lint (repo-wide; the hard §4 gate)
if "$REPO/install-helpers/lint-carbon-tokens.sh" >/dev/null 2>&1; then CARBON_OK=true; else CARBON_OK=false; fi

# 2. raw hex in this surface (Color::from_rgb without a // carbon-ok marker)
RAW_HEX=$(grep -rnE 'Color::from_rgb8?\(' "$SRC" 2>/dev/null | grep -vc 'carbon-ok' || true)
RAW_HEX=${RAW_HEX:-0}

# 3. raw spacing/padding integer LITERALS — §4 wants mde-theme Space tokens, not
#    raw ints. Flag any literal NOT on MCNF's density scale (the worst offenders:
#    a value off BOTH the MCNF Space scale and the Carbon scale is an unambiguous
#    finding). MCNF Space BASE = 4,6,8,10,14,17,20,24,28,34,40,48 ∪ Carbon set.
TOKENS=" 0 2 4 6 8 10 12 14 16 17 20 24 28 32 34 40 48 64 80 96 160 "
OFFSCALE=0
while read -r n; do
  [ -z "$n" ] && continue
  case "$TOKENS" in *" $n "*) :;; *) OFFSCALE=$((OFFSCALE+1));; esac
done < <(grep -rhoE '(padding|spacing)\(\s*[0-9]+' "$SRC" 2>/dev/null | grep -oE '[0-9]+$')

# 4. linear / ad-hoc easing references in animation contexts
EASING=$(grep -rnE 'Easing::Linear|\blinear\b' "$SRC" 2>/dev/null | grep -ciE 'eas|anim|tween|curve|motion' || true)
EASING=${EASING:-0}

# 5. score (NOT floored — must stay sensitive so a per-round delta is always
#    visible even when a surface starts deep in the red; 100 = clean).
SCORE=$((100 - 10*RAW_HEX - 5*OFFSCALE - 3*EASING))
[ "$CARBON_OK" = false ] && SCORE=$((SCORE-20))

python3 - "$SCORE" "$CRATE" "$PNG" "$CARBON_OK" "$RAW_HEX" "$OFFSCALE" "$EASING" <<'PY'
import json,sys
score,crate,png,carbon,rh,off,eas = sys.argv[1:8]
findings=[]
if carbon=="false": findings.append({"criterion":"carbon:tokens","note":"lint-carbon-tokens.sh failed (raw colour outside mde-theme)"})
if int(rh)>0:  findings.append({"criterion":"carbon:color-tokens","count":int(rh),"note":"raw Color::from_rgb in surface code"})
if int(off)>0: findings.append({"criterion":"carbon:spacing","count":int(off),"note":"off-scale spacing/padding literals"})
if int(eas)>0: findings.append({"criterion":"carbon:motion","count":int(eas),"note":"linear/ad-hoc easing in animation code"})
print(json.dumps({"score":int(score),"crate":crate,"png":png,
  "checks":{"carbon_lint_ok":carbon=="true","raw_hex":int(rh),"off_scale_px":int(off),"linear_easing":int(eas)},
  "findings":findings,
  "note":"source-level machine signal; combine with a Read of the PNG for visual criteria (contrast, focus ring, state coverage)"},
  indent=1))
PY
