#!/usr/bin/env bash
# Resolve the immutable source receipt consumed by promotable RPM builds.
set -euo pipefail

die() {
  printf 'source-revision-receipt: %s\n' "$*" >&2
  exit 1
}

valid_revision() {
  [[ "${1:-}" =~ ^[0-9a-f]{40}$|^[0-9a-f]{64}$ ]]
}

resolve_receipt() {
  local repo="$1" revision epoch status
  revision="$(git -C "$repo" rev-parse --verify 'HEAD^{commit}' 2>/dev/null)" \
    || die "cannot resolve HEAD to an immutable commit"
  valid_revision "$revision" || die "HEAD is not an exact lowercase Git object ID"

  status="$(git -C "$repo" status --porcelain=v1 --untracked-files=normal 2>/dev/null)" \
    || die "cannot determine checkout cleanliness"
  [ -z "$status" ] || die "checkout is dirty; refusing a promotable source receipt"

  epoch="$(git -C "$repo" show -s --format=%ct "$revision" 2>/dev/null)" \
    || die "cannot resolve the commit timestamp"
  [[ "$epoch" =~ ^[0-9]+$ ]] || die "commit timestamp is not a non-negative Unix epoch"
  printf '%s\t%s\n' "$revision" "$epoch"
}

verify_receipt() {
  valid_revision "${1:-}" || die "revision must be an exact lowercase 40- or 64-hex Git object ID"
  [[ "${2:-}" =~ ^[0-9]+$ ]] || die "epoch must be a non-negative integer"
  printf '%s\t%s\n' "$1" "$2"
}

self_test() {
  local root clean dirty revision epoch output
  root="$(mktemp -d)"
  trap 'rm -rf -- "$root"' EXIT
  git -C "$root" init -q
  git -C "$root" config user.name receipt-test
  git -C "$root" config user.email receipt-test.invalid
  printf 'receipt fixture\n' >"$root/source"
  git -C "$root" add source
  GIT_AUTHOR_DATE=1700000000 GIT_COMMITTER_DATE=1700000000 \
    git -C "$root" commit -q -m fixture

  IFS=$'\t' read -r revision epoch < <(resolve_receipt "$root")
  valid_revision "$revision" || die "self-test did not emit an exact revision"
  [ "$epoch" = 1700000000 ] || die "self-test did not emit the commit epoch"
  output="$(verify_receipt "$revision" "$epoch")"
  [ "$output" = "$revision"$'\t'"$epoch" ] || die "self-test receipt validation drifted"

  printf 'dirty\n' >>"$root/source"
  dirty="$("$0" --repo "$root" 2>&1 || true)"
  [[ "$dirty" == *"checkout is dirty"* ]] || die "self-test accepted a dirty checkout"
  clean="$("$0" --verify bad 1700000000 2>&1 || true)"
  [[ "$clean" == *"revision must be an exact"* ]] || die "self-test accepted a malformed revision"
  rm -rf -- "$root"
  trap - EXIT
  printf 'source-revision-receipt: self-test passed (clean exact receipt; dirty and malformed fail closed)\n'
}

case "${1:-}" in
  --verify)
    [ "$#" -eq 3 ] || die "usage: $0 --verify REVISION EPOCH"
    verify_receipt "$2" "$3"
    ;;
  --self-test)
    [ "$#" -eq 1 ] || die "usage: $0 --self-test"
    self_test
    ;;
  --repo)
    [ "$#" -eq 2 ] || die "usage: $0 --repo PATH"
    resolve_receipt "$2"
    ;;
  *)
    die "usage: $0 --repo PATH | --verify REVISION EPOCH | --self-test"
    ;;
esac
