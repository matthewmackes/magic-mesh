#!/usr/bin/env bash
# dnf-channel-up.sh — DAR-23: a SOVEREIGN dnf channel served by the control VM,
# laid out like the gh-pages channel (fedora-N-x86_64/repodata/repomd.xml + the
# RPM-GPG-KEY), so a freshly-provisioned peer can dnf-install from the mesh when
# GitHub is unreachable (air-gapped provisioning).
#
# Layout (matches the gh-pages baseurl shape so do-lighthouse-cloudinit.sh works
# UNCHANGED with REPO_BASEURL pointed here):
#   <root>/fedora-<N>-x86_64/repodata/repomd.xml   ← createrepo_c metadata (SIGNED only)
#   <root>/fedora-<N>-x86_64/HOLD/                  ← DAR-24: CI stages UNSIGNED here
#   <root>/fedora-<N>-x86_64/ROLLED-BACK/           ← WL-BUILD-003: rollback quarantine
#   <root>/RPM-GPG-KEY-magic-mesh                   ← the published public key
#
# ROLLED-BACK/ (WL-BUILD-003): a rollback (mcnf-channel-rollback.sh) demotes a
# too-new NEVRA out of the client-facing set by moving it here. Like HOLD/, this
# subtree is EXCLUDED from the index, so a rolled-back RPM is never re-advertised
# on a later `dnf-channel-up.sh` refresh; a re-promote moves it back.
#
# Signing stays OPERATOR-GATED (sign-release.sh / the /release step). CI stages
# UNSIGNED RPMs into HOLD/; an operator signs (rpmsign --addsign, EFF-30) and
# promotes them OUT of HOLD/ into the arch dir. The channel is served over the
# control VM OVERLAY IP (podman + a static httpd), never 0.0.0.0 / a hardcoded LAN IP.
#
# SUPPLY-CHAIN (build-deploy-6): the client-facing repodata/ indexes ONLY promoted,
# signed RPMs. Two guards enforce this: (1) createrepo_c EXCLUDES the HOLD/ staging
# subtree from the index, and (2) a fail-closed signature gate refuses to index any
# RPM that does not carry an embedded PGP signature. So an un-promoted / unsigned CI
# RPM is NEVER advertised in the metadata a client installs from. Set
# MCNF_DNF_ALLOW_UNSIGNED=1 to bypass the gate ONLY for non-production bring-up.
#
# Usage: dnf-channel-up.sh [--host <overlay-ip>] [--fedora <N,...>] [--self-test]
# Env: MCNF_HOST_IP, MCNF_DNF_ROOT (/var/lib/mcnf-dnf-channel),
#      MCNF_DNF_PORT (8480), MCNF_FEDORA_VERSIONS (43 44),
#      MCNF_GPG_KEY (public key for the advisory trust check),
#      MCNF_DNF_ALLOW_UNSIGNED (=1 to bypass the signature gate — NOT for prod).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_REPO="$(cd "$HERE/../.." && pwd)"
GPG_KEY="${MCNF_GPG_KEY:-$SRC_REPO/packaging/repo/RPM-GPG-KEY-magic-mesh}"

HOST_IP="${MCNF_HOST_IP:-}"
ROOT="${MCNF_DNF_ROOT:-/var/lib/mcnf-dnf-channel}"
PORT="${MCNF_DNF_PORT:-8480}"
FEDORAS="${MCNF_FEDORA_VERSIONS:-43 44}"
ALLOW_UNSIGNED="${MCNF_DNF_ALLOW_UNSIGNED:-}"

# ── build-deploy-6 supply-chain guards ──────────────────────────────────────
# The RPMs the client-facing index will cover = everything under the arch dir
# EXCEPT the excluded staging subtrees HOLD/ (unsigned CI) and ROLLED-BACK/
# (WL-BUILD-003 rollback quarantine) — mirrors createrepo_c
# --excludes 'HOLD/*' --excludes 'ROLLED-BACK/*'.
indexable_rpms() {
  local arch_dir="$1"
  [ -d "$arch_dir" ] || return 0
  find "$arch_dir" \( -path "$arch_dir/HOLD" -o -path "$arch_dir/ROLLED-BACK" \) -prune \
       -o -type f -name '*.rpm' -print 2>/dev/null | sort
}

# Key-independent: does the RPM carry an embedded PGP/RSA/DSA signature?
# NOTE: `rpm --checksig`/`rpm -K` EXIT 0 even on an UNSIGNED rpm (it only fails on
# a BAD digest/signature), so the exit code cannot detect "unsigned" — we inspect
# the signature-header tags instead (empty / "(none)" ⇒ unsigned).
rpm_carries_signature() {
  local sig
  sig="$(rpm -qp --qf '%{RSAHEADER:pgpsig}|%{DSAHEADER:pgpsig}|%{SIGPGP:pgpsig}|%{SIGGPG:pgpsig}' "$1" 2>/dev/null \
        | sed 's/(none)//g; s/|//g; s/[[:space:]]//g')"
  [ -n "$sig" ]
}

# Advisory trust check: import the published public key into a SCRATCH rpm keyring
# and checksig. Fails ONLY on an explicit BAD / NOT OK signature (tampering / wrong
# key). A NOKEY / unreadable-key result is tolerated — signed-ness is already
# enforced by rpm_carries_signature, and an EL9 host cannot read the RSA-4096
# signing subkey (see sign-release.sh) so it reports NOKEY on a correctly-signed
# RPM. The authoritative trust check is gpgcheck=1 on the F43 client at install.
rpm_signature_not_bad() {
  local rpm_file="$1" key="$2" db out rc=0
  [ -f "$key" ] || return 0                 # no key to check against → advisory pass
  command -v rpm >/dev/null 2>&1 || return 0
  db="$(mktemp -d)" || return 0
  if rpm --dbpath "$db" --initdb >/dev/null 2>&1 && rpm --dbpath "$db" --import "$key" >/dev/null 2>&1; then
    out="$(rpm --dbpath "$db" --checksig "$rpm_file" 2>&1 || true)"
    case "$out" in
      *"NOT OK"*|*BAD*) rc=1 ;;
    esac
  fi
  rm -rf "$db"
  return "$rc"
}

# Fail-closed gate: abort if any RPM about to be indexed (the non-HOLD set) lacks a
# signature or carries a bad one. HOLD/ content is never inspected here — it is
# excluded from the index, so staging unsigned CI RPMs there is fine.
verify_indexable_signatures() {
  local arch_dir="$1" rpm_file bad=0
  while IFS= read -r rpm_file; do
    [ -n "$rpm_file" ] || continue
    if ! rpm_carries_signature "$rpm_file"; then
      echo "   UNSIGNED — refusing to index: ${rpm_file#"$arch_dir"/}" >&2
      bad=1
    elif ! rpm_signature_not_bad "$rpm_file" "$GPG_KEY"; then
      echo "   BAD SIGNATURE — refusing to index: ${rpm_file#"$arch_dir"/}" >&2
      bad=1
    fi
  done < <(indexable_rpms "$arch_dir")
  if [ "$bad" -ne 0 ]; then
    if [ -n "$ALLOW_UNSIGNED" ]; then
      echo "   !! MCNF_DNF_ALLOW_UNSIGNED set — indexing unsigned/invalid content ANYWAY (NOT for production)" >&2
      return 0
    fi
    echo "dnf-channel-up: unsigned/invalid RPM(s) in the client-facing set — aborting." >&2
    echo "  sign (sign-release.sh) + promote out of HOLD/ first, or set MCNF_DNF_ALLOW_UNSIGNED=1 to override." >&2
    return 1
  fi
  return 0
}

# Re-index an arch dir the SAFE way: run the signature gate over the client-facing
# set, then createrepo_c with the HOLD/ staging subtree EXCLUDED.
reindex_arch() {
  local arch_dir="$1"
  verify_indexable_signatures "$arch_dir" || exit 1
  createrepo_c --update --excludes 'HOLD/*' --excludes 'ROLLED-BACK/*' "$arch_dir" >/dev/null
}

# --self-test: prove (no podman/overlay/network) that HOLD/ is excluded from the
# indexed set and that unsigned client-facing content is refused (fail-closed).
run_self_test() {
  local tmp arch arch2 idx fails=0
  tmp="$(mktemp -d)"; arch="$tmp/fedora-99-x86_64"; mkdir -p "$arch/HOLD/nested"
  : > "$arch/promoted.rpm"            # top-level (unsigned dummy) = client-facing
  : > "$arch/HOLD/held.rpm"           # staged in HOLD/
  : > "$arch/HOLD/nested/deep.rpm"    # nested in HOLD/

  # T1 — the HOLD/ subtree is NOT in the indexable (client-facing) set.
  idx="$(indexable_rpms "$arch")"
  if printf '%s\n' "$idx" | grep -q 'promoted.rpm' && ! printf '%s\n' "$idx" | grep -q 'HOLD/'; then
    echo "  [PASS] T1 index set excludes HOLD/ (indexes:$(printf '%s' "$idx" | sed "s#$arch##g" | tr '\n' ' '))"
  else
    echo "  [FAIL] T1 index set did not exclude HOLD/: $idx"; fails=1
  fi

  # T2 — an unsigned RPM in the client-facing (top-level) set aborts the gate.
  ALLOW_UNSIGNED=""
  if verify_indexable_signatures "$arch" >/dev/null 2>&1; then
    echo "  [FAIL] T2 gate PASSED an unsigned top-level RPM"; fails=1
  else
    echo "  [PASS] T2 gate rejects an unsigned top-level RPM (fail-closed)"
  fi

  # T3 — unsigned content confined to HOLD/ does NOT trip the gate (it is excluded).
  arch2="$tmp/holdonly/fedora-99-x86_64"; mkdir -p "$arch2/HOLD"
  : > "$arch2/HOLD/unsigned-ci.rpm"
  ALLOW_UNSIGNED=""
  if verify_indexable_signatures "$arch2" >/dev/null 2>&1; then
    echo "  [PASS] T3 unsigned RPM in HOLD/ is tolerated (excluded from the index)"
  else
    echo "  [FAIL] T3 gate tripped on HOLD-only unsigned content"; fails=1
  fi

  # T4 — the explicit operator override lets unsigned content through (opt-in).
  ALLOW_UNSIGNED=1
  if verify_indexable_signatures "$arch" >/dev/null 2>&1; then
    echo "  [PASS] T4 MCNF_DNF_ALLOW_UNSIGNED override permits unsigned (opt-in bypass)"
  else
    echo "  [FAIL] T4 override did not permit unsigned"; fails=1
  fi
  ALLOW_UNSIGNED=""

  # T5 — a rolled-back RPM under ROLLED-BACK/ is NOT in the indexable set
  # (WL-BUILD-003: rollback quarantine is excluded, like HOLD/).
  mkdir -p "$arch/ROLLED-BACK"
  : > "$arch/ROLLED-BACK/demoted.rpm"
  idx="$(indexable_rpms "$arch")"
  if printf '%s\n' "$idx" | grep -q 'promoted.rpm' && ! printf '%s\n' "$idx" | grep -q 'ROLLED-BACK/'; then
    echo "  [PASS] T5 index set excludes ROLLED-BACK/ (rollback quarantine)"
  else
    echo "  [FAIL] T5 index set did not exclude ROLLED-BACK/: $idx"; fails=1
  fi

  rm -rf "$tmp"
  if [ "$fails" -eq 0 ]; then echo "self-test: ALL PASS"; return 0; fi
  echo "self-test: FAILURES"; return 1
}

SELFTEST=""
while [ $# -gt 0 ]; do
  case "$1" in
    --host)      HOST_IP="$2"; shift 2 ;;
    --fedora)    FEDORAS="${2//,/ }"; shift 2 ;;
    --self-test) SELFTEST=1; shift ;;
    -h|--help)   awk 'NR==1{next} /^#/{sub(/^# ?/,"");print;next} {exit}' "$0"; exit 0 ;;
    *) shift ;;
  esac
done

# build-deploy-6 self-test runs BEFORE the runtime prereqs (no podman/overlay).
if [ -n "$SELFTEST" ]; then
  run_self_test; exit $?
fi

detect_overlay() { ip -o -4 addr show 2>/dev/null | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}'; }
[ -n "$HOST_IP" ] || HOST_IP="$(detect_overlay)"
[ -n "$HOST_IP" ] || { echo "dnf-channel-up: no overlay IP (pass --host)" >&2; exit 1; }

command -v podman >/dev/null || { echo "podman required" >&2; exit 1; }
command -v createrepo_c >/dev/null || { echo "createrepo_c required (dnf install createrepo_c)" >&2; exit 1; }
command -v rpm >/dev/null || { echo "rpm required (signature gate)" >&2; exit 1; }

echo "==> lay out the channel under $ROOT (gh-pages shape) for fedora: $FEDORAS"
mkdir -p "$ROOT"
# Publish the public GPG key at the channel root (gpgcheck=1 path).
[ -f "$GPG_KEY" ] && cp -f "$GPG_KEY" "$ROOT/RPM-GPG-KEY-magic-mesh"
for n in $FEDORAS; do
  arch_dir="$ROOT/fedora-${n}-x86_64"
  mkdir -p "$arch_dir/HOLD" "$arch_dir/ROLLED-BACK"
  # build-deploy-6: index ONLY promoted+signed content. reindex_arch runs the
  # fail-closed signature gate over the client-facing set, then createrepo_c with
  # the HOLD/ staging subtree EXCLUDED — so an unsigned CI RPM staged in HOLD/ is
  # NOT advertised in the repodata/ a client installs from. An operator promotes by
  # signing (sign-release.sh) + moving the RPM OUT of HOLD/ into the arch dir.
  echo "   createrepo_c fedora-${n}-x86_64 (HOLD/ excluded, signed-only)"
  reindex_arch "$arch_dir"
done

# A ready-to-drop client repo file (mirrors gh-pages magic-mesh.repo but pointed at
# this sovereign channel). do-lighthouse-cloudinit.sh renders its own from
# REPO_BASEURL; this is for hand-install + verification.
cat > "$ROOT/magic-mesh.repo" <<EOF
# Sovereign mesh dnf channel (DAR-23) — served by the control VM over the overlay.
[magic-mesh]
name=Magic Mesh (sovereign mesh channel)
baseurl=http://${HOST_IP}:${PORT}/fedora-\$releasever-\$basearch/
type=rpm-md
skip_if_unavailable=True
gpgcheck=1
gpgkey=http://${HOST_IP}:${PORT}/RPM-GPG-KEY-magic-mesh
repo_gpgcheck=0
enabled=1
EOF

echo "==> serve $ROOT over overlay ${HOST_IP}:${PORT} (static httpd, overlay-only bind)"
if podman container exists mcnf-dnf-channel 2>/dev/null; then
  echo "   mcnf-dnf-channel already present — content refreshed in place (idempotent)"
else
  podman run -d --name mcnf-dnf-channel --restart=always \
    -p "${HOST_IP}:${PORT}:80" \
    -v "$ROOT:/usr/share/nginx/html:ro,Z" \
    docker.io/library/nginx:alpine >/dev/null
fi

echo "==> wait for the channel"
for _ in $(seq 1 15); do curl -s -o /dev/null -w '%{http_code}' "http://${HOST_IP}:${PORT}/RPM-GPG-KEY-magic-mesh" 2>/dev/null | grep -q 200 && break; sleep 1; done

cat <<EOF
Sovereign dnf channel → http://${HOST_IP}:${PORT}
  repomd:  http://${HOST_IP}:${PORT}/fedora-<N>-x86_64/repodata/repomd.xml  (SIGNED, promoted only)
  gpg key: http://${HOST_IP}:${PORT}/RPM-GPG-KEY-magic-mesh
  HOLD:    <root>/fedora-<N>-x86_64/HOLD/ (DAR-24 stages UNSIGNED CI RPMs; EXCLUDED from the index)
Point do-lighthouse-cloudinit.sh REPO_BASEURL=http://${HOST_IP}:${PORT} for air-gap provisioning.
Signing stays operator-gated (sign-release.sh) — promotion out of HOLD is NOT automated.
build-deploy-6: client metadata indexes signed content only (HOLD/ excluded + fail-closed signature gate).
EOF
