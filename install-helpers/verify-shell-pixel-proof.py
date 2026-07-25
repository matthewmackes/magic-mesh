#!/usr/bin/env python3
"""Deterministic pixel checks for Construct/Car live KMS PNG proof artifacts.

This verifier does not capture the screen, contact Workloads/libvirt, or infer
operator acceptance from a file existing.  It reads one already-captured PNG,
decodes it with the Python standard library, and checks for the stable pixel
features that make the current shell profile recognizable:

* Construct home: the 24 px top status rail, several shared springboard tile
  plate colours, enough white glyph/text paint, and the bounded floating
  navigation pill rather than a full-width bottom taskbar-shaped bar.
* Car home: the Ford SYNC3 near-black ground, raised dashboard cards, Ford-blue
  accent caps, bottom app strip/card paint, and strong glance text.

Use this after a live `.15` KMS/linear-GBM capture to turn manual pixel
inspection into a repeatable fail-closed check.  It intentionally does not prove
physical pointer input, VDI guest acceptance, MG90 SSH/drive data, or freshness
of a Bus mirror; pair it with the relevant live mirror/input proof when those
claims are needed.
"""

from __future__ import annotations

import argparse
import binascii
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
import tempfile
from typing import Callable, Iterable
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
SYNC3_TEXT_STRONG = (0xFF, 0xFF, 0xFF)
SYNC3_ACCENT = (0x2E, 0x9B, 0xE6)
SYNC3_ACCENT_HI = (0x5F, 0xB8, 0xF2)

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
    for y in range(y0, y1):
        row = y * image.width * 3
        for x in range(x0, x1):
            offset = row + x * 3
            if pred((image.rgb[offset], image.rgb[offset + 1], image.rgb[offset + 2])):
                count += 1
    return count


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


def validate_car_home(image: PngImage) -> dict[str, object]:
    if image.width < 1024 or image.height < 640:
        raise ProofError(f"Car home proof must be at least 1024x640, got {image.width}x{image.height}")
    luma_min, luma_max = _sample_luma_spread(image)
    full = (0, 0, image.width, image.height)
    bottom = (0, int(image.height * 0.70), image.width, image.height)

    bg = _count_where(image, full, _near_any((SYNC3_BG,), 10))
    surface = _count_where(image, full, _near_any((SYNC3_SURFACE, SYNC3_SURFACE_HI), 12))
    accent = _count_where(image, full, _near_any((SYNC3_ACCENT, SYNC3_ACCENT_HI), 14))
    text = _count_where(image, full, _near_any((SYNC3_TEXT_STRONG,), 20))
    bottom_surface = _count_where(
        image, bottom, _near_any((SYNC3_SURFACE, SYNC3_SURFACE_HI, SYNC3_ACCENT), 14)
    )

    total = image.pixels
    bottom_total = _rect_pixels(image, bottom)
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
        "bottom_strip_card_ratio": _require_ratio(
            "bottom app-strip/card paint", bottom_surface, bottom_total, 0.10
        ),
    }
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


def _fixture_car(path: Path, *, blank: bool = False) -> None:
    width, height = 1024, 640
    base = SYNC3_BG if not blank else (0x08, 0x08, 0x08)
    buf = bytearray(bytes(base) * width * height)
    if not blank:
        _fill_rect(buf, width, height, (36, 40, 340, 66), SYNC3_TEXT_STRONG)
        cards = (
            (24, 104, 560, 470),
            (584, 104, 1000, 278),
            (584, 302, 1000, 470),
        )
        for rect in cards:
            _fill_rect(buf, width, height, rect, SYNC3_SURFACE)
            _fill_rect(buf, width, height, (rect[0], rect[1], rect[2], rect[1] + 4), SYNC3_ACCENT)
            _fill_rect(buf, width, height, (rect[0] + 24, rect[3] - 42, rect[2] - 24, rect[3] - 30), SYNC3_TEXT_STRONG)
        x = 24
        for _ in range(6):
            _fill_rect(buf, width, height, (x, 496, x + 152, 616), SYNC3_SURFACE)
            _fill_rect(buf, width, height, (x, 496, x + 152, 500), SYNC3_ACCENT)
            _fill_rect(buf, width, height, (x + 56, 524, x + 96, 556), SYNC3_ACCENT_HI)
            _fill_rect(buf, width, height, (x + 42, 584, x + 110, 592), SYNC3_TEXT_STRONG)
            x += 164
    _write_png(path, width, height, bytes(buf))


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
        construct_taskbar = root / "construct-taskbar.png"
        blank_car = root / "blank-car.png"
        _fixture_construct(construct)
        _fixture_car(car)
        _fixture_construct(construct_taskbar, taskbar=True)
        _fixture_car(blank_car, blank=True)
        validate_construct_home(read_png(construct))
        validate_car_home(read_png(car))
        _expect_fail(lambda: validate_construct_home(read_png(construct_taskbar)), "taskbar-shaped construct fixture")
        _expect_fail(lambda: validate_car_home(read_png(blank_car)), "blank Car fixture")
    print("verify-shell-pixel-proof: self-test passed")


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Validate Construct/Car KMS PNG pixel proof artifacts without Workloads/libvirt probes."
    )
    parser.add_argument(
        "--profile",
        choices=("construct-home", "car-home"),
        help="pixel contract to validate",
    )
    parser.add_argument("--png", type=Path, help="already-captured PNG artifact to validate")
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
        if args.profile == "construct-home":
            metrics = validate_construct_home(image)
        else:
            metrics = validate_car_home(image)
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
