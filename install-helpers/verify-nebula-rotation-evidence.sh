#!/usr/bin/env bash
# verify-nebula-rotation-evidence.sh — WL-SEC-006 read-only proof helper.
#
# This helper does not rotate Nebula, reload services, prune files, or mutate the
# live mesh. It collects bounded before/after evidence so the remaining live
# Nebula identity rotation drill can prove:
#   * the active identity generation and certificate changed;
#   * Nebula stayed/recovered active with the overlay interface present;
#   * old supervisor-created identity generations were pruned; and
#   * replicated workgroup state does not contain obvious private-key material.
#
# Typical live flow:
#   sudo install-helpers/verify-nebula-rotation-evidence.sh collect \
#     --out /var/tmp/nebula-rotation-before --probe 10.42.0.1
#   # run the controlled rotation drill separately
#   sudo install-helpers/verify-nebula-rotation-evidence.sh collect \
#     --out /var/tmp/nebula-rotation-after --probe 10.42.0.1
#   install-helpers/verify-nebula-rotation-evidence.sh compare \
#     /var/tmp/nebula-rotation-before /var/tmp/nebula-rotation-after
set -euo pipefail

DEFAULT_CONFIG_DIR="/etc/nebula"
DEFAULT_WORKGROUP_ROOT="${MDE_WORKGROUP_ROOT:-/mnt/mesh-storage}"
DEFAULT_IFACE="nebula1"
DEFAULT_MAX_SCAN_FILES="${MCNF_NEBULA_EVIDENCE_MAX_FILES:-4096}"
DEFAULT_MAX_SCAN_BYTES="${MCNF_NEBULA_EVIDENCE_MAX_BYTES:-1048576}"

usage() {
  sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
  cat <<'USAGE'

Commands:
  collect [--out DIR] [--config-dir DIR] [--workgroup-root DIR]
          [--iface IFACE] [--probe OVERLAY_IP] [--no-live]
          [--max-scan-files N] [--max-scan-bytes N]
  compare [--before DIR --after DIR] | compare BEFORE_DIR AFTER_DIR
  --self-test

Exit status:
  collect exits nonzero when a hard invariant cannot be evidenced.
  compare exits nonzero unless the snapshots prove a real identity change and
  the after snapshot is clean. Snapshots made with --no-live can prove the
  filesystem/rotation harness only; they cannot claim live reconnect evidence.
USAGE
}

die() {
  echo "verify-nebula-rotation-evidence: $*" >&2
  exit 2
}

is_uint() {
  [[ "${1:-}" =~ ^[0-9]+$ ]]
}

is_generation_name() {
  [[ "${1:-}" =~ ^generation-[0-9]+-[0-9A-Fa-f]{16}$ ]]
}

kv_get() {
  local dir="$1" key="$2" file
  file="$dir/snapshot.env"
  [ -f "$file" ] || die "missing snapshot: $file"
  awk -F= -v k="$key" '$1 == k {print substr($0, length(k) + 2); found=1; exit} END {if (!found) exit 1}' "$file" 2>/dev/null || true
}

sha_or_unreadable() {
  local path="$1"
  if [ -f "$path" ] && [ -r "$path" ]; then
    sha256sum -- "$path" 2>/dev/null | awk '{print $1}'
  else
    printf 'unreadable\n'
  fi
}

prepare_output_dir() {
  local out_dir="$1"
  if [ -e "$out_dir" ]; then
    [ -d "$out_dir" ] || die "--out exists and is not a directory: $out_dir"
    if find "$out_dir" -mindepth 1 -maxdepth 1 -print -quit | grep -q .; then
      die "refusing to collect into non-empty output directory: $out_dir"
    fi
  else
    mkdir -m 700 -p "$out_dir"
  fi
}

stat_value() {
  local follow="$1" fmt="$2" path="$3"
  if [ "$follow" = "follow" ]; then
    stat -L -c "$fmt" -- "$path" 2>/dev/null || printf 'missing\n'
  else
    stat -c "$fmt" -- "$path" 2>/dev/null || printf 'missing\n'
  fi
}

bool_from_systemctl_active() {
  local unit="$1"
  if ! command -v systemctl >/dev/null 2>&1; then
    printf 'unknown\n'
    return 0
  fi
  systemctl is-active --quiet "$unit" 2>/dev/null && printf 'true\n' || printf 'false\n'
}

iface_up() {
  local iface="$1"
  ip -o link show "$iface" 2>/dev/null | grep -q '<[^>]*UP' && printf 'true\n' || printf 'false\n'
}

overlay_ipv4() {
  local iface="$1"
  ip -o -4 addr show "$iface" 2>/dev/null | awk '{split($4, a, "/"); print a[1]; exit}' || true
}

write_kv() {
  local key="$1" value="$2"
  printf '%s=%s\n' "$key" "$value" >>"$SNAPSHOT_FILE"
}

scan_replicated_state() {
  local root="$1" hits_file="$2" max_files="$3" max_bytes="$4"
  local count=0 hits=0 truncated=false path pattern

  : >"$hits_file"
  if [ ! -d "$root" ]; then
    printf '%s %s %s\n' 0 0 missing
    return 0
  fi

  # Do not print matching lines: a bad replicated file may contain the secret.
  pattern='-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----|private_key_pem|peer_private_key|relay_private_key|ca_private_key|ca_key_pem|host_key_pem|private_seed|relay_authority_seed'
  while IFS= read -r -d '' path; do
    count=$((count + 1))
    if [ "$count" -gt "$max_files" ]; then
      truncated=true
      break
    fi
    if LC_ALL=C grep -aEq -- "$pattern" "$path" 2>/dev/null; then
      printf '%s\n' "$path" >>"$hits_file"
      hits=$((hits + 1))
    fi
  done < <(find "$root" -xdev -type f -size "-${max_bytes}c" -print0 2>>"$hits_file.find-errors")

  printf '%s %s %s\n' "$count" "$hits" "$truncated"
}

collect_mode() {
  local config_dir="$DEFAULT_CONFIG_DIR"
  local workgroup_root="$DEFAULT_WORKGROUP_ROOT"
  local iface="$DEFAULT_IFACE"
  local out_dir=""
  local probe_target=""
  local no_live=0
  local max_scan_files="$DEFAULT_MAX_SCAN_FILES"
  local max_scan_bytes="$DEFAULT_MAX_SCAN_BYTES"

  while [ "$#" -gt 0 ]; do
    case "$1" in
      --out)
        [ "$#" -ge 2 ] || die "--out needs DIR"
        out_dir="$2"; shift 2 ;;
      --config-dir)
        [ "$#" -ge 2 ] || die "--config-dir needs DIR"
        config_dir="$2"; shift 2 ;;
      --workgroup-root)
        [ "$#" -ge 2 ] || die "--workgroup-root needs DIR"
        workgroup_root="$2"; shift 2 ;;
      --iface)
        [ "$#" -ge 2 ] || die "--iface needs IFACE"
        iface="$2"; shift 2 ;;
      --probe)
        [ "$#" -ge 2 ] || die "--probe needs OVERLAY_IP"
        probe_target="$2"; shift 2 ;;
      --no-live)
        no_live=1; shift ;;
      --max-scan-files)
        [ "$#" -ge 2 ] || die "--max-scan-files needs N"
        is_uint "$2" || die "--max-scan-files must be an integer"
        max_scan_files="$2"; shift 2 ;;
      --max-scan-bytes)
        [ "$#" -ge 2 ] || die "--max-scan-bytes needs N"
        is_uint "$2" || die "--max-scan-bytes must be an integer"
        max_scan_bytes="$2"; shift 2 ;;
      -h|--help)
        usage; return 0 ;;
      *)
        if [ -z "$out_dir" ]; then
          out_dir="$1"; shift
        else
          die "unexpected collect argument: $1"
        fi ;;
    esac
  done

  if [ -z "$out_dir" ]; then
    out_dir="$(mktemp -d "${TMPDIR:-/tmp}/nebula-rotation-evidence.XXXXXX")"
  fi
  prepare_output_dir "$out_dir"
  SNAPSHOT_FILE="$out_dir/snapshot.env"

  local fail=0 summary="$out_dir/summary.txt"
  : >"$SNAPSHOT_FILE"
  : >"$summary"

  local identity_dir="$config_dir/identity"
  local current_link="$identity_dir/current"
  local current_target="" current_generation="" current_link_type="missing"
  if [ -L "$current_link" ]; then
    current_link_type="symlink"
    current_target="$(readlink "$current_link" 2>/dev/null || true)"
    if is_generation_name "$current_target"; then
      current_generation="$current_target"
    fi
  elif [ -e "$current_link" ]; then
    current_link_type="$(stat_value nofollow %F "$current_link")"
  fi

  write_kv format 1
  write_kv collected_at_utc "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  write_kv hostname "$(hostname -f 2>/dev/null || hostname 2>/dev/null || printf unknown)"
  write_kv machine_id_sha256 "$(sha_or_unreadable /etc/machine-id)"
  write_kv euid "$(id -u)"
  write_kv config_dir "$config_dir"
  write_kv workgroup_root "$workgroup_root"
  write_kv iface "$iface"
  write_kv identity_dir "$identity_dir"
  write_kv identity_root_mode "$(stat_value nofollow %a "$identity_dir")"
  write_kv identity_root_uid "$(stat_value nofollow %u "$identity_dir")"
  write_kv current_link_type "$current_link_type"
  write_kv current_target "$current_target"
  write_kv current_generation "$current_generation"
  write_kv active_cert_sha256 "$(sha_or_unreadable "$current_link/host.crt")"
  write_kv active_key_sha256 "$(sha_or_unreadable "$current_link/host.key")"
  write_kv active_key_mode "$(stat_value follow %a "$current_link/host.key")"
  write_kv compat_cert_link "$(readlink "$config_dir/host.crt" 2>/dev/null || true)"
  write_kv compat_key_link "$(readlink "$config_dir/host.key" 2>/dev/null || true)"

  {
    printf 'identity_dir=%s\n' "$identity_dir"
    printf 'current_link_type=%s\n' "$current_link_type"
    printf 'current_target=%s\n' "$current_target"
  } >"$out_dir/identity-generations.txt"

  if [ "$(stat_value nofollow %a "$identity_dir")" != "700" ]; then
    printf 'FAIL identity root is not mode 0700: %s\n' "$identity_dir" >>"$summary"
    fail=1
  fi
  if [ "$(stat_value nofollow %u "$identity_dir")" != "$(id -u)" ]; then
    printf 'FAIL identity root is not owned by current verifier uid: %s\n' "$identity_dir" >>"$summary"
    fail=1
  fi
  if [ "$current_link_type" != "symlink" ] || [ -z "$current_generation" ]; then
    printf 'FAIL active identity switch is missing or not a generation symlink: %s\n' "$current_link" >>"$summary"
    fail=1
  fi
  if [ "$(stat_value follow %a "$current_link/host.key")" != "600" ]; then
    printf 'FAIL active Nebula private key target is not mode 0600: %s/host.key\n' "$current_link" >>"$summary"
    fail=1
  fi

  local generation_count=0 stale_generation_count=0 unsafe_generation_count=0
  local entry name entry_mode entry_uid entry_type
  if [ -d "$identity_dir" ]; then
    while IFS= read -r -d '' entry; do
      name="$(basename "$entry")"
      if [ "$name" = "current" ]; then
        continue
      fi
      if is_generation_name "$name"; then
        generation_count=$((generation_count + 1))
        entry_mode="$(stat_value nofollow %a "$entry")"
        entry_uid="$(stat_value nofollow %u "$entry")"
        entry_type="$(stat_value nofollow %F "$entry")"
        printf 'generation name=%s type=%s mode=%s uid=%s active=%s\n' \
          "$name" "$entry_type" "$entry_mode" "$entry_uid" \
          "$([ "$name" = "$current_generation" ] && printf true || printf false)" \
          >>"$out_dir/identity-generations.txt"
        if [ "$name" != "$current_generation" ]; then
          if [ "$entry_type" = "directory" ] && [ "$entry_mode" = "700" ] && [ "$entry_uid" = "$(id -u)" ]; then
            stale_generation_count=$((stale_generation_count + 1))
          else
            unsafe_generation_count=$((unsafe_generation_count + 1))
          fi
        fi
      elif [[ "$name" == generation-* ]]; then
        unsafe_generation_count=$((unsafe_generation_count + 1))
        printf 'unexpected-generation-like-entry name=%s type=%s mode=%s uid=%s\n' \
          "$name" "$(stat_value nofollow %F "$entry")" \
          "$(stat_value nofollow %a "$entry")" "$(stat_value nofollow %u "$entry")" \
          >>"$out_dir/identity-generations.txt"
      fi
    done < <(find "$identity_dir" -mindepth 1 -maxdepth 1 -print0 2>>"$out_dir/identity-generations.find-errors")
  fi
  write_kv generation_count "$generation_count"
  write_kv stale_generation_count "$stale_generation_count"
  write_kv unsafe_generation_count "$unsafe_generation_count"
  if [ "$generation_count" -ne 1 ] || [ "$stale_generation_count" -ne 0 ] || [ "$unsafe_generation_count" -ne 0 ]; then
    printf 'FAIL identity generations are not pruned to the single active generation (total=%s stale=%s unsafe=%s)\n' \
      "$generation_count" "$stale_generation_count" "$unsafe_generation_count" >>"$summary"
    fail=1
  fi

  local scan_result scan_count scan_hits scan_truncated
  scan_result="$(scan_replicated_state "$workgroup_root" "$out_dir/replicated-secret-scan.txt" "$max_scan_files" "$max_scan_bytes")"
  scan_count="$(printf '%s\n' "$scan_result" | awk '{print $1}')"
  scan_hits="$(printf '%s\n' "$scan_result" | awk '{print $2}')"
  scan_truncated="$(printf '%s\n' "$scan_result" | awk '{print $3}')"
  write_kv replicated_scan_files "$scan_count"
  write_kv replicated_scan_hits "$scan_hits"
  write_kv replicated_scan_truncated "$scan_truncated"
  write_kv replicated_scan_max_files "$max_scan_files"
  write_kv replicated_scan_max_bytes "$max_scan_bytes"
  if [ "$scan_truncated" = "missing" ]; then
    printf 'FAIL replicated workgroup root is missing: %s\n' "$workgroup_root" >>"$summary"
    fail=1
  elif [ "$scan_truncated" = "true" ]; then
    printf 'FAIL replicated secret-marker scan truncated at %s files under %s\n' "$max_scan_files" "$workgroup_root" >>"$summary"
    fail=1
  fi
  if [ "$scan_hits" != "0" ]; then
    printf 'FAIL replicated secret-marker scan found %s suspicious file(s); see replicated-secret-scan.txt\n' "$scan_hits" >>"$summary"
    fail=1
  fi

  if [ "$no_live" -eq 1 ]; then
    write_kv live_checks skipped
    write_kv nebula_active skipped
    write_kv nebula_iface_up skipped
    write_kv overlay_ipv4 skipped
    write_kv probe_target "$probe_target"
    write_kv probe_reachable skipped
    printf 'WARN live checks skipped by --no-live; this snapshot cannot prove reconnect\n' >>"$summary"
  else
    local nebula_active mackesd_active link_up overlay_ip reachable="skipped"
    nebula_active="$(bool_from_systemctl_active nebula.service)"
    mackesd_active="$(bool_from_systemctl_active mackesd.service)"
    link_up="$(iface_up "$iface")"
    overlay_ip="$(overlay_ipv4 "$iface")"
    write_kv live_checks enabled
    write_kv nebula_active "$nebula_active"
    write_kv mackesd_active "$mackesd_active"
    write_kv nebula_iface_up "$link_up"
    write_kv overlay_ipv4 "$overlay_ip"
    write_kv probe_target "$probe_target"
    if [ -n "$probe_target" ]; then
      if timeout 6 ping -c1 -W2 "$probe_target" >/dev/null 2>&1; then
        reachable=true
      else
        reachable=false
      fi
    fi
    write_kv probe_reachable "$reachable"
    systemctl show nebula.service mackesd.service \
      -p Id -p LoadState -p ActiveState -p SubState -p NRestarts \
      -p ActiveEnterTimestamp -p ExecMainPID --no-pager \
      >"$out_dir/systemd-services.txt" 2>&1 || true
    ip -o addr show "$iface" >"$out_dir/overlay-addresses.txt" 2>&1 || true
    journalctl -u nebula.service -n 80 --no-pager >"$out_dir/journal-nebula-tail.txt" 2>&1 || true
    if [ "$nebula_active" != "true" ]; then
      printf 'FAIL nebula.service is not active\n' >>"$summary"
      fail=1
    fi
    if [ "$link_up" != "true" ] || [ -z "$overlay_ip" ]; then
      printf 'FAIL %s is not up with an IPv4 overlay address\n' "$iface" >>"$summary"
      fail=1
    fi
    if [ "$reachable" = "false" ]; then
      printf 'FAIL overlay probe target is unreachable: %s\n' "$probe_target" >>"$summary"
      fail=1
    fi
  fi

  if [ "$fail" -eq 0 ]; then
    printf 'PASS snapshot is clean: %s\n' "$out_dir" >>"$summary"
    printf 'WL-SEC-006 Nebula rotation evidence snapshot clean: %s\n' "$out_dir"
  else
    printf 'FAIL snapshot has evidence gaps: %s\n' "$out_dir" >>"$summary"
    sed -n '1,120p' "$summary" >&2
    return 1
  fi
}

snapshot_has_clean_summary() {
  local dir="$1" summary="$1/summary.txt"
  [ -f "$summary" ] || return 1
  grep -Eq '^PASS snapshot is clean: ' "$summary" || return 1
  ! grep -Eq '^FAIL ' "$summary"
}

require_same_snapshot_field() {
  local before_dir="$1" after_dir="$2" key="$3" label="$4"
  local before_value after_value
  before_value="$(kv_get "$before_dir" "$key")"
  after_value="$(kv_get "$after_dir" "$key")"
  if [ -z "$before_value" ] || [ -z "$after_value" ]; then
    echo "FAIL snapshots are missing $label metadata (before=${before_value:-missing} after=${after_value:-missing})"
    return 1
  fi
  if [ "$before_value" != "$after_value" ]; then
    echo "FAIL snapshots do not describe the same $label ($before_value -> $after_value)"
    return 1
  fi
}

compare_mode() {
  local before="" after="" fail=0 live_claim=1
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --before)
        [ "$#" -ge 2 ] || die "--before needs DIR"
        before="$2"; shift 2 ;;
      --after)
        [ "$#" -ge 2 ] || die "--after needs DIR"
        after="$2"; shift 2 ;;
      -h|--help)
        usage; return 0 ;;
      *)
        if [ -z "$before" ]; then
          before="$1"; shift
        elif [ -z "$after" ]; then
          after="$1"; shift
        else
          die "unexpected compare argument: $1"
        fi ;;
    esac
  done
  [ -n "$before" ] && [ -n "$after" ] || die "compare needs BEFORE_DIR and AFTER_DIR"

  echo "== WL-SEC-006 Nebula rotation evidence compare =="
  if snapshot_has_clean_summary "$before"; then
    echo "PASS before snapshot was a clean collection"
  else
    echo "FAIL before snapshot was not a clean collection: $before"
    fail=1
  fi
  if snapshot_has_clean_summary "$after"; then
    echo "PASS after snapshot was a clean collection"
  else
    echo "FAIL after snapshot was not a clean collection: $after"
    fail=1
  fi
  require_same_snapshot_field "$before" "$after" format "snapshot format" || fail=1
  require_same_snapshot_field "$before" "$after" hostname "node hostname" || fail=1
  require_same_snapshot_field "$before" "$after" machine_id_sha256 "node machine-id fingerprint" || fail=1
  require_same_snapshot_field "$before" "$after" config_dir "Nebula config directory" || fail=1
  require_same_snapshot_field "$before" "$after" identity_dir "Nebula identity directory" || fail=1
  require_same_snapshot_field "$before" "$after" workgroup_root "replicated workgroup root" || fail=1
  require_same_snapshot_field "$before" "$after" iface "Nebula interface" || fail=1

  local before_gen after_gen before_cert after_cert before_key after_key
  before_gen="$(kv_get "$before" current_generation)"
  after_gen="$(kv_get "$after" current_generation)"
  before_cert="$(kv_get "$before" active_cert_sha256)"
  after_cert="$(kv_get "$after" active_cert_sha256)"
  before_key="$(kv_get "$before" active_key_sha256)"
  after_key="$(kv_get "$after" active_key_sha256)"

  if [ -z "$before_gen" ] || [ -z "$after_gen" ] || [ "$before_gen" = "$after_gen" ]; then
    echo "FAIL active identity generation did not change ($before_gen -> $after_gen)"
    fail=1
  else
    echo "PASS active identity generation changed ($before_gen -> $after_gen)"
  fi
  if [ -z "$before_cert" ] || [ -z "$after_cert" ] || [ "$before_cert" = "$after_cert" ] || [ "$after_cert" = "unreadable" ]; then
    echo "FAIL active certificate hash did not change or is unreadable"
    fail=1
  else
    echo "PASS active certificate hash changed"
  fi
  if [ "$before_key" != "unreadable" ] && [ "$after_key" != "unreadable" ]; then
    if [ "$before_key" = "$after_key" ]; then
      echo "FAIL active private-key hash did not change"
      fail=1
    else
      echo "PASS active private-key hash changed (hash only; key material not copied)"
    fi
  else
    echo "WARN private-key hash comparison skipped because one snapshot could not read the key"
  fi

  local after_stale after_unsafe after_scan_hits after_scan_truncated before_live after_live
  after_stale="$(kv_get "$after" stale_generation_count)"
  after_unsafe="$(kv_get "$after" unsafe_generation_count)"
  after_scan_hits="$(kv_get "$after" replicated_scan_hits)"
  after_scan_truncated="$(kv_get "$after" replicated_scan_truncated)"
  if [ "${after_stale:-missing}" = "0" ] && [ "${after_unsafe:-missing}" = "0" ]; then
    echo "PASS after snapshot has no stale/unsafe identity generations"
  else
    echo "FAIL after snapshot still has stale/unsafe identity generations (stale=${after_stale:-missing} unsafe=${after_unsafe:-missing})"
    fail=1
  fi
  if [ "${after_scan_hits:-missing}" = "0" ] && [ "${after_scan_truncated:-missing}" = "false" ]; then
    echo "PASS after snapshot replicated secret-marker scan is clean and bounded"
  else
    echo "FAIL after snapshot replicated secret-marker scan is not clean (hits=${after_scan_hits:-missing} truncated=${after_scan_truncated:-missing})"
    fail=1
  fi

  before_live="$(kv_get "$before" live_checks)"
  after_live="$(kv_get "$after" live_checks)"
  if [ "$before_live" = "enabled" ] && [ "$after_live" = "enabled" ]; then
    for dir in "$before" "$after"; do
      local active iface probe target
      active="$(kv_get "$dir" nebula_active)"
      iface="$(kv_get "$dir" nebula_iface_up)"
      probe="$(kv_get "$dir" probe_reachable)"
      target="$(kv_get "$dir" probe_target)"
      if [ "$active" != "true" ] || [ "$iface" != "true" ]; then
        echo "FAIL live reconnect invariant missing in $dir (active=$active iface_up=$iface)"
        fail=1
      fi
      if [ -n "$target" ] && [ "$probe" != "true" ]; then
        echo "FAIL overlay probe was requested but did not pass in $dir (target=$target probe=$probe)"
        fail=1
      fi
    done
    [ "$fail" -ne 0 ] || echo "PASS before/after live snapshots show Nebula active with overlay present"
  else
    live_claim=0
    echo "WARN one or both snapshots skipped live checks; compare cannot claim live reconnect proof"
  fi

  if [ "$fail" -eq 0 ]; then
    if [ "$live_claim" -eq 1 ]; then
      echo "PASS WL-SEC-006 live rotation/reconnect/prune evidence is complete for these snapshots"
    else
      echo "PASS WL-SEC-006 deterministic rotation/prune harness evidence is complete; live reconnect proof remains external"
    fi
    return 0
  fi
  echo "FAIL WL-SEC-006 evidence compare did not prove the drill"
  return 1
}

make_fake_identity() {
  local config_dir="$1" generation="$2" cert="$3" key="$4"
  mkdir -p "$config_dir/identity/$generation"
  chmod 700 "$config_dir/identity" "$config_dir/identity/$generation"
  printf '%s\n' "$cert" >"$config_dir/identity/$generation/host.crt"
  printf '%s\n' "$key" >"$config_dir/identity/$generation/host.key"
  chmod 600 "$config_dir/identity/$generation/host.key"
  rm -f "$config_dir/identity/current" "$config_dir/host.crt" "$config_dir/host.key"
  ln -s "$generation" "$config_dir/identity/current"
  ln -s "identity/current/host.crt" "$config_dir/host.crt"
  ln -s "identity/current/host.key" "$config_dir/host.key"
}

self_test() {
  local td fails=0
  td="$(mktemp -d "${TMPDIR:-/tmp}/nebula-rotation-evidence-test.XXXXXX")"
  trap "rm -rf '$td'" EXIT

  local config_dir="$td/etc/nebula" workgroup_root="$td/workgroup"
  local before="$td/before" after="$td/after" leak="$td/leak"
  local unsafe="$td/unsafe" mismatched="$td/mismatched" nonempty="$td/nonempty"
  local live_missing="$td/live-missing"
  mkdir -p "$config_dir/identity" "$workgroup_root"
  chmod 700 "$config_dir/identity"
  printf '{"peer_cert_pem":"public-only"}\n' >"$workgroup_root/nebula-bundle.json"

  make_fake_identity "$config_dir" generation-1-1111111111111111 cert-a key-a
  if collect_mode --config-dir "$config_dir" --workgroup-root "$workgroup_root" --out "$before" --no-live >/dev/null; then
    echo "  ok: clean before snapshot collects"
  else
    echo "  FAIL: clean before snapshot should collect" >&2
    fails=$((fails + 1))
  fi

  rm -rf "$config_dir/identity/generation-1-1111111111111111"
  make_fake_identity "$config_dir" generation-2-2222222222222222 cert-b key-b
  if collect_mode --config-dir "$config_dir" --workgroup-root "$workgroup_root" --out "$after" --no-live >/dev/null; then
    echo "  ok: clean after snapshot collects"
  else
    echo "  FAIL: clean after snapshot should collect" >&2
    fails=$((fails + 1))
  fi

  if compare_mode "$before" "$after" >/dev/null; then
    echo "  ok: changed clean snapshots compare"
  else
    echo "  FAIL: changed clean snapshots should compare" >&2
    fails=$((fails + 1))
  fi

  if compare_mode "$before" "$before" >/dev/null 2>&1; then
    echo "  FAIL: unchanged snapshots must not prove rotation" >&2
    fails=$((fails + 1))
  else
    echo "  ok: unchanged snapshots fail compare"
  fi

  cp -a "$after" "$mismatched"
  sed -i 's/^iface=.*/iface=nebula99/' "$mismatched/snapshot.env"
  if compare_mode "$before" "$mismatched" >/dev/null 2>&1; then
    echo "  FAIL: mismatched snapshot metadata must not prove rotation" >&2
    fails=$((fails + 1))
  else
    echo "  ok: mismatched snapshot metadata fails compare"
  fi

  chmod 644 "$config_dir/identity/generation-2-2222222222222222/host.key"
  if collect_mode --config-dir "$config_dir" --workgroup-root "$workgroup_root" --out "$unsafe" --no-live >/dev/null 2>&1; then
    echo "  FAIL: unsafe active key mode should fail collection" >&2
    fails=$((fails + 1))
  else
    echo "  ok: unsafe active key mode fails collection"
  fi
  if compare_mode "$before" "$unsafe" >/dev/null 2>&1; then
    echo "  FAIL: failed snapshots must not prove rotation" >&2
    fails=$((fails + 1))
  else
    echo "  ok: failed snapshots are refused by compare"
  fi
  chmod 600 "$config_dir/identity/generation-2-2222222222222222/host.key"

  mkdir -p "$nonempty"
  printf 'stale\n' >"$nonempty/old-summary.txt"
  if ( collect_mode --config-dir "$config_dir" --workgroup-root "$workgroup_root" --out "$nonempty" --no-live ) >/dev/null 2>&1; then
    echo "  FAIL: non-empty output directories must be refused" >&2
    fails=$((fails + 1))
  else
    echo "  ok: non-empty output directory is refused"
  fi

  if collect_mode --config-dir "$config_dir" --workgroup-root "$workgroup_root" --out "$live_missing" --iface definitely-missing-nebula-test0 >/dev/null 2>&1; then
    echo "  FAIL: missing live interface should fail collection" >&2
    fails=$((fails + 1))
  elif grep -q '^live_checks=enabled$' "$live_missing/snapshot.env" \
    && grep -q 'FAIL definitely-missing-nebula-test0 is not up with an IPv4 overlay address' "$live_missing/summary.txt"; then
    echo "  ok: missing live interface records live evidence gaps"
  else
    echo "  FAIL: missing live interface did not leave diagnosable evidence" >&2
    fails=$((fails + 1))
  fi

  printf '%s\n' '-----BEGIN PRIVATE KEY-----' >"$workgroup_root/leaked.json"
  if collect_mode --config-dir "$config_dir" --workgroup-root "$workgroup_root" --out "$leak" --no-live >/dev/null 2>&1; then
    echo "  FAIL: replicated private-key marker should fail collection" >&2
    fails=$((fails + 1))
  else
    echo "  ok: replicated private-key marker fails collection"
  fi

  if [ "$fails" -eq 0 ]; then
    echo "verify-nebula-rotation-evidence.sh: self-test passed"
    return 0
  fi
  echo "verify-nebula-rotation-evidence.sh: SELF-TEST FAILED ($fails)" >&2
  return 1
}

case "${1:-}" in
  collect)
    shift
    collect_mode "$@"
    ;;
  compare)
    shift
    compare_mode "$@"
    ;;
  --self-test)
    self_test
    ;;
  -h|--help|"")
    usage
    ;;
  *)
    die "unknown command: $1"
    ;;
esac
