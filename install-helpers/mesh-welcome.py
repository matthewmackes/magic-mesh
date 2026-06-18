#!/usr/bin/python3
"""MESHSHELL SHELL-3 — the MCNF welcome greeting.

Renders a Carbon-styled banner from the cached /run/mde/mesh-status.json
(refreshed ~30s by mesh-status.timer — the read is instant, so a shell never
hangs even when the mesh is down). Shows: a Mackes ASCII wordmark, color-coded
per-node health + summary, a per-node service matrix, and per-node updates.
Printed on every interactive bash shell by /etc/profile.d/zz-mde-welcome.sh.
"""
import json
import os
import sys
import time

SNAP = "/run/mde/mesh-status.json"

# Carbon ANSI (256-color): Blue-60 #0f62fe≈33, Gray-50≈244, green≈42, amber≈214, red≈203.
BLUE, GRAY, GREEN, AMBER, RED, BOLD, RST = (
    "\033[38;5;33m", "\033[38;5;244m", "\033[38;5;42m",
    "\033[38;5;214m", "\033[38;5;203m", "\033[1m", "\033[0m",
)


def c(s, color):
    return f"{color}{s}{RST}"


def load():
    try:
        with open(SNAP) as f:
            return json.load(f)
    except Exception:
        return None


SERVICES = [
    ("bus", "Bus"), ("lizardfs", "FS"), ("nebula", "Neb"), ("dns", "DNS"),
    ("voice", "Voi"), ("music", "Mus"), ("kdc", "KDC"), ("workbench", "WB"),
]
DOT = {"online": (GREEN, "●"), "idle": (AMBER, "●"), "offline": (RED, "○")}


def main():
    d = load()
    # Wordmark header (Carbon blue box).
    line = "─" * 52
    print()
    print(c(f"┌{line}┐", BLUE))
    print(c("│", BLUE) + c("  ⬢  M A G I C   M E S H", BOLD + BLUE)
          + " " * 27 + c("│", BLUE))
    print(c("│", BLUE) + c("     Mackes Desktop Environment", GRAY)
          + " " * 21 + c("│", BLUE))
    print(c(f"└{line}┘", BLUE))

    if not d:
        print(c("  mesh status unavailable (snapshot not yet written)", GRAY))
        print(c("  type ", GRAY) + c("mesh-help", BOLD + BLUE)
              + c(" for the command cheat sheet", GRAY) + "\n")
        return

    nodes = d.get("nodes", [])
    age = max(0, int((time.time() * 1000 - d.get("generated_ms", 0)) / 1000))
    latest = d.get("latest_version") or "?"
    online, total = d.get("online", 0), d.get("total", len(nodes))
    hcol = GREEN if online == total and total else (AMBER if online else RED)
    print("  " + c(f"{online}/{total} nodes online", BOLD + hcol)
          + c(f"   · self {d.get('self','?')} · latest {latest} · data {age}s old", GRAY))

    # Per-node health + version/update.
    print()
    for n in nodes:
        col, dot = DOT.get(n.get("presence", "offline"), (RED, "○"))
        host = (n.get("hostname") or "?")[:16].ljust(16)
        ip = (n.get("overlay_ip") or "").ljust(13)
        ver = n.get("version") or "—"
        upd = c("update→%s" % latest, AMBER) if n.get("update") else c("up to date", GRAY)
        print(f"  {col}{dot}{RST} {c(host, BOLD)} {c(ip, GRAY)} {ver:<9} {upd}")

    # Service matrix.
    print()
    hdr = "  " + " " * 17 + " ".join(lbl.center(4) for _, lbl in SERVICES)
    print(c(hdr, GRAY))
    for n in nodes:
        host = (n.get("hostname") or "?")[:16].ljust(16)
        svc = n.get("services") or {}
        cells = []
        for key, _ in SERVICES:
            if key in svc:
                cells.append(c(" ✓ ", GREEN) if svc[key] else c(" · ", GRAY))
            else:
                cells.append(c(" ? ", GRAY))
        print(f"  {c(host, BOLD)} " + " ".join(cell.center(4) for cell in cells))

    print()
    print(c("  type ", GRAY) + c("mesh-help", BOLD + BLUE)
          + c(" for the command cheat sheet", GRAY) + "\n")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        sys.exit(0)  # never break a shell login
