#!/bin/sh
# qnm-mount — BOOT-XPA8-4: the QNM-Shared (LizardFS) wedge-proof mount loop,
# extracted out of setup-qnm-shared.sh's inline ExecStart heredoc so it ships as
# a real file at /usr/libexec/mackesd/qnm-mount and UPDATES on every RPM upgrade
# (instead of being sed-rewritten in-place into the unit — fragile, XPA8). The
# qnm-shared.service ExecStart/ExecStop just call this; the node-specific mount
# point + master IP are passed as args (the unit bakes them at write time, so a
# logic fix flows through the libexec script on upgrade while the unit keeps the
# node's path/master).
#
# POSIX /bin/sh ONLY — the unit runs `/bin/sh` and on some nodes that is dash, so
# NO bashisms (no /dev/tcp, no [[ ]]). This preserves the exact contract of the
# old inline loop (the LH-JOIN-QNM-1 / BOOT-REC-2 / XPA-8 fixes baked into it):
#   - 15-iteration bounded retry (a cold boot brings nebula + the LizardFS master
#     up AFTER this unit is first scheduled, so RETRY until the master answers);
#   - every check that touches the mount is `timeout 6`-guarded so a half-formed /
#     stale FUSE mount in uninterruptible D-state can NEVER hang the loop;
#   - lazy `fusermount -uz` + `umount -l` + `pkill mfsmount` recovery so a wedged
#     mount actually detaches (plain -u cannot);
#   - stray-file recovery: a node that wrote into the UNmounted path gets those
#     files swept aside to /var/lib/mde/qnm-stray-<ts> so mfsmount's `nonempty`
#     can't fail / shadow them;
#   - mfsmount with `allow_other,nonempty` (allow_other lets the uid-1000 desktop
#     GUIs read the root-owned FUSE mount).
# The normal path exits 0 within a few seconds once the master answers; on a
# genuinely-down master it exits 1 → the unit's Restart=on-failure + the
# mesh-health watchdog keep retrying.
#
# Usage:
#   qnm-mount <qnm-path> <master-ip>   the boot-race mount loop (ExecStart)
#   qnm-mount --stop <qnm-path>        lazy unmount cleanup       (ExecStop)
#   qnm-mount --self-test              `sh -n` syntax check + arg validation
#   qnm-mount -h | --help              this header

usage() { sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; }

# do_mount <qnm-path> <master-ip> — the 15-iteration wedge-proof retry loop.
# EXACT port of the old inline ExecStart (only the heredoc escaping is gone +
# $QNM_PATH/$MASTER_IP are now the positional args). exit 0 on a live mount,
# exit 1 once the 15 iterations are exhausted.
do_mount() {
  QNM_PATH="$1"; MASTER_IP="$2"
  if [ -z "$QNM_PATH" ] || [ -z "$MASTER_IP" ]; then
    echo "qnm-mount: usage: qnm-mount <qnm-path> <master-ip>" >&2; exit 2
  fi
  i=0
  while [ "$i" -lt 15 ]; do
    timeout 6 mountpoint -q "$QNM_PATH" && exit 0
    fusermount -uz "$QNM_PATH" 2>/dev/null
    umount -l "$QNM_PATH" 2>/dev/null
    pkill -f "mfsmount $QNM_PATH" 2>/dev/null
    sleep 1
    if [ -n "$(timeout 6 ls -A "$QNM_PATH" 2>/dev/null)" ]; then
      d=/var/lib/mde/qnm-stray-$(date +%s 2>/dev/null || echo bk)
      mkdir -p "$d"
      mv "$QNM_PATH"/* "$QNM_PATH"/.[!.]* "$d"/ 2>/dev/null
    fi
    mfsmount "$QNM_PATH" -H "$MASTER_IP" -o allow_other,nonempty 2>/dev/null
    sleep 3
    timeout 6 mountpoint -q "$QNM_PATH" && exit 0
    i=$((i + 1))
    sleep 2
  done
  exit 1
}

# do_stop <qnm-path> — lazy detach (EXACT port of the old inline ExecStop).
do_stop() {
  QNM_PATH="$1"
  [ -n "$QNM_PATH" ] || { echo "qnm-mount: usage: qnm-mount --stop <qnm-path>" >&2; exit 2; }
  fusermount -uz "$QNM_PATH" 2>/dev/null
  umount -l "$QNM_PATH" 2>/dev/null
  true
}

# do_self_test — `sh -n` syntax check of this very file + arg-validation asserts.
# No mount I/O (safe on any host, incl. CI / the dev box with no LizardFS).
do_self_test() {
  fails=0
  st_check() { # st_check <label> <got> <want>
    if [ "$2" = "$3" ]; then echo "  ok: $1"
    else echo "  FAIL: $1 — got '$2' want '$3'" >&2; fails=$((fails + 1)); fi
  }
  echo "qnm-mount --self-test:"

  # 1) the shipped script must be syntactically valid /bin/sh (the whole point of
  #    BOOT-XPA8-4 — a real file we can lint, vs an un-lintable inline heredoc).
  if sh -n "$0" 2>/dev/null; then echo "  ok: sh -n (POSIX syntax)"
  else echo "  FAIL: sh -n reported a syntax error" >&2; fails=$((fails + 1)); fi
  # bash -n too where bash exists (the unit may run under either).
  if command -v bash >/dev/null 2>&1; then
    if bash -n "$0" 2>/dev/null; then echo "  ok: bash -n"
    else echo "  FAIL: bash -n reported a syntax error" >&2; fails=$((fails + 1)); fi
  fi

  # 2) arg validation: a mount/stop with a missing arg must refuse (rc 2), never
  #    run a half-specified mount. Probe in a subshell so the `exit 2` is local.
  st_check "mount: missing both args refused" \
    "$( (do_mount "" "" >/dev/null 2>&1); echo $? )" 2
  st_check "mount: missing master refused" \
    "$( (do_mount /mnt/mesh-storage "" >/dev/null 2>&1); echo $? )" 2
  st_check "stop: missing path refused" \
    "$( (do_stop "" >/dev/null 2>&1); echo $? )" 2

  if [ "$fails" -eq 0 ]; then echo "qnm-mount: self-test passed"; exit 0; fi
  echo "qnm-mount: SELF-TEST FAILED ($fails)" >&2; exit 1
}

case "${1:-}" in
  --self-test)  do_self_test ;;
  --stop)       shift; do_stop "$1" ;;
  -h|--help)    usage ;;
  "")           usage; exit 2 ;;
  *)            do_mount "$1" "$2" ;;
esac
