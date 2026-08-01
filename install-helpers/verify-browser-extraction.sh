#!/usr/bin/env bash
# Verify or generate the WL-ARCH-008 host-Browser extraction manifest.
#
# This helper is deliberately read-only by default.  It never creates a clone,
# rewrites history, publishes a repository, or removes source.  --write only
# regenerates the checked-in manifest after the current source snapshot has
# been audited.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
MANIFEST="$ROOT/docs/design/browser-stack-extraction/manifest.tsv"

readonly SIGNAL_RE='mde-web|mde-browser|Surface::Browser|BrowserEngine|MDE_(WEB|CEF)|browser_(policy|security_update|passkey|offline_cache|session_sync|share|protocol|tab_suspend|translate|voice_command|read_aloud|media)|magic-mesh-browser|widevine|WebExtension|Servo'

usage() {
  cat <<'EOF'
usage: install-helpers/verify-browser-extraction.sh [--check|--write|--self-test]

  --check  verify the recorded source commit, blob IDs, classifications, and
           generated candidate set (the default)
  --write  regenerate manifest.tsv from the current committed source snapshot
           after refusing untracked Browser candidates
  --self-test  exercise the classification and derived provenance rules without
               requiring a standalone repository or Git history
EOF
}

die() {
  echo "verify-browser-extraction: $*" >&2
  exit 1
}

git_cmd() {
  git -C "$ROOT" "$@"
}

path_is_named_candidate() {
  case "$1" in
    docs/design/browser-stack-extraction/*|install-helpers/verify-browser-extraction.sh)
      return 1
      ;;
    Cargo.toml|Cargo.lock|\
    crates/desktop/mde-web-*|crates/desktop/mde-shell-egui/src/web/*|\
    crates/mesh/mde-browser-workers/*|\
    crates/mesh/mackesd/src/workers/transfers/lane/browser_media.rs|\
    crates/services/mde-adblock/*|crates/services/mde-bookmarks/*|\
    crates/desktop/mde-bookmarks-egui/*|crates/mesh/mde-seal/*|\
    crates/mesh/mde-worker-core/*|\
    crates/mesh/mackes-mesh-types/src/lib.rs|crates/mesh/mackes-mesh-types/src/mesh_storage.rs|\
    docs/design/browser-*.md|docs/design/mesh-bookmarks.md|docs/design/rpm-size-split.md|\
    docs/THREAT_MODEL.md|\
    install-helpers/browser-*|install-helpers/install-browser-*|\
    install-helpers/install-cef-runtime.sh|install-helpers/install-widevine-cdm.sh|\
    install-helpers/mirror-cef-runtime-to-spaces.sh|\
    install-helpers/setup-selinux-web-cef.sh|install-helpers/setup-selinux-web-preview.sh|\
    install-helpers/build-rpm-fedora43.sh|install-helpers/rpm-features.sh|\
    install-helpers/verify-rpm-payload.sh|install-helpers/lint-layered-tiers.sh|\
    install-helpers/lint-style-leaks.sh|install-helpers/xcp-build.sh|\
    packaging/browser/*|packaging/selinux/mde-web-*|\
    packaging/systemd/mde-browser-*|packaging/systemd/mde-cef-runtime-setup.service|\
    packaging/systemd/mde-widevine-cdm-setup.service|packaging/systemd/mde-web-*|\
    packaging/bootc/Containerfile|packaging/bootc/build-image.sh|\
    packaging/bootc/verify-image.sh|packaging/bootc/units/mde-shell-egui.service|\
    packaging/kickstart/magic-on-quasar.ks|LICENSE|NOTICE)
      return 0
      ;;
    crates/desktop/mde-shell-egui/Cargo.toml|\
    crates/desktop/mde-shell-egui/src/{console/custom_sync.rs,datacenter.rs,front_door.rs,main.rs,nav_bar.rs,storage/tests.rs,surfaces.rs,switcher.rs,toast_bridge.rs}|\
    crates/mesh/mackesd/Cargo.toml|\
    crates/mesh/mackesd/src/{bin/mackesd/spawn.rs,ca/backup.rs,ipc/secret_store.rs,lib.rs,onboard/role_provision.rs,worker_role.rs}|\
    crates/mesh/mackesd/src/workers/{adfilter.rs,mod.rs,storage.rs}|\
    crates/mesh/mackesd/src/workers/kdc_host/{media.rs,mod.rs,tests.rs}|\
    crates/mesh/mackesd/src/workers/transfers/lane/{mod.rs,tests.rs})
      return 0
      ;;
  esac
  return 1
}

class_for_path() {
  case "$1" in
    docs/design/browser-stack-extraction/*|install-helpers/verify-browser-extraction.sh)
      return 1
      ;;
    crates/desktop/mde-web-*|crates/desktop/mde-shell-egui/src/web/*|\
    crates/mesh/mde-browser-workers/*|\
    crates/mesh/mackesd/src/workers/transfers/lane/browser_media.rs|\
    crates/services/mde-adblock/*|docs/design/browser-*.md|\
    install-helpers/browser-*|install-helpers/install-browser-*|\
    install-helpers/install-cef-runtime.sh|install-helpers/install-widevine-cdm.sh|\
    install-helpers/mirror-cef-runtime-to-spaces.sh|\
    install-helpers/setup-selinux-web-cef.sh|install-helpers/setup-selinux-web-preview.sh|\
    packaging/browser/*|packaging/selinux/mde-web-*|\
    packaging/systemd/mde-browser-*|packaging/systemd/mde-cef-runtime-setup.service|\
    packaging/systemd/mde-widevine-cdm-setup.service|packaging/systemd/mde-web-*)
      printf '%s\n' browser-owned
      ;;
    Cargo.toml|Cargo.lock|crates/desktop/mde-shell-egui/Cargo.toml|\
    crates/desktop/mde-shell-egui/src/console/custom_sync.rs|\
    crates/desktop/mde-shell-egui/src/datacenter.rs|\
    crates/desktop/mde-shell-egui/src/front_door.rs|\
    crates/desktop/mde-shell-egui/src/main.rs|\
    crates/desktop/mde-shell-egui/src/nav_bar.rs|\
    crates/desktop/mde-shell-egui/src/storage/tests.rs|\
    crates/desktop/mde-shell-egui/src/surfaces.rs|\
    crates/desktop/mde-shell-egui/src/switcher.rs|\
    crates/desktop/mde-shell-egui/src/toast_bridge.rs|\
    crates/mesh/mackesd/Cargo.toml|\
    crates/mesh/mackesd/src/bin/mackesd/spawn.rs|\
    crates/mesh/mackesd/src/onboard/role_provision.rs|\
    crates/mesh/mackesd/src/worker_role.rs|\
    crates/mesh/mackesd/src/workers/adfilter.rs|\
    crates/mesh/mackesd/src/workers/mod.rs|\
    crates/mesh/mackesd/src/workers/kdc_host/media.rs|\
    crates/mesh/mackesd/src/workers/kdc_host/mod.rs|\
    crates/mesh/mackesd/src/workers/kdc_host/tests.rs|\
    crates/mesh/mackesd/src/workers/transfers/lane/mod.rs|\
    crates/mesh/mackesd/src/workers/transfers/lane/tests.rs|\
    packaging/bootc/Containerfile|packaging/bootc/build-image.sh|\
    packaging/bootc/verify-image.sh|packaging/bootc/units/mde-shell-egui.service|\
    packaging/kickstart/magic-on-quasar.ks|\
    install-helpers/build-rpm-fedora43.sh|install-helpers/rpm-features.sh|\
    install-helpers/verify-rpm-payload.sh|docs/design/mesh-bookmarks.md|\
    docs/design/rpm-size-split.md|docs/THREAT_MODEL.md)
      printf '%s\n' mixed-purpose
      ;;
    crates/services/mde-bookmarks/*|crates/desktop/mde-bookmarks-egui/*|\
    crates/mesh/mde-seal/*|crates/mesh/mde-worker-core/*|\
    crates/mesh/mackes-mesh-types/src/lib.rs|crates/mesh/mackes-mesh-types/src/mesh_storage.rs|\
    crates/mesh/mackesd/src/ca/backup.rs|crates/mesh/mackesd/src/ipc/secret_store.rs|\
    crates/mesh/mackesd/src/lib.rs|crates/mesh/mackesd/src/workers/storage.rs|\
    install-helpers/lint-layered-tiers.sh|install-helpers/lint-style-leaks.sh|\
    install-helpers/xcp-build.sh|LICENSE|NOTICE)
      printf '%s\n' shared
      ;;
    *)
      return 1
      ;;
  esac
}

reason_for_path() {
  case "$1" in
    crates/desktop/mde-web-*) printf '%s\n' browser-helper-crate ;;
    crates/desktop/mde-shell-egui/src/web/*) printf '%s\n' host-browser-shell ;;
    crates/mesh/mde-browser-workers/*) printf '%s\n' browser-worker-family ;;
    crates/mesh/mackesd/src/workers/transfers/lane/browser_media.rs) printf '%s\n' browser-transfer-lane ;;
    crates/services/mde-adblock/*) printf '%s\n' browser-adblock-engine ;;
    docs/design/browser-*.md) printf '%s\n' browser-implementation-doc ;;
    install-helpers/browser-*|install-helpers/install-browser-*|\
    install-helpers/install-cef-runtime.sh|install-helpers/install-widevine-cdm.sh|\
    install-helpers/mirror-cef-runtime-to-spaces.sh|\
    install-helpers/setup-selinux-web-cef.sh|install-helpers/setup-selinux-web-preview.sh|\
    packaging/browser/*|packaging/selinux/mde-web-*|\
    packaging/systemd/mde-browser-*|packaging/systemd/mde-cef-runtime-setup.service|\
    packaging/systemd/mde-widevine-cdm-setup.service|packaging/systemd/mde-web-*)
      printf '%s\n' browser-runtime-packaging
      ;;
    Cargo.toml|Cargo.lock|*/Cargo.toml) printf '%s\n' workspace-package-edge ;;
    crates/desktop/mde-shell-egui/src/*) printf '%s\n' shell-route-or-shared-state ;;
    crates/mesh/mackesd/src/bin/mackesd/spawn.rs|crates/mesh/mackesd/src/worker_role.rs|\
    crates/mesh/mackesd/src/workers/mod.rs|crates/mesh/mackesd/src/workers/adfilter.rs)
      printf '%s\n' daemon-worker-registration
      ;;
    crates/mesh/mackesd/src/workers/kdc_host/*|crates/mesh/mackesd/src/workers/transfers/lane/*)
      printf '%s\n' mixed-daemon-consumer
      ;;
    crates/mesh/mackesd/src/onboard/role_provision.rs|packaging/bootc/*|packaging/kickstart/*|\
    install-helpers/build-rpm-fedora43.sh|install-helpers/rpm-features.sh|\
    install-helpers/verify-rpm-payload.sh)
      printf '%s\n' image-or-package-integration
      ;;
    docs/design/mesh-bookmarks.md|docs/design/rpm-size-split.md|docs/THREAT_MODEL.md)
      printf '%s\n' mixed-implementation-document
      ;;
    crates/services/mde-bookmarks/*|crates/desktop/mde-bookmarks-egui/*)
      printf '%s\n' shared-bookmark-contract
      ;;
    crates/mesh/mde-seal/*) printf '%s\n' shared-secret-contract ;;
    crates/mesh/mde-worker-core/*) printf '%s\n' shared-worker-contract ;;
    crates/mesh/mackes-mesh-types/*) printf '%s\n' shared-mesh-contract ;;
    crates/mesh/mackesd/src/ca/backup.rs|crates/mesh/mackesd/src/ipc/secret_store.rs)
      printf '%s\n' shared-secret-consumer
      ;;
    crates/mesh/mackesd/src/lib.rs|crates/mesh/mackesd/src/workers/storage.rs|\
    install-helpers/lint-layered-tiers.sh|install-helpers/lint-style-leaks.sh|\
    install-helpers/xcp-build.sh)
      printf '%s\n' shared-infrastructure-reference
      ;;
    LICENSE|NOTICE) printf '%s\n' shared-legal-root ;;
    *) printf '%s\n' unclassified ;;
  esac
}

destination_for() {
  case "$(class_for_path "$1")" in
    browser-owned) printf 'magic-mesh-browser-stack:%s\n' "$1" ;;
    mixed-purpose) printf 'split-in-magic-mesh-browser-stack:%s#browser-sections\n' "$1" ;;
    shared) printf 'retain-in-magic-mesh:%s#shared-contract-or-reference\n' "$1" ;;
    *) return 1 ;;
  esac
}

candidate_scope() {
  case "$1" in
    docs/design/browser-stack-extraction/*|install-helpers/verify-browser-extraction.sh)
      return 1
      ;;
    Cargo.toml|Cargo.lock|crates/desktop/mde-shell-egui/src/*|\
    crates/mesh/mackesd/src/*|crates/mesh/mackes-mesh-types/src/*|\
    crates/mesh/mde-seal/*|crates/mesh/mde-worker-core/*|\
    crates/services/mde-adblock/*|crates/services/mde-bookmarks/*|\
    crates/desktop/mde-bookmarks-egui/*|packaging/*|install-helpers/*)
      return 0
      ;;
  esac
  return 1
}

is_content_candidate() {
  local path="$1"
  candidate_scope "$path" \
    && [ -f "$ROOT/$path" ] \
    && grep -Il -E "$SIGNAL_RE" -- "$ROOT/$path" >/dev/null 2>&1
}

is_browser_candidate_path() {
  path_is_named_candidate "$1" || is_content_candidate "$1"
}

filter_browser_candidates() {
  local paths="$1" discovered="$2" path
  : > "$discovered"
  while IFS= read -r path; do
    if is_browser_candidate_path "$path"; then
      printf '%s\n' "$path" >> "$discovered"
    fi
  done < "$paths"
  sort -u "$discovered" -o "$discovered"
}

refuse_ignored_browser_candidates() {
  local ignored_paths="$1" ignored_candidates="$2"
  git_cmd ls-files --others --ignored --exclude-standard > "$ignored_paths"
  filter_browser_candidates "$ignored_paths" "$ignored_candidates"
  if [ -s "$ignored_candidates" ]; then
    echo "ignored Browser candidates are not history-bearing:" >&2
    sed 's/^/  /' "$ignored_candidates" >&2
    exit 1
  fi
}

discover_candidates() {
  local all_paths="$1" discovered="$2" content_paths="$3" path
  : > "$discovered"

  while IFS= read -r path; do
    if path_is_named_candidate "$path"; then
      printf '%s\n' "$path" >> "$discovered"
    fi
  done < "$all_paths"

  : > "$content_paths"
  git_cmd grep -Il -E "$SIGNAL_RE" -- \
    Cargo.toml Cargo.lock \
    crates/desktop/mde-shell-egui/src \
    crates/mesh/mackesd/src \
    crates/mesh/mackes-mesh-types/src \
    crates/mesh/mde-seal crates/mesh/mde-worker-core \
    crates/services/mde-adblock crates/services/mde-bookmarks \
    crates/desktop/mde-bookmarks-egui packaging install-helpers \
    > "$content_paths" || :
  # This verifier necessarily contains the Browser vocabulary it scans for;
  # it is extraction governance, not a Browser-stack source candidate.
  grep -v -F -- 'install-helpers/verify-browser-extraction.sh' "$content_paths" \
    > "$content_paths.filtered" || :
  mv -- "$content_paths.filtered" "$content_paths"
  cat "$content_paths" >> "$discovered"

  while IFS= read -r path; do
    if is_content_candidate "$path"; then
      printf '%s\n' "$path" >> "$discovered"
    fi
  done < "$all_paths"

  sort -u "$discovered"
}

make_all_paths() {
  local out="$1"
  {
    git_cmd ls-files --cached
    git_cmd ls-files --others --exclude-standard
  } | sort -u > "$out"
}

worktree_evidence_is_consistent() {
  local recorded_blob="$1" worktree_blob="$2" worktree_state="$3" actual_blob="$4"
  [[ "$actual_blob" == "$worktree_blob" ]] || return 1
  case "$worktree_state" in
    clean) [[ "$actual_blob" == "$recorded_blob" ]] ;;
    dirty) [[ "$actual_blob" != "$recorded_blob" ]] ;;
    *) return 1 ;;
  esac
}

validate_manifest_rows() {
  local rows="$1" path class destination reason recorded_blob worktree_blob worktree_state actual_blob source_commit expected_destination expected_reason
  source_commit="$2"
  while IFS=$'\t' read -r class path destination reason recorded_blob worktree_blob worktree_state; do
    [[ -n "$path" ]] || die "manifest contains an empty source path"
    [[ "$path" != *$'\t'* && "$path" != *$'\n'* ]] || die "manifest source path contains a control character"
    case "$class" in browser-owned|mixed-purpose|shared) ;; *) die "unclassified manifest row: $path ($class)" ;; esac
    [[ -n "$destination" && -n "$reason" && -n "$recorded_blob" && -n "$worktree_blob" ]] || die "incomplete manifest row: $path"
    case "$worktree_state" in
      clean) ;;
      dirty)
        [[ "$class" != browser-owned ]] || die "dirty Browser-owned source must be committed before extraction: $path"
        ;;
      *) die "invalid worktree state for $path: $worktree_state" ;;
    esac
    class_for_path "$path" >/dev/null || die "manifest path is not classified by verifier: $path"
    [[ "$(class_for_path "$path")" == "$class" ]] || die "manifest class drift for $path"
    expected_destination="$(destination_for "$path")" || die "manifest path has no destination rule: $path"
    expected_reason="$(reason_for_path "$path")"
    [[ "$destination" == "$expected_destination" ]] || die "manifest destination drift for $path"
    [[ "$reason" == "$expected_reason" ]] || die "manifest reason drift for $path"
    git_cmd ls-files --error-unmatch -- "$path" >/dev/null 2>&1 || die "manifest source is not tracked: $path"
    git_cmd cat-file -e "${source_commit}:$path" 2>/dev/null || die "source commit is missing $path"
    actual_blob="$(git_cmd rev-parse "${source_commit}:$path")"
    [[ "$actual_blob" == "$recorded_blob" ]] || die "recorded source blob drift for $path"
    actual_blob="$(git_cmd hash-object --no-filters -- "$ROOT/$path")"
    if ! worktree_evidence_is_consistent "$recorded_blob" "$worktree_blob" "$worktree_state" "$actual_blob"; then
      [[ "$actual_blob" == "$worktree_blob" ]] || die "recorded current worktree blob drift for $path"
      if [[ "$worktree_state" == clean ]]; then
        die "current clean worktree blob drift for $path"
      else
        die "dirty mixed/shared path returned clean: $path"
      fi
    fi
  done < "$rows"
}

generate_manifest() {
  local all_paths="$1" candidates="$2" source_commit="$3" tmp="$4" path class reason destination blob worktree_blob worktree_state remote branch source_date
  while IFS= read -r path; do
    git_cmd ls-files --error-unmatch -- "$path" >/dev/null 2>&1 || die "Browser candidate is untracked; commit it before generating provenance: $path"
    class="$(class_for_path "$path")" || die "unclassified Browser candidate: $path"
    reason="$(reason_for_path "$path")"
    destination="$(destination_for "$path")"
    blob="$(git_cmd rev-parse "${source_commit}:$path")" || die "source commit does not contain $path"
    worktree_blob="$(git_cmd hash-object --no-filters -- "$ROOT/$path")"
    if [[ "$worktree_blob" == "$blob" ]]; then
      worktree_state=clean
    else
      worktree_state=dirty
      [[ "$class" != browser-owned ]] || die "dirty Browser-owned source must be committed before extraction: $path"
    fi
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$class" "$path" "$destination" "$reason" "$blob" "$worktree_blob" "$worktree_state" >> "$tmp.rows"
  done < "$candidates"

  remote="$(git_cmd remote get-url origin 2>/dev/null || printf '%s' unknown)"
  branch="$(git_cmd branch --show-current)"
  source_date="$(git_cmd show -s --format=%cI "$source_commit")"
  {
    printf '# browser-stack-extraction-manifest v1\n'
    printf '# source_commit=%s\n' "$source_commit"
    printf '# source_commit_date=%s\n' "$source_date"
    printf '# source_branch=%s\n' "$branch"
    printf '# source_remote=%s\n' "$remote"
    printf '# extraction_destination=matthewmackes/magic-mesh-browser-stack\n'
    printf '# generation=git-ls-files plus scoped host-Browser signal scan\n'
    printf '# columns=class\tsource_path\tdestination\treason\tsource_blob_sha\tworktree_blob_sha\tworktree_state\n'
    sort -t $'\t' -k2,2 "$tmp.rows"
  } > "$MANIFEST"
}

check_manifest() {
  local tmp="$1" source_commit rows manifest_paths candidates missing extra untracked candidate_count row_count head header
  [[ -f "$MANIFEST" ]] || die "manifest is missing: $MANIFEST"
  header="$(sed -n '1p' "$MANIFEST")"
  [[ "$header" == '# browser-stack-extraction-manifest v1' ]] || die "manifest schema header is missing or unsupported"
  header="$(sed -n '7p' "$MANIFEST")"
  [[ "$header" == '# generation=git-ls-files plus scoped host-Browser signal scan' ]] || die "manifest generation evidence header is missing or unsupported"
  header="$(sed -n '8p' "$MANIFEST")"
  [[ "$header" == $'# columns=class\tsource_path\tdestination\treason\tsource_blob_sha\tworktree_blob_sha\tworktree_state' ]] || die "manifest column header is missing or unsupported"
  source_commit="$(awk -F= '/^# source_commit=/{print $2; exit}' "$MANIFEST")"
  [[ "$source_commit" =~ ^[0-9a-f]{40}$ ]] || die "manifest has no valid immutable source_commit"
  git_cmd cat-file -e "${source_commit}^{commit}" 2>/dev/null || die "source commit is unavailable: $source_commit"
  head="$(git_cmd rev-parse HEAD)"
  git_cmd merge-base --is-ancestor "$source_commit" "$head" || die "current HEAD is not descended from manifest source commit"

  rows="$tmp/rows"
  sed -n '/^[^#]/p' "$MANIFEST" > "$rows"
  row_count="$(wc -l < "$rows")"
  [[ "$row_count" -gt 0 ]] || die "manifest has no source rows"
  awk -F $'\t' 'NF != 7 { print "bad manifest column count on line " NR > "/dev/stderr"; bad=1 } END { exit bad }' "$rows" || exit 1
  validate_manifest_rows "$rows" "$source_commit"

  manifest_paths="$tmp/manifest-paths"
  candidates="$tmp/candidates"
  make_all_paths "$tmp/all-paths"
  discover_candidates "$tmp/all-paths" "$tmp/discovered" "$tmp/content-paths" > "$candidates"
  cut -f2 "$rows" | sort -u > "$manifest_paths"
  missing="$tmp/missing"
  extra="$tmp/extra"
  comm -23 "$candidates" "$manifest_paths" > "$missing"
  comm -13 "$candidates" "$manifest_paths" > "$extra"
  [[ ! -s "$missing" ]] || { echo "missing Browser manifest rows:" >&2; sed 's/^/  /' "$missing" >&2; exit 1; }
  [[ ! -s "$extra" ]] || { echo "manifest rows are not discovered from the current source tree:" >&2; sed 's/^/  /' "$extra" >&2; exit 1; }

  untracked="$tmp/untracked"
  git_cmd ls-files --others --exclude-standard > "$untracked"
  if comm -12 "$candidates" "$untracked" | grep -q .; then
    echo "untracked Browser candidates are not history-bearing:" >&2
    comm -12 "$candidates" "$untracked" | sed 's/^/  /' >&2
    exit 1
  fi
  candidate_count="$(wc -l < "$candidates")"
  echo "Browser extraction manifest OK: $candidate_count source paths; source commit $source_commit"
  echo "  classes: $(awk -F $'\t' '{count[$1]++} END {printf "browser-owned=%d mixed-purpose=%d shared=%d", count["browser-owned"], count["mixed-purpose"], count["shared"]}' "$rows")"
}

self_test() {
  local failures=0 actual expected path test_paths test_candidates
  check() {
    actual="$1"
    expected="$2"
    if [[ "$actual" != "$expected" ]]; then
      echo "FAIL: got [$actual], want [$expected]" >&2
      failures=$((failures + 1))
    fi
  }

  path='crates/desktop/mde-web-preview/src/lib.rs'
  check "$(class_for_path "$path")" browser-owned
  check "$(destination_for "$path")" "magic-mesh-browser-stack:$path"
  check "$(reason_for_path "$path")" browser-helper-crate

  path='crates/desktop/mde-shell-egui/src/web/mod.rs'
  check "$(class_for_path "$path")" browser-owned
  check "$(destination_for "$path")" "magic-mesh-browser-stack:$path"
  check "$(reason_for_path "$path")" host-browser-shell

  path='crates/desktop/mde-shell-egui/src/main.rs'
  check "$(class_for_path "$path")" mixed-purpose
  check "$(destination_for "$path")" "split-in-magic-mesh-browser-stack:$path#browser-sections"
  check "$(reason_for_path "$path")" shell-route-or-shared-state

  path='crates/services/mde-bookmarks/src/lib.rs'
  check "$(class_for_path "$path")" shared
  check "$(destination_for "$path")" "retain-in-magic-mesh:$path#shared-contract-or-reference"
  check "$(reason_for_path "$path")" shared-bookmark-contract

  if path_is_named_candidate install-helpers/verify-browser-extraction.sh; then
    echo 'FAIL: verifier classified itself as a source candidate' >&2
    failures=$((failures + 1))
  fi

  if ! worktree_evidence_is_consistent source source clean source; then
    echo 'FAIL: clean worktree evidence was rejected' >&2
    failures=$((failures + 1))
  fi
  if ! worktree_evidence_is_consistent source current dirty current; then
    echo 'FAIL: current dirty worktree evidence was rejected' >&2
    failures=$((failures + 1))
  fi
  if worktree_evidence_is_consistent source recorded dirty current; then
    echo 'FAIL: stale dirty worktree evidence was accepted' >&2
    failures=$((failures + 1))
  fi

  test_paths="$(mktemp)"
  test_candidates="$(mktemp)"
  printf '%s\n' \
    install-helpers/install-browser-stt-model.sh \
    target/browser-cache-note.txt \
    > "$test_paths"
  filter_browser_candidates "$test_paths" "$test_candidates"
  if ! grep -Fxq install-helpers/install-browser-stt-model.sh "$test_candidates"; then
    echo 'FAIL: named Browser candidate was not detected' >&2
    failures=$((failures + 1))
  fi
  if grep -Fxq target/browser-cache-note.txt "$test_candidates"; then
    echo 'FAIL: non-candidate path was detected as Browser source' >&2
    failures=$((failures + 1))
  fi
  rm -f "$test_paths" "$test_candidates"

  if [[ "$failures" -ne 0 ]]; then
    return 1
  fi
  echo 'Browser extraction verifier self-test: PASS'
}

main() {
  local mode="--check" tmp source_commit
  case "${1:-}" in
    "") ;;
    --check|--write) mode="$1" ;;
    --self-test) self_test; return $? ;;
    -h|--help) usage; return 0 ;;
    *) usage >&2; return 2 ;;
  esac
  [[ -d "$ROOT/.git" || -f "$ROOT/.git" ]] || die "not a Git worktree: $ROOT"
  tmp="$(mktemp -d)"
  trap 'rm -rf "${tmp:-}"' EXIT
  make_all_paths "$tmp/all-paths"
  discover_candidates "$tmp/all-paths" "$tmp/discovered" "$tmp/content-paths" > "$tmp/candidates"
  refuse_ignored_browser_candidates "$tmp/ignored-paths" "$tmp/ignored-candidates"

  if [[ "$mode" == --write ]]; then
    source_commit="$(git_cmd rev-parse HEAD)"
    generate_manifest "$tmp/all-paths" "$tmp/candidates" "$source_commit" "$tmp"
    echo "Generated $MANIFEST from source commit $source_commit ($(wc -l < "$tmp/candidates") source paths)"
  fi
  check_manifest "$tmp"
}

main "$@"
