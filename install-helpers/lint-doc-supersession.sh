#!/usr/bin/env bash
# lint-doc-supersession.sh — keep historical design notes honestly labelled.
#
# Many docs under docs/ still describe retired architecture (the iced/libcosmic
# `mde-workbench` desktop era, the LizardFS substrate, the cloud-hypervisor/
# `mde-kvm` VM path). A reader who lands on one of those docs with no banner
# cannot tell live design from a historical record. This gate greps docs/ for a
# curated list of retired terms and FAILS if a hit lacks a supersession/historical
# banner in its head — unless the file is on the allowlist of docs that reference
# the retired term as living context (current rescope/replacement docs, ledgers,
# self-caveated notes, pattern citations).
#
# A "banner" = one of SUPERSEDED / HISTORICAL / RETIRED / DEPRECATED (any case) in
# the first BANNER_LINES lines. Add one at the very top, e.g.:
#   > **HISTORICAL / SUPERSEDED (YYYY-MM-DD):** describes retired <X>; see <doc>.
#
# Run with `--self-test` to exercise planted cases. Exit 0 = clean, 1 = violation.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DOCS="${MCNF_DOCS:-$ROOT/docs}"
BANNER_LINES="${MCNF_DOC_BANNER_LINES:-40}"

# Curated retired-architecture terms. Extended-regex, matched case-insensitively.
TERMS=(
  'libcosmic'
  '\bmde-cosmic-applet\b'
  '\bmde-workbench\b'
  'lizardfs'
  '\bcloud-hypervisor\b'
  '\bmde-kvm\b'
  '\biced\b'
  '\bKolla\b'
  'Keystone absorbs'
  'Nova \+ Placement'
)

# A banner is any of these words in the head of the file. The SUPERSE stem
# covers supersede/superseded/supersedes/supersession/superseding.
BANNER_RE='SUPERSE|HISTORICAL|RETIRED|DEPRECAT'

usage() { sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; }

# Files that legitimately reference a retired term without a top banner:
# current replacement/rescope designs, ledgers and meta trackers, self-caveated
# notes, and docs citing a retired crate only as a code-pattern example.
allowlisted() {
  case "$1" in
    */docs/architecture.md) return 0 ;;                       # current; retired tech named in the ban-list
    */docs/DECISIONS.md) return 0 ;;                          # append-only ADR ledger
    */docs/COMPLIANCE.md) return 0 ;;                         # compliance ledger of resolved items
    */docs/BUILD-ENVIRONMENT.md) return 0 ;;                  # current; notes a legacy leftover as removed
    */docs/RECONCILE-PLAN.md) return 0 ;;                     # worklist reconcile meta
    */docs/NEEDS-OPERATOR.md) return 0 ;;                     # archived queue pointer (see WORKLIST.md)
    */docs/platform/WORKLIST.md) return 0 ;;                  # active worklist
    */docs/platform/DRAIN-RECONCILIATION-*.md) return 0 ;;    # drain meta
    */docs/platform/WORKLIST-RECONCILIATION-*.md) return 0 ;; # reconcile meta
    */docs/platform/WL-ARCH-001-openstack-deletion-blueprint.md) return 0 ;; # current delete blueprint; retired crates named only in the audit ban-list
    */docs/design/e12-9-10-libvirt-rescope.md) return 0 ;;    # current rescope off cloud-hypervisor
    */docs/design/qc23-virtio-gpu-zerocopy-rescope.md) return 0 ;; # current rescope
    */docs/design/onboarding-wizard.md) return 0 ;;           # self-caveated: cloud-hypervisor/mde-kvm path deleted
    */docs/design/workbench-storage-plane.md) return 0 ;;     # self-caveated Correction 2026-07-10
    */docs/design/mesh-chat-icq.md) return 0 ;;               # mde-kvm named only as a code-pattern citation
    */docs/design/quasar-host-controls.md) return 0 ;;        # mde-kvm named only as a code-pattern citation
    */docs/design/voice-vitelity-per-node-sip.md) return 0 ;; # mde-kvm named only as a code-pattern citation
    */docs/design/mesh-shell.md) return 0 ;;                  # self-caveated pre-SUBSTRATE-6 inline
    */docs/design/xcp-ng-integration.md) return 0 ;;          # self-caveated post-SUBSTRATE-6 inline
    */docs/design/whitepaper-brief.md) return 0 ;;            # current; LizardFS-to-etcd is a noted internal detail
    */docs/design/build-platform.md) return 0 ;;              # current build doc; mde-workbench is an incidental example
    */docs/ops/promotion-pipeline.md) return 0 ;;             # current; asserts LizardFS is NOT enabled
    *) return 1 ;;
  esac
}

# Emit files under DOCS (excluding archives/review) that contain a retired term.
hit_files() {
  local -a args=()
  local t raw
  for t in "${TERMS[@]}"; do args+=(-e "$t"); done
  if command -v rg >/dev/null 2>&1; then
    raw="$(rg -l -i "${args[@]}" "$DOCS" 2>/dev/null || true)"
  else
    raw="$(grep -RIl -i -E --binary-files=without-match \
      "$(IFS='|'; echo "${TERMS[*]}")" "$DOCS" 2>/dev/null || true)"
  fi
  # Deterministically drop archived/review copies regardless of glob semantics.
  printf '%s\n' "$raw" | grep -Ev '/(worklist-archive|design-archive|review)/' || true
}

has_banner() {
  head -n "$BANNER_LINES" "$1" 2>/dev/null | grep -q -i -E "$BANNER_RE"
}

scan() {
  local f rc=0 printed=0
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    allowlisted "$f" && continue
    if ! has_banner "$f"; then
      if [ "$printed" -eq 0 ]; then
        echo "lint-doc-supersession.sh: docs reference retired architecture but carry no banner:" >&2
        printed=1
      fi
      printf '  %s\n' "${f#"$ROOT"/}" >&2
      rc=1
    fi
  done < <(hit_files)
  if [ "$rc" -eq 0 ]; then
    echo "lint-doc-supersession.sh: clean — every retired-term doc is bannered or allowlisted"
  else
    echo "lint-doc-supersession.sh: add a top banner (SUPERSEDED/HISTORICAL) or allowlist the file above" >&2
  fi
  return "$rc"
}

self_test() {
  local td fails=0
  td="$(mktemp -d "${TMPDIR:-/tmp}/lint-doc-supersession.XXXXXX")" || return 1
  trap "rm -rf '$td'" EXIT
  mkdir -p "$td/docs/design" "$td/docs/worklist-archive"

  # Clean doc: no retired term.
  printf '%s\n' '# Current design' 'The live shell is mde-shell-egui.' \
    >"$td/docs/design/clean.md"
  # Bannered historical doc: retired term + top banner.
  printf '%s\n' '# Old surface' \
    '> **HISTORICAL / SUPERSEDED (2026-07-19):** retired mde-workbench panel.' \
    'It used iced.' >"$td/docs/design/bannered.md"
  # Naked historical doc: retired term, no banner.
  printf '%s\n' '# Naked surface' 'Built on libcosmic and mde-workbench.' \
    >"$td/docs/design/naked.md"
  # Archived docs with a retired term must be ignored (excluded dirs).
  printf '%s\n' '# archived' 'lizardfs everywhere' \
    >"$td/docs/worklist-archive/old.md"
  mkdir -p "$td/docs/design-archive"
  printf '%s\n' '# archived design' 'the mde-workbench era' \
    >"$td/docs/design-archive/old-design.md"

  run() {
    local save="$DOCS" rc
    DOCS="$td/docs"
    scan >/dev/null 2>/dev/null
    rc=$?
    DOCS="$save"
    return "$rc"
  }

  # With only clean + bannered + archived, the scan is green.
  rm -f "$td/docs/design/naked.md"
  if run; then echo "  ok: bannered + clean + archived passes"; else
    echo "  FAIL: bannered/clean/archived should pass" >&2; fails=$((fails + 1)); fi

  # Re-add the naked doc: scan must fail.
  printf '%s\n' '# Naked surface' 'Built on libcosmic and mde-workbench.' \
    >"$td/docs/design/naked.md"
  if run; then echo "  FAIL: naked retired-term doc should fail" >&2; fails=$((fails + 1)); else
    echo "  ok: naked retired-term doc fails"; fi

  # Allowlist covers a naked current-context doc by exact path.
  rm -f "$td/docs/design/naked.md"
  printf '%s\n' '# architecture' 'no Gluster/LizardFS/Ceph — etcd + Syncthing' \
    >"$td/docs/architecture.md"
  if run; then echo "  ok: allowlisted architecture.md passes without a banner"; else
    echo "  FAIL: allowlisted architecture.md should pass" >&2; fails=$((fails + 1)); fi

  if [ "$fails" -eq 0 ]; then
    echo "lint-doc-supersession.sh: self-test passed"
    return 0
  fi
  echo "lint-doc-supersession.sh: SELF-TEST FAILED ($fails)" >&2
  return 1
}

case "${1:-}" in
  --self-test) self_test ;;
  -h|--help) usage ;;
  "") scan ;;
  *) DOCS="$1"; scan ;;
esac
