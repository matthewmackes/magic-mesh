#!/usr/bin/env bash
# EFF-30 — operator-gated release signing (run by /release, never CI).
#
# Signs the built RPM with the project GPG key (EFF-17 — public half
# committed at packaging/repo/RPM-GPG-KEY-magic-mesh; private half in
# the release operator's keyring) and emits SHA256SUMS + a detached
# armored signature covering every artifact handed to it — the ISO
# checksum/signature half of EFF-30.
#
# Usage:
#   ./install-helpers/sign-release.sh <artifact>...
#     e.g.  ./install-helpers/sign-release.sh \
#             target/generate-rpm/magic-mesh-*.rpm magic-mesh-cosmic.iso
#
# Requires: gpg with the "Magic Mesh Release Signing" secret key (the key uid is
# an infra id — UNCHANGED by the MCNF rebrand, like the magic-mesh package id);
# rpm-sign (for rpmsign) when an .rpm is among the artifacts.
set -euo pipefail

KEY_ID="${MAGIC_MESH_SIGN_KEY:-Magic Mesh Release Signing}"

if [ "$#" -lt 1 ]; then
  echo "usage: $0 <artifact>...  (RPMs are rpmsign'd; everything lands in SHA256SUMS + .asc)" >&2
  exit 2
fi

# Refuse to run without the secret key — better a clear early error
# than a half-signed release dir.
if ! gpg --list-secret-keys "$KEY_ID" >/dev/null 2>&1; then
  echo "sign-release: secret key '$KEY_ID' not in this keyring — run on the release operator's machine" >&2
  exit 1
fi

for artifact in "$@"; do
  [ -f "$artifact" ] || { echo "sign-release: missing artifact: $artifact" >&2; exit 1; }
done

# 1. RPM signature(s) — embedded, what `dnf` verifies against the
#    installed RPM-GPG-KEY-magic-mesh (gpgcheck=1 in the shipped repo).
for artifact in "$@"; do
  case "$artifact" in
    *.rpm)
      command -v rpmsign >/dev/null || { echo "sign-release: rpmsign missing (dnf install rpm-sign)" >&2; exit 1; }
      rpmsign --define "_gpg_name $KEY_ID" --addsign "$artifact"
      # Informational only, and NON-FATAL: an EL9 dev host's older rpm cannot
      # read the RSA-4096 signing subkey (key d0921c73, added rpm#2351 for F43
      # sequoia conformance) and prints "SIGNATURES NOT OK / NOKEY" even though
      # the signature is embedded correctly — the canonical verification is on
      # the F43+ target via the imported RPM-GPG-KEY-magic-mesh. Under `set -e`
      # this used to abort the script before it wrote SHA256SUMS, leaving a
      # stale sums file from the previous cut. `|| true` keeps it advisory.
      rpm --checksig "$artifact" || true
      ;;
  esac
done

# 2. SHA256SUMS + one detached armored signature over the sums file —
#    covers the ISO (and every other artifact) for download verification:
#      sha256sum -c SHA256SUMS && gpg --verify SHA256SUMS.asc SHA256SUMS
outdir="$(dirname "$1")"
( cd "$outdir" || exit 1
  : > SHA256SUMS )
for artifact in "$@"; do
  ( cd "$(dirname "$artifact")" && sha256sum "$(basename "$artifact")" ) >> "$outdir/SHA256SUMS"
done
gpg --armor --detach-sign --local-user "$KEY_ID" --yes \
  --output "$outdir/SHA256SUMS.asc" "$outdir/SHA256SUMS"

echo "sign-release: signed $# artifact(s); wrote $outdir/SHA256SUMS{,.asc}"
echo "verify with:  sha256sum -c SHA256SUMS && gpg --verify SHA256SUMS.asc SHA256SUMS"
