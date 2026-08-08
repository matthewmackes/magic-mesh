#!/usr/bin/env bash
# Fail closed when a production host Browser engine, helper, or package seam is
# reintroduced. Browser VM guest/image code and typed VDI integration remain in
# scope and are intentionally excluded from engine-signature scanning.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

usage() {
  printf '%s\n' 'usage: install-helpers/lint-browser-vm-boundary.sh [--self-test]'
}

scan_tree() {
  local root="$1" findings="$2" path rel
  : > "$findings"

  for path in "$root/Cargo.toml" "$root/crates" "$root/packaging" "$root/install-helpers"; do
    if [[ ! -e "$path" ]]; then
      printf 'missing required scan root: %s\n' "${path#"$root/"}" >> "$findings"
    fi
  done
  [[ -s "$findings" ]] && return 0

  # Whole path families from the retired host implementation are forbidden.
  while IFS= read -r path; do
    rel="${path#"$root/"}"
    case "$rel" in
      crates/desktop/mde-web-*/*|crates/mesh/mde-browser-workers/*|crates/services/mde-adblock/*|\
      packaging/browser/*|packaging/selinux/mde-web-*|\
      packaging/systemd/mde-web-*|packaging/systemd/mde-browser-*|\
      packaging/systemd/mde-cef-*|packaging/systemd/mde-widevine-*|\
      install-helpers/install-browser-*|install-helpers/install-cef-*|\
      install-helpers/install-widevine-*|install-helpers/mirror-cef-*|\
      install-helpers/setup-selinux-web-*)
        printf 'retired host Browser path: %s\n' "$rel" >> "$findings"
        ;;
    esac
  done < <(find "$root/crates" "$root/packaging" "$root/install-helpers" \
    -type f -not -path '*/__pycache__/*' -print 2>/dev/null | sort)

  # Exact engine/package identifiers in production manifests, source, image,
  # and runtime policy catch renamed remnants without rejecting browser-vm or
  # VDI controller names. Historical extraction governance is not production.
  while IFS= read -r path; do
    rel="${path#"$root/"}"
    case "$rel" in
      packaging/browser-vm/*|install-helpers/browser-vm-production-control/*|\
      install-helpers/fixtures/*|install-helpers/verify-browser-extraction.sh|\
      install-helpers/lint-browser-vm-boundary.sh)
        continue
        ;;
    esac
    # Rust comments cannot create production reachability. Strip comment-only
    # lines from Rust before signature matching while retaining full scans for
    # manifests, scripts, image definitions, and runtime policy.
    if [[ "$path" == *.rs ]]; then
      sed '/^[[:space:]]*\/\//d' -- "$path" | grep -InE \
        '(mde[-_]web([_-](preview|cef))?|mde[-_]adblock|mde[-_]browser[-_]workers|BrowserEngine|(^|[^[:alnum:]_])(cef|servo)::|host_browser[[:space:]]*=[[:space:]]*true|mde[-_]widevine)' \
        >/dev/null 2>&1 || continue
      sed '/^[[:space:]]*\/\//d' -- "$path" | grep -InE \
        '(mde[-_]web([_-](preview|cef))?|mde[-_]adblock|mde[-_]browser[-_]workers|BrowserEngine|(^|[^[:alnum:]_])(cef|servo)::|host_browser[[:space:]]*=[[:space:]]*true|mde[-_]widevine)' \
        | sed "s#^#retired host Browser signature in $rel:#" >> "$findings"
    elif grep -InE \
      '(mde[-_]web([_-](preview|cef))?|mde[-_]adblock|mde[-_]browser[-_]workers|BrowserEngine|(^|[^[:alnum:]_])(cef|servo)::|host_browser[[:space:]]*=[[:space:]]*true|mde[-_]widevine)' \
      -- "$path" >/dev/null 2>&1; then
      grep -InE \
        '(mde[-_]web([_-](preview|cef))?|mde[-_]adblock|mde[-_]browser[-_]workers|BrowserEngine|(^|[^[:alnum:]_])(cef|servo)::|host_browser[[:space:]]*=[[:space:]]*true|mde[-_]widevine)' \
        -- "$path" | sed "s#^#retired host Browser signature in $rel:#" >> "$findings"
    fi
  done < <(find "$root" \
    \( -path "$root/.git" -o -path "$root/docs" -o -path "$root/target" -o \
       -path "$root/vendor" -o -path '*/__pycache__' \) -prune -o \
    -type f \( -name 'Cargo.toml' -o -name '*.rs' -o -name '*.service' -o \
      -name '*.conf' -o -name '*.policy' -o -name '*.te' -o -name '*.fc' -o \
      -name '*.sh' -o -name '*.ks' -o -name 'Containerfile' \) -print | sort)

  sort -u "$findings" -o "$findings"
}

run_lint() {
  local root="$1" findings
  findings="$(mktemp)"
  trap 'rm -f "${findings:-}"' RETURN
  scan_tree "$root" "$findings"
  if [[ -s "$findings" ]]; then
    printf '%s\n' 'lint-browser-vm-boundary.sh: host Browser production boundary violation(s):' >&2
    sed 's/^/  /' "$findings" >&2
    return 1
  fi
  printf '%s\n' 'lint-browser-vm-boundary.sh: clean — Browser engines remain guest-only'
}

self_test() {
  local fixture findings failures=0
  fixture="$(mktemp -d)"
  findings="$(mktemp)"
  trap 'rm -rf "${fixture:-}"; rm -f "${findings:-}"' RETURN
  mkdir -p "$fixture/crates/desktop/mde-shell-egui/src/web" \
    "$fixture/packaging/browser-vm" "$fixture/packaging/systemd" \
    "$fixture/install-helpers/browser-vm-production-control" \
    "$fixture/install-helpers/fixtures"
  printf '[workspace]\nmembers = []\n' > "$fixture/Cargo.toml"
  printf '%s\n' 'const VM_WORKLOAD: &str = "browser-vm";' \
    > "$fixture/crates/desktop/mde-shell-egui/src/web/mod.rs"
  printf '%s\n' 'RUN dnf install -y chromium' > "$fixture/packaging/browser-vm/Containerfile"
  printf '%s\n' 'typed Browser VM reconnect hook' \
    > "$fixture/install-helpers/browser-vm-production-control/README.md"
  scan_tree "$fixture" "$findings"
  if [[ -s "$findings" ]]; then
    printf '%s\n' 'lint-browser-vm-boundary.sh: SELF-TEST FAILED — guest/VDI fixture rejected' >&2
    sed 's/^/  /' "$findings" >&2
    failures=$((failures + 1))
  fi

  mkdir -p "$fixture/crates/services/mde-adblock/src"
  mkdir -p "$fixture/packaging/browser"
  printf '%s\n' 'pub struct Engine;' > "$fixture/crates/services/mde-adblock/src/lib.rs"
  printf '%s\n' 'legacy host Browser RPM' > "$fixture/packaging/browser/browser.spec"
  printf '%s\n' 'mde_web_cef_t' > "$fixture/packaging/systemd/legacy.policy"
  printf '%s\n' '#!/usr/bin/env bash' > "$fixture/install-helpers/install-cef-runtime.sh"
  scan_tree "$fixture" "$findings"
  if ! grep -Fq 'retired host Browser path: crates/services/mde-adblock/src/lib.rs' "$findings"; then
    printf '%s\n' 'lint-browser-vm-boundary.sh: SELF-TEST FAILED — retired crate path accepted' >&2
    failures=$((failures + 1))
  fi
  if ! grep -Fq 'retired host Browser signature in packaging/systemd/legacy.policy' "$findings"; then
    printf '%s\n' 'lint-browser-vm-boundary.sh: SELF-TEST FAILED — renamed runtime policy accepted' >&2
    failures=$((failures + 1))
  fi
  if ! grep -Fq 'retired host Browser path: packaging/browser/browser.spec' "$findings"; then
    printf '%s\n' 'lint-browser-vm-boundary.sh: SELF-TEST FAILED — host Browser package accepted' >&2
    failures=$((failures + 1))
  fi
  if ! grep -Fq 'retired host Browser path: install-helpers/install-cef-runtime.sh' "$findings"; then
    printf '%s\n' 'lint-browser-vm-boundary.sh: SELF-TEST FAILED — host engine installer accepted' >&2
    failures=$((failures + 1))
  fi

  rm -rf "$fixture/install-helpers"
  scan_tree "$fixture" "$findings"
  if ! grep -Fq 'missing required scan root: install-helpers' "$findings"; then
    printf '%s\n' 'lint-browser-vm-boundary.sh: SELF-TEST FAILED — incomplete scan root accepted' >&2
    failures=$((failures + 1))
  fi

  [[ "$failures" -eq 0 ]] || return 1
  printf '%s\n' 'lint-browser-vm-boundary.sh: self-test passed'
}

case "${1:-}" in
  '') run_lint "$ROOT" ;;
  --self-test) self_test ;;
  -h|--help) usage ;;
  *) usage >&2; exit 2 ;;
esac
