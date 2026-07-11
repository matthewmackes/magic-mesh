#!/usr/bin/env bash
# verify-rpm-payload.sh — the "compiles ≠ ships" + "shipped-but-dead surface" gate.
#
# WHY (two real, recurring regression classes this static gate prevents):
#
#   (a) COMPILES ≠ SHIPS. The workspace can build 100% green while the RPM omits
#       a binary that a `target/release/…` asset entry promises. This actually
#       shipped once: a strip/replace dropped the iced GUIs but never added the
#       egui shell to the `generate-rpm` assets → 11.4.0 shipped with the shell
#       binary MISSING even though every crate compiled. DoD for a strip/replace
#       is therefore `rpm -qlp <rpm> | grep <replacement-bin>` — this script is
#       that check, plus a static dry-run that needs no RPM.
#
#   (b) SHIPPED-BUT-DEAD SURFACE. A whole `mde-*-egui` surface epic (terminal,
#       media) can build green yet be UNREACHABLE — its lib compiled into no
#       shipped binary and mounted at no dock Surface, with zero launchers. That
#       is a surface that exists in the tree, passes tests, and reaches no user.
#       TERM-16 / MEDIA-18 wired the two that had regressed; this script guards
#       every future `mde-*-egui` the same way.
#
# Neither regression has an automated gate today. This one is STATIC (no build,
# no cargo, no network): it parses the RPM asset manifest out of
# `crates/mesh/mackesd/Cargo.toml` ([package.metadata.generate-rpm].assets) and
# greps the shell's dock. It is fast enough to run on every push.
#
# ─────────────────────────────────────────────────────────────────────────────
# HOW TO RUN
#
#   install-helpers/verify-rpm-payload.sh                # dry-run BOTH checks (default)
#   install-helpers/verify-rpm-payload.sh all            # same as no args
#   install-helpers/verify-rpm-payload.sh payload        # RPM-payload check, dry-run (no RPM)
#   install-helpers/verify-rpm-payload.sh payload a.rpm  # validate a REAL built RPM's file list
#   install-helpers/verify-rpm-payload.sh surfaces       # surface-reachability check only
#   install-helpers/verify-rpm-payload.sh --self-test    # exercise the parser on good+broken fixtures
#   install-helpers/verify-rpm-payload.sh --help
#
# Exit code is 0 only when every check passes; any FAIL exits non-zero so the
# script is drop-in for a gate. Output is greppable: each check prints one of
#   [OK] / [FAIL] / [WARN] / [INFO] / [SKIP]
#
# ADVISORY / NOT AUTO-ENABLED. Nothing runs this for you yet. It is meant to gate
# a release CUT — wire it into the farm CI gate (install-helpers/ci-gate.sh, the
# always-on farm gate) as a pre-cut stage, or call it by hand right before
# `/release`. It deliberately does NOT cut an RPM or run a release (both are
# operator-gated); its job is only to VERIFY. The real-RPM mode expects an RPM a
# gated build already produced.
#
# ─────────────────────────────────────────────────────────────────────────────
# DRY-RUN semantics (no RPM):
#   payload  : lists the expected asset set; for every asset SOURCE it asserts —
#              * target/…            → some workspace crate builds a bin of that
#                                      name (the static proxy for "the build will
#                                      produce it"); a name nothing builds FAILs.
#              * vendor/birthright/… → fetched+verified at build time; INFO, skipped.
#              * anything else       → the file/glob exists in the tree now; a
#                                      missing packaging source FAILs.
#              Extra hard emphasis on the replacement bins mde-shell-egui, mackesd,
#              mde-web-preview: each MUST appear as a target/release asset.
#   surfaces : every mde-*-egui crate under crates/desktop (minus the shell host
#              and the documented EXEMPT list) MUST be BOTH dock-mounted (named in
#              the shell's dock.rs Surface enum) AND shipped (a path-dep of
#              mde-shell-egui, whose binary is itself in the asset set). A surface
#              that is one but not the other FAILs.
#
# Real-RPM semantics (`payload <rpm>`): runs `rpm -qlp <rpm>` and asserts every
# expected install path is in the payload (globs are checked best-effort by dest
# prefix; the key bins are checked exactly).
#
# ─────────────────────────────────────────────────────────────────────────────
# EXEMPT surface crates — mde-*-egui crates under crates/desktop that are NOT dock
# surfaces and so are not required to be dock-mounted. Keep this list SHORT and
# justify every entry; the whole point of the gate is that new surfaces cannot
# silently join this set.
#   mde-panel-egui : the E12-7 egui panel CLIENT (the retired cosmic-applet's
#                    replacement), not a dock Surface. It renders standalone, not
#                    inside the shell's dock. If it is ever wired into the shell
#                    or retired, drop it from this list.
# (mde-shell-egui is the dock HOST itself, handled separately — never a surface.)
#
# Env overrides (mostly for --self-test; default to the live repo layout):
#   CARGO_TOML   RPM manifest         (default crates/mesh/mackesd/Cargo.toml)
#   SHELL_CARGO  shell manifest       (default crates/desktop/mde-shell-egui/Cargo.toml)
#   DOCK_RS      dock source          (default crates/desktop/mde-shell-egui/src/dock.rs)
#   DESKTOP_DIR  surface-crate dir    (default crates/desktop)
#   REPO_ROOT    tree root for assets (default: the git worktree this script is in)
#   MCNF_FAKE_RPM_LIST  a file whose lines stand in for `rpm -qlp` (real-RPM test hook)
set -uo pipefail
shopt -s globstar nullglob

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "$HERE/.." && pwd)}"
CARGO_TOML="${CARGO_TOML:-$REPO_ROOT/crates/mesh/mackesd/Cargo.toml}"
SHELL_CARGO="${SHELL_CARGO:-$REPO_ROOT/crates/desktop/mde-shell-egui/Cargo.toml}"
DOCK_RS="${DOCK_RS:-$REPO_ROOT/crates/desktop/mde-shell-egui/src/dock.rs}"
DESKTOP_DIR="${DESKTOP_DIR:-$REPO_ROOT/crates/desktop}"

# The dock HOST (never a surface) and the justified non-surface egui crates.
readonly SHELL_HOST_CRATE="mde-shell-egui"
EXEMPT_SURFACES=("mde-panel-egui")

# The three replacement binaries the (a)-class regression is really about.
readonly KEY_BINS=("mde-shell-egui" "mackesd" "mde-web-preview")

FAILS=0
ok()   { printf '[OK]   %s\n' "$*"; }
fail() { printf '[FAIL] %s\n' "$*"; FAILS=$((FAILS + 1)); }
info() { printf '[INFO] %s\n' "$*"; }
skip() { printf '[SKIP] %s\n' "$*"; }
hdr()  { printf '\n=== %s ===\n' "$*"; }

# ── parse_assets <cargo.toml> ────────────────────────────────────────────────
# Emit one "source<TAB>dest" line per asset in the MAIN [package.metadata.
# generate-rpm].assets array (NOT the .variants.server array — that headless
# subset legitimately omits the GUI bins). Stops at the array's closing `]`.
parse_assets() {
  awk '
    /^\[package\.metadata\.generate-rpm\]$/ { main = 1; next }
    main && /^assets = \[/               { ina = 1; next }
    ina {
      if ($0 ~ /^[[:space:]]*\][[:space:]]*$/) { exit }
      if ($0 ~ /source[[:space:]]*=/) {
        src = $0; sub(/.*source[[:space:]]*=[[:space:]]*"/, "", src); sub(/".*/, "", src)
        dst = ""
        if ($0 ~ /dest[[:space:]]*=/) { dst = $0; sub(/.*dest[[:space:]]*=[[:space:]]*"/, "", dst); sub(/".*/, "", dst) }
        print src "\t" dst
      }
    }
  ' "$1"
}

# Does a path contain a glob metacharacter?
has_glob() { case "$1" in *[\*\?\[]*) return 0 ;; *) return 1 ;; esac; }

# Does ANY workspace crate build a binary of this exact name? (static proxy for
# "the release build will produce target/release/<name>")
crate_builds_bin() {
  grep -rlqE "name = \"$1\"" --include=Cargo.toml "$REPO_ROOT/crates" 2>/dev/null
}

# ── surface universe ─────────────────────────────────────────────────────────
# All mde-*-egui crate dirs under DESKTOP_DIR, basename only.
list_surface_crates() {
  local d
  for d in "$DESKTOP_DIR"/mde-*-egui; do
    [ -d "$d" ] && printf '%s\n' "${d##*/}"
  done
}
is_exempt() {
  local c="$1" e
  for e in "${EXEMPT_SURFACES[@]}"; do [ "$c" = "$e" ] && return 0; done
  return 1
}
# Is crate a path-dependency of the shell? (i.e. compiled INTO the shipped shell)
shell_depends_on() {
  grep -qE "^[[:space:]]*$1[[:space:]]*=[[:space:]]*\{[[:space:]]*path" "$SHELL_CARGO"
}
# Is the crate named in dock.rs? (its Surface variant doc names it as `mde-x-egui`)
dock_mounts() { grep -q "$1" "$DOCK_RS"; }

# ═════════════════════════════════════════════════════════════════════════════
# CHECK 1 — RPM payload (build-deploy-2)
# ═════════════════════════════════════════════════════════════════════════════
check_payload_dryrun() {
  hdr "payload (dry-run, static) — manifest: ${CARGO_TOML#"$REPO_ROOT"/}"
  local total=0 src dst
  # collect the parsed source set (for the key-bin emphasis at the end)
  local -A src_seen=()
  while IFS=$'\t' read -r src dst; do
    [ -n "$src" ] || continue
    total=$((total + 1))
    src_seen["$src"]=1
    case "$src" in
      target/*)
        local bin="${src##*/}"
        if crate_builds_bin "$bin"; then
          ok "build-output   $src  (a workspace crate builds '$bin')"
        else
          fail "build-output   $src  → NO workspace crate builds a binary named '$bin' (RPM would ship a bin nothing produces)"
        fi
        ;;
      vendor/birthright/*)
        info "build-fetch    $src  (fetched+sha256-verified at build by vendor-birthright-blobs.sh; skipped)"
        ;;
      *)
        if has_glob "$src"; then
          # shellcheck disable=SC2206  # intentional glob expansion (globstar/nullglob on)
          local m=( $REPO_ROOT/$src )
          if [ "${#m[@]}" -gt 0 ]; then
            ok "repo-glob      $src  (${#m[@]} match(es))"
          else
            fail "repo-glob      $src  → matches NOTHING in the tree (packaging source vanished)"
          fi
        else
          if [ -e "$REPO_ROOT/$src" ]; then
            ok "repo-file      $src"
          else
            fail "repo-file      $src  → MISSING from the tree (asset source absent)"
          fi
        fi
        ;;
    esac
  done < <(parse_assets "$CARGO_TOML")

  info "parsed $total asset entries from the main generate-rpm array"

  hdr "key replacement binaries (must be shipped)"
  local kb
  for kb in "${KEY_BINS[@]}"; do
    if [ -n "${src_seen["target/release/$kb"]:-}" ]; then
      ok "key-bin        target/release/$kb  is in the asset set"
    else
      fail "key-bin        target/release/$kb  is NOT in the asset set (the exact 'compiles ≠ ships' regression)"
    fi
  done
}

# Read the RPM file list (real, or a fake listing for --self-test).
rpm_file_list() {
  if [ -n "${MCNF_FAKE_RPM_LIST:-}" ]; then
    cat "$MCNF_FAKE_RPM_LIST"
  else
    rpm -qlp "$1"
  fi
}

check_payload_rpm() {
  local rpm="$1"
  hdr "payload (real RPM) — $rpm"
  if [ -z "${MCNF_FAKE_RPM_LIST:-}" ] && [ ! -f "$rpm" ]; then
    fail "RPM not found: $rpm"
    return
  fi
  local listing
  if ! listing="$(rpm_file_list "$rpm")"; then
    fail "could not read file list from $rpm"
    return
  fi

  # Key bins: exact install-path assertions (the DoD line for a strip/replace).
  hdr "key replacement binaries present in payload"
  local want
  for want in /usr/bin/mde-shell-egui /usr/bin/mackesd /usr/bin/mde-web-preview; do
    if grep -Fxq "$want" <<<"$listing"; then
      ok "key-bin        $want present in rpm -qlp"
    else
      fail "key-bin        $want MISSING from the RPM payload"
    fi
  done

  hdr "every manifest asset present in payload"
  local src dst
  while IFS=$'\t' read -r src dst; do
    if [ -z "$src" ] || [ -z "$dst" ]; then continue; fi
    if has_glob "$src"; then
      # Best-effort: assert the dest directory prefix has at least one entry.
      local pref="${dst%/}/"
      if grep -Fq "$pref" <<<"$listing"; then
        ok "glob-dest      $src → $pref (present)"
      else
        fail "glob-dest      $src → $pref has NO entries in the payload"
      fi
    else
      local want_path
      case "$dst" in
        */) want_path="${dst}${src##*/}" ;;
        *)  want_path="$dst" ;;
      esac
      if grep -Fxq "$want_path" <<<"$listing"; then
        ok "asset          $want_path"
      else
        fail "asset          $want_path MISSING (source $src)"
      fi
    fi
  done < <(parse_assets "$CARGO_TOML")
}

check_payload() {
  if [ -n "${1:-}" ]; then
    check_payload_rpm "$1"
  else
    check_payload_dryrun
  fi
}

# ═════════════════════════════════════════════════════════════════════════════
# CHECK 2 — surface reachability (test-obs-4)
# ═════════════════════════════════════════════════════════════════════════════
check_surfaces() {
  hdr "surfaces (dry-run, static) — dock: ${DOCK_RS#"$REPO_ROOT"/}"

  # Precompute: is the shell binary itself shipped? A surface can only "ship"
  # by being compiled into a shipped binary — and mde-shell-egui is that binary.
  # (Capture once into a var — piping awk into `grep -q` trips SIGPIPE+pipefail
  # on the large manifest, which would read as a false no-match.)
  local assets_out shell_shipped=0
  assets_out="$(parse_assets "$CARGO_TOML")"
  if grep -q "^target/release/${SHELL_HOST_CRATE}"$'\t' <<<"$assets_out"; then
    shell_shipped=1
    ok "shell-host     target/release/${SHELL_HOST_CRATE} is in the asset set (surfaces ride it)"
  else
    fail "shell-host     target/release/${SHELL_HOST_CRATE} NOT in the asset set — NO surface can ship"
  fi

  local c
  while IFS= read -r c; do
    [ -n "$c" ] || continue
    if [ "$c" = "$SHELL_HOST_CRATE" ]; then
      info "host           $c is the dock host, not a surface (skipped)"
      continue
    fi
    if is_exempt "$c"; then
      # Still report its wiring truthfully so an accidental one is visible.
      local dep="no" mnt="no"
      shell_depends_on "$c" && dep="yes"
      dock_mounts "$c" && mnt="yes"
      skip "exempt         $c (documented non-dock-surface; shell-dep=$dep dock-ref=$mnt)"
      continue
    fi

    local is_dep=0 is_mnt=0
    shell_depends_on "$c" && is_dep=1
    dock_mounts "$c" && is_mnt=1

    if [ "$is_dep" -eq 1 ] && [ "$is_mnt" -eq 1 ] && [ "$shell_shipped" -eq 1 ]; then
      ok "surface        $c  mounted in dock.rs AND compiled into the shipped shell"
    else
      local why=""
      [ "$is_mnt" -eq 1 ] || why+=" NOT-mounted(no dock.rs Surface reference)"
      [ "$is_dep" -eq 1 ] || why+=" NOT-shipped(not a mde-shell-egui path-dep → compiled into no shipped bin)"
      [ "$shell_shipped" -eq 1 ] || why+=" shell-bin-unshipped"
      fail "surface        $c  built-but-dead:$why"
    fi
  done < <(list_surface_crates)
}

# ═════════════════════════════════════════════════════════════════════════════
# SELF-TEST — exercise the parser + classifiers on good and broken fixtures.
# ═════════════════════════════════════════════════════════════════════════════
self_test() {
  hdr "SELF-TEST"
  local tmp rc
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  local st_fail=0

  # ---- fixture A: a KNOWN-GOOD manifest -------------------------------------
  local good="$tmp/good.toml"
  cat >"$good" <<'TOML'
[package.metadata.generate-rpm]
name = "fixture"
assets = [
    { source = "target/release/mde-shell-egui", dest = "/usr/bin/mde-shell-egui", mode = "755" },
    { source = "target/release/mackesd",        dest = "/usr/bin/mackesd",        mode = "755" },
    { source = "packaging/x.service",           dest = "/usr/lib/systemd/system/x.service", mode = "644" },
]
[package.metadata.generate-rpm.variants.server]
assets = [
    { source = "target/release/should-be-ignored", dest = "/usr/bin/should-be-ignored", mode = "755" },
]
TOML
  local n
  n="$(parse_assets "$good" | wc -l | tr -d ' ')"
  if [ "$n" -eq 3 ]; then
    ok "self-test: parser reads exactly the 3 MAIN assets (ignores the server variant)"
  else
    fail "self-test: expected 3 main assets, got $n"; st_fail=1
  fi
  if parse_assets "$good" | grep -q "^target/release/mde-shell-egui"$'\t'"/usr/bin/mde-shell-egui$"; then
    ok "self-test: parser captures source<TAB>dest correctly"
  else
    fail "self-test: parser did not capture the shell asset row"; st_fail=1
  fi
  if parse_assets "$good" | grep -q "should-be-ignored"; then
    fail "self-test: parser LEAKED a server-variant asset"; st_fail=1
  else
    ok "self-test: parser does NOT leak server-variant assets"
  fi

  # ---- fixture B: a SYNTHETICALLY-BROKEN manifest ---------------------------
  # Drops the shell key-bin, adds a bin nothing builds, adds a missing file.
  local bad="$tmp/bad.toml"
  cat >"$bad" <<'TOML'
[package.metadata.generate-rpm]
name = "fixture"
assets = [
    { source = "target/release/mackesd",         dest = "/usr/bin/mackesd",         mode = "755" },
    { source = "target/release/mde-web-preview",  dest = "/usr/bin/mde-web-preview",  mode = "755" },
    { source = "target/release/mde-ghost-bin",    dest = "/usr/bin/mde-ghost-bin",    mode = "755" },
    { source = "packaging/definitely-missing.service", dest = "/usr/lib/systemd/system/definitely-missing.service", mode = "644" },
]
TOML
  # Run the dry-run against the broken fixture; it MUST fail and name the issues.
  local out
  out="$(CARGO_TOML="$bad" FAILS=0 bash "$0" payload 2>&1)"; rc=$?
  if [ "$rc" -ne 0 ]; then
    ok "self-test: broken manifest makes the payload check EXIT NON-ZERO ($rc)"
  else
    fail "self-test: broken manifest did NOT fail the payload check"; st_fail=1
  fi
  if grep -q "mde-ghost-bin" <<<"$out" && grep -q "NO workspace crate builds" <<<"$out"; then
    ok "self-test: flags target/release/mde-ghost-bin (bin nothing builds)"
  else
    fail "self-test: did not flag the un-buildable ghost binary"; st_fail=1
  fi
  if grep -q "definitely-missing.service" <<<"$out" && grep -q "MISSING from the tree" <<<"$out"; then
    ok "self-test: flags the missing packaging source"; else
    fail "self-test: did not flag the missing packaging source"; st_fail=1
  fi
  if grep -q "target/release/mde-shell-egui  is NOT in the asset set" <<<"$out"; then
    ok "self-test: flags the dropped mde-shell-egui key bin (the real 11.4.0 regression)"
  else
    fail "self-test: did not flag the dropped shell key-bin"; st_fail=1
  fi

  # ---- fixture C: surface reachability on a synthetic desktop tree ----------
  local dt="$tmp/desktop"
  mkdir -p "$dt/mde-shell-egui/src" "$dt/mde-good-egui" "$dt/mde-orphan-egui"
  local scargo="$dt/mde-shell-egui/Cargo.toml"
  local sdock="$dt/mde-shell-egui/src/dock.rs"
  cat >"$scargo" <<'TOML'
[dependencies]
mde-good-egui = { path = "../mde-good-egui" }
TOML
  cat >"$sdock" <<'RS'
pub enum Surface {
    /// The good surface (`mde-good-egui`).
    Good,
}
RS
  # good manifest that ships the shell so surfaces can ride it
  local scmani="$tmp/surf.toml"
  cat >"$scmani" <<'TOML'
[package.metadata.generate-rpm]
assets = [
    { source = "target/release/mde-shell-egui", dest = "/usr/bin/mde-shell-egui", mode = "755" },
]
TOML
  out="$(DESKTOP_DIR="$dt" SHELL_CARGO="$scargo" DOCK_RS="$sdock" CARGO_TOML="$scmani" bash "$0" surfaces 2>&1)"; rc=$?
  if grep -q "surface        mde-good-egui  mounted in dock.rs AND compiled" <<<"$out"; then
    ok "self-test: a properly wired surface PASSES"
  else
    fail "self-test: wired surface did not pass"; st_fail=1
  fi
  if grep -q "mde-orphan-egui  built-but-dead" <<<"$out" && [ "$rc" -ne 0 ]; then
    ok "self-test: an unmounted+unshipped surface FAILS (the term/media regression)"
  else
    fail "self-test: orphan surface was not caught"; st_fail=1
  fi

  hdr "SELF-TEST RESULT"
  if [ "$st_fail" -eq 0 ]; then
    ok "self-test: all assertions passed"
    return 0
  fi
  fail "self-test: $st_fail assertion group(s) failed"
  return 1
}

usage() {
  sed -n '2,60p' "$0" | sed 's/^# \{0,1\}//'
}

main() {
  local cmd="${1:-all}"
  case "$cmd" in
    -h|--help|help) usage; exit 0 ;;
    --self-test|self-test) self_test; exit $? ;;
    payload)  shift; check_payload "${1:-}" ;;
    surfaces) check_surfaces ;;
    all|"")   check_payload_dryrun; check_surfaces ;;
    *) printf 'unknown command: %s\n\n' "$cmd" >&2; usage >&2; exit 2 ;;
  esac

  hdr "SUMMARY"
  if [ "$FAILS" -eq 0 ]; then
    ok "all checks passed"
    exit 0
  fi
  fail "$FAILS check(s) failed"
  exit 1
}

main "$@"
