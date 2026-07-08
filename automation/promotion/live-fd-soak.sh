#!/usr/bin/env bash
# live-fd-soak.sh — BUG-BROWSER-7 live nofile/EMFILE soak.
#
# Samples the promoted DO lighthouses plus Eagle for service health, fd count,
# LimitNOFILE, and recent EMFILE/Too-many-open-files journal lines. Defaults to
# the one-hour release evidence window; override duration/interval with
# MCNF_LIVE_FD_SOAK_SECONDS and MCNF_LIVE_FD_SOAK_INTERVAL.
set -euo pipefail

DO_TAG="${MCNF_DO_TAG:-magic-lighthouse}"
SSH_KEY="${MCNF_SSH_KEY:-/root/.ssh/id_ed25519}"
EAGLE="${MCNF_EAGLE_HOST:-172.20.146.13}"
EAGLE_USER="${MCNF_EAGLE_USER:-mm}"
EAGLE_PASS_FILE="${MCNF_EAGLE_PASS_FILE:-/root/.mcnf-xapi-cred}"
SOAK_SECONDS="${MCNF_LIVE_FD_SOAK_SECONDS:-3600}"
INTERVAL="${MCNF_LIVE_FD_SOAK_INTERVAL:-300}"
START_EPOCH="$(date +%s)"
SINCE="$(date -u +'%Y-%m-%d %H:%M:%S UTC')"

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required"; }

do_lighthouse_ips() {
  doctl compute droplet list --tag-name "$DO_TAG" --format PublicIPv4 --no-header | sed '/^$/d'
}

sample_do() {
  local ip="$1"
  ssh -i "$SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new "root@$ip" \
    "p=\$(systemctl show -p MainPID --value mackesd.service); \
     fds=\$(find /proc/\$p/fd -mindepth 1 -maxdepth 1 -printf . 2>/dev/null | wc -c); \
     emfile=\$(journalctl -u mackesd --since '$SINCE' --no-pager | grep -Eic 'EMFILE|Too many open files' || true); \
     active=\$(systemctl is-active mackesd nebula etcd syncthing | paste -sd, -); \
     nofile=\$(systemctl show -p LimitNOFILE --value mackesd.service); \
     rpm=\$(rpm -q magic-mesh); \
     printf 'target=%s rpm=%s active=%s nofile=%s pid=%s fds=%s emfile_since_start=%s\n' '$ip' \"\$rpm\" \"\$active\" \"\$nofile\" \"\$p\" \"\$fds\" \"\$emfile\"; \
     test \"\$active\" = active,active,active,active && test \"\$nofile\" -ge 65536 && test \"\$fds\" -gt 0 && test \"\$fds\" -lt 1024 && test \"\$emfile\" -eq 0"
}

sample_eagle() {
  local pass
  pass="$(cat "$EAGLE_PASS_FILE")"
  printf '%s\n' "$pass" | sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$EAGLE" \
    "p=\$(systemctl show -p MainPID --value mackesd.service); \
     fds=\$(sudo -S -p '' find /proc/\$p/fd -mindepth 1 -maxdepth 1 -printf . 2>/dev/null | wc -c); \
     emfile=\$(journalctl -u mackesd --since '$SINCE' --no-pager | grep -Eic 'EMFILE|Too many open files' || true); \
     active=\$(systemctl is-active mackesd nebula syncthing | paste -sd, -); \
     nofile=\$(systemctl show -p LimitNOFILE --value mackesd.service); \
     rpm=\$(rpm -q magic-mesh); \
     printf 'target=%s rpm=%s active=%s nofile=%s pid=%s fds=%s emfile_since_start=%s\n' '$EAGLE' \"\$rpm\" \"\$active\" \"\$nofile\" \"\$p\" \"\$fds\" \"\$emfile\"; \
     test \"\$active\" = active,active,active && test \"\$nofile\" -ge 65536 && test \"\$fds\" -gt 0 && test \"\$fds\" -lt 1024 && test \"\$emfile\" -eq 0"
}

sample_all() {
  local ip failed=0
  for ip in "${DO_IPS[@]}"; do
    sample_do "$ip" || failed=1
  done
  sample_eagle || failed=1
  return "$failed"
}

need doctl
need sshpass
[ -f "$EAGLE_PASS_FILE" ] || die "missing Eagle password file: $EAGLE_PASS_FILE"
mapfile -t DO_IPS < <(do_lighthouse_ips)
[ "${#DO_IPS[@]}" -gt 0 ] || die "no DO lighthouses found for tag $DO_TAG"

log "live fd soak start=$SINCE duration=${SOAK_SECONDS}s interval=${INTERVAL}s targets=${DO_IPS[*]} $EAGLE"
while :; do
  now="$(date +%s)"
  elapsed=$((now - START_EPOCH))
  printf -- '-- sample elapsed=%ss --\n' "$elapsed"
  sample_all
  [ "$elapsed" -ge "$SOAK_SECONDS" ] && break
  sleep_for="$INTERVAL"
  remaining=$((SOAK_SECONDS - elapsed))
  [ "$remaining" -lt "$sleep_for" ] && sleep_for="$remaining"
  sleep "$sleep_for"
done
log "live fd soak passed duration=${SOAK_SECONDS}s since=$SINCE"
