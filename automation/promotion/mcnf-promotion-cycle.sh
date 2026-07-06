#!/usr/bin/env bash
# mcnf-promotion-cycle.sh — staged Build -> Testbed -> Eagle -> DO promotion.
#
# The loop is intentionally boring: measure account capacity first, run gates in
# order, promote only when the previous stage is green. Live DO promotion requires
# MCNF_ARM_LIVE=1 so an unattended test cycle cannot replace lighthouses by
# accident.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ARTIFACTS="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"
RPM="${MCNF_RPM:-}"
DO_DOMAIN="${MCNF_DO_DOMAIN:-matthewmackes.com}"
DO_TAG="${MCNF_DO_TAG:-magic-lighthouse}"
DO_REGION="${MCNF_DO_REGION:-nyc3}"
DO_SIZE="${MCNF_DO_SIZE:-s-2vcpu-2gb}"
DO_MAX_ACTIVE="${MCNF_DO_MAX_ACTIVE:-8}"
DO_MIN_FREE="${MCNF_DO_MIN_FREE:-2}"
EAGLE="${MCNF_EAGLE_HOST:-172.20.146.13}"
EAGLE_USER="${MCNF_EAGLE_USER:-mm}"
EAGLE_PASS_FILE="${MCNF_EAGLE_PASS_FILE:-/root/.mcnf-xapi-cred}"
SSH_KEY="${MCNF_SSH_KEY:-/root/.ssh/id_ed25519}"

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required"; }

latest_rpm() {
  if [ -n "$RPM" ]; then printf '%s\n' "$RPM"; return; fi
  ls -t "$ARTIFACTS"/*.rpm 2>/dev/null | head -1
}

do_active_count() {
  doctl compute droplet list --format ID --no-header | sed '/^$/d' | wc -l
}

do_limit() {
  doctl account get --format DropletLimit --no-header | awk '{print $1}'
}

do_lighthouse_count() {
  doctl compute droplet list --format Tags --no-header | grep -c "\b${DO_TAG}\b" || true
}

do_lighthouse_ips() {
  doctl compute droplet list --tag-name "$DO_TAG" --format PublicIPv4 --no-header | sed '/^$/d'
}

rpm_version_token() {
  local name stem rest
  name="$(basename "$1")"
  stem="${name%.rpm}"
  rest="${stem#magic-mesh-}"
  rest="${rest%.*}"
  printf '%s\n' "$rest"
}

publish_promote() {
  local stage="$1" version="$2" status="${3:-ready}" detail="${4:-}"
  local body qbody
  version="${version//\\/\\\\}"; version="${version//\"/\\\"}"
  status="${status//\\/\\\\}"; status="${status//\"/\\\"}"
  detail="${detail//\\/\\\\}"; detail="${detail//\"/\\\"}"
  body="$(printf '{"stage":"%s","version":"%s","status":"%s","detail":"%s"}' \
    "$stage" "$version" "$status" "$detail")"
  if command -v mde-bus >/dev/null 2>&1; then
    mde-bus publish "event/dc/promote/$stage" --body-flag "$body" >/dev/null 2>&1 || true
    return 0
  fi
  command -v sshpass >/dev/null 2>&1 || return 0
  [ -f "$EAGLE_PASS_FILE" ] || return 0
  qbody="$(printf '%q' "$body")"
  sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$EAGLE" \
    "command -v mde-bus >/dev/null 2>&1 && mde-bus publish event/dc/promote/$stage --body-flag $qbody" >/dev/null 2>&1 || true
}

publish_lighthouse_version() {
  local ip="$1" version="$2" status="${3:-ready}" name="${4:-}"
  [ -n "$version" ] || return 0
  local target="${name:-$ip}" body qbody safe_stage
  target="${target//\\/\\\\}"; target="${target//\"/\\\"}"
  version="${version//\\/\\\\}"; version="${version//\"/\\\"}"
  status="${status//\\/\\\\}"; status="${status//\"/\\\"}"
  ip="${ip//\\/\\\\}"; ip="${ip//\"/\\\"}"
  body="$(printf '{"stage":"lighthouse:%s","version":"%s","status":"%s","target":"%s","detail":"%s"}' \
    "$ip" "$version" "$status" "$target" "$ip")"
  safe_stage="${ip//[^A-Za-z0-9_.-]/_}"
  if command -v mde-bus >/dev/null 2>&1; then
    mde-bus publish "event/dc/promote/lighthouse-$safe_stage" --body-flag "$body" >/dev/null 2>&1 || true
    return 0
  fi
  command -v sshpass >/dev/null 2>&1 || return 0
  [ -f "$EAGLE_PASS_FILE" ] || return 0
  qbody="$(printf '%q' "$body")"
  sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$EAGLE" \
    "command -v mde-bus >/dev/null 2>&1 && mde-bus publish event/dc/promote/lighthouse-$safe_stage --body-flag $qbody" >/dev/null 2>&1 || true
}

node_version() {
  local ip="$1"
  ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new "root@$ip" \
    "rpm -q --qf '%{VERSION}-%{RELEASE}' magic-mesh" 2>/dev/null || true
}

node_hostname() {
  local ip="$1"
  ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new "root@$ip" \
    "hostname" 2>/dev/null || true
}

eagle_version() {
  sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$EAGLE" \
    "rpm -q --qf '%{VERSION}-%{RELEASE}' magic-mesh" 2>/dev/null || true
}

check_limits() {
  need doctl
  local limit active free projected_lh
  limit="$(do_limit)"
  active="$(do_active_count)"
  free=$((limit - active))
  projected_lh="$(do_lighthouse_count)"
  log "DO account: active=$active limit=$limit free=$free lighthouse_tagged=$projected_lh max_active=$DO_MAX_ACTIVE min_free=$DO_MIN_FREE"
  [ "$active" -le "$DO_MAX_ACTIVE" ] || die "active droplets ($active) exceed MCNF_DO_MAX_ACTIVE=$DO_MAX_ACTIVE"
  [ "$free" -ge "$DO_MIN_FREE" ] || die "free droplet slots ($free) below MCNF_DO_MIN_FREE=$DO_MIN_FREE"
}

inventory() {
  check_limits
  log "DO droplets"
  doctl compute droplet list --format ID,Name,PublicIPv4,Status,Memory,VCPUs,Disk,Tags --no-header
  log "DO lighthouse DNS"
  doctl compute domain records list "$DO_DOMAIN" --format ID,Type,Name,Data,TTL --no-header | grep -E 'lighthouse|voip|@' || true
  if command -v rclone >/dev/null 2>&1; then
    log "Spaces buckets visible through rclone"
    rclone lsd mcnf-spaces: 2>/dev/null || true
  fi
}

build_rpm() {
  log "L0 build/RPM on farm"
  "$ROOT/install-helpers/xcp-build.sh" rpm
  RPM="$(latest_rpm)"
  [ -n "$RPM" ] && [ -f "$RPM" ] || die "farm build did not leave an RPM in $ARTIFACTS"
  log "RPM=$RPM"
  publish_promote build "$(rpm_version_token "$RPM")" ready "farm-rpm"
}

run_l1() {
  RPM="$(latest_rpm)"; [ -f "$RPM" ] || die "no RPM; run build first"
  log "L1 install gate"
  "$ROOT/automation/testbed/test-install.sh" "$RPM"
}

run_l2() {
  RPM="$(latest_rpm)"; [ -f "$RPM" ] || die "no RPM; run build first"
  log "L2 feature mini-mesh gate"
  "$ROOT/automation/testbed/test-feature.sh" "$RPM"
}

run_l3() {
  RPM="$(latest_rpm)"; [ -f "$RPM" ] || die "no RPM; run build first"
  log "L3 stability gate"
  "$ROOT/automation/testbed/test-stability.sh" "$RPM"
}

run_l4() {
  RPM="$(latest_rpm)"; [ -f "$RPM" ] || die "no RPM; run build first"
  log "L4 staged lighthouse replace gate"
  "$ROOT/automation/testbed/test-lighthouse-replace.sh" "$RPM"
}

promote_eagle() {
  RPM="$(latest_rpm)"; [ -f "$RPM" ] || die "no RPM; run build first"
  need sshpass
  log "Promote to Eagle ($EAGLE)"
  publish_promote build "$(rpm_version_token "$RPM")" ready "candidate"
  sshpass -f "$EAGLE_PASS_FILE" scp -o StrictHostKeyChecking=accept-new "$RPM" "$EAGLE_USER@$EAGLE:/tmp/mcnf-promote.rpm"
  sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$EAGLE" \
    "sudo -S -p '' dnf install -y /tmp/mcnf-promote.rpm && sudo systemctl restart mackesd && sleep 10 && rpm -q magic-mesh && systemctl is-active mackesd nebula syncthing" <"$EAGLE_PASS_FILE"
  publish_promote eagle "$(rpm_version_token "$RPM")" ready "$EAGLE"
}

live_smoke() {
  log "Live mesh smoke"
  ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new root@165.227.188.238 \
    'ETCDCTL_API=3 etcdctl --endpoints=http://10.42.0.1:2379 endpoint health --cluster &&
     mackesd peers &&
     df -h /run | awk "NR==2" &&
     test "$(journalctl -u mackesd --since "70 sec ago" | grep -c ABRT)" = 0'
}

audit_do_node() {
  local ip="$1"
  ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new "root@$ip" \
    'echo "HOST=$(hostname)";
     echo "RPM=$(rpm -q magic-mesh)";
     echo "CORE=$(systemctl is-active mackesd nebula etcd syncthing | paste -sd, -)";
     echo "QNM_ACTIVE=$(systemctl is-active qnm-shared 2>/dev/null || true)";
     echo "QNM_ENABLED=$(systemctl is-enabled qnm-shared 2>/dev/null || true)";
     echo "LIZARD_ACTIVE=$(systemctl is-active lizardfs-master lizardfs-chunkserver 2>/dev/null | paste -sd, - || true)";
     echo "LIZARD_ENABLED=$(systemctl is-enabled lizardfs-master lizardfs-chunkserver 2>/dev/null | paste -sd, - || true)";
     echo "FUSE_MOUNTS=$(findmnt -rn -t fuse,fuse.lizardfs -o TARGET,SOURCE 2>/dev/null | paste -sd "|" - || true)";
     test "$(systemctl is-active mackesd nebula etcd syncthing | grep -vc active)" = 0;
     test -z "$(findmnt -rn -t fuse,fuse.lizardfs -o TARGET,SOURCE 2>/dev/null)";
     ! systemctl is-enabled qnm-shared lizardfs-master lizardfs-chunkserver >/dev/null 2>&1'
}

audit_eagle() {
  need sshpass
  sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$EAGLE" \
    'echo "HOST=$(hostname)";
     echo "RPM=$(rpm -q magic-mesh)";
     echo "CORE=$(systemctl is-active mackesd nebula syncthing | paste -sd, -)";
     echo "QNM_ACTIVE=$(systemctl is-active qnm-shared 2>/dev/null || true)";
     echo "QNM_ENABLED=$(systemctl is-enabled qnm-shared 2>/dev/null || true)";
     echo "LIZARD_ACTIVE=$(systemctl is-active lizardfs-master lizardfs-chunkserver 2>/dev/null | paste -sd, - || true)";
     echo "LIZARD_ENABLED=$(systemctl is-enabled lizardfs-master lizardfs-chunkserver 2>/dev/null | paste -sd, - || true)";
     echo "FUSE_MOUNTS=$(findmnt -rn -t fuse,fuse.lizardfs -o TARGET,SOURCE 2>/dev/null | paste -sd "|" - || true)";
     test "$(systemctl is-active mackesd nebula syncthing | grep -vc active)" = 0;
     test -z "$(findmnt -rn -t fuse,fuse.lizardfs -o TARGET,SOURCE 2>/dev/null)";
     ! systemctl is-enabled qnm-shared lizardfs-master lizardfs-chunkserver >/dev/null 2>&1'
}

live_audit() {
  log "Live fleet audit"
  check_limits
  for ip in $(do_lighthouse_ips); do
    log "Audit DO lighthouse $ip"
    audit_do_node "$ip"
    publish_lighthouse_version "$ip" "$(node_version "$ip")" ready "$(node_hostname "$ip")"
  done
  log "Audit Eagle ($EAGLE)"
  audit_eagle
  publish_promote eagle "$(eagle_version)" ready "$EAGLE"
}

media_verify() {
  log "MEDIA-LIGHTHOUSE live verification"
  check_limits
  local host="${MCNF_MEDIA_VERIFY_HOST:-}"
  if [ -z "$host" ]; then
    host="$(do_lighthouse_ips | head -1)"
  fi
  [ -n "$host" ] || die "no lighthouse available for media verification"
  log "MEDIA verifier host: $host"
  scp -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
    "$ROOT/automation/media/verify-media-lighthouse.sh" "root@$host:/tmp/verify-media-lighthouse.sh"
  ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new "root@$host" \
    "chmod 700 /tmp/verify-media-lighthouse.sh && MCNF_MEDIA_ENV_FILE=/etc/mackesd/media-spaces.env /tmp/verify-media-lighthouse.sh $(printf '%q ' "$@")"
}

promote_do() {
  check_limits
  [ "${MCNF_ARM_LIVE:-0}" = 1 ] || die "set MCNF_ARM_LIVE=1 to promote to live DO lighthouses"
  RPM="$(latest_rpm)"; [ -f "$RPM" ] || die "no RPM; run build first"
  log "Promote live DO lighthouses in-place"
  publish_promote build "$(rpm_version_token "$RPM")" ready "candidate"
  for ip in $(do_lighthouse_ips); do
    log "DO lighthouse $ip"
    scp -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new "$RPM" "root@$ip:/tmp/mcnf-promote.rpm"
    ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new "root@$ip" \
      'dnf install -y /tmp/mcnf-promote.rpm &&
       systemctl restart mackesd &&
       sleep 10 &&
       rpm -q magic-mesh &&
       systemctl is-active mackesd nebula etcd syncthing'
    publish_lighthouse_version "$ip" "$(node_version "$ip")" ready "$(node_hostname "$ip")"
  done
  publish_promote "do" "$(rpm_version_token "$RPM")" ready "$DO_TAG"
}

cycle() {
  inventory
  build_rpm
  run_l1
  run_l2
  run_l3
  run_l4
  promote_eagle
  live_smoke
  promote_do
  live_smoke
  live_audit
}

case "${1:-cycle}" in
  inventory) inventory ;;
  check-limits) check_limits ;;
  build|l0) build_rpm ;;
  l1) run_l1 ;;
  l2) run_l2 ;;
  l3) run_l3 ;;
  l4|lighthouse-replace) run_l4 ;;
  eagle) promote_eagle ;;
  live-smoke) live_smoke ;;
  live-audit) live_audit ;;
  media-verify) shift; media_verify "$@" ;;
  do) promote_do ;;
  cycle) cycle ;;
  *) die "usage: $0 {inventory|check-limits|build|l1|l2|l3|l4|lighthouse-replace|eagle|live-smoke|live-audit|media-verify|do|cycle}" ;;
esac
