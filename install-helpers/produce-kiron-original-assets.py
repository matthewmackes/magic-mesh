#!/usr/bin/env python3
"""Produce the original, deterministic UX-014 Kiron asset package.

The package deliberately uses authored SVG scene tiers and short synthesized
PCM cues.  They are source-owned originals, reproducible from this script, and
remain subject to the normal manifest/package admission gate.
"""

from __future__ import annotations

import hashlib
import json
import math
import subprocess
import wave
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ASSET_ROOT = ROOT / "assets" / "kiron"
PAYLOAD_ROOT = ASSET_ROOT / "payload"
GRADES = "ABCDEF"
MODES = ("live-3d", "pre-rendered", "static")
COLORS = {
    "A": ("#42be65", "Optimal"),
    "B": ("#82cfff", "Good"),
    "C": ("#f1c21b", "Watch"),
    "D": ("#ff832b", "Degraded"),
    "E": ("#fa4d56", "Critical"),
    "F": ("#d12771", "Offline"),
}


def revision() -> str:
    commit = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
    ).strip()
    return hashlib.sha256(commit.encode()).hexdigest()


def scene(grade: str, mode: str) -> bytes:
    color, label = COLORS[grade]
    mode_label = {"live-3d": "LIVE", "pre-rendered": "RENDER", "static": "STATIC"}[mode]
    # The visual is intentionally simple and source-owned: a bounded lower
    # third, grade marker, and recovery rail that every renderer tier can map.
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="220" viewBox="0 0 1280 220">
  <title>KIRON {grade} {label} {mode_label}</title>
  <rect width="1280" height="220" rx="18" fill="#161616"/>
  <rect width="16" height="220" rx="8" fill="{color}"/>
  <circle cx="90" cy="110" r="48" fill="{color}" opacity=".18" stroke="{color}" stroke-width="3"/>
  <text x="90" y="128" text-anchor="middle" font-family="sans-serif" font-size="54" font-weight="700" fill="{color}">{grade}</text>
  <text x="170" y="92" font-family="sans-serif" font-size="34" font-weight="700" fill="#f4f4f4">KIRON HEALTH · {label.upper()}</text>
  <text x="170" y="138" font-family="sans-serif" font-size="22" fill="#c6c6c6">Node health transition · {mode_label} fallback tier</text>
  <path d="M170 178 H1110" stroke="{color}" stroke-width="4" opacity=".8"/>
  <circle cx="1110" cy="178" r="7" fill="{color}"/>
</svg>\n'''.encode()


def cue(grade_index: int) -> bytes:
    sample_rate = 48_000
    frames = 2_400  # 50 ms: enough for a bounded cue, short by design.
    frequency = 330 + grade_index * 55
    with wave.open(str(PAYLOAD_ROOT / "audio" / f"{GRADES[grade_index].lower()}-cue.wav"), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(sample_rate)
        values = bytearray()
        for index in range(frames):
            envelope = min(1.0, index / 240, (frames - index) / 480)
            sample = int(10_000 * envelope * math.sin(2 * math.pi * frequency * index / sample_rate))
            values += int(sample).to_bytes(2, "little", signed=True)
        out.writeframes(bytes(values))
    return (PAYLOAD_ROOT / "audio" / f"{GRADES[grade_index].lower()}-cue.wav").read_bytes()


def row(path: Path, grade: str, mode: str, source_revision: str) -> dict[str, object]:
    data = path.read_bytes()
    return {
        "grade": grade, "mode": mode, "path": str(path.relative_to(ASSET_ROOT)),
        "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
        "license": "CC0-1.0",
        "provenance": {"origin": "original", "creator": "MDE contributors", "source_revision": source_revision},
    }


def main() -> None:
    source_revision = revision()
    ASSET_ROOT.mkdir(parents=True, exist_ok=True)
    PAYLOAD_ROOT.mkdir(parents=True, exist_ok=True)
    (PAYLOAD_ROOT / "audio").mkdir(exist_ok=True)
    # Remove only the obsolete paths from the initial pre-payload layout.
    for grade in GRADES:
        for mode in MODES:
            (ASSET_ROOT / f"{grade.lower()}-{mode}.svg").unlink(missing_ok=True)
        (ASSET_ROOT / "audio" / f"{grade.lower()}-cue.wav").unlink(missing_ok=True)
    try:
        (ASSET_ROOT / "audio").rmdir()
    except OSError:
        pass
    scenes = []
    for grade in GRADES:
        for mode in MODES:
            path = PAYLOAD_ROOT / f"{grade.lower()}-{mode}.svg"
            path.write_bytes(scene(grade, mode))
            path.chmod(0o644)
            scenes.append(row(path, grade, mode, source_revision))
    audio = []
    for index, grade in enumerate(GRADES):
        path = PAYLOAD_ROOT / "audio" / f"{grade.lower()}-cue.wav"
        data = cue(index)
        audio.append({
            "grade": grade, "path": str(path.relative_to(ASSET_ROOT)),
            "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
            "license": "CC0-1.0",
            "provenance": {"origin": "original", "creator": "MDE contributors", "source_revision": source_revision},
            "channels": 1, "sample_rate_hz": 48_000, "frames": 2_400,
        })
    manifest = {"kind": "mcnf-kiron-asset-manifest", "schema_version": 2, "scenes": scenes, "audio": audio}
    manifest_path = ASSET_ROOT / "manifest-v2.json"
    manifest_path.write_text(json.dumps(manifest, separators=(",", ":")) + "\n", encoding="utf-8")
    manifest_path.chmod(0o644)
    print(f"wrote {len(scenes)} original scenes and {len(audio)} original cues")


if __name__ == "__main__":
    main()
