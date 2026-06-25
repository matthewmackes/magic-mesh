#!/bin/bash
# MESHSHELL SHELL-4 — the MCNF command cheat sheet (`mesh-help`).
#
# Task-grouped, curated for accuracy, with a LIVE appendix that enumerates each
# installed mesh tool's actual subcommands (parsed from `--help`, so it never
# drifts from what's installed). Carbon-styled.
BLUE=$'\033[38;5;33m'; GRAY=$'\033[38;5;244m'; BOLD=$'\033[1m'; RST=$'\033[0m'

have(){ command -v "$1" >/dev/null 2>&1; }
header(){ printf '\n%s  %s%s\n' "$BOLD$BLUE" "$1" "$RST"; }
cmd(){ printf '    %s%-36s%s %s%s%s\n' "$BOLD" "$1" "$RST" "$GRAY" "$2" "$RST"; }

# Live subcommand list for a clap tool: the 2-space-indented command lines under
# "Commands:" (continuation lines are indented deeper, so they're skipped), with
# the first line of each description truncated.
verbs(){
  have "$1" || { printf '    %s(%s not installed)%s\n' "$GRAY" "$1" "$RST"; return 0; }
  "$1" --help 2>&1 | awk '
    /^Commands:/{f=1; next}
    f && /^[A-Za-z]/{f=0}
    f && /^  [a-z][A-Za-z0-9_-]+([[:space:]]|$)/ {
      c=$1; $1=""; sub(/^[[:space:]]+/,""); d=$0;
      if (c=="help") next;
      if (length(d)>50) d=substr(d,1,50) "\xe2\x80\xa6";
      printf "    \033[1m%-26s\033[0m \033[38;5;244m%s\033[0m\n", c, d
    }'
}

printf '%s┌──────────────────────────────────────────────┐%s\n' "$BLUE" "$RST"
printf '%s│%s  %s⬢  M A G I C   M E S H  —  cheat sheet%s       %s│%s\n' "$BLUE" "$RST" "$BOLD$BLUE" "$RST" "$BLUE" "$RST"
printf '%s└──────────────────────────────────────────────┘%s\n' "$BLUE" "$RST"

header "Enrollment & join"
cmd "mackesd found <name>"            "found a new mesh (this node = first lighthouse)"
cmd "mackesd add-peer --role <role>"  "mint a single-use v3 join token (on a lighthouse)"
cmd "mackesd join <token>"            "join an existing mesh with an add-peer token"
cmd "mde-enroll"                       "interactive enroll/join TUI"
cmd "mackesd leave"                    "voluntarily exit the mesh"

header "Status & health"
cmd "meshctl status"                  "this node + fleet status"
cmd "mackesd healthz"                 "health JSON (leader, node counts, workers)"
cmd "mackesd peers --json"            "the joined peer directory"
cmd "mde-bus request action/mesh/directory" "live roster (export MDE_BUS_ROOT=/run/mde-bus)"
cmd "mackesd validate run|status"     "overlay-reachability validation"
cmd "cat /run/mde/mesh-status.json"   "the cached snapshot (prompt/greeting source)"

header "Storage (Mesh Sync — Syncthing)"
cmd "systemctl status syncthing"      "the file plane (replicates /mnt/mesh-storage)"
cmd "ls /mnt/mesh-storage"            "the shared workgroup dir (plain, no FUSE)"
cmd "/usr/libexec/mackesd/syncthing-reconcile" "re-wire the device list from the etcd registry"

header "Services & logs"
cmd "systemctl status mackesd nebula" "core daemon + overlay"
cmd "systemctl --user status mde-musicd" "music daemon (Workstation)"
cmd "mde-bus tail 'action/#'"         "follow live bus traffic"
cmd "journalctl -u mackesd -f"        "daemon logs (follow)"
cmd "journalctl -u mesh-health -e"    "watchdog recovery log"

header "Fedora / system"
cmd "dnf upgrade magic-mesh"          "update the platform from the mesh dnf channel"
cmd "systemctl restart mackesd"       "restart the daemon"
cmd "rpm -q magic-mesh"               "installed version"
cmd "systemctl list-timers 'mesh*'"   "mesh snapshot + health timers"

# Live appendix — actual installed subcommands (auto-generated, never drifts).
header "All mackesd verbs (live)"
verbs mackesd
header "All meshctl verbs (live)"
verbs meshctl
printf '\n'
