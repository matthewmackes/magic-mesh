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
PROMOTION_STATE_DIR="${MCNF_PROMOTION_STATE_DIR:-$ROOT/automation/.state/promotion}"
PROMOTION_EVIDENCE_LOG="${MCNF_PROMOTION_EVIDENCE_LOG:-$PROMOTION_STATE_DIR/evidence.jsonl}"
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
DECLARATION_FILE="${MCNF_RELEASE_DECLARATION:-$ROOT/docs/ops/production-release-declaration.md}"

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required"; }

ts() { date -u +%Y-%m-%dT%H:%M:%SZ; }

json_escape() {
  local s="${1//\\/\\\\}"
  s="${s//\"/\\\"}"
  s="${s//$'\n'/ }"
  printf '%s' "$s"
}

latest_rpm() {
  if [ -n "$RPM" ]; then printf '%s\n' "$RPM"; return; fi
  ls -t "$ARTIFACTS"/*.rpm 2>/dev/null | head -1
}

rpm_sha256() {
  local rpm="$1"
  [ -f "$rpm" ] || return 0
  sha256sum "$rpm" 2>/dev/null | awk '{print $1}'
}

record_gate() {
  local stage="$1" result="${2:-pass}" detail="${3:-}" rpm version sha
  rpm="$(latest_rpm || true)"
  if [ -n "$rpm" ] && [ -f "$rpm" ]; then
    version="$(rpm_version_token "$rpm")"
    sha="$(rpm_sha256 "$rpm")"
  else
    rpm=""
    version=""
    sha=""
  fi
  mkdir -p "$PROMOTION_STATE_DIR"
  printf '{"ts":"%s","stage":"%s","result":"%s","candidate_rpm":"%s","candidate_version":"%s","candidate_sha256":"%s","detail":"%s"}\n' \
    "$(ts)" \
    "$(json_escape "$stage")" \
    "$(json_escape "$result")" \
    "$(json_escape "$rpm")" \
    "$(json_escape "$version")" \
    "$(json_escape "$sha")" \
    "$(json_escape "$detail")" >>"$PROMOTION_EVIDENCE_LOG"
}

worklist_open_count() {
  awk '
    /^[[:space:]]*- \[[^]]+\]/ {
      marker = substr($0, index($0, "[") + 1, index($0, "]") - index($0, "[") - 1)
      if (marker == " " || marker == ">" || marker == "!" || marker == "→" || marker == "~" || marker == "◐") {
        count++
      }
    }
    END { print count + 0 }
  ' "$ROOT/docs/WORKLIST.md"
}

worklist_marker_count() {
  local want="$1"
  awk -v want="$want" '
    /^[[:space:]]*- \[[^]]+\]/ {
      marker = substr($0, index($0, "[") + 1, index($0, "]") - index($0, "[") - 1)
      if (marker == want) {
        count++
      }
    }
    END { print count + 0 }
  ' "$ROOT/docs/WORKLIST.md"
}

worklist_active_breakdown() {
  printf 'open=%s in_progress=%s blocked=%s delegated=%s partial=%s review=%s\n' \
    "$(worklist_marker_count ' ')" \
    "$(worklist_marker_count '>')" \
    "$(worklist_marker_count '!')" \
    "$(worklist_marker_count '→')" \
    "$(worklist_marker_count '~')" \
    "$(worklist_marker_count '◐')"
}

worklist_next_candidates() {
  local limit="${1:-8}"
  awk -v limit="$limit" '
    /^[[:space:]]*- \[[ >]\][[:space:]]+\*\*[A-Za-z0-9._-]+[: ]/ {
      line = $0
      sub(/^[[:space:]]*- \[[ >]\][[:space:]]+\*\*/, "", line)
      sub(/\*\*.*/, "", line)
      print line
      count++
      if (count >= limit) {
        exit
      }
    }
  ' "$ROOT/docs/WORKLIST.md"
}

worklist_farm_job_count() {
  local farm_jobs="$ROOT/automation/lib/farm-jobs.sh"
  if [ ! -x "$farm_jobs" ]; then
    printf 'unavailable\n'
    return 0
  fi
  "$farm_jobs" active 2>/dev/null | sed '/^$/d' | wc -l | tr -d ' '
}

json_value() {
  local line="$1" key="$2"
  printf '%s\n' "$line" | sed -n "s/.*\"$key\":\"\\([^\"]*\\)\".*/\\1/p"
}

latest_gate_evidence() {
  local stage="$1" sha="$2"
  [ -n "$sha" ] || return 1
  [ -f "$PROMOTION_EVIDENCE_LOG" ] || return 1
  grep "\"stage\":\"$stage\"" "$PROMOTION_EVIDENCE_LOG" 2>/dev/null \
    | grep "\"candidate_sha256\":\"$sha\"" \
    | tail -1
}

missing_gate_list() {
  local sha="$1"; shift
  local stage missing="" sep="" line
  for stage in "$@"; do
    line="$(latest_gate_evidence "$stage" "$sha" || true)"
    if [ -z "$line" ] || [ "$(json_value "$line" result)" != pass ]; then
      missing="${missing}${sep}${stage}"
      sep=","
    fi
  done
  printf '%s\n' "${missing:-none}"
}

gate_blocker_summary() {
  local sha="$1"; shift
  local stage line result detail blockers="" sep=""
  for stage in "$@"; do
    line="$(latest_gate_evidence "$stage" "$sha" || true)"
    if [ -z "$line" ]; then
      blockers="${blockers}${sep}${stage}=missing"
      sep=","
      continue
    fi
    result="$(json_value "$line" result)"
    if [ "$result" != pass ]; then
      detail="$(json_value "$line" detail)"
      blockers="${blockers}${sep}${stage}=${detail:-$result}"
      sep=","
    fi
  done
  printf '%s\n' "${blockers:-none}"
}

status_gate_evidence() {
  local sha="$1" stage line result gate_ts detail farm_missing post_missing
  farm_missing="$(missing_gate_list "$sha" build l1 l2 l3 l4 eagle do)"
  post_missing="$(missing_gate_list "$sha" live-smoke live-audit media-verify fd-soak)"
  if [ "$farm_missing" = none ] && [ "$post_missing" = none ]; then
    echo "  candidate_gate_evidence: green"
  else
    echo "  candidate_gate_evidence: red"
  fi
  echo "  candidate_gate_missing_farm: $farm_missing"
  echo "  candidate_gate_missing_postroll: $post_missing"
  echo "  candidate_gate_blockers: $(gate_blocker_summary "$sha" build l1 l2 l3 l4 eagle do live-smoke live-audit media-verify fd-soak)"
  echo "  promotion_evidence_log: $PROMOTION_EVIDENCE_LOG"
  echo "  promotion_evidence:"
  for stage in build l1 l2 l3 l4 eagle do live-smoke live-audit media-verify fd-soak; do
    if line="$(latest_gate_evidence "$stage" "$sha")" && [ -n "$line" ]; then
      gate_ts="$(json_value "$line" ts)"
      result="$(json_value "$line" result)"
      detail="$(json_value "$line" detail)"
      if [ -n "$detail" ]; then
        echo "    - $stage: $result at $gate_ts ($detail)"
      else
        echo "    - $stage: $result at $gate_ts"
      fi
    else
      echo "    - $stage: missing"
    fi
  done
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
  timeout 20 sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$EAGLE" \
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
  timeout 20 sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$EAGLE" \
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

remote_diag() {
  local target="$1" prefix="${2:-ssh}"
  case "$prefix" in
    sshpass)
      sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$target" \
        "rpm -q magic-mesh || true; ps -eo pid,ppid,stat,etime,cmd | grep -E 'dnf|rpm' | grep -v grep || true; sudo tail -80 /var/log/dnf.log /var/log/dnf.rpm.log 2>/dev/null || true" || true
      ;;
    *)
      ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new "root@$target" \
        "rpm -q magic-mesh || true; ps -eo pid,ppid,stat,etime,cmd | grep -E 'dnf|rpm' | grep -v grep || true; tail -80 /var/log/dnf.log /var/log/dnf.rpm.log 2>/dev/null || true" || true
      ;;
  esac
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
  record_gate build pass "farm-rpm"
}

adopt_existing_candidate() {
  RPM="$(latest_rpm)"
  [ -n "$RPM" ] && [ -f "$RPM" ] || die "no RPM candidate to adopt"
  rpm -qp "$RPM" >/dev/null || die "candidate is not a readable RPM: $RPM"
  log "Adopt existing candidate RPM=$RPM sha256=$(rpm_sha256 "$RPM")"
  record_gate build pass "adopted-existing-rpm"
}

run_l1() {
  RPM="$(latest_rpm)"; [ -f "$RPM" ] || die "no RPM; run build first"
  log "L1 install gate"
  "$ROOT/automation/testbed/test-install.sh" "$RPM"
  record_gate l1 pass "clean-install"
}

run_l2() {
  RPM="$(latest_rpm)"; [ -f "$RPM" ] || die "no RPM; run build first"
  log "L2 feature mini-mesh gate"
  "$ROOT/automation/testbed/test-feature.sh" "$RPM"
  record_gate l2 pass "mini-mesh"
}

run_l3() {
  RPM="$(latest_rpm)"; [ -f "$RPM" ] || die "no RPM; run build first"
  log "L3 stability gate"
  "$ROOT/automation/testbed/test-stability.sh" "$RPM"
  record_gate l3 pass "stability"
}

run_l4() {
  RPM="$(latest_rpm)"; [ -f "$RPM" ] || die "no RPM; run build first"
  log "L4 staged lighthouse replace gate"
  "$ROOT/automation/testbed/test-lighthouse-replace.sh" "$RPM"
  record_gate l4 pass "staged-lighthouse-replace"
}

promote_eagle() {
  RPM="$(latest_rpm)"; [ -f "$RPM" ] || die "no RPM; run build first"
  need sshpass
  log "Promote to Eagle ($EAGLE)"
  publish_promote build "$(rpm_version_token "$RPM")" ready "candidate"
  sshpass -f "$EAGLE_PASS_FILE" scp -o StrictHostKeyChecking=accept-new "$RPM" "$EAGLE_USER@$EAGLE:/tmp/mcnf-promote.rpm"
  if ! timeout 900 sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$EAGLE" \
    "sudo -S -p '' dnf install -y /tmp/mcnf-promote.rpm && sudo rpm -Uvh --replacepkgs /tmp/mcnf-promote.rpm && sudo systemctl restart mackesd && sleep 10 && rpm -q magic-mesh && systemctl is-active mackesd nebula syncthing" <"$EAGLE_PASS_FILE"; then
    log "Eagle promotion failed or timed out; diagnostics follow"
    remote_diag "$EAGLE" sshpass
    return 1
  fi
  publish_promote eagle "$(rpm_version_token "$RPM")" ready "$EAGLE"
  record_gate eagle pass "$EAGLE"
}

live_smoke() {
  log "Live mesh smoke"
  local bad_peers out
  if ! out="$(ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new root@165.227.188.238 \
    'ETCDCTL_API=3 etcdctl --endpoints=http://10.42.0.1:2379 endpoint health --cluster &&
     mackesd peers &&
     df -h /run | awk "NR==2" &&
     test "$(journalctl -u mackesd --since "70 sec ago" | grep -Eic "SIGABRT|ABRT|dumped|coredump")" = 0')"; then
    printf '%s\n' "$out"
    record_gate live-smoke fail "live-mesh-command"
    return 1
  fi
  printf '%s\n' "$out"
  bad_peers="$(peer_health_blockers "$out")"
  if [ -n "$bad_peers" ]; then
    record_gate live-smoke fail "peer-health:$bad_peers"
    return 1
  fi
  record_gate live-smoke pass "live-mesh"
}

peer_health_blockers() {
  printf '%s\n' "$1" | awk '
    $1 == "PEER" || $1 == "fleet" || $1 ~ /^http/ || $1 == "Filesystem" || $1 == "tmpfs" || $1 ~ /^\// { next }
    NF >= 4 && ($2 != "online" || $3 != "healthy") {
      printf "%s%s=%s/%s", sep, $1, $2, $3
      sep = ","
    }
  '
}

live_blockers() {
  log "Live blocker diagnostics"
  local bad_peers out
  if ! out="$(ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new root@165.227.188.238 'mackesd peers')"; then
    printf '%s\n' "$out"
    echo "live_blockers: unable to query live peers from lighthouse 165.227.188.238"
    return 1
  fi

  printf '%s\n' "$out"
  bad_peers="$(peer_health_blockers "$out")"
  if [ -z "$bad_peers" ]; then
    echo "live_blockers: none"
    return 0
  fi

  echo "live_blockers: peer-health:$bad_peers"
  if printf '%s\n' "$bad_peers" | grep -q 'UNIT-EAGLE='; then
    need sshpass
    echo "eagle_diagnostics:"
    sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$EAGLE" \
      'echo "  host=$(hostname)";
       echo "  rpm=$(rpm -q magic-mesh 2>/dev/null || true)";
       echo "  services=$(systemctl is-active mackesd nebula syncthing 2>/dev/null | paste -sd, -)";
       echo "  power:";
       upower -d 2>/dev/null | awk "/native-path:|online:|state:|warning-level:|percentage:|time to empty:/ { gsub(/^[ \t]+/, \"\"); print \"    \" \$0 }" || true;
       echo "  recent_mackesd_warnings:";
       journalctl -u mackesd --since "15 min ago" -p warning --no-pager 2>/dev/null | tail -20 | sed "s/^/    /" || true' || true
  fi
  return 1
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
     stop_usec="$(systemctl show mackesd -p TimeoutStopUSec --value)";
     stop_mode="$(systemctl show mackesd -p TimeoutStopFailureMode --value)";
     echo "STOP_POLICY=${stop_usec},${stop_mode}";
     test "$stop_usec" = "1min 30s";
     test "$stop_mode" = "terminate";
     test -f /usr/lib/systemd/system/mackesd.service.d/90-stop-policy.conf;
     if [ -f /etc/systemd/system/mackesd.service.d/watchdog.conf ]; then
       ! grep -Eq "TimeoutStop(FailureMode=abort|USec=20s|Sec=20)" /etc/systemd/system/mackesd.service.d/watchdog.conf;
     fi;
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
     stop_usec="$(systemctl show mackesd -p TimeoutStopUSec --value)";
     stop_mode="$(systemctl show mackesd -p TimeoutStopFailureMode --value)";
     echo "STOP_POLICY=${stop_usec},${stop_mode}";
     test "$stop_usec" = "1min 30s";
     test "$stop_mode" = "terminate";
     test -f /usr/lib/systemd/system/mackesd.service.d/90-stop-policy.conf;
     if [ -f /etc/systemd/system/mackesd.service.d/watchdog.conf ]; then
       ! grep -Eq "TimeoutStop(FailureMode=abort|USec=20s|Sec=20)" /etc/systemd/system/mackesd.service.d/watchdog.conf;
     fi;
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
  record_gate live-audit pass "do-lighthouses-and-eagle"
}

media_verify() {
  log "MEDIA-LIGHTHOUSE live verification"
  check_limits
  local host="${MCNF_MEDIA_VERIFY_HOST:-}" extra_args=""
  if [ -z "$host" ]; then
    host="$(do_lighthouse_ips | head -1)"
  fi
  [ -n "$host" ] || die "no lighthouse available for media verification"
  log "MEDIA verifier host: $host"
  if [ "$#" -gt 0 ]; then
    extra_args="$(printf '%q ' "$@")"
  fi
  scp -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
    "$ROOT/automation/media/verify-media-lighthouse.sh" "root@$host:/tmp/verify-media-lighthouse.sh"
  ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new "root@$host" \
    "chmod 700 /tmp/verify-media-lighthouse.sh && MCNF_MEDIA_ENV_FILE=/etc/mackesd/media-spaces.env /tmp/verify-media-lighthouse.sh $extra_args"
  record_gate media-verify pass "${extra_args:-non-mutating}"
}

media_verify_for_cycle() {
  if [ "${MCNF_MEDIA_MUTATE_PLAYLIST:-0}" = 1 ]; then
    media_verify --mutate-playlist
  else
    media_verify
  fi
}

live_fd_soak() {
  log "Live fd/EMFILE soak"
  "$ROOT/automation/promotion/live-fd-soak.sh"
  record_gate fd-soak pass "duration=${MCNF_LIVE_FD_SOAK_SECONDS:-3600}s"
}

declaration_status() {
  if [ ! -f "$DECLARATION_FILE" ]; then
    printf 'missing (%s)\n' "$DECLARATION_FILE"
    return 0
  fi
  if rg -qi 'operator.*declare|production release.*complete|release complete' "$DECLARATION_FILE" 2>/dev/null; then
    printf 'present (%s)\n' "$DECLARATION_FILE"
  else
    printf 'present-but-unrecognized (%s)\n' "$DECLARATION_FILE"
  fi
}

status_report() {
  local rpm version sha open_count active_breakdown next_work farm_jobs
  rpm="$(latest_rpm || true)"
  open_count="$(worklist_open_count)"
  active_breakdown="$(worklist_active_breakdown)"
  next_work="$(worklist_next_candidates "${MCNF_STATUS_NEXT_WORK:-8}" | sed 's/^/    - /')"
  farm_jobs="$(worklist_farm_job_count)"
  cat <<EOF
MCNF promotion status
  worklist_open_or_in_progress: $open_count
  worklist_active_breakdown: $active_breakdown
  worklist_farm_jobs_active: $farm_jobs
  worklist_next_unblocked:
${next_work:-    - none}
  release_declaration: $(declaration_status)
EOF
  if [ -n "$rpm" ] && [ -f "$rpm" ]; then
    version="$(rpm_version_token "$rpm")"
    sha="$(rpm_sha256 "$rpm")"
    cat <<EOF
  candidate_rpm: $rpm
  candidate_version: $version
  candidate_sha256: $sha
EOF
  else
    echo "  candidate_rpm: missing"
  fi
  status_gate_evidence "${sha:-}"

  if command -v doctl >/dev/null 2>&1; then
    local limit active free lh
    if limit="$(do_limit 2>/dev/null)" && active="$(do_active_count 2>/dev/null)"; then
      free=$((limit - active))
      lh="$(do_lighthouse_count 2>/dev/null || true)"
      cat <<EOF
  do_account: active=$active limit=$limit free=$free tagged_lighthouses=${lh:-0} max_active=$DO_MAX_ACTIVE min_free=$DO_MIN_FREE
EOF
      if [ "$active" -gt "$DO_MAX_ACTIVE" ] || [ "$free" -lt "$DO_MIN_FREE" ]; then
        echo "  do_account_gate: red"
      else
        echo "  do_account_gate: green"
      fi
      local ip
      for ip in $(do_lighthouse_ips 2>/dev/null || true); do
        echo "  lighthouse[$ip]: version=$(node_version "$ip") host=$(node_hostname "$ip")"
      done
    else
      echo "  do_account: unavailable"
    fi
  else
    echo "  do_account: doctl missing"
  fi

  if command -v sshpass >/dev/null 2>&1 && [ -f "$EAGLE_PASS_FILE" ]; then
    echo "  eagle[$EAGLE]: version=$(eagle_version)"
  else
    echo "  eagle[$EAGLE]: unavailable (sshpass or password file missing)"
  fi

  if [ -x "$ROOT/install-helpers/farm-topology.sh" ]; then
    echo "  farm:"
    "$ROOT/install-helpers/farm-topology.sh" table 2>/dev/null | sed 's/^/    /' || echo "    unavailable"
  fi

  cat <<EOF
  required_before_complete:
    - worklist_open_or_in_progress must be 0
    - all current gates must pass for the final candidate
    - live lighthouse/Eagle audit, media verification, and fd soak must pass after the final candidate
    - operator must create the production release declaration
EOF
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
    if ! timeout 900 ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new "root@$ip" \
      'dnf install -y /tmp/mcnf-promote.rpm &&
       rpm -Uvh --replacepkgs /tmp/mcnf-promote.rpm &&
       systemctl restart mackesd &&
       sleep 10 &&
       rpm -q magic-mesh &&
       systemctl is-active mackesd nebula etcd syncthing'; then
      log "DO lighthouse $ip promotion failed or timed out; diagnostics follow"
      remote_diag "$ip" ssh
      return 1
    fi
    publish_lighthouse_version "$ip" "$(node_version "$ip")" ready "$(node_hostname "$ip")"
  done
  publish_promote "do" "$(rpm_version_token "$RPM")" ready "$DO_TAG"
  record_gate do pass "$DO_TAG"
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
  media_verify_for_cycle
  live_fd_soak
}

case "${1:-cycle}" in
  status|statrep) status_report ;;
  inventory) inventory ;;
  adopt-build|adopt-candidate) adopt_existing_candidate ;;
  check-limits) check_limits ;;
  build|l0) build_rpm ;;
  l1) run_l1 ;;
  l2) run_l2 ;;
  l3) run_l3 ;;
  l4|lighthouse-replace) run_l4 ;;
  eagle) promote_eagle ;;
  live-smoke) live_smoke ;;
  live-blockers) live_blockers ;;
  live-audit) live_audit ;;
  media-verify) shift; media_verify "$@" ;;
  fd-soak|live-fd-soak) live_fd_soak ;;
  do) promote_do ;;
  cycle) cycle ;;
  *) die "usage: $0 {status|statrep|inventory|adopt-build|check-limits|build|l1|l2|l3|l4|lighthouse-replace|eagle|live-smoke|live-blockers|live-audit|media-verify|fd-soak|do|cycle}" ;;
esac
