#!/usr/bin/env bash
# mcnf-channel-rollback.sh — WL-BUILD-003: the INVERSE of channel promotion.
#
# Promotion (dnf-channel-up.sh / the /release step) lands a SIGNED RPM in the
# sovereign channel's client-facing arch dir (fedora-<N>-x86_64/) and reindexes
# with createrepo_c, so the channel advertises that NEVRA as the newest package
# a client installs. This script adds the missing return path: re-point the
# channel to the PREVIOUS NEVRA (a rollback/downgrade) — and its partners
# `promote`, `re-promote`, `list`, and a non-production `drill`.
#
# HOW A ROLLBACK RE-POINTS THE CHANNEL
#   The channel serves EVERY RPM in fedora-<N>-x86_64/ that is not under an
#   excluded staging subtree; a dnf client installs the highest-version one.
#   To roll back we move the current newest NEVRA OUT of the client-facing set
#   into a first-class quarantine subtree, ROLLED-BACK/ (a sibling of HOLD/,
#   both excluded from the index), then reindex. The channel's advertised
#   "latest" reverts to the previous NEVRA. Fleet hosts then converge with
#   `dnf distro-sync magic-mesh` (this script prints the exact command; it never
#   SSHes into a node itself). `re-promote` moves a quarantined NEVRA back into
#   the client-facing set — an undo of the rollback.
#
# SAFETY (this is a live platform channel — see docs/RELEASE-ROLLBACK.md)
#   * DRY-RUN IS THE DEFAULT. Without --apply the script only prints the plan and
#     mutates nothing.
#   * --apply mutates the on-disk channel. On a PRODUCTION root (the default
#     channel root, or any root not marked --non-prod) --apply ALSO requires the
#     typed token `--confirm ROLLBACK`, mirroring the downgrade-guard mandate in
#     docs/POSTMORTEM-line-divergence.md ("a downgrade must be an explicit,
#     typed-confirm override").
#   * --non-prod skips the typed-confirm for scratch/test roots and REFUSES to
#     touch the production default root, so a drill can never mutate prod.
#   * `drill` runs a full promote -> rollback -> re-promote cycle on a throwaway
#     temp root with dummy NEVRAs and NEVER touches production. It skips
#     createrepo_c (like dnf-channel-up.sh --self-test) so it runs with no
#     external deps and validates the ladder/quarantine/guard LOGIC end to end.
#
# Usage:
#   mcnf-channel-rollback.sh list        [--channel-root DIR] [--fedora N]
#   mcnf-channel-rollback.sh promote  RPM [--channel-root DIR] [--fedora N] --apply [--confirm ROLLBACK|--non-prod]
#   mcnf-channel-rollback.sh rollback     [--channel-root DIR] [--fedora N] [--to NVRA] --apply [--confirm ROLLBACK|--non-prod]
#   mcnf-channel-rollback.sh re-promote NVRA [--channel-root DIR] [--fedora N] --apply [--confirm ROLLBACK|--non-prod]
#   mcnf-channel-rollback.sh drill       [--fedora N]
#
# Env: MCNF_DNF_ROOT (default /var/lib/mcnf-dnf-channel), MCNF_FEDORA_VERSIONS
#      (first token is the default --fedora), MCNF_PKG_NAME (default magic-mesh).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_REPO="$(cd "$HERE/../.." && pwd)"

PROD_DEFAULT_ROOT="/var/lib/mcnf-dnf-channel"
ROOT="${MCNF_DNF_ROOT:-$PROD_DEFAULT_ROOT}"
PKG_NAME="${MCNF_PKG_NAME:-magic-mesh}"
FEDORA="${MCNF_FEDORA_VERSIONS-}"; FEDORA="${FEDORA%% *}"; FEDORA="${FEDORA:-44}"
QUARANTINE="ROLLED-BACK"          # sibling of HOLD/, excluded from the client index

APPLY=""
NONPROD=""
CONFIRM=""
TO_NVRA=""
DRILL=""                          # set by the drill verb → skip createrepo_c

log()  { printf '==> %s\n' "$*" >&2; }
note() { printf '    %s\n' "$*" >&2; }
die()  { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

# ── channel layout helpers ──────────────────────────────────────────────────
arch_dir() { printf '%s/fedora-%s-x86_64\n' "$ROOT" "$FEDORA"; }

# NVRA (name-version-release.arch) = the RPM filename with the .rpm suffix removed.
nvra_of() { local b; b="$(basename "$1")"; printf '%s\n' "${b%.rpm}"; }

# rpm-vercmp-friendly version-release token, for `sort -V` ordering of the ladder
# (mirrors mcnf-promotion-cycle.sh:rpm_version_token — strip the pkg-name prefix
# and the trailing .<arch>).
ver_token() {
  local stem rest
  stem="$(nvra_of "$1")"
  rest="${stem#"$PKG_NAME"-}"
  rest="${rest%.*}"                # drop .<arch>
  printf '%s\n' "$rest"
}

# The client-facing set = every *.rpm in the arch dir EXCEPT the excluded staging
# subtrees (HOLD/ = unsigned CI staging, ROLLED-BACK/ = quarantined downgrades).
# This mirrors dnf-channel-up.sh:indexable_rpms so the two agree on what a client
# can see. Emitted newest-LAST by version sort.
client_facing_rpms() {
  local ad; ad="$(arch_dir)"
  [ -d "$ad" ] || return 0
  local f tok
  find "$ad" \( -path "$ad/HOLD" -o -path "$ad/$QUARANTINE" \) -prune \
       -o -type f -name '*.rpm' -print 2>/dev/null |
  while IFS= read -r f; do
    printf '%s\t%s\n' "$(ver_token "$f")" "$f"
  done | sort -V | cut -f2-
}

quarantined_rpms() {
  local qd; qd="$(arch_dir)/$QUARANTINE"
  [ -d "$qd" ] || return 0
  local f
  find "$qd" -type f -name '*.rpm' -print 2>/dev/null |
  while IFS= read -r f; do printf '%s\t%s\n' "$(ver_token "$f")" "$f"; done |
  sort -V | cut -f2-
}

latest_rpm()   { client_facing_rpms | tail -1; }
previous_rpm() { client_facing_rpms | tail -2 | head -1; }

# ── mutation core (all writes flow through here) ────────────────────────────
reindex() {
  local ad; ad="$(arch_dir)"
  if [ -n "$DRILL" ]; then
    note "reindex: skipped (drill/logic-only mode)"; return 0
  fi
  if ! command -v createrepo_c >/dev/null 2>&1; then
    note "reindex: createrepo_c not found — channel metadata NOT regenerated."
    note "         run automation/forgejo/dnf-channel-up.sh on the control VM to refresh repodata."
    return 0
  fi
  # HOLD/ and ROLLED-BACK/ are both kept out of the client-facing index.
  createrepo_c --update --excludes 'HOLD/*' --excludes "$QUARANTINE/*" "$ad" >/dev/null
  note "reindex: createrepo_c refreshed $(basename "$ad")/repodata (HOLD/ + $QUARANTINE/ excluded)"
}

# Gate every real mutation: dry-run unless --apply; typed-confirm on prod roots.
guard_mutation() {
  local action="$1"
  if [ -z "$APPLY" ]; then
    log "DRY-RUN ($action): no --apply, nothing was changed. Re-run with --apply to mutate."
    return 1
  fi
  if [ -n "$NONPROD" ]; then
    [ "$ROOT" = "$PROD_DEFAULT_ROOT" ] && \
      die "--non-prod refuses the production channel root ($PROD_DEFAULT_ROOT); point --channel-root at a scratch dir."
    return 0
  fi
  # Production apply → require the typed token.
  if [ "$CONFIRM" != "ROLLBACK" ]; then
    die "$action on a production channel root requires the typed token: --confirm ROLLBACK (or --non-prod for a scratch root)."
  fi
  return 0
}

# Best-effort datacenter event, mirroring mcnf-promotion-cycle.sh:publish_promote.
# Only fires on a real prod apply; never in a drill.
publish_rollback_event() {
  local from="$1" to="$2" body
  [ -n "$APPLY" ] && [ -z "$NONPROD" ] || return 0
  command -v mde-bus >/dev/null 2>&1 || return 0
  body="$(printf '{"stage":"rollback","fedora":"%s","from":"%s","to":"%s","channel_root":"%s"}' \
    "$FEDORA" "$from" "$to" "$ROOT")"
  mde-bus publish "event/dc/promote/rollback" --body-flag "$body" >/dev/null 2>&1 || true
}

fleet_downgrade_hint() {
  local to_ver="$1"
  cat >&2 <<EOF
    Fleet convergence (run on each installed host, operator-gated):
      dnf clean metadata && dnf distro-sync $PKG_NAME     # re-sync to the channel's now-latest NEVRA
      # or pin explicitly: dnf -y downgrade ${PKG_NAME}-${to_ver}
    Note: hosts on a scriptlet/pinned build refuse silent downgrade by design
          (docs/design/platform-survey-answers.md Q78); use distro-sync/downgrade.
EOF
}

# ── verbs ───────────────────────────────────────────────────────────────────
do_list() {
  local ad; ad="$(arch_dir)"
  log "channel $ROOT  fedora-$FEDORA-x86_64"
  if [ ! -d "$ad" ]; then note "(arch dir does not exist yet)"; return 0; fi
  local f latest previous n=0
  latest="$(latest_rpm)"; previous="$(previous_rpm)"
  echo "  client-facing NEVRA ladder (oldest -> newest):"
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    n=$((n+1))
    if [ "$f" = "$latest" ]; then
      printf '    %2d. %s   <- CURRENT (channel latest)\n' "$n" "$(nvra_of "$f")"
    elif [ "$f" = "$previous" ]; then
      printf '    %2d. %s   <- rollback target (previous)\n' "$n" "$(nvra_of "$f")"
    else
      printf '    %2d. %s\n' "$n" "$(nvra_of "$f")"
    fi
  done < <(client_facing_rpms)
  [ "$n" -eq 0 ] && echo "    (none)"
  echo "  quarantined ($QUARANTINE/, not served):"
  local q qn=0
  while IFS= read -r q; do [ -n "$q" ] || continue; qn=$((qn+1)); printf '    - %s\n' "$(nvra_of "$q")"; done < <(quarantined_rpms)
  [ "$qn" -eq 0 ] && echo "    (none)"
}

do_promote() {
  local rpm="$1"
  [ -n "$rpm" ] || die "promote needs an RPM path"
  [ -f "$rpm" ] || die "no such RPM: $rpm"
  local ad; ad="$(arch_dir)"
  local dest="$ad/$(basename "$rpm")"
  # Signature gate (skipped for --non-prod scratch/drill roots). Signed-ness is
  # authoritative in dnf-channel-up.sh; here we refuse an obviously unsigned RPM
  # onto a production channel.
  if [ -z "$NONPROD" ] && [ -z "$DRILL" ] && command -v rpm >/dev/null 2>&1; then
    local sig
    sig="$(rpm -qp --qf '%{RSAHEADER:pgpsig}%{SIGPGP:pgpsig}%{SIGGPG:pgpsig}' "$rpm" 2>/dev/null | sed 's/(none)//g; s/[[:space:]]//g')"
    [ -n "$sig" ] || die "refusing to promote an UNSIGNED RPM to a production channel: $rpm (sign-release.sh first, or use --non-prod)."
  fi
  log "promote $(nvra_of "$rpm") -> channel fedora-$FEDORA-x86_64"
  guard_mutation "promote" || return 0
  mkdir -p "$ad"
  cp -f "$rpm" "$dest"
  reindex
  note "promoted: channel latest is now $(nvra_of "$(latest_rpm)")"
}

do_rollback() {
  local ad; ad="$(arch_dir)"
  local qdir="$ad/$QUARANTINE"
  local latest previous
  latest="$(latest_rpm)"
  [ -n "$latest" ] || die "no client-facing RPM in fedora-$FEDORA-x86_64 — nothing to roll back."

  local -a to_move=()
  local target_nvra
  if [ -n "$TO_NVRA" ]; then
    # Roll back to a specific NVRA: quarantine everything strictly newer than it.
    local found="" f
    while IFS= read -r f; do
      [ "$(nvra_of "$f")" = "$TO_NVRA" ] && found="$f"
    done < <(client_facing_rpms)
    [ -n "$found" ] || die "target NVRA not present in the client-facing set: $TO_NVRA (see: $0 list)."
    local target_tok; target_tok="$(ver_token "$found")"
    while IFS= read -r f; do
      [ -n "$f" ] || continue
      # newer than target (and not the target itself) → quarantine
      if [ "$(printf '%s\n%s\n' "$target_tok" "$(ver_token "$f")" | sort -V | tail -1)" = "$(ver_token "$f")" ] \
         && [ "$(ver_token "$f")" != "$target_tok" ]; then
        to_move+=("$f")
      fi
    done < <(client_facing_rpms)
    [ "${#to_move[@]}" -gt 0 ] || die "nothing newer than $TO_NVRA to roll back."
    target_nvra="$TO_NVRA"
  else
    # Default: quarantine ONLY the current latest → revert to the previous NEVRA.
    previous="$(previous_rpm)"
    [ -n "$previous" ] && [ "$previous" != "$latest" ] || \
      die "only one NEVRA in the channel — no previous NEVRA to roll back to (would empty the channel). Use --to to target explicitly."
    to_move=("$latest")
    target_nvra="$(nvra_of "$previous")"
  fi

  log "rollback fedora-$FEDORA-x86_64: quarantine ${#to_move[@]} NEVRA(s); channel latest -> $target_nvra"
  local m
  for m in "${to_move[@]}"; do note "quarantine: $(nvra_of "$m")  ->  $QUARANTINE/"; done
  note "rollback target (new channel latest): $target_nvra"
  guard_mutation "rollback" || return 0

  mkdir -p "$qdir"
  local from_nvra; from_nvra="$(nvra_of "$latest")"
  for m in "${to_move[@]}"; do mv -f "$m" "$qdir/"; done
  reindex
  local now; now="$(nvra_of "$(latest_rpm)")"
  note "ROLLED BACK: channel latest is now $now"
  publish_rollback_event "$from_nvra" "$now"
  fleet_downgrade_hint "$(ver_token "$(latest_rpm)")"
}

do_re_promote() {
  local nvra="$1"
  [ -n "$nvra" ] || die "re-promote needs a quarantined NVRA (see: $0 list)"
  local ad; ad="$(arch_dir)"
  local src="$ad/$QUARANTINE/${nvra}.rpm"
  [ -f "$src" ] || die "not in quarantine: ${nvra}.rpm (see: $0 list)"
  log "re-promote $nvra: $QUARANTINE/ -> client-facing set"
  guard_mutation "re-promote" || return 0
  mv -f "$src" "$ad/"
  reindex
  note "re-promoted: channel latest is now $(nvra_of "$(latest_rpm)")"
}

# ── drill: full promote -> rollback -> re-promote on a throwaway temp root ───
run_drill() {
  local tmp fails=0
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  DRILL=1
  ROOT="$tmp/channel"
  FEDORA="99"
  local ad; ad="$(arch_dir)"; mkdir -p "$ad"
  local v1="magic-mesh-9.9.0-1.x86_64.rpm" v2="magic-mesh-9.9.0-2.x86_64.rpm"
  : > "$tmp/$v1"; : > "$tmp/$v2"

  _assert() { if [ "$2" = "$3" ]; then echo "  [PASS] $1"; else echo "  [FAIL] $1 (got '$2' want '$3')"; fails=1; fi; }

  echo "drill: promote -> rollback -> re-promote on throwaway root $ROOT"

  # 1. promote v1, then v2 (non-prod, applied)
  APPLY=1; NONPROD=1; CONFIRM=""
  do_promote "$tmp/$v1" >/dev/null 2>&1
  _assert "promote v1 -> latest=v1" "$(nvra_of "$(latest_rpm)")" "magic-mesh-9.9.0-1.x86_64"
  do_promote "$tmp/$v2" >/dev/null 2>&1
  _assert "promote v2 -> latest=v2" "$(nvra_of "$(latest_rpm)")" "magic-mesh-9.9.0-2.x86_64"

  # 2. rollback → latest reverts to v1, v2 quarantined & no longer client-facing
  do_rollback >/dev/null 2>&1
  _assert "rollback -> latest=v1"            "$(nvra_of "$(latest_rpm)")"          "magic-mesh-9.9.0-1.x86_64"
  _assert "rollback -> v2 quarantined"       "$(quarantined_rpms | xargs -r -n1 basename | tr '\n' ' ' | sed 's/ $//')" "magic-mesh-9.9.0-2.x86_64.rpm"
  _assert "rollback -> v2 not client-facing" "$(client_facing_rpms | grep -c 'magic-mesh-9.9.0-2' || true)" "0"

  # 3. re-promote v2 → latest back to v2, quarantine empty
  do_re_promote "magic-mesh-9.9.0-2.x86_64" >/dev/null 2>&1
  _assert "re-promote v2 -> latest=v2"    "$(nvra_of "$(latest_rpm)")" "magic-mesh-9.9.0-2.x86_64"
  _assert "re-promote -> quarantine empty" "$(quarantined_rpms | wc -l | tr -d ' ')" "0"

  # 4. SAFETY: dry-run (no --apply) mutates nothing
  APPLY=""; NONPROD=""; CONFIRM=""
  do_rollback >/dev/null 2>&1 || true
  _assert "dry-run leaves latest unchanged" "$(nvra_of "$(latest_rpm)")" "magic-mesh-9.9.0-2.x86_64"

  # 5. SAFETY: --apply on a non-(--non-prod) root without --confirm is refused.
  # (subshell: the guard aborts via `die`/exit, which the subshell contains.)
  APPLY=1; NONPROD=""; CONFIRM=""
  if ( do_rollback ) >/dev/null 2>&1; then
    echo "  [FAIL] guard: apply without --confirm should be refused"; fails=1
  else
    echo "  [PASS] guard: apply without --confirm is refused (typed-confirm required)"
  fi
  _assert "guard: refused apply mutated nothing" "$(nvra_of "$(latest_rpm)")" "magic-mesh-9.9.0-2.x86_64"

  # 6. SAFETY: --non-prod refuses the production default root.
  APPLY=1; NONPROD=1; CONFIRM=""; ROOT="$PROD_DEFAULT_ROOT"
  if ( do_rollback ) >/dev/null 2>&1; then
    echo "  [FAIL] guard: --non-prod should refuse the production root"; fails=1
  else
    echo "  [PASS] guard: --non-prod refuses the production default root"
  fi

  if [ "$fails" -eq 0 ]; then echo "drill: ALL PASS"; return 0; fi
  echo "drill: FAILURES"; return 1
}

# ── arg parse + dispatch ─────────────────────────────────────────────────────
[ $# -ge 1 ] || die "usage: $0 {list|promote RPM|rollback|re-promote NVRA|drill} [flags] (see --help)"
VERB="$1"; shift || true

case "$VERB" in -h|--help) awk 'NR==1{next} /^#/{sub(/^# ?/,"");print;next} {exit}' "$0"; exit 0 ;; esac

POSARG=""
while [ $# -gt 0 ]; do
  case "$1" in
    --channel-root) ROOT="$2"; shift 2 ;;
    --fedora)       FEDORA="$2"; shift 2 ;;
    --to)           TO_NVRA="$2"; shift 2 ;;
    --apply)        APPLY=1; shift ;;
    --non-prod)     NONPROD=1; shift ;;
    --confirm)      CONFIRM="$2"; shift 2 ;;
    -h|--help)      awk 'NR==1{next} /^#/{sub(/^# ?/,"");print;next} {exit}' "$0"; exit 0 ;;
    --*)            die "unknown flag: $1" ;;
    *)              POSARG="$1"; shift ;;
  esac
done

case "$VERB" in
  list)               do_list ;;
  promote)            do_promote "$POSARG" ;;
  rollback)           do_rollback ;;
  re-promote|repromote) do_re_promote "$POSARG" ;;
  drill|self-test)    run_drill ;;
  *) die "unknown verb: $VERB (want: list|promote|rollback|re-promote|drill)" ;;
esac
