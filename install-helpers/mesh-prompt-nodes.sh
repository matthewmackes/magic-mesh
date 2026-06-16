#!/bin/bash
# MESHSHELL SHELL-2 — emit the online mesh hostnames for the starship `mesh`
# module. Reads the cached snapshot (no live query → zero prompt lag). Silent
# (exit 0, no output) when the snapshot is absent so the prompt stays clean.
S=/run/mde/mesh-status.json
[ -r "$S" ] || exit 0
python3 - "$S" <<'PY' 2>/dev/null
import json, sys
try: d = json.load(open(sys.argv[1]))
except Exception: sys.exit(0)
on = [n["hostname"] for n in d.get("nodes", []) if n.get("presence") == "online"]
off = d.get("total", 0) - len(on)
if on:
    tail = f"  +{off}○" if off else ""
    print("⬢ " + " ".join(on) + tail)
PY
