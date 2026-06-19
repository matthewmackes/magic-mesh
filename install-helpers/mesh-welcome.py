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

    # Per-node health + version/update. LIGHTHOUSE-7 — lighthouses get a beacon
    # marker (◉) + master/shadow tag, the beacon coloured by the node's health.
    leader = (d.get("network") or {}).get("leader") or ""
    print()
    for n in nodes:
        col, dot = DOT.get(n.get("presence", "offline"), (RED, "○"))
        host = (n.get("hostname") or "?")[:16].ljust(16)
        ip = (n.get("overlay_ip") or "").ljust(13)
        ver = n.get("version") or "—"
        upd = c("update→%s" % latest, AMBER) if n.get("update") else c("up to date", GRAY)
        if n.get("role") == "lighthouse":
            role = "master" if n.get("hostname") == leader else "shadow"
            tag = c(" ◉ lighthouse·%s" % role, col)
        else:
            tag = ""
        print(f"  {col}{dot}{RST} {c(host, BOLD)} {c(ip, GRAY)} {ver:<9} {upd}{tag}")

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

    network_overview(d)

    print()
    print(c("  type ", GRAY) + c("mesh-help", BOLD + BLUE)
          + c(" for the command cheat sheet", GRAY) + "\n")


def network_overview(d):
    """SHELL-NET — an ASCII diagram of the current network state + the subnets
    routable within the mesh, including the external gateways. Fed by the
    `network` block of /run/mde/mesh-status.json (this node's overlay view)."""
    net = d.get("network") or {}
    nodes = d.get("nodes", [])
    self_host = d.get("self", "")
    cidr = net.get("overlay_cidr") or "—"
    routes = net.get("routes") or ([cidr] if cidr != "—" else [])
    gw_eps = net.get("gateway_endpoints") or []
    defgw = net.get("default_gw") or ""

    print()
    print(c("  Network Overview", BOLD + BLUE))

    # ── ASCII diagram: internet → external gateways → overlay → nodes ──
    print(c("    ☁  internet", GRAY)
          + (c(f"  ─ gw {defgw}", GRAY) if defgw else ""))
    print(c("    │", GRAY))
    if gw_eps:
        print(c("    ▲  external gateways", GRAY))
        for ep in gw_eps:
            print("       " + c(ep, BLUE))
        print(c("    │", GRAY))
    head = f"  ┌─ overlay {cidr} "
    print(c(head + "─" * max(2, 50 - len(head)) + "┐", BLUE))
    # A dot strip — one mark per node, coloured by presence. LIGHTHOUSE-7 —
    # lighthouses render as a beacon (◉) instead of the plain ● so the anchor
    # nodes stand out at a glance.
    online = sum(1 for n in nodes if n.get("presence") == "online")
    lh = sum(1 for n in nodes if n.get("role") == "lighthouse")

    def _mark(n):
        col, dot = DOT.get(n.get("presence", "offline"), (RED, "○"))
        glyph = "◉" if n.get("role") == "lighthouse" else dot
        return f"{col}{glyph}{RST}"
    dots = " ".join(_mark(n) for n in nodes)
    tail = f"   {len(nodes)} nodes ({online} online)" + (f" · {lh} ◉ lighthouse" if lh else "")
    print("    " + dots + c(tail, GRAY))
    if self_host:
        print("    " + c(f"this node: {self_host} {net.get('overlay_ip','')}", GRAY))
    print(c("  └" + "─" * 50 + "┘", BLUE))

    # ── Routable subnets (within the mesh) ──
    print(c("  Routable subnets:", GRAY))
    if routes:
        for r in routes:
            tag = "  (overlay)" if r == cidr else ""
            print("    " + c(r, GREEN) + c(tag, GRAY))
    else:
        print("    " + c("none — overlay down?", GRAY))

    # ── External gateways ──
    print(c("  External gateways:", GRAY))
    if gw_eps:
        for ep in gw_eps:
            print("    " + c(ep, BLUE) + c("  lighthouse", GRAY))
    if defgw:
        print("    " + c(defgw, BLUE) + c("  internet (default route)", GRAY))
    if not gw_eps and not defgw:
        print("    " + c("none", GRAY))


if __name__ == "__main__":
    try:
        main()
    except Exception:
        sys.exit(0)  # never break a shell login
