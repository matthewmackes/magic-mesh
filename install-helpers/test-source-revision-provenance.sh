#!/usr/bin/env bash
# Focused contract test for the governed source receipt and mde-theme build stamp.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
FIXTURE="$WORK/repo"
ARTIFACTS="$WORK/artifacts"
trap 'rm -rf -- "$WORK"' EXIT
mkdir "$FIXTURE" "$ARTIFACTS"

"$REPO/install-helpers/source-revision-receipt.sh" --self-test
rustc --edition=2021 "$REPO/crates/shared/mde-theme/build.rs" -o "$ARTIFACTS/theme-build-script"

git -C "$FIXTURE" init -q
git -C "$FIXTURE" config user.name provenance-test
git -C "$FIXTURE" config user.email provenance-test.invalid
printf 'immutable source\n' >"$FIXTURE/source"
git -C "$FIXTURE" add source
GIT_AUTHOR_DATE=1700000000 GIT_COMMITTER_DATE=1700000000 \
  git -C "$FIXTURE" commit -q -m fixture
IFS=$'\t' read -r REVISION EPOCH \
  < <("$REPO/install-helpers/source-revision-receipt.sh" --repo "$FIXTURE")

(
  cd "$FIXTURE"
  CARGO_PKG_VERSION=13.0.0 \
  MCNF_BUILD_SOURCE_REVISION="$REVISION" \
  MCNF_BUILD_PROMOTABLE=1 \
  SOURCE_DATE_EPOCH="$EPOCH" \
    "$ARTIFACTS/theme-build-script" >"$ARTIFACTS/promotable.out"
)
grep -Fqx "cargo:rustc-env=MDE_BUILD_GIT_HASH=$REVISION" "$ARTIFACTS/promotable.out"

if (
  cd "$FIXTURE"
  CARGO_PKG_VERSION=13.0.0 MCNF_BUILD_PROMOTABLE=1 \
    "$ARTIFACTS/theme-build-script" >"$ARTIFACTS/missing.out" 2>"$ARTIFACTS/missing.err"
); then
  printf 'test-source-revision-provenance: missing promotable receipt was accepted\n' >&2
  exit 1
fi
grep -Fq 'promotable build requires MCNF_BUILD_SOURCE_REVISION' "$ARTIFACTS/missing.err"

if (
  cd "$FIXTURE"
  CARGO_PKG_VERSION=13.0.0 \
  MCNF_BUILD_SOURCE_REVISION=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  MCNF_BUILD_PROMOTABLE=1 \
    "$ARTIFACTS/theme-build-script" >"$ARTIFACTS/mismatch.out" 2>"$ARTIFACTS/mismatch.err"
); then
  printf 'test-source-revision-provenance: mismatched receipt was accepted\n' >&2
  exit 1
fi
grep -Fq 'source receipt does not match checkout HEAD' "$ARTIFACTS/mismatch.err"

printf 'dirty\n' >>"$FIXTURE/source"
if (
  cd "$FIXTURE"
  CARGO_PKG_VERSION=13.0.0 \
  MCNF_BUILD_SOURCE_REVISION="$REVISION" \
  MCNF_BUILD_PROMOTABLE=1 \
    "$ARTIFACTS/theme-build-script" >"$ARTIFACTS/dirty.out" 2>"$ARTIFACTS/dirty.err"
); then
  printf 'test-source-revision-provenance: dirty promotable source was accepted\n' >&2
  exit 1
fi
grep -Fq 'promotable build checkout is dirty' "$ARTIFACTS/dirty.err"

mkdir "$WORK/gitless"
(
  cd "$WORK/gitless"
  CARGO_PKG_VERSION=13.0.0 "$ARTIFACTS/theme-build-script" >"$ARTIFACTS/gitless.out"
)
grep -Fqx 'cargo:rustc-env=MDE_BUILD_GIT_HASH=non-promotable-unresolved' "$ARTIFACTS/gitless.out"
! grep -Fq 'MDE_BUILD_GIT_HASH=nogit' "$ARTIFACTS/gitless.out"

printf 'test-source-revision-provenance: passed (exact receipt stamped; mismatch, dirty, and unreceipted states fail or identify as non-promotable)\n'
