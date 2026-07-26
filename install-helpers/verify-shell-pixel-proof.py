#!/usr/bin/env python3
"""Deterministic pixel checks for Construct/Car live KMS PNG proof artifacts.

This verifier does not capture the screen, contact Workloads/libvirt, or infer
operator acceptance from a file existing.  It reads one already-captured PNG,
decodes it with the Python standard library, and checks for the stable pixel
features that make the current shell profile recognizable:

* Construct home: the 24 px top status rail, several shared springboard tile
  plate colours, enough white glyph/text paint, and the bounded floating
  navigation pill rather than a full-width bottom taskbar-shaped bar.
* Car screen: the Ford SYNC3 near-black ground plus a populated left driver
  instrument strip in the reserved Car frame.
* Car home: the Car-screen guard, raised dashboard cards in the right
  workspace, Ford-blue accent caps, a six-slot bottom app strip, and strong
  glance text.

Use this after a live `.15` KMS/linear-GBM capture to turn manual pixel
inspection into a repeatable fail-closed check.  It intentionally does not prove
physical pointer input, VDI guest acceptance, or MG90 SSH/drive data.  Pixel-only
mode also cannot prove Bus freshness; installed Car evidence can require a
same-run `verify-live-mirrors.py --vehicle-node ... --require-online` JSON result
with `--require-car-instrument-freshness`.
"""

from __future__ import annotations

import argparse
import binascii
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import sys
import tempfile
from typing import Any, Callable, Iterable
import zlib


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
MAX_PNG_BYTES = 128 * 1024 * 1024

# Keep these constants synchronized with `crates/shared/mde-egui/src/style.rs`.
STYLE_BG = (0x16, 0x16, 0x1A)
STYLE_SURFACE = (0x1F, 0x1F, 0x25)
STYLE_TEXT = (0xE6, 0xE6, 0xEC)
STYLE_TEXT_STRONG = (0xF4, 0xF4, 0xF4)
STYLE_NAV_BAR_BG = (0x00, 0x00, 0x00)
STYLE_TILE_GLYPH = (0xFF, 0xFF, 0xFF)
SYNC3_BG = (0x04, 0x07, 0x0B)
SYNC3_SURFACE = (0x12, 0x17, 0x1E)
SYNC3_SURFACE_HI = (0x1C, 0x24, 0x2E)
SYNC3_TEXT_DIM = (0xA6, 0xB4, 0xC2)
SYNC3_TEXT_STRONG = (0xFF, 0xFF, 0xFF)
SYNC3_ACCENT = (0x2E, 0x9B, 0xE6)
SYNC3_ACCENT_HI = (0x5F, 0xB8, 0xF2)
CAR_TILE_ACCENTS = (
    (0x42, 0xBE, 0x65),  # ACCENT_MESH, Navigation glyph
    (0xFF, 0x7E, 0xB6),  # ACCENT_MEDIA, Media/Music glyphs
    (0x5B, 0x8C, 0xFF),  # ACCENT, Mesh Teams glyph
    (0x4F, 0xD0, 0x8A),  # OK, Vehicle glyph
    (0xF1, 0xC2, 0x1B),  # ACCENT_SYSTEM, Settings glyph
)

CONSTRUCT_ACCENTS = (
    (0x33, 0xB1, 0xFF),  # ACCENT_COMMS
    (0xA5, 0x6E, 0xFF),  # ACCENT_WORKLOADS
    (0x08, 0xBD, 0xBA),  # ACCENT_TERMINALS
    (0x0B, 0x57, 0xD0),  # ACCENT_WEB
    (0x42, 0xBE, 0x65),  # ACCENT_MESH
    (0xF1, 0xC2, 0x1B),  # ACCENT_SYSTEM
    (0xFF, 0x7E, 0xB6),  # ACCENT_MEDIA
)
TILE_PLATE_ALPHA = 0.38
STATUS_BAR_H = 24
FLOATING_NAV_X = 16
FLOATING_NAV_BOTTOM_MARGIN = 16
FLOATING_NAV_W = 240
FLOATING_NAV_H = 56
CAR_INSTRUMENT_FRACTION = 1.0 / 3.0
CAR_PANEL_PAD = 24
CAR_HEADER_H = 26 + 16  # Style::DISPLAY + Style::SP_M
CAR_GAP = 16
CAR_TOUCH_TARGET = 44
CAR_MIN_STRIP_H = CAR_TOUCH_TARGET + 32  # Density::Touch + Style::SP_XL
CAR_MIN_CARDS_H = CAR_TOUCH_TARGET * 2 + CAR_GAP
CAR_STRIP_TILES = 6
MAX_EVIDENCE_JSON_BYTES = 1024 * 1024
DEFAULT_CAR_MIRROR_MAX_AGE_SECONDS = 120.0
DEFAULT_CAR_EVIDENCE_MAX_SKEW_SECONDS = 120.0


Color = tuple[int, int, int]
Predicate = Callable[[Color], bool]


class ProofError(Exception):
    """A fail-closed pixel proof validation error."""


class PngImage:
    """A small RGB image wrapper backed by row-major RGB bytes."""

    def __init__(self, width: int, height: int, rgb: bytes, sha256: str = "") -> None:
        self.width = width
        self.height = height
        self.rgb = rgb
        self.sha256 = sha256

    @property
    def pixels(self) -> int:
        return self.width * self.height

    def pixel(self, x: int, y: int) -> Color:
        offset = (y * self.width + x) * 3
        return (self.rgb[offset], self.rgb[offset + 1], self.rgb[offset + 2])


def _paeth(left: int, up: int, up_left: int) -> int:
    p = left + up - up_left
    pa = abs(p - left)
    pb = abs(p - up)
    pc = abs(p - up_left)
    if pa <= pb and pa <= pc:
        return left
    if pb <= pc:
        return up
    return up_left


def _read_regular_file(path: Path) -> bytes:
    try:
        stat_result = path.lstat()
    except OSError as exc:
        raise ProofError(f"PNG path is not readable: {path}") from exc
    if os.path.islink(path):
        raise ProofError(f"PNG path must be a regular file, not a symlink: {path}")
    if not path.is_file():
        raise ProofError(f"PNG path must be a regular file: {path}")
    if stat_result.st_size > MAX_PNG_BYTES:
        raise ProofError(
            f"PNG is too large for bounded proof parsing: {stat_result.st_size} bytes"
        )
    return path.read_bytes()


def read_png(path: Path) -> PngImage:
    data = _read_regular_file(path)
    if not data.startswith(PNG_SIGNATURE):
        raise ProofError("not a PNG file")
    pos = len(PNG_SIGNATURE)
    width = height = bit_depth = color_type = interlace = None
    idat = bytearray()
    saw_iend = False
    while pos + 12 <= len(data):
        length = struct.unpack(">I", data[pos : pos + 4])[0]
        chunk_type = data[pos + 4 : pos + 8]
        pos += 8
        if pos + length + 4 > len(data):
            raise ProofError(f"truncated PNG chunk {chunk_type!r}")
        chunk = data[pos : pos + length]
        pos += length
        expected_crc = struct.unpack(">I", data[pos : pos + 4])[0]
        pos += 4
        actual_crc = binascii.crc32(chunk_type + chunk) & 0xFFFFFFFF
        if actual_crc != expected_crc:
            raise ProofError(f"PNG chunk {chunk_type.decode('latin1')} CRC mismatch")
        if chunk_type == b"IHDR":
            if len(chunk) != 13:
                raise ProofError("invalid IHDR length")
            width, height, bit_depth, color_type, compression, filter_method, interlace = (
                struct.unpack(">IIBBBBB", chunk)
            )
            if width <= 0 or height <= 0:
                raise ProofError(f"invalid PNG dimensions: {width}x{height}")
            if bit_depth != 8 or color_type not in {2, 6}:
                raise ProofError(
                    f"unsupported PNG format: bit_depth={bit_depth} color_type={color_type}"
                )
            if compression != 0 or filter_method != 0 or interlace != 0:
                raise ProofError("unsupported PNG compression/filter/interlace method")
        elif chunk_type == b"IDAT":
            idat.extend(chunk)
        elif chunk_type == b"IEND":
            saw_iend = True
            break
    if not saw_iend:
        raise ProofError("PNG has no IEND chunk")
    if width is None or height is None or bit_depth is None or color_type is None:
        raise ProofError("PNG has no IHDR chunk")
    if not idat:
        raise ProofError("PNG has no IDAT data")
    channels = 3 if color_type == 2 else 4
    stride = width * channels
    try:
        raw = zlib.decompress(bytes(idat))
    except zlib.error as exc:
        raise ProofError("PNG IDAT zlib stream is invalid") from exc
    expected = height * (1 + stride)
    if len(raw) != expected:
        raise ProofError(f"PNG raster length mismatch: got {len(raw)}, expected {expected}")

    out = bytearray(width * height * 3)
    prev = bytearray(stride)
    source_pos = 0
    dest_pos = 0
    for _y in range(height):
        filter_type = raw[source_pos]
        source_pos += 1
        scan = bytearray(raw[source_pos : source_pos + stride])
        source_pos += stride
        if filter_type not in {0, 1, 2, 3, 4}:
            raise ProofError(f"unsupported PNG filter type: {filter_type}")
        if filter_type:
            for i, value in enumerate(scan):
                left = scan[i - channels] if i >= channels else 0
                up = prev[i]
                up_left = prev[i - channels] if i >= channels else 0
                if filter_type == 1:
                    recon = left
                elif filter_type == 2:
                    recon = up
                elif filter_type == 3:
                    recon = (left + up) // 2
                else:
                    recon = _paeth(left, up, up_left)
                scan[i] = (value + recon) & 0xFF
        for x in range(width):
            source = x * channels
            out[dest_pos : dest_pos + 3] = scan[source : source + 3]
            dest_pos += 3
        prev = scan
    return PngImage(width, height, bytes(out), hashlib.sha256(data).hexdigest())


def _clamp_rect(image: PngImage, rect: tuple[int, int, int, int]) -> tuple[int, int, int, int]:
    x0, y0, x1, y1 = rect
    x0 = max(0, min(image.width, x0))
    x1 = max(0, min(image.width, x1))
    y0 = max(0, min(image.height, y0))
    y1 = max(0, min(image.height, y1))
    if x1 < x0:
        x0, x1 = x1, x0
    if y1 < y0:
        y0, y1 = y1, y0
    return x0, y0, x1, y1


def _rect_pixels(image: PngImage, rect: tuple[int, int, int, int]) -> int:
    x0, y0, x1, y1 = _clamp_rect(image, rect)
    return (x1 - x0) * (y1 - y0)


def _near(color: Color, target: Color, tolerance: int) -> bool:
    return all(abs(int(channel) - int(want)) <= tolerance for channel, want in zip(color, target))


def _near_any(targets: Iterable[Color], tolerance: int) -> Predicate:
    targets = tuple(targets)
    return lambda color: any(_near(color, target, tolerance) for target in targets)


def _count_where(image: PngImage, rect: tuple[int, int, int, int], pred: Predicate) -> int:
    x0, y0, x1, y1 = _clamp_rect(image, rect)
    count = 0
    rgb = image.rgb
    width = image.width
    for y in range(y0, y1):
        row = y * width * 3
        for x in range(x0, x1):
            offset = row + x * 3
            if pred((rgb[offset], rgb[offset + 1], rgb[offset + 2])):
                count += 1
    return count


def _count_near_any(
    image: PngImage,
    rect: tuple[int, int, int, int],
    targets: Iterable[Color],
    tolerance: int,
) -> int:
    """Count near-colour pixels without per-pixel predicate/tuple allocation."""

    targets = tuple(targets)
    x0, y0, x1, y1 = _clamp_rect(image, rect)
    rgb = image.rgb
    width = image.width
    count = 0
    for y in range(y0, y1):
        row = y * width * 3
        for x in range(x0, x1):
            offset = row + x * 3
            r = rgb[offset]
            g = rgb[offset + 1]
            b = rgb[offset + 2]
            for tr, tg, tb in targets:
                if (
                    abs(r - tr) <= tolerance
                    and abs(g - tg) <= tolerance
                    and abs(b - tb) <= tolerance
                ):
                    count += 1
                    break
    return count


def _count_groups(
    image: PngImage,
    rect: tuple[int, int, int, int],
    groups: dict[str, tuple[Iterable[Color], int]],
) -> dict[str, int]:
    """Count multiple near-colour groups in one bounded rect scan."""

    prepared = [
        (name, tuple(targets), tolerance)
        for name, (targets, tolerance) in groups.items()
    ]
    counts = {name: 0 for name, _targets, _tolerance in prepared}
    x0, y0, x1, y1 = _clamp_rect(image, rect)
    rgb = image.rgb
    width = image.width
    for y in range(y0, y1):
        row = y * width * 3
        for x in range(x0, x1):
            offset = row + x * 3
            r = rgb[offset]
            g = rgb[offset + 1]
            b = rgb[offset + 2]
            for name, targets, tolerance in prepared:
                for tr, tg, tb in targets:
                    if (
                        abs(r - tr) <= tolerance
                        and abs(g - tg) <= tolerance
                        and abs(b - tb) <= tolerance
                    ):
                        counts[name] += 1
                        break
    return counts


def _fraction(numerator: int, denominator: int) -> float:
    if denominator <= 0:
        return 0.0
    return numerator / denominator


def _require_ratio(name: str, numerator: int, denominator: int, minimum: float) -> float:
    ratio = _fraction(numerator, denominator)
    if ratio < minimum:
        raise ProofError(f"{name} ratio too low: {ratio:.4f} < {minimum:.4f}")
    return ratio


def _require_minimum(name: str, value: int, minimum: int) -> None:
    if value < minimum:
        raise ProofError(f"{name} too low: {value} < {minimum}")


def _luma(color: Color) -> int:
    return (color[0] * 299 + color[1] * 587 + color[2] * 114) // 1000


def _sample_luma_spread(image: PngImage) -> tuple[int, int]:
    step = max(1, image.pixels // 50_000)
    lows = 255
    highs = 0
    for pixel_index in range(0, image.pixels, step):
        offset = pixel_index * 3
        lum = _luma((image.rgb[offset], image.rgb[offset + 1], image.rgb[offset + 2]))
        lows = min(lows, lum)
        highs = max(highs, lum)
    if highs - lows < 32:
        raise ProofError(f"image is too flat for visual proof: luma spread {highs - lows}")
    return lows, highs


def _blend(a: Color, b: Color, t: float) -> Color:
    return tuple(round(float(x) * (1.0 - t) + float(y) * t) for x, y in zip(a, b))  # type: ignore[return-value]


def _construct_tile_plate_colors() -> tuple[Color, ...]:
    return tuple(_blend(accent, STYLE_BG, 1.0 - TILE_PLATE_ALPHA) for accent in CONSTRUCT_ACCENTS)


def _rect_i(x0: float, y0: float, x1: float, y1: float) -> tuple[int, int, int, int]:
    return (math.floor(x0), math.floor(y0), math.ceil(x1), math.ceil(y1))


def _inset_rect(
    rect: tuple[int, int, int, int], inset: int
) -> tuple[int, int, int, int]:
    x0, y0, x1, y1 = rect
    return (x0 + inset, y0 + inset, x1 - inset, y1 - inset)


def _car_frame_geometry(width: int, height: int) -> dict[str, object]:
    """Mirror the shell's Car frame: left third instrument strip + right home."""

    instrument_w = max(1, math.floor(width * CAR_INSTRUMENT_FRACTION))
    workspace = (instrument_w, 0, width, height)
    inner = (
        workspace[0] + CAR_PANEL_PAD,
        workspace[1] + CAR_PANEL_PAD,
        workspace[2] - CAR_PANEL_PAD,
        workspace[3] - CAR_PANEL_PAD,
    )
    body = (
        inner[0],
        inner[1] + CAR_HEADER_H,
        inner[2],
        inner[3],
    )
    body_w = body[2] - body[0]
    body_h = body[3] - body[1]
    min_body_width = CAR_TOUCH_TARGET * CAR_STRIP_TILES + CAR_GAP * (CAR_STRIP_TILES - 1)
    min_body_height = CAR_MIN_STRIP_H + CAR_GAP + CAR_MIN_CARDS_H
    if body_w < min_body_width or body_h < min_body_height:
        raise ProofError(
            "Car frame is too small for the instrument strip plus six-slot dashboard "
            f"body: body={body_w}x{body_h}, minimum={min_body_width}x{min_body_height}"
        )

    strip_h = min(max(body_h * 0.26, CAR_MIN_STRIP_H), body_h * 0.45)
    cards_h = max(body_h - strip_h - CAR_GAP, 1.0)
    nav_w = max((body_w - CAR_GAP) * 0.56, 1.0)
    nav_card = _rect_i(body[0], body[1], body[0] + nav_w, body[1] + cards_h)
    right_x = body[0] + nav_w + CAR_GAP
    right_w = max(body[2] - right_x, 1.0)
    half_h = max((cards_h - CAR_GAP) / 2.0, 1.0)
    media_card = _rect_i(right_x, body[1], right_x + right_w, body[1] + half_h)
    glance_card = _rect_i(
        right_x,
        body[1] + half_h + CAR_GAP,
        right_x + right_w,
        body[1] + half_h + CAR_GAP + half_h,
    )
    strip_top = body[3] - strip_h
    tile_w = (body_w - CAR_GAP * (CAR_STRIP_TILES - 1)) / CAR_STRIP_TILES
    strip = tuple(
        _rect_i(
            body[0] + index * (tile_w + CAR_GAP),
            strip_top,
            body[0] + index * (tile_w + CAR_GAP) + tile_w,
            strip_top + strip_h,
        )
        for index in range(CAR_STRIP_TILES)
    )
    return {
        "instrument": (0, 0, instrument_w, height),
        "workspace": workspace,
        "body": body,
        "nav_card": nav_card,
        "media_card": media_card,
        "glance_card": glance_card,
        "strip": strip,
    }


def _validate_car_instrument_strip_pixels(
    image: PngImage, instrument: tuple[int, int, int, int]
) -> dict[str, object]:
    """Require the persistent left Car instrument strip to be visibly populated."""

    instrument_total = _rect_pixels(image, instrument)
    instrument_counts = _count_groups(
        image,
        instrument,
        {
            "bg": ((SYNC3_BG,), 10),
            "surface": ((SYNC3_SURFACE, SYNC3_SURFACE_HI), 12),
            "accent": ((SYNC3_ACCENT, SYNC3_ACCENT_HI), 14),
            "text": ((SYNC3_TEXT_DIM, SYNC3_TEXT_STRONG), 24),
        },
    )
    instrument_bg = instrument_counts["bg"]
    instrument_surface = instrument_counts["surface"]
    instrument_accent = instrument_counts["accent"]
    instrument_text = instrument_counts["text"]
    instrument_strip_bg_ratio = _require_ratio(
        "left driver instrument strip ground", instrument_bg, instrument_total, 0.20
    )
    instrument_strip_surface_ratio = _require_ratio(
        "left driver instrument strip status tiles",
        instrument_surface,
        instrument_total,
        0.03,
    )
    _require_minimum(
        "left driver instrument strip accent pixels",
        instrument_accent,
        max(120, instrument_total // 3500),
    )
    _require_minimum(
        "left driver instrument strip text/readout pixels",
        instrument_text,
        max(300, instrument_total // 1200),
    )
    return {
        "instrument_strip_bg_ratio": instrument_strip_bg_ratio,
        "instrument_strip_surface_ratio": instrument_strip_surface_ratio,
        "instrument_strip_accent_pixels": instrument_accent,
        "instrument_strip_text_pixels": instrument_text,
    }


def validate_car_screen(image: PngImage) -> dict[str, object]:
    """Validate the persistent Car frame that should appear on every Car screen."""

    if image.width < 1024 or image.height < 640:
        raise ProofError(f"Car screen proof must be at least 1024x640, got {image.width}x{image.height}")
    luma_min, luma_max = _sample_luma_spread(image)
    geom = _car_frame_geometry(image.width, image.height)
    full = (0, 0, image.width, image.height)
    instrument = geom["instrument"]  # type: ignore[assignment]
    workspace = geom["workspace"]  # type: ignore[assignment]
    instrument_metrics = _validate_car_instrument_strip_pixels(image, instrument)

    full_counts = _count_groups(
        image,
        full,
        {
            "bg": ((SYNC3_BG,), 10),
            "surface": ((SYNC3_SURFACE, SYNC3_SURFACE_HI), 12),
            "accent": ((SYNC3_ACCENT, SYNC3_ACCENT_HI), 14),
            "text": ((SYNC3_TEXT_DIM, SYNC3_TEXT_STRONG), 24),
        },
    )
    workspace_bg = _count_near_any(image, workspace, (SYNC3_BG,), 10)
    workspace_total = _rect_pixels(image, workspace)
    workspace_paint = max(0, workspace_total - workspace_bg)
    metrics = {
        "profile": "car-screen",
        "width": image.width,
        "height": image.height,
        "sha256": image.sha256,
        "luma_min": luma_min,
        "luma_max": luma_max,
        "sync3_bg_ratio": _require_ratio("SYNC3 ground", full_counts["bg"], image.pixels, 0.10),
        "sync3_card_ratio": _require_ratio("SYNC3 raised card", full_counts["surface"], image.pixels, 0.03),
        "sync3_accent_pixels": full_counts["accent"],
        "sync3_text_pixels": full_counts["text"],
        "right_workspace_paint_ratio": _require_ratio(
            "right Car workspace paint", workspace_paint, workspace_total, 0.01
        ),
    }
    metrics.update(instrument_metrics)
    _require_minimum("Ford-blue accent pixels", full_counts["accent"], max(160, image.pixels // 1200))
    _require_minimum("Car text/glyph pixels", full_counts["text"], max(250, image.pixels // 1200))
    return metrics


def _read_json_object(path: Path, label: str) -> dict[str, Any]:
    try:
        stat_result = path.lstat()
    except OSError as exc:
        raise ProofError(f"{label} path is not readable: {path}") from exc
    if path.is_symlink():
        raise ProofError(f"{label} path must be a regular file, not a symlink: {path}")
    if not path.is_file():
        raise ProofError(f"{label} path must be a regular file: {path}")
    if stat_result.st_size > MAX_EVIDENCE_JSON_BYTES:
        raise ProofError(
            f"{label} JSON is too large: {stat_result.st_size} bytes > {MAX_EVIDENCE_JSON_BYTES}"
        )
    try:
        raw = path.read_bytes()
        data = json.loads(raw)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ProofError(f"{label} JSON cannot be read: {exc}") from exc
    if not isinstance(data, dict):
        raise ProofError(f"{label} JSON must be an object")
    return data


def _integer_ms(value: Any, field: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise ProofError(f"{field} must be an integer millisecond timestamp/count")
    return value


def _vehicle_result_from_live_mirror_report(
    report: dict[str, Any], max_age_ms: int
) -> tuple[int, dict[str, object]]:
    observed_at_ms = _integer_ms(report.get("observed_at_ms"), "observed_at_ms")
    results = report.get("results")
    if not isinstance(results, list):
        raise ProofError("vehicle evidence JSON must contain a results array")
    vehicles = [entry for entry in results if isinstance(entry, dict) and entry.get("kind") == "vehicle"]
    if len(vehicles) != 1:
        raise ProofError(
            "vehicle evidence JSON must contain exactly one verify-live-mirrors vehicle result"
        )
    vehicle = vehicles[0]
    errors = vehicle.get("errors")
    if errors:
        raise ProofError(f"vehicle evidence contains errors: {errors}")
    topic = vehicle.get("topic")
    if not isinstance(topic, str) or not topic.startswith("state/vehicle/"):
        raise ProofError(f"vehicle evidence topic is not state/vehicle/<node>: {topic!r}")
    host = vehicle.get("host")
    if not isinstance(host, str) or not host:
        raise ProofError("vehicle evidence host is missing")
    if topic != f"state/vehicle/{host}":
        raise ProofError(f"vehicle evidence topic/host mismatch: topic={topic!r}, host={host!r}")
    if vehicle.get("fresh") is not True:
        raise ProofError("vehicle evidence is not fresh")
    if vehicle.get("online") is not True:
        raise ProofError("vehicle evidence is not online")
    age_ms = _integer_ms(vehicle.get("age_ms"), "vehicle.age_ms")
    if age_ms > max_age_ms:
        raise ProofError(f"vehicle evidence is stale by policy: {age_ms} ms > {max_age_ms} ms")

    return observed_at_ms, {
        "car_instrument_freshness": "fresh-vehicle-mirror",
        "car_vehicle_report_ok": report.get("ok") is True,
        "car_vehicle_topic": topic,
        "car_vehicle_host": host,
        "car_vehicle_age_ms": age_ms,
        "car_vehicle_online": vehicle.get("online"),
        "car_vehicle_model": vehicle.get("model"),
        "car_vehicle_mgos_version": vehicle.get("mgos_version"),
        "car_vehicle_fix_type": vehicle.get("fix_type"),
        "car_vehicle_satellites": vehicle.get("satellites"),
        "car_vehicle_has_fix": vehicle.get("has_fix"),
    }


def validate_car_instrument_freshness(
    png_path: Path,
    evidence_path: Path | None,
    captured_at_ms: int | None,
    mirror_max_age_seconds: float,
    evidence_max_skew_seconds: float,
) -> dict[str, object]:
    """Tie a Car PNG to same-run fresh vehicle mirror evidence."""

    if evidence_path is None:
        raise ProofError(
            "--require-car-instrument-freshness needs --car-vehicle-evidence-json from "
            "verify-live-mirrors.py"
        )
    if (
        not math.isfinite(mirror_max_age_seconds)
        or not math.isfinite(evidence_max_skew_seconds)
        or mirror_max_age_seconds < 0
        or evidence_max_skew_seconds < 0
    ):
        raise ProofError("Car freshness age/skew windows must be non-negative")
    report = _read_json_object(evidence_path, "vehicle evidence")
    mirror_max_age_ms = int(mirror_max_age_seconds * 1000)
    observed_at_ms, metrics = _vehicle_result_from_live_mirror_report(report, mirror_max_age_ms)

    if captured_at_ms is None:
        try:
            captured_at_ms = round(png_path.stat().st_mtime_ns / 1_000_000)
        except OSError as exc:
            raise ProofError(f"cannot stat PNG capture time: {png_path}") from exc
    else:
        _integer_ms(captured_at_ms, "captured_at_ms")
    skew_ms = abs(captured_at_ms - observed_at_ms)
    max_skew_ms = int(evidence_max_skew_seconds * 1000)
    if skew_ms > max_skew_ms:
        raise ProofError(
            f"PNG capture time and vehicle evidence observation differ by {skew_ms} ms "
            f"(limit {max_skew_ms} ms)"
        )
    metrics.update(
        {
            "car_capture_observed_at_ms": captured_at_ms,
            "car_vehicle_observed_at_ms": observed_at_ms,
            "car_capture_vehicle_evidence_skew_ms": skew_ms,
            "car_vehicle_mirror_max_age_ms": mirror_max_age_ms,
        }
    )
    return metrics


def validate_car_home(image: PngImage) -> dict[str, object]:
    if image.width < 1024 or image.height < 640:
        raise ProofError(f"Car home proof must be at least 1024x640, got {image.width}x{image.height}")
    luma_min, luma_max = _sample_luma_spread(image)
    geom = _car_frame_geometry(image.width, image.height)
    full = (0, 0, image.width, image.height)
    instrument = geom["instrument"]  # type: ignore[assignment]
    nav_card = geom["nav_card"]  # type: ignore[assignment]
    media_card = geom["media_card"]  # type: ignore[assignment]
    glance_card = geom["glance_card"]  # type: ignore[assignment]
    strip = geom["strip"]  # type: ignore[assignment]
    strip_band = (
        min(tile[0] for tile in strip),  # type: ignore[index]
        min(tile[1] for tile in strip),  # type: ignore[index]
        max(tile[2] for tile in strip),  # type: ignore[index]
        max(tile[3] for tile in strip),  # type: ignore[index]
    )

    total = image.pixels
    bottom_total = _rect_pixels(image, strip_band)
    instrument_metrics = _validate_car_instrument_strip_pixels(image, instrument)

    nav_interior = _inset_rect(nav_card, CAR_PANEL_PAD)
    nav_surface = _count_near_any(image, nav_interior, (SYNC3_SURFACE, SYNC3_SURFACE_HI), 12)
    nav_total = _rect_pixels(image, nav_interior)
    media_interior = _inset_rect(media_card, CAR_PANEL_PAD)
    media_surface = _count_near_any(
        image, media_interior, (SYNC3_SURFACE, SYNC3_SURFACE_HI), 12
    )
    media_total = _rect_pixels(image, media_interior)
    glance_interior = _inset_rect(glance_card, CAR_PANEL_PAD)
    glance_surface = _count_near_any(
        image, glance_interior, (SYNC3_SURFACE, SYNC3_SURFACE_HI), 12
    )
    glance_total = _rect_pixels(image, glance_interior)

    nav_cap = (nav_card[0], nav_card[1], nav_card[2], min(nav_card[3], nav_card[1] + 4))
    nav_cap_accent = _count_near_any(image, nav_cap, (SYNC3_ACCENT,), 8)
    nav_cap_total = _rect_pixels(image, nav_cap)

    strip_slot_surfaces = [
        _require_ratio(
            f"Car app-strip slot {idx + 1} surface",
            _count_near_any(image, tile, (SYNC3_SURFACE, SYNC3_SURFACE_HI), 12),
            _rect_pixels(image, tile),
            0.35,
        )
        for idx, tile in enumerate(strip)
    ]
    strip_accent = _count_near_any(image, strip_band, CAR_TILE_ACCENTS, 18)
    bottom_surface = _count_near_any(
        image, strip_band, (SYNC3_SURFACE, SYNC3_SURFACE_HI, SYNC3_ACCENT), 14
    )

    bottom_strip_card_ratio = _require_ratio(
        "bottom app-strip/card paint", bottom_surface, bottom_total, 0.10
    )
    dashboard_nav_card_surface_ratio = _require_ratio(
        "Car Navigation dashboard card surface", nav_surface, nav_total, 0.45
    )
    dashboard_media_card_surface_ratio = _require_ratio(
        "Car Media dashboard card surface", media_surface, media_total, 0.35
    )
    dashboard_glance_card_surface_ratio = _require_ratio(
        "Car Vehicle/Mesh Teams dashboard card surface", glance_surface, glance_total, 0.35
    )
    dashboard_nav_accent_cap_ratio = _require_ratio(
        "Car Navigation Ford-blue card cap", nav_cap_accent, nav_cap_total, 0.55
    )
    _require_minimum("Car app-strip accent/glyph pixels", strip_accent, max(160, total // 2500))

    full_counts = _count_groups(
        image,
        full,
        {
            "bg": ((SYNC3_BG,), 10),
            "surface": ((SYNC3_SURFACE, SYNC3_SURFACE_HI), 12),
            "accent": ((SYNC3_ACCENT, SYNC3_ACCENT_HI), 14),
            "text": ((SYNC3_TEXT_STRONG,), 20),
        },
    )
    bg = full_counts["bg"]
    surface = full_counts["surface"]
    accent = full_counts["accent"]
    text = full_counts["text"]
    metrics = {
        "profile": "car-home",
        "width": image.width,
        "height": image.height,
        "sha256": image.sha256,
        "luma_min": luma_min,
        "luma_max": luma_max,
        "sync3_bg_ratio": _require_ratio("SYNC3 ground", bg, total, 0.10),
        "sync3_card_ratio": _require_ratio("SYNC3 raised card", surface, total, 0.14),
        "sync3_accent_pixels": accent,
        "sync3_text_pixels": text,
        "bottom_strip_card_ratio": bottom_strip_card_ratio,
        "dashboard_nav_card_surface_ratio": dashboard_nav_card_surface_ratio,
        "dashboard_media_card_surface_ratio": dashboard_media_card_surface_ratio,
        "dashboard_glance_card_surface_ratio": dashboard_glance_card_surface_ratio,
        "dashboard_nav_accent_cap_ratio": dashboard_nav_accent_cap_ratio,
        "app_strip_slots": len(strip_slot_surfaces),
        "app_strip_accent_pixels": strip_accent,
    }
    metrics.update(instrument_metrics)
    _require_minimum("Ford-blue accent pixels", accent, max(250, total // 500))
    _require_minimum("strong Car text/glyph pixels", text, max(250, total // 1000))
    return metrics


def validate_construct_home(image: PngImage) -> dict[str, object]:
    if image.width < 1280 or image.height < 720:
        raise ProofError(
            f"Construct home proof must be at least 1280x720, got {image.width}x{image.height}"
        )
    luma_min, luma_max = _sample_luma_spread(image)
    rail_h = min(STATUS_BAR_H, image.height)
    top_rail = (0, 0, image.width, rail_h)
    content = (0, rail_h, image.width, image.height - (FLOATING_NAV_H + FLOATING_NAV_BOTTOM_MARGIN))
    pill = (
        FLOATING_NAV_X,
        image.height - FLOATING_NAV_BOTTOM_MARGIN - FLOATING_NAV_H,
        min(image.width, FLOATING_NAV_X + FLOATING_NAV_W),
        image.height - FLOATING_NAV_BOTTOM_MARGIN,
    )
    bottom_band = (
        0,
        max(0, image.height - (FLOATING_NAV_H + FLOATING_NAV_BOTTOM_MARGIN)),
        image.width,
        image.height,
    )

    top_bg = _count_where(image, top_rail, _near_any((STYLE_BG,), 8))
    plate_colors = _construct_tile_plate_colors()
    tile_plate_total = _count_where(image, content, _near_any(plate_colors, 10))
    tile_families = {
        f"plate_{idx}": _count_where(image, content, _near_any((color,), 10))
        for idx, color in enumerate(plate_colors)
    }
    visible_tile_families = sum(1 for count in tile_families.values() if count >= 120)
    text = _count_where(
        image, (0, rail_h, image.width, image.height), _near_any((STYLE_TEXT, STYLE_TEXT_STRONG, STYLE_TILE_GLYPH), 18)
    )
    pill_black = _count_where(image, pill, _near_any((STYLE_NAV_BAR_BG,), 4))

    wide_black_rows = 0
    x0, y0, x1, y1 = _clamp_rect(image, bottom_band)
    for y in range(y0, y1):
        row_black = _count_where(image, (x0, y, x1, y + 1), _near_any((STYLE_NAV_BAR_BG,), 4))
        if _fraction(row_black, max(1, x1 - x0)) >= 0.60:
            wide_black_rows += 1
    if wide_black_rows >= 24:
        raise ProofError(
            "bottom band has too many full-width black rows; this looks taskbar-shaped, not a floating nav pill"
        )

    top_total = _rect_pixels(image, top_rail)
    content_total = _rect_pixels(image, content)
    pill_total = _rect_pixels(image, pill)
    metrics = {
        "profile": "construct-home",
        "width": image.width,
        "height": image.height,
        "sha256": image.sha256,
        "luma_min": luma_min,
        "luma_max": luma_max,
        "top_status_rail_bg_ratio": _require_ratio("top status rail BG", top_bg, top_total, 0.70),
        "tile_plate_ratio": _require_ratio("Construct tile plate paint", tile_plate_total, content_total, 0.004),
        "tile_plate_families": visible_tile_families,
        "text_glyph_pixels": text,
        "floating_nav_pill_black_ratio": _require_ratio(
            "floating navigation pill black", pill_black, pill_total, 0.45
        ),
        "wide_bottom_black_rows": wide_black_rows,
    }
    _require_minimum("distinct Construct tile plate families", visible_tile_families, 3)
    _require_minimum("Construct text/glyph pixels", text, max(300, image.pixels // 1800))
    return metrics


def _write_png(path: Path, width: int, height: int, rgb: bytes) -> None:
    if len(rgb) != width * height * 3:
        raise AssertionError("fixture RGB length mismatch")

    def chunk(kind: bytes, payload: bytes) -> bytes:
        crc = binascii.crc32(kind + payload) & 0xFFFFFFFF
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", crc)

    rows = bytearray()
    stride = width * 3
    for y in range(height):
        rows.append(0)
        rows.extend(rgb[y * stride : (y + 1) * stride])
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    path.write_bytes(PNG_SIGNATURE + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(bytes(rows))) + chunk(b"IEND", b""))


def _fill_rect(buf: bytearray, width: int, height: int, rect: tuple[int, int, int, int], color: Color) -> None:
    x0, y0, x1, y1 = rect
    x0 = max(0, min(width, x0))
    x1 = max(0, min(width, x1))
    y0 = max(0, min(height, y0))
    y1 = max(0, min(height, y1))
    for y in range(y0, y1):
        row = y * width * 3
        for x in range(x0, x1):
            offset = row + x * 3
            buf[offset : offset + 3] = bytes(color)


def _fixture_construct(path: Path, *, taskbar: bool = False) -> None:
    width, height = 1280, 720
    buf = bytearray(bytes(STYLE_BG) * width * height)
    _fill_rect(buf, width, height, (0, 0, width, STATUS_BAR_H), STYLE_BG)
    plate_colors = _construct_tile_plate_colors()
    tile_w = tile_h = 88
    start_x, start_y = 260, 220
    for index, color in enumerate(plate_colors):
        x = start_x + (index % 4) * 128
        y = start_y + (index // 4) * 136
        _fill_rect(buf, width, height, (x, y, x + tile_w, y + tile_h), color)
        _fill_rect(buf, width, height, (x + 28, y + 28, x + 60, y + 60), STYLE_TILE_GLYPH)
        _fill_rect(buf, width, height, (x + 12, y + tile_h + 10, x + 72, y + tile_h + 14), STYLE_TEXT)
    if taskbar:
        _fill_rect(buf, width, height, (0, height - 56, width, height - 8), STYLE_NAV_BAR_BG)
    else:
        _fill_rect(
            buf,
            width,
            height,
            (
                FLOATING_NAV_X,
                height - FLOATING_NAV_BOTTOM_MARGIN - FLOATING_NAV_H,
                FLOATING_NAV_X + FLOATING_NAV_W,
                height - FLOATING_NAV_BOTTOM_MARGIN,
            ),
            STYLE_NAV_BAR_BG,
        )
        for x in (56, 112, 168):
            _fill_rect(buf, width, height, (x, height - 48, x + 24, height - 24), STYLE_TILE_GLYPH)
    _write_png(path, width, height, bytes(buf))


def _fixture_car(
    path: Path,
    *,
    blank: bool = False,
    missing_instrument: bool = False,
    missing_dashboard: bool = False,
    missing_strip: bool = False,
) -> None:
    width, height = 1024, 640
    base = SYNC3_BG if not blank else (0x08, 0x08, 0x08)
    buf = bytearray(bytes(base) * width * height)
    if not blank:
        geom = _car_frame_geometry(width, height)
        instrument = geom["instrument"]  # type: ignore[assignment]
        if not missing_instrument:
            x0, y0, x1, y1 = instrument
            _fill_rect(buf, width, height, (x0 + 24, y0 + 36, x1 - 24, y0 + 44), SYNC3_ACCENT)
            _fill_rect(
                buf,
                width,
                height,
                (x0 + 96, y0 + 126, x1 - 96, y0 + 188),
                SYNC3_TEXT_DIM,
            )
            _fill_rect(
                buf,
                width,
                height,
                (x0 + 176, y0 + 204, x1 - 176, y0 + 220),
                SYNC3_TEXT_DIM,
            )
            grid_top = y0 + int((y1 - y0) * 0.46)
            tile_gap = 8
            tile_w = max(24, ((x1 - x0) - 48 - tile_gap) // 2)
            tile_h = 58
            for row in range(3):
                for col in range(2):
                    tx = x0 + 24 + col * (tile_w + tile_gap)
                    ty = grid_top + row * (tile_h + tile_gap)
                    rect = (tx, ty, tx + tile_w, ty + tile_h)
                    _fill_rect(buf, width, height, rect, SYNC3_SURFACE)
                    _fill_rect(buf, width, height, (rect[0] + 1, rect[1] + 4, rect[0] + 5, rect[3] - 4), SYNC3_ACCENT)
                    _fill_rect(buf, width, height, (rect[0] + 16, rect[1] + 12, rect[2] - 16, rect[1] + 18), SYNC3_TEXT_DIM)
                    _fill_rect(buf, width, height, (rect[0] + 16, rect[1] + 32, rect[2] - 28, rect[1] + 42), SYNC3_TEXT_STRONG)
        if not missing_dashboard:
            _fill_rect(buf, width, height, (instrument[2] + 24, 24, instrument[2] + 300, 52), SYNC3_TEXT_STRONG)
            cards = (
                geom["nav_card"],
                geom["media_card"],
                geom["glance_card"],
            )
            for rect in cards:
                rect = rect  # type: ignore[assignment]
                _fill_rect(buf, width, height, rect, SYNC3_SURFACE)
                _fill_rect(buf, width, height, (rect[0], rect[1], rect[2], rect[1] + 4), SYNC3_ACCENT)
                _fill_rect(
                    buf,
                    width,
                    height,
                    (rect[0] + 24, rect[3] - 42, rect[2] - 24, rect[3] - 30),
                    SYNC3_TEXT_STRONG,
                )
            if not missing_strip:
                strip = geom["strip"]  # type: ignore[assignment]
                for index, rect in enumerate(strip):
                    accent = CAR_TILE_ACCENTS[index % len(CAR_TILE_ACCENTS)]
                    _fill_rect(buf, width, height, rect, SYNC3_SURFACE)
                    _fill_rect(buf, width, height, (rect[0], rect[1], rect[2], rect[1] + 4), accent)
                    _fill_rect(buf, width, height, (rect[0] + 44, rect[1] + 26, rect[2] - 44, rect[1] + 58), accent)
                    _fill_rect(buf, width, height, (rect[0] + 36, rect[3] - 34, rect[2] - 36, rect[3] - 24), SYNC3_TEXT_STRONG)
    _write_png(path, width, height, bytes(buf))


def _fixture_vehicle_evidence(
    path: Path,
    *,
    observed_at_ms: int,
    age_ms: int = 10_000,
    fresh: bool = True,
    online: bool = True,
    host: str = "test-node",
) -> None:
    payload = {
        "observed_at_ms": observed_at_ms,
        "bus_root": "/run/mde-bus",
        "read_only": True,
        "same_host_required": False,
        "ok": fresh and online,
        "results": [
            {
                "kind": "vehicle",
                "topic": f"state/vehicle/{host}",
                "host": host,
                "age_ms": age_ms,
                "fresh": fresh,
                "online": online,
                "model": "MG90",
                "mgos_version": "4.3.0.1",
                "fix_type": "no-fix",
                "satellites": 0,
                "has_fix": False,
                "errors": [],
            }
        ],
    }
    path.write_text(json.dumps(payload, sort_keys=True), encoding="utf-8")


def _expect_fail(fn: Callable[[], object], label: str) -> None:
    try:
        fn()
    except ProofError:
        return
    raise AssertionError(f"{label} unexpectedly passed")


def run_self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="verify-shell-pixel-proof-") as temp:
        root = Path(temp)
        construct = root / "construct.png"
        car = root / "car.png"
        car_evidence = root / "car-vehicle-evidence.json"
        car_stale_evidence = root / "car-stale-vehicle-evidence.json"
        car_skewed_evidence = root / "car-skewed-vehicle-evidence.json"
        construct_taskbar = root / "construct-taskbar.png"
        blank_car = root / "blank-car.png"
        car_no_instrument = root / "car-no-instrument.png"
        car_no_dashboard = root / "car-no-dashboard.png"
        car_no_strip = root / "car-no-strip.png"
        observed_at_ms = 1_700_000_000_000
        _fixture_construct(construct)
        _fixture_car(car)
        _fixture_vehicle_evidence(car_evidence, observed_at_ms=observed_at_ms)
        _fixture_vehicle_evidence(
            car_stale_evidence, observed_at_ms=observed_at_ms, age_ms=180_000
        )
        _fixture_vehicle_evidence(
            car_skewed_evidence, observed_at_ms=observed_at_ms + 600_000
        )
        _fixture_construct(construct_taskbar, taskbar=True)
        _fixture_car(blank_car, blank=True)
        _fixture_car(car_no_instrument, missing_instrument=True)
        _fixture_car(car_no_dashboard, missing_dashboard=True)
        _fixture_car(car_no_strip, missing_strip=True)
        validate_construct_home(read_png(construct))
        validate_car_screen(read_png(car))
        validate_car_home(read_png(car))
        fresh_metrics = validate_car_instrument_freshness(
            car,
            car_evidence,
            observed_at_ms,
            DEFAULT_CAR_MIRROR_MAX_AGE_SECONDS,
            DEFAULT_CAR_EVIDENCE_MAX_SKEW_SECONDS,
        )
        assert fresh_metrics["car_vehicle_topic"] == "state/vehicle/test-node", fresh_metrics
        assert (
            main(
                [
                    "--profile",
                    "car-screen",
                    "--png",
                    str(car),
                    "--require-car-instrument-freshness",
                    "--car-vehicle-evidence-json",
                    str(car_evidence),
                    "--car-captured-at-ms",
                    str(observed_at_ms),
                ]
            )
            == 0
        )
        _expect_fail(lambda: validate_construct_home(read_png(construct_taskbar)), "taskbar-shaped construct fixture")
        _expect_fail(lambda: validate_car_home(read_png(blank_car)), "blank Car fixture")
        _expect_fail(lambda: validate_car_home(read_png(car_no_instrument)), "Car fixture missing driver strip")
        _expect_fail(lambda: validate_car_screen(read_png(car_no_instrument)), "Car screen missing driver strip")
        _expect_fail(lambda: validate_car_home(read_png(car_no_dashboard)), "Car fixture missing dashboard cards")
        _expect_fail(lambda: validate_car_home(read_png(car_no_strip)), "Car fixture missing six-slot app strip")
        _expect_fail(
            lambda: validate_car_instrument_freshness(
                car,
                None,
                observed_at_ms,
                DEFAULT_CAR_MIRROR_MAX_AGE_SECONDS,
                DEFAULT_CAR_EVIDENCE_MAX_SKEW_SECONDS,
            ),
            "required Car freshness evidence omitted",
        )
        _expect_fail(
            lambda: validate_car_instrument_freshness(
                car,
                car_stale_evidence,
                observed_at_ms,
                DEFAULT_CAR_MIRROR_MAX_AGE_SECONDS,
                DEFAULT_CAR_EVIDENCE_MAX_SKEW_SECONDS,
            ),
            "stale vehicle evidence",
        )
        _expect_fail(
            lambda: validate_car_instrument_freshness(
                car,
                car_skewed_evidence,
                observed_at_ms,
                DEFAULT_CAR_MIRROR_MAX_AGE_SECONDS,
                DEFAULT_CAR_EVIDENCE_MAX_SKEW_SECONDS,
            ),
            "skewed vehicle evidence",
        )
    print("verify-shell-pixel-proof: self-test passed")


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Validate Construct/Car KMS PNG pixel proof artifacts without Workloads/libvirt probes."
    )
    parser.add_argument(
        "--profile",
        choices=("construct-home", "car-screen", "car-home"),
        help="pixel contract to validate",
    )
    parser.add_argument("--png", type=Path, help="already-captured PNG artifact to validate")
    parser.add_argument(
        "--require-car-instrument-freshness",
        action="store_true",
        help=(
            "for Car profiles, require same-run fresh vehicle evidence from "
            "verify-live-mirrors.py"
        ),
    )
    parser.add_argument(
        "--car-vehicle-evidence-json",
        type=Path,
        help=(
            "JSON output from verify-live-mirrors.py --vehicle-node <node> "
            "--require-online"
        ),
    )
    parser.add_argument(
        "--car-captured-at-ms",
        type=int,
        help=(
            "capture timestamp in Unix milliseconds; defaults to the PNG mtime "
            "when Car freshness evidence is required"
        ),
    )
    parser.add_argument(
        "--car-mirror-max-age-seconds",
        type=float,
        default=DEFAULT_CAR_MIRROR_MAX_AGE_SECONDS,
        help=(
            "maximum vehicle mirror age accepted by --require-car-instrument-freshness "
            f"(default: {DEFAULT_CAR_MIRROR_MAX_AGE_SECONDS:g})"
        ),
    )
    parser.add_argument(
        "--car-evidence-max-skew-seconds",
        type=float,
        default=DEFAULT_CAR_EVIDENCE_MAX_SKEW_SECONDS,
        help=(
            "maximum time skew between PNG capture time and live-mirror observation "
            f"(default: {DEFAULT_CAR_EVIDENCE_MAX_SKEW_SECONDS:g})"
        ),
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="emit the passing metric bundle as JSON instead of the concise text report",
    )
    parser.add_argument("--self-test", action="store_true", help="run generated-fixture self-tests")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    try:
        if args.self_test:
            run_self_test()
            return 0
        if not args.profile or not args.png:
            raise ProofError("--profile and --png are required unless --self-test is used")
        image = read_png(args.png)
        car_profile = args.profile in {"car-screen", "car-home"}
        require_car_freshness = args.require_car_instrument_freshness or args.car_vehicle_evidence_json is not None
        if require_car_freshness and not car_profile:
            raise ProofError("Car freshness evidence options require --profile car-screen or car-home")
        if args.car_captured_at_ms is not None and not car_profile:
            raise ProofError("--car-captured-at-ms requires --profile car-screen or car-home")
        if args.profile == "construct-home":
            metrics = validate_construct_home(image)
        elif args.profile == "car-home":
            metrics = validate_car_home(image)
        else:
            metrics = validate_car_screen(image)
        if require_car_freshness:
            metrics.update(
                validate_car_instrument_freshness(
                    args.png,
                    args.car_vehicle_evidence_json,
                    args.car_captured_at_ms,
                    args.car_mirror_max_age_seconds,
                    args.car_evidence_max_skew_seconds,
                )
            )
    except ProofError as exc:
        print(f"verify-shell-pixel-proof: FAIL: {exc}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(metrics, sort_keys=True))
    else:
        print(
            "verify-shell-pixel-proof: OK "
            f"profile={metrics['profile']} size={metrics['width']}x{metrics['height']} "
            f"sha256={metrics['sha256']}"
        )
        for key, value in metrics.items():
            if key in {"profile", "width", "height", "sha256"}:
                continue
            if isinstance(value, float):
                print(f"  {key}={value:.4f}")
            else:
                print(f"  {key}={value}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
