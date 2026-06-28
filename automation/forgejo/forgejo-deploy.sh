#!/usr/bin/env bash
# forgejo-deploy.sh — DAR-26: ONE entrypoint that stands up the entire forgejo-ci
# subsystem from the mesh + secret store, so the backoffice comes along and can
# reconstitute the current setup. Wired into the Full-tier backoffice hook (Phase
# 4); Minimal tier never calls it.
#
# Sequence (each step idempotent — a re-run reconstitutes without duplicates):
#   1. forgejo-up.sh        — server + admin + runner token (overlay bind, store-backed)
#   2. forgejo-runner-up.sh — host-native act_runner, label farm
#   3. forgejo-seed.sh      — repo (GitHub pull-mirror, else on-disk air-gap seed)
#   4. dnf-channel-up.sh    — sovereign dnf channel (gh-pages shape, HOLD area)
#
# Then OWN post-checks: healthz pass, runner active, repo present, channel served.
# (The secret-store self-init — mcnf-secret.sh init-self — is the control VM's
# boot step, NOT re-done here; this assumes the VM already holds its own key + the
# store is re-sealed to it. forgejo-up.sh get/mints the forgejo-* secrets.)
#
# Usage: forgejo-deploy.sh [--host <overlay-ip>] [--dry-run]
#   --dry-run  print the ordered steps + resolved host; run NOTHING.
# Env: MCNF_HOST_IP, MCNF_FORGEJO_ADMIN, MCNF_REPO.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

HOST_IP="${MCNF_HOST_IP:-}"
DRY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --host)    HOST_IP="$2"; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    -h|--help) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) shift ;;
  esac
done

detect_overlay() { ip -o -4 addr show 2>/dev/null | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}'; }
[ -n "$HOST_IP" ] || HOST_IP="$(detect_overlay)"

if [ "$DRY" -eq 1 ]; then
  cat <<EOF
forgejo-deploy --dry-run (host=${HOST_IP:-<auto-detect-overlay>}):
  1. forgejo-up.sh --host <overlay>        (server + admin + runner token; store-backed)
  2. forgejo-runner-up.sh --host <overlay> (host-native act_runner, label farm)
  3. forgejo-seed.sh --host <overlay>      (repo: GitHub pull-mirror | on-disk air-gap seed)
  4. dnf-channel-up.sh --host <overlay>    (sovereign dnf channel, gh-pages shape, HOLD area)
  post-checks: /api/healthz=pass · runner is-active · repo present · channel repomd served
Nothing run (dry-run).
EOF
  exit 0
fi

[ -n "$HOST_IP" ] || { echo "forgejo-deploy: no overlay IP found (pass --host)" >&2; exit 1; }
HARGS=(--host "$HOST_IP")

echo "== forgejo-deploy: stand up the forgejo-ci subsystem on overlay $HOST_IP =="
bash "$HERE/forgejo-up.sh"        "${HARGS[@]}"
bash "$HERE/forgejo-runner-up.sh" "${HARGS[@]}"
bash "$HERE/forgejo-seed.sh"      "${HARGS[@]}"
bash "$HERE/dnf-channel-up.sh"    "${HARGS[@]}"

echo "== post-checks =="
fail=0
chk() { if eval "$2" >/dev/null 2>&1; then echo "  OK  $1"; else echo "  FAIL $1" >&2; fail=1; fi; }
chk "Forgejo /api/healthz = pass" "curl -s http://${HOST_IP}:3000/api/healthz | grep -q pass"
chk "runner mcnf-forgejo-runner active" "[ \"\$(systemctl is-active mcnf-forgejo-runner 2>/dev/null)\" = active ]"
chk "repo magic-mesh present" "curl -s http://${HOST_IP}:3000/api/v1/repos/search?q=magic-mesh | grep -q magic-mesh"
chk "dnf channel repomd served" "curl -s -o /dev/null -w '%{http_code}' http://${HOST_IP}:8480/fedora-43-x86_64/repodata/repomd.xml | grep -q 200"

if [ "$fail" -eq 0 ]; then
  echo "forgejo-deploy: ALL post-checks PASS — CI subsystem up on $HOST_IP"
else
  echo "forgejo-deploy: some post-checks FAILED (see above)" >&2
fi
exit "$fail"
