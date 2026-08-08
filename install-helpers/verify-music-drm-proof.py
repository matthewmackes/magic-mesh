#!/usr/bin/env python3
"""Validate one already-captured direct-DRM PNG for Music proof.

This is intentionally an artifact-only verifier.  It does not capture frames,
use SSH, change services, or inspect credentials.  The decoder is kept here
instead of depending on Pillow or another image package so that the proof can
run in the small install-helper environment.

The check is deliberately conservative: a frame must be large enough to be a
real direct-DRM desktop capture, contain meaningful luma variation, and have
more than a small splash/logo-sized foreground region.  A passing result is a
single deterministic JSON object containing the artifact and producer-metadata
hashes plus pixel metrics.  The producer metadata is an integrity/consistency
record, not a signature: a pass is not by itself live hardware proof.
"""

from __future__ import annotations

import argparse
import binascii
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import struct
import sys
import tempfile
import zlib


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

# These limits keep both the compressed artifact and decoded raster bounded.
MAX_PNG_BYTES = 64 * 1024 * 1024
MAX_METADATA_BYTES = 16 * 1024
MAX_DIMENSION = 16_384
MAX_PIXELS = 32 * 1024 * 1024

MIN_WIDTH = 1_280
MIN_HEIGHT = 720
MIN_LUMA_SPREAD = 32
MAX_BLACK_LUMA = 8
MIN_NON_BACKGROUND_RATIO = 0.005
MIN_FOREGROUND_COMPONENTS = 3
FOREGROUND_GRID_SIZE = 8
BACKGROUND_BUCKET_SIZE = 16
BACKGROUND_DISTANCE = 24
PROOF_SOURCE = "direct-drm-egl-readback"
PROOF_METADATA_FIELDS = frozenset({"source", "width", "height", "gbm_format"})
GBM_FORMAT_RE = re.compile(r"DrmFourcc\([A-Za-z0-9_+-]{1,64}\)\Z")


class ProofError(Exception):
    """The PNG cannot support a direct-DRM proof claim."""


class PngImage:
    """A decoded opaque RGB PNG and its source-artifact digest."""

    __slots__ = ("width", "height", "rgb", "sha256")

    def __init__(self, width: int, height: int, rgb: bytes, sha256: str) -> None:
        self.width = width
        self.height = height
        self.rgb = rgb
        self.sha256 = sha256

    @property
    def pixels(self) -> int:
        return self.width * self.height


def _read_bounded_regular_file(path: Path, label: str, maximum: int) -> bytes:
    """Read one bounded regular file without following a symlink."""

    try:
        initial_stat = path.lstat()
    except OSError as exc:
        raise ProofError(f"{label} path is not readable: {path}") from exc
    if stat.S_ISLNK(initial_stat.st_mode) or os.path.islink(path):
        raise ProofError(f"{label} path must not be a symlink")
    if not stat.S_ISREG(initial_stat.st_mode):
        raise ProofError(f"{label} path must be a regular file")
    if initial_stat.st_size > maximum:
        raise ProofError(
            f"{label} exceeds the {maximum} byte size limit: {initial_stat.st_size}"
        )

    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    flags |= getattr(os, "O_NOFOLLOW", 0)
    try:
        file_descriptor = os.open(os.fspath(path), flags)
    except OSError as exc:
        raise ProofError(f"{label} path cannot be opened safely: {path}") from exc
    try:
        opened_stat = os.fstat(file_descriptor)
        if not stat.S_ISREG(opened_stat.st_mode):
            raise ProofError(f"{label} path is not a regular file")
        if opened_stat.st_size > maximum:
            raise ProofError(
                f"{label} exceeds the {maximum} byte size limit: {opened_stat.st_size}"
            )
        data = bytearray()
        while len(data) <= maximum:
            chunk = os.read(file_descriptor, min(1024 * 1024, maximum + 1 - len(data)))
            if not chunk:
                break
            data.extend(chunk)
        if len(data) > maximum:
            raise ProofError(f"{label} exceeds the {maximum} byte size limit")
        return bytes(data)
    except OSError as exc:
        raise ProofError(f"{label} path could not be read: {path}") from exc
    finally:
        os.close(file_descriptor)


def _read_regular_file(path: Path) -> bytes:
    """Read one bounded PNG without following a symlink."""

    return _read_bounded_regular_file(path, "PNG", MAX_PNG_BYTES)


def _check_dimensions(width: int, height: int) -> None:
    if width < MIN_WIDTH or height < MIN_HEIGHT:
        raise ProofError(
            f"PNG dimensions {width}x{height} are below the minimum "
            f"{MIN_WIDTH}x{MIN_HEIGHT}"
        )
    if width > MAX_DIMENSION or height > MAX_DIMENSION:
        raise ProofError(
            f"PNG dimensions {width}x{height} exceed the {MAX_DIMENSION} pixel limit"
        )
    if width * height > MAX_PIXELS:
        raise ProofError(
            f"PNG raster has too many pixels: {width * height} > {MAX_PIXELS}"
        )


def _paeth(left: int, above: int, upper_left: int) -> int:
    estimate = left + above - upper_left
    left_distance = abs(estimate - left)
    above_distance = abs(estimate - above)
    upper_left_distance = abs(estimate - upper_left)
    if left_distance <= above_distance and left_distance <= upper_left_distance:
        return left
    if above_distance <= upper_left_distance:
        return above
    return upper_left


def _decode_scanlines(
    width: int, height: int, color_type: int, compressed: bytes
) -> bytes:
    channels = 3 if color_type == 2 else 4
    stride = width * channels
    expected_length = height * (stride + 1)
    decompressor = zlib.decompressobj()
    try:
        raw = decompressor.decompress(compressed, expected_length + 1)
        if len(raw) > expected_length:
            raise ProofError("PNG decompression exceeds the expected raster length")
        if not decompressor.eof:
            raise ProofError("PNG IDAT stream is truncated")
        if decompressor.unused_data or decompressor.unconsumed_tail:
            raise ProofError("PNG IDAT stream has trailing or unconsumed data")
        raw += decompressor.flush()
    except zlib.error as exc:
        raise ProofError("PNG IDAT zlib stream is invalid") from exc
    if len(raw) != expected_length:
        raise ProofError(
            f"PNG raster length mismatch: got {len(raw)}, expected {expected_length}"
        )

    rgb = bytearray(width * height * 3)
    previous = bytearray(stride)
    source_offset = 0
    destination_offset = 0
    for _row in range(height):
        filter_type = raw[source_offset]
        source_offset += 1
        scanline = bytearray(raw[source_offset : source_offset + stride])
        source_offset += stride
        if filter_type not in (0, 1, 2, 3, 4):
            raise ProofError(f"PNG uses an invalid filter type: {filter_type}")
        if filter_type:
            for index, value in enumerate(scanline):
                left = scanline[index - channels] if index >= channels else 0
                above = previous[index]
                upper_left = previous[index - channels] if index >= channels else 0
                if filter_type == 1:
                    prediction = left
                elif filter_type == 2:
                    prediction = above
                elif filter_type == 3:
                    prediction = (left + above) // 2
                else:
                    prediction = _paeth(left, above, upper_left)
                scanline[index] = (value + prediction) & 0xFF

        for x in range(width):
            source = x * channels
            red = scanline[source]
            green = scanline[source + 1]
            blue = scanline[source + 2]
            if channels == 4 and scanline[source + 3] != 255:
                alpha = scanline[source + 3]
                # Composite against black so transparent colored bytes cannot
                # masquerade as visible proof pixels.
                red = (red * alpha + 127) // 255
                green = (green * alpha + 127) // 255
                blue = (blue * alpha + 127) // 255
            rgb[destination_offset : destination_offset + 3] = bytes(
                (red, green, blue)
            )
            destination_offset += 3
        previous = scanline
    return bytes(rgb)


def read_png(path: Path) -> PngImage:
    """Read and strictly decode one non-interlaced 8-bit RGB/RGBA PNG."""

    data = _read_regular_file(path)
    if not data.startswith(PNG_SIGNATURE):
        raise ProofError("file does not begin with the PNG signature")

    position = len(PNG_SIGNATURE)
    width = height = color_type = None
    idat = bytearray()
    saw_ihdr = False
    saw_idat = False
    idat_finished = False
    saw_iend = False

    while position < len(data):
        if len(data) - position < 8:
            raise ProofError("PNG has a truncated chunk header")
        length = struct.unpack(">I", data[position : position + 4])[0]
        chunk_type = data[position + 4 : position + 8]
        position += 8
        if len(chunk_type) != 4 or not all(
            byte in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
            for byte in chunk_type
        ):
            raise ProofError("PNG contains an invalid chunk type")
        if position + length + 4 > len(data):
            raise ProofError(f"PNG chunk {chunk_type!r} is truncated")
        chunk = data[position : position + length]
        position += length
        expected_crc = struct.unpack(">I", data[position : position + 4])[0]
        position += 4
        actual_crc = binascii.crc32(chunk_type + chunk) & 0xFFFFFFFF
        if actual_crc != expected_crc:
            raise ProofError(f"PNG chunk {chunk_type.decode('ascii')} has an invalid CRC")

        if not saw_ihdr and chunk_type != b"IHDR":
            raise ProofError("PNG must begin with an IHDR chunk")
        if chunk_type == b"IHDR":
            if saw_ihdr or length != 13:
                raise ProofError("PNG has an invalid or duplicate IHDR chunk")
            (
                width,
                height,
                bit_depth,
                color_type,
                compression,
                filter_method,
                interlace,
            ) = struct.unpack(">IIBBBBB", chunk)
            if bit_depth != 8 or color_type not in (2, 6):
                raise ProofError(
                    f"PNG format is not supported: bit_depth={bit_depth} "
                    f"color_type={color_type}"
                )
            if compression != 0 or filter_method != 0 or interlace != 0:
                raise ProofError("PNG uses unsupported compression, filtering, or interlace")
            _check_dimensions(width, height)
            saw_ihdr = True
        elif chunk_type == b"IDAT":
            if idat_finished:
                raise ProofError("PNG IDAT chunks are not contiguous")
            saw_idat = True
            idat.extend(chunk)
        elif chunk_type == b"IEND":
            if length != 0 or not saw_idat:
                raise ProofError("PNG has an invalid IEND position or length")
            saw_iend = True
            break
        else:
            if saw_idat:
                idat_finished = True
            # Unknown ancillary chunks are legal.  Unknown critical chunks are
            # not safe to ignore because they can change pixel interpretation.
            if chunk_type not in (b"PLTE", b"tRNS") and not (chunk_type[0] & 0x20):
                raise ProofError(
                    f"PNG contains an unknown critical chunk {chunk_type.decode('ascii')}"
                )
            if chunk_type == b"PLTE" and (
                length == 0 or length % 3 != 0 or length > 256 * 3
            ):
                raise ProofError("PNG has an invalid PLTE chunk")

    if not saw_iend or position != len(data):
        raise ProofError("PNG is truncated or has trailing data after IEND")
    if width is None or height is None or color_type is None or not idat:
        raise ProofError("PNG is missing required image data")
    rgb = _decode_scanlines(width, height, color_type, bytes(idat))
    return PngImage(width, height, rgb, hashlib.sha256(data).hexdigest())


def _reject_duplicate_json_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ProofError(f"DRM metadata contains duplicate JSON field: {key}")
        result[key] = value
    return result


def _reject_json_constant(value: str) -> object:
    raise ProofError(f"DRM metadata contains non-finite JSON value: {value}")


def _read_drm_metadata(path: Path, image: PngImage) -> dict[str, object]:
    """Validate the runtime's adjacent direct-DRM producer record."""

    metadata_path = path.with_suffix(".json")
    data = _read_bounded_regular_file(
        metadata_path, "DRM metadata", MAX_METADATA_BYTES
    )
    try:
        metadata = json.loads(
            data.decode("utf-8"),
            object_pairs_hook=_reject_duplicate_json_keys,
            parse_constant=_reject_json_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError) as exc:
        raise ProofError(f"DRM metadata is not valid UTF-8 JSON: {exc}") from exc
    if not isinstance(metadata, dict):
        raise ProofError("DRM metadata must be one JSON object")
    if frozenset(metadata) != PROOF_METADATA_FIELDS:
        missing = sorted(PROOF_METADATA_FIELDS - frozenset(metadata))
        unknown = sorted(frozenset(metadata) - PROOF_METADATA_FIELDS)
        details = []
        if missing:
            details.append(f"missing={','.join(missing)}")
        if unknown:
            details.append(f"unknown={','.join(unknown)}")
        raise ProofError(f"DRM metadata fields are not exact ({'; '.join(details)})")

    if metadata["source"] != PROOF_SOURCE:
        raise ProofError(
            f"DRM metadata source is not {PROOF_SOURCE!r}: {metadata['source']!r}"
        )
    for field, expected in (("width", image.width), ("height", image.height)):
        value = metadata[field]
        if type(value) is not int or value != expected:
            raise ProofError(
                f"DRM metadata {field} does not match PNG: {value!r} != {expected}"
            )
    gbm_format = metadata["gbm_format"]
    if not isinstance(gbm_format, str) or GBM_FORMAT_RE.fullmatch(gbm_format) is None:
        raise ProofError("DRM metadata gbm_format is not a bounded DrmFourcc value")

    return {
        "source": PROOF_SOURCE,
        "gbm_format": gbm_format,
        "sha256": hashlib.sha256(data).hexdigest(),
    }


def _luma(red: int, green: int, blue: int) -> int:
    return (red * 299 + green * 587 + blue * 114) // 1000


def _metrics(image: PngImage) -> dict[str, object]:
    """Calculate exact, deterministic metrics over the decoded RGB raster."""

    luma_min = 255
    luma_max = 0
    buckets: dict[tuple[int, int, int], int] = {}
    for offset in range(0, len(image.rgb), 3):
        red = image.rgb[offset]
        green = image.rgb[offset + 1]
        blue = image.rgb[offset + 2]
        pixel_luma = _luma(red, green, blue)
        luma_min = min(luma_min, pixel_luma)
        luma_max = max(luma_max, pixel_luma)
        bucket = (
            red // BACKGROUND_BUCKET_SIZE,
            green // BACKGROUND_BUCKET_SIZE,
            blue // BACKGROUND_BUCKET_SIZE,
        )
        buckets[bucket] = buckets.get(bucket, 0) + 1

    background_bucket = min(
        buckets,
        key=lambda bucket: (-buckets[bucket], bucket),
    )
    background = tuple(
        channel * BACKGROUND_BUCKET_SIZE + BACKGROUND_BUCKET_SIZE // 2
        for channel in background_bucket
    )
    non_background_pixels = 0
    grid_width = (image.width + FOREGROUND_GRID_SIZE - 1) // FOREGROUND_GRID_SIZE
    grid_height = (image.height + FOREGROUND_GRID_SIZE - 1) // FOREGROUND_GRID_SIZE
    foreground_grid = bytearray(grid_width * grid_height)
    for offset in range(0, len(image.rgb), 3):
        if max(
            abs(image.rgb[offset] - background[0]),
            abs(image.rgb[offset + 1] - background[1]),
            abs(image.rgb[offset + 2] - background[2]),
        ) > BACKGROUND_DISTANCE:
            non_background_pixels += 1
            pixel = offset // 3
            grid_x = (pixel % image.width) // FOREGROUND_GRID_SIZE
            grid_y = (pixel // image.width) // FOREGROUND_GRID_SIZE
            foreground_grid[grid_y * grid_width + grid_x] = 1

    foreground_components = 0
    for index, occupied in enumerate(foreground_grid):
        if not occupied:
            continue
        foreground_components += 1
        foreground_grid[index] = 0
        pending = [index]
        while pending:
            current = pending.pop()
            x = current % grid_width
            y = current // grid_width
            for neighbor in (
                current - 1 if x else -1,
                current + 1 if x + 1 < grid_width else -1,
                current - grid_width if y else -1,
                current + grid_width if y + 1 < grid_height else -1,
            ):
                if neighbor >= 0 and foreground_grid[neighbor]:
                    foreground_grid[neighbor] = 0
                    pending.append(neighbor)

    luma_spread = luma_max - luma_min
    non_background_ratio = non_background_pixels / image.pixels
    if luma_max <= MAX_BLACK_LUMA:
        raise ProofError("frame is all-black or near-black")
    if luma_spread < MIN_LUMA_SPREAD:
        raise ProofError(
            f"frame is uniform or splash-like near-uniform: luma spread {luma_spread} "
            f"< {MIN_LUMA_SPREAD}"
        )
    if non_background_ratio < MIN_NON_BACKGROUND_RATIO:
        raise ProofError(
            "frame is splash-like or lacks enough non-background content: "
            f"ratio {non_background_ratio:.6f} < {MIN_NON_BACKGROUND_RATIO:.6f}"
        )
    if foreground_components < MIN_FOREGROUND_COMPONENTS:
        raise ProofError(
            "frame is splash-like or lacks separated Music UI content: "
            f"components {foreground_components} < {MIN_FOREGROUND_COMPONENTS}"
        )

    return {
        "status": "passed",
        "dimensions": f"{image.width}x{image.height}",
        "width": image.width,
        "height": image.height,
        "sha256": image.sha256,
        "luma_min": luma_min,
        "luma_max": luma_max,
        "luma_spread": luma_spread,
        "background_rgb": list(background),
        "non_background_pixels": non_background_pixels,
        "non_background_ratio": round(non_background_ratio, 6),
        "foreground_components": foreground_components,
    }


def validate_png(path: Path) -> dict[str, object]:
    image = read_png(path)
    metadata = _read_drm_metadata(path, image)
    metrics = _metrics(image)
    metrics.update(
        {
            "metadata_sha256": metadata["sha256"],
            "provenance_source": metadata["source"],
            "gbm_format": metadata["gbm_format"],
        }
    )
    return metrics


def _png_chunk(chunk_type: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + chunk_type
        + payload
        + struct.pack(">I", binascii.crc32(chunk_type + payload) & 0xFFFFFFFF)
    )


def _encode_rgb_png(width: int, height: int, rgb: bytes) -> bytes:
    if len(rgb) != width * height * 3:
        raise AssertionError("fixture RGB length mismatch")
    scanlines = bytearray()
    for row in range(height):
        start = row * width * 3
        scanlines.append(0)
        scanlines.extend(rgb[start : start + width * 3])
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return PNG_SIGNATURE + _png_chunk(b"IHDR", ihdr) + _png_chunk(
        b"IDAT", zlib.compress(bytes(scanlines), level=9)
    ) + _png_chunk(b"IEND", b"")


def _fixture_rgb(width: int, height: int, kind: str) -> bytes:
    pixels = bytearray(width * height * 3)
    for y in range(height):
        for x in range(width):
            if kind == "black":
                red, green, blue = 0, 0, 0
            elif kind == "uniform":
                red, green, blue = 40, 40, 40
            elif kind == "near_uniform":
                red, green, blue = 20 + (x % 4), 22 + (y % 4), 24
            elif kind == "splash":
                red, green, blue = 8, 12, 20
                if width // 2 - 100 <= x < width // 2 + 100 and height // 2 - 50 <= y < height // 2 + 50:
                    red, green, blue = 240, 240, 240
            elif kind == "rich":
                if abs(x - width // 2) < 64 or abs(y - height // 2) < 64:
                    red, green, blue = 8, 12, 20
                elif y < height // 2 and x < width // 2:
                    red, green, blue = 12 + (x % 8), 24, 48
                elif y < height // 2:
                    red, green, blue = 180, 52 + (y % 16), 32
                elif x < width // 2:
                    red, green, blue = 24, 132, 76 + (x % 16)
                else:
                    red, green, blue = 218, 188, 48 + (y % 16)
            else:
                raise AssertionError(f"unknown fixture kind: {kind}")
            offset = (y * width + x) * 3
            pixels[offset : offset + 3] = bytes((red, green, blue))
    return bytes(pixels)


def _write_metadata(
    path: Path,
    *,
    width: int,
    height: int,
    source: str = PROOF_SOURCE,
    gbm_format: str = "DrmFourcc(XR30)",
) -> None:
    path.with_suffix(".json").write_text(
        json.dumps(
            {
                "source": source,
                "width": width,
                "height": height,
                "gbm_format": gbm_format,
            },
            separators=(",", ":"),
        )
        + "\n",
        encoding="utf-8",
    )


def _expect_rejected(path: Path, message: str) -> None:
    try:
        validate_png(path)
    except ProofError as exc:
        if message not in str(exc):
            raise AssertionError(f"expected {message!r}, got {exc!s}") from exc
    else:
        raise AssertionError(f"accepted fixture that should contain {message!r}")


def self_test() -> None:
    """Exercise generated valid, malformed, bounded, and visually bad fixtures."""

    with tempfile.TemporaryDirectory(prefix="music-drm-proof-") as temporary:
        root = Path(temporary)
        valid_bytes = _encode_rgb_png(
            1280, 720, _fixture_rgb(1280, 720, "rich")
        )
        valid_path = root / "valid.png"
        valid_path.write_bytes(valid_bytes)
        _write_metadata(valid_path, width=1280, height=720)
        result = validate_png(valid_path)
        assert result["status"] == "passed"
        assert result["dimensions"] == "1280x720"
        assert result["sha256"] == hashlib.sha256(valid_bytes).hexdigest()
        metadata_bytes = valid_path.with_suffix(".json").read_bytes()
        assert result["metadata_sha256"] == hashlib.sha256(metadata_bytes).hexdigest()
        assert result["provenance_source"] == PROOF_SOURCE
        assert result["gbm_format"] == "DrmFourcc(XR30)"
        assert result["luma_spread"] >= MIN_LUMA_SPREAD
        assert result["non_background_ratio"] >= MIN_NON_BACKGROUND_RATIO
        assert result["foreground_components"] >= MIN_FOREGROUND_COMPONENTS
        assert result == validate_png(valid_path)

        small_path = root / "small.png"
        small_path.write_bytes(_encode_rgb_png(1279, 720, _fixture_rgb(1279, 720, "rich")))
        _expect_rejected(small_path, "below the minimum")

        black_path = root / "black.png"
        black_path.write_bytes(_encode_rgb_png(1280, 720, _fixture_rgb(1280, 720, "black")))
        _write_metadata(black_path, width=1280, height=720)
        _expect_rejected(black_path, "all-black")

        uniform_path = root / "uniform.png"
        uniform_path.write_bytes(
            _encode_rgb_png(1280, 720, _fixture_rgb(1280, 720, "uniform"))
        )
        _write_metadata(uniform_path, width=1280, height=720)
        _expect_rejected(uniform_path, "uniform")

        near_uniform_path = root / "near-uniform.png"
        near_uniform_path.write_bytes(
            _encode_rgb_png(1280, 720, _fixture_rgb(1280, 720, "near_uniform"))
        )
        _write_metadata(near_uniform_path, width=1280, height=720)
        _expect_rejected(near_uniform_path, "near-uniform")

        splash_path = root / "splash.png"
        splash_path.write_bytes(
            _encode_rgb_png(1280, 720, _fixture_rgb(1280, 720, "splash"))
        )
        _write_metadata(splash_path, width=1280, height=720)
        _expect_rejected(splash_path, "splash-like")

        missing_metadata_path = root / "missing-metadata.png"
        missing_metadata_path.write_bytes(valid_bytes)
        _expect_rejected(missing_metadata_path, "DRM metadata path is not readable")

        wrong_source_path = root / "wrong-source.png"
        wrong_source_path.write_bytes(valid_bytes)
        _write_metadata(
            wrong_source_path, width=1280, height=720, source="windowed-screenshot"
        )
        _expect_rejected(wrong_source_path, "source is not")

        mismatched_dimensions_path = root / "mismatched-metadata.png"
        mismatched_dimensions_path.write_bytes(valid_bytes)
        _write_metadata(mismatched_dimensions_path, width=1920, height=1080)
        _expect_rejected(mismatched_dimensions_path, "does not match PNG")

        duplicate_metadata_path = root / "duplicate-metadata.png"
        duplicate_metadata_path.write_bytes(valid_bytes)
        duplicate_metadata_path.with_suffix(".json").write_text(
            '{"source":"direct-drm-egl-readback","source":"duplicate",'
            '"width":1280,"height":720,"gbm_format":"DrmFourcc(XR30)"}',
            encoding="utf-8",
        )
        _expect_rejected(duplicate_metadata_path, "duplicate JSON field")

        truncated_path = root / "truncated.png"
        truncated_path.write_bytes(valid_bytes[:-1])
        _expect_rejected(truncated_path, "truncated")

        bad_crc = bytearray(valid_bytes)
        bad_crc[20] ^= 1
        bad_crc_path = root / "bad-crc.png"
        bad_crc_path.write_bytes(bad_crc)
        _expect_rejected(bad_crc_path, "invalid CRC")

        invalid_path = root / "invalid.png"
        invalid_path.write_bytes(b"not a PNG")
        _expect_rejected(invalid_path, "PNG signature")

        oversized_path = root / "oversized.png"
        with oversized_path.open("wb") as oversized_file:
            oversized_file.truncate(MAX_PNG_BYTES + 1)
        _expect_rejected(oversized_path, "size limit")

        symlink_path = root / "symlink.png"
        os.symlink(valid_path, symlink_path)
        _expect_rejected(symlink_path, "symlink")

        symlink_metadata_path = root / "symlink-metadata.png"
        symlink_metadata_path.write_bytes(valid_bytes)
        os.symlink(valid_path.with_suffix(".json"), symlink_metadata_path.with_suffix(".json"))
        _expect_rejected(symlink_metadata_path, "DRM metadata path must not be a symlink")


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Validate an already-captured direct-DRM Music PNG without capture or host access."
    )
    parser.add_argument("png", nargs="?", type=Path, help="PNG artifact to validate")
    parser.add_argument(
        "--self-test", action="store_true", help="run generated-fixture self-tests"
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    if args.self_test:
        if args.png is not None:
            _build_parser().error("--self-test does not accept a PNG path")
        try:
            self_test()
        except (AssertionError, ProofError, OSError) as exc:
            print(f"verify-music-drm-proof: self-test failed: {exc}", file=sys.stderr)
            return 1
        print("verify-music-drm-proof: self-test passed")
        return 0
    if args.png is None:
        _build_parser().error("a PNG path is required unless --self-test is used")
    try:
        print(json.dumps(validate_png(args.png), sort_keys=True, separators=(",", ":")))
        return 0
    except ProofError as exc:
        print(f"verify-music-drm-proof: rejected: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
