#!/usr/bin/env bash
# CUT-3 — no-crates.io-iced guard.
#
# The whole GUI rides libcosmic's vendored iced fork (the git source). A
# crates.io-registry `iced` / `iced_core` / `iced_layershell` re-entering the
# tree means some crate regressed onto upstream iced — which won't unify with
# the fork's types (crates.io iced_core::Color != fork Color) and reintroduces
# the duplicate-build CUT-1/CUT-2 eliminated. cargo-deny's multiple-versions
# can't target this precisely (a big tree has dozens of benign dup versions),
# so this lint pins the exact regression: any iced* package sourced from the
# crates.io registry in Cargo.lock.
set -euo pipefail
cd "$(dirname "$0")/.."

# Find lock stanzas: name = "iced*" followed (within the stanza) by a
# crates.io registry source line.
hits=$(awk '
  /^name = "iced/ { name=$0; want=1; next }
  want && /^source = "registry\+https:\/\/github\.com\/rust-lang\/crates\.io-index"/ {
    print name; want=0; next
  }
  /^$/ { want=0 }
' Cargo.lock || true)

if [[ -n "$hits" ]]; then
  echo "lint-no-cratesio-iced.sh: FAIL — crates.io-sourced iced package(s) back in Cargo.lock:" >&2
  echo "$hits" >&2
  echo "The GUI must ride libcosmic's vendored iced fork (CUT-1/2/3). Find the" >&2
  echo "crate that pulled crates.io iced (cargo tree -i <pkg>) and move it onto" >&2
  echo "the fork, or drop the dep." >&2
  exit 1
fi

echo "lint-no-cratesio-iced.sh: clean — no crates.io-sourced iced* in the tree (CUT-3)"
