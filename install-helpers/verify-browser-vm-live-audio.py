#!/usr/bin/env python3
"""Validate sample-backed Browser VM playback, capture, and recovery evidence.

This validator consumes four bounded PCM WAV captures: playback and capture
before a recorded transport disconnect, then playback and capture after the
recorded reconnect.  Every capture must contain its gate-owned test tone for
the entire sample and is verified from the sample bytes.  Endpoint inventory,
positive counters, claimant-selected tones, and an unbound recovery boolean
are not accepted as substitutes.

Digital samples cannot prove that a physical speaker was audible to a person.
The result therefore keeps physical audibility explicitly operator-confirmed.

Usage:
  verify-browser-vm-live-audio.py validate evidence.json
  verify-browser-vm-live-audio.py --self-test
"""

from __future__ import annotations

import argparse
from array import array
from datetime import datetime, timedelta, timezone
import hashlib
import io
from itertools import groupby
import json
import math
import os
from pathlib import Path
import re
import stat
import sys
import tempfile
from typing import Any, NoReturn
import wave


SCHEMA_VERSION = 1
MAX_MANIFEST_BYTES = 256 * 1024
MAX_WAV_BYTES = 32 * 1024 * 1024
MIN_DURATION_SECONDS = 1.0
MAX_DURATION_SECONDS = 30.0
MIN_RMS_DBFS = -45.0
MIN_ACTIVE_RATIO = 0.01
MIN_TONE_ENERGY_RATIO = 0.20
WINDOW_DURATION_SECONDS = 0.025
MIN_WINDOW_RMS_DBFS = -50.0
MIN_WINDOW_TONE_ENERGY_RATIO = 0.10
MAX_CLIPPED_RATIO = 0.01
MAX_EVIDENCE_AGE = timedelta(hours=24)
MAX_SCENARIO_DURATION = timedelta(minutes=10)
MAX_RECONNECT_DURATION = timedelta(minutes=5)
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
IMAGE_DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
UTC_TIMESTAMP_RE = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"
)

EXPECTED_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "profile",
        "image",
        "source_commit",
        "image_digest",
        "status",
        "source",
        "transport",
        "disconnect_observed_at",
        "reconnect_observed_at",
        "captures",
        "recorded_at",
    }
)
CAPTURE_FIELDS = frozenset(
    {
        "phase",
        "direction",
        "capture_point",
        "path",
        "sha256",
        "captured_at",
        "expected_tone_hz",
    }
)
EXPECTED_CAPTURE_POINTS = {
    "playback": "host-pipewire-browser-vm-playback",
    "capture": "guest-browser-vm-capture-input",
}
EXPECTED_MATRIX = {
    ("before-recovery", "playback"),
    ("before-recovery", "capture"),
    ("after-recovery", "playback"),
    ("after-recovery", "capture"),
}
EXPECTED_TONES = {
    ("before-recovery", "playback"): 523,
    ("before-recovery", "capture"): 719,
    ("after-recovery", "playback"): 977,
    ("after-recovery", "capture"): 1301,
}


class EvidenceError(ValueError):
    """The supplied files cannot support Browser VM live-audio acceptance."""


def fail(message: str) -> NoReturn:
    raise EvidenceError(message)


def reject_json_constant(value: str) -> NoReturn:
    fail(f"non-finite JSON number is not allowed: {value}")


def reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def read_private_regular_file(
    path: Path,
    label: str,
    *,
    maximum_bytes: int,
    minimum_bytes: int = 0,
) -> bytes:
    """Read one bounded evidence file without following a final symlink."""
    try:
        before = path.lstat()
    except OSError as exc:
        fail(f"{label} is not readable: {exc}")
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
        fail(f"{label} must be a regular non-symlink file")
    if before.st_mode & 0o077 or before.st_mode & 0o111:
        fail(f"{label} must be private and non-executable")
    if not minimum_bytes <= before.st_size <= maximum_bytes:
        fail(f"{label} has an invalid bounded size")

    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as exc:
        fail(f"{label} is not readable without following symlinks: {exc}")
    try:
        with os.fdopen(descriptor, "rb") as source:
            opened = os.fstat(source.fileno())
            if not stat.S_ISREG(opened.st_mode):
                fail(f"{label} changed to a non-regular file while opening")
            if (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino):
                fail(f"{label} changed while it was being opened")
            payload = source.read(maximum_bytes + 1)
            after = os.fstat(source.fileno())
    except OSError as exc:
        fail(f"{label} is not readable: {exc}")

    before_signature = (
        before.st_dev,
        before.st_ino,
        before.st_size,
        before.st_mtime_ns,
        before.st_ctime_ns,
    )
    after_signature = (
        after.st_dev,
        after.st_ino,
        after.st_size,
        after.st_mtime_ns,
        after.st_ctime_ns,
    )
    if before_signature != after_signature or len(payload) != before.st_size:
        fail(f"{label} changed while it was being read")
    return payload


def read_manifest(path: Path) -> Any:
    raw = read_private_regular_file(
        path,
        "evidence manifest",
        maximum_bytes=MAX_MANIFEST_BYTES,
    )
    try:
        return json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=reject_duplicate_keys,
            parse_constant=reject_json_constant,
        )
    except (UnicodeError, json.JSONDecodeError) as exc:
        fail(f"evidence manifest is invalid JSON: {exc}")


def require_string(value: Any, field: str, pattern: re.Pattern[str] | None = None) -> str:
    if not isinstance(value, str) or not value:
        fail(f"{field} must be a non-empty string")
    if pattern is not None and pattern.fullmatch(value) is None:
        fail(f"{field} has an invalid format")
    return value


def parse_timestamp(value: Any, field: str) -> datetime:
    text = require_string(value, field, UTC_TIMESTAMP_RE)
    try:
        return datetime.strptime(text, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as exc:
        raise EvidenceError(f"{field} is not a valid UTC timestamp") from exc


def read_artifact(
    root: Path, capture: dict[str, Any], index: int
) -> tuple[Path, bytes]:
    relative = require_string(capture["path"], f"captures[{index}].path")
    candidate = Path(relative)
    if candidate.is_absolute() or ".." in candidate.parts:
        fail(f"captures[{index}].path must stay within the evidence directory")
    unresolved = root / candidate
    try:
        resolved = unresolved.resolve(strict=True)
        resolved.relative_to(root.resolve(strict=True))
    except OSError as exc:
        fail(f"captures[{index}].path is not readable: {exc}")
    except ValueError as exc:
        raise EvidenceError(
            f"captures[{index}].path escapes the evidence directory"
        ) from exc
    artifact = read_private_regular_file(
        unresolved,
        f"captures[{index}].path",
        maximum_bytes=MAX_WAV_BYTES,
        minimum_bytes=45,
    )
    expected_digest = require_string(
        capture["sha256"], f"captures[{index}].sha256", SHA256_RE
    )
    actual_digest = hashlib.sha256(artifact).hexdigest()
    if actual_digest != expected_digest:
        fail(f"captures[{index}].sha256 does not match the WAV artifact")
    return resolved, artifact


def goertzel_energy_ratio(samples: list[float], sample_rate: int, frequency: float) -> float:
    if not samples:
        return 0.0
    omega = 2.0 * math.pi * frequency / sample_rate
    coefficient = 2.0 * math.cos(omega)
    previous = 0.0
    previous_two = 0.0
    total_energy = 0.0
    for sample in samples:
        current = sample + coefficient * previous - previous_two
        previous_two = previous
        previous = current
        total_energy += sample * sample
    if total_energy <= 0.0:
        return 0.0
    power = (
        previous_two * previous_two
        + previous * previous
        - coefficient * previous * previous_two
    )
    return max(0.0, min(1.0, 2.0 * power / (len(samples) * total_energy)))


def analyze_channel(
    samples: list[int], sample_rate: int, expected_tone_hz: int, label: str
) -> dict[str, Any]:
    """Validate one PCM channel without allowing another channel to mask it."""
    floating = [float(sample) for sample in samples]
    mean = sum(floating) / len(floating)
    centered = [sample - mean for sample in floating]
    square_sum = sum(sample * sample for sample in centered)
    rms = math.sqrt(square_sum / len(centered))
    rms_dbfs = 20.0 * math.log10(max(rms / 32768.0, 1e-12))
    peak = max(abs(sample) for sample in centered)
    active_ratio = sum(abs(sample) >= 328.0 for sample in centered) / len(centered)
    clipped_ratio = sum(abs(sample) >= 32760 for sample in samples) / len(samples)
    if rms_dbfs < MIN_RMS_DBFS or active_ratio < MIN_ACTIVE_RATIO:
        fail(f"{label} contains no intentional non-silent sample signal")
    if clipped_ratio > MAX_CLIPPED_RATIO:
        fail(f"{label} is clipped above the allowed ratio")

    dropout_limit = max(1, sample_rate // 20)
    dropout_runs = 0
    for is_quiet, run in groupby(centered, key=lambda sample: abs(sample) <= 16.0):
        if is_quiet and sum(1 for _ in run) >= dropout_limit:
            dropout_runs += 1
    if dropout_runs:
        fail(f"{label} contains {dropout_runs} digital-silence dropout run(s)")

    # The acceptance stimulus is continuous. Checking only a prefix lets a
    # recording with a healthy opening and a later wrong-tone/noisy dropout
    # masquerade as recovery evidence, so verify both the whole artifact and
    # every bounded 25 ms window.
    tone_ratio = goertzel_energy_ratio(centered, sample_rate, expected_tone_hz)
    if tone_ratio < MIN_TONE_ENERGY_RATIO:
        fail(
            f"{label} does not contain the declared {expected_tone_hz} Hz "
            "acceptance stimulus"
        )

    duration = len(samples) / sample_rate
    window_count = max(1, int(duration / WINDOW_DURATION_SECONDS))
    quiet_windows = 0
    wrong_tone_windows = 0
    minimum_window_tone_ratio = 1.0
    for window_index in range(window_count):
        start = window_index * len(floating) // window_count
        end = (window_index + 1) * len(floating) // window_count
        window = floating[start:end]
        window_mean = sum(window) / len(window)
        centered_window = [sample - window_mean for sample in window]
        window_rms = math.sqrt(
            sum(sample * sample for sample in centered_window) / len(centered_window)
        )
        window_rms_dbfs = 20.0 * math.log10(
            max(window_rms / 32768.0, 1e-12)
        )
        if window_rms_dbfs < MIN_WINDOW_RMS_DBFS:
            quiet_windows += 1
            continue
        window_tone_ratio = goertzel_energy_ratio(
            centered_window, sample_rate, expected_tone_hz
        )
        minimum_window_tone_ratio = min(
            minimum_window_tone_ratio, window_tone_ratio
        )
        if window_tone_ratio < MIN_WINDOW_TONE_ENERGY_RATIO:
            wrong_tone_windows += 1
    if quiet_windows:
        fail(
            f"{label} contains {quiet_windows} near-silent acceptance-tone "
            "dropout window(s)"
        )
    if wrong_tone_windows:
        fail(
            f"{label} loses the declared {expected_tone_hz} Hz stimulus in "
            f"{wrong_tone_windows} window(s) (wrong tone or noisy dropout)"
        )

    return {
        "rms_dbfs": round(rms_dbfs, 2),
        "peak_dbfs": round(20.0 * math.log10(max(peak / 32768.0, 1e-12)), 2),
        "active_ratio": round(active_ratio, 6),
        "clipped_ratio": round(clipped_ratio, 6),
        "tone_energy_ratio": round(tone_ratio, 6),
        "tone_windows": window_count,
        "minimum_window_tone_energy_ratio": round(
            minimum_window_tone_ratio, 6
        ),
        "dropouts": 0,
    }


def analyze_wav(artifact: bytes, expected_tone_hz: int, label: str) -> dict[str, Any]:
    try:
        with wave.open(io.BytesIO(artifact), "rb") as source:
            channels = source.getnchannels()
            sample_width = source.getsampwidth()
            sample_rate = source.getframerate()
            frame_count = source.getnframes()
            compression = source.getcomptype()
            frames = source.readframes(frame_count)
    except (OSError, EOFError, wave.Error) as exc:
        fail(f"{label} is not a readable PCM WAV: {exc}")

    if compression != "NONE" or sample_width != 2:
        fail(f"{label} must be uncompressed signed 16-bit PCM WAV")
    if channels not in (1, 2):
        fail(f"{label} must have one or two channels")
    if sample_rate not in (44_100, 48_000):
        fail(f"{label} sample rate must be 44100 or 48000 Hz")
    duration = frame_count / sample_rate if sample_rate else 0.0
    if not MIN_DURATION_SECONDS <= duration <= MAX_DURATION_SECONDS:
        fail(
            f"{label} duration must be between {MIN_DURATION_SECONDS:g} and "
            f"{MAX_DURATION_SECONDS:g} seconds"
        )
    expected_bytes = frame_count * channels * sample_width
    if len(frames) != expected_bytes:
        fail(f"{label} PCM payload is truncated")

    values = array("h")
    values.frombytes(frames)
    if sys.byteorder != "little":
        values.byteswap()
    channel_metrics = []
    for channel in range(channels):
        samples = list(values[channel::channels])
        metrics = analyze_channel(
            samples,
            sample_rate,
            expected_tone_hz,
            f"{label} channel {channel + 1}",
        )
        channel_metrics.append({"channel": channel + 1, **metrics})

    # Report conservative whole-artifact summaries: every quality minimum is
    # the weakest channel, while peak and clipping are the strongest channel.
    return {
        "sample_rate_hz": sample_rate,
        "channels": channels,
        "sample_frames": frame_count,
        "pcm_bytes": len(frames),
        "duration_ms": round(duration * 1000),
        "rms_dbfs": min(item["rms_dbfs"] for item in channel_metrics),
        "peak_dbfs": max(item["peak_dbfs"] for item in channel_metrics),
        "active_ratio": min(item["active_ratio"] for item in channel_metrics),
        "clipped_ratio": max(item["clipped_ratio"] for item in channel_metrics),
        "tone_energy_ratio": min(
            item["tone_energy_ratio"] for item in channel_metrics
        ),
        "tone_windows": min(item["tone_windows"] for item in channel_metrics),
        "minimum_window_tone_energy_ratio": min(
            item["minimum_window_tone_energy_ratio"]
            for item in channel_metrics
        ),
        "dropouts": 0,
        "channel_metrics": channel_metrics,
    }


def validate_document(
    data: Any, artifact_root: Path, *, now: datetime | None = None
) -> dict[str, Any]:
    if not isinstance(data, dict) or frozenset(data) != EXPECTED_FIELDS:
        fail("evidence manifest has missing or unexpected fields")
    if data["schema_version"] != SCHEMA_VERSION or isinstance(data["schema_version"], bool):
        fail("schema_version is invalid")
    if data["kind"] != "browser_vm_live_audio_samples":
        fail("kind is not browser_vm_live_audio_samples")
    if data["profile"] != "browser-vm-chromium" or data["image"] != "browser-vm-chromium":
        fail("evidence is not bound to browser-vm-chromium")
    source_commit = require_string(data["source_commit"], "source_commit", COMMIT_RE)
    image_digest = require_string(data["image_digest"], "image_digest", IMAGE_DIGEST_RE)
    if source_commit == "0" * 40 or image_digest == "sha256:" + "0" * 64:
        fail("provenance must not use null values")
    if data["status"] != "observed" or data["source"] != "live-browser-vm-audio-capture":
        fail("evidence must be an observed live Browser VM audio capture")
    if data["transport"] not in ("rdp", "sunshine"):
        fail("transport must be rdp or sunshine")

    disconnect = parse_timestamp(data["disconnect_observed_at"], "disconnect_observed_at")
    reconnect = parse_timestamp(data["reconnect_observed_at"], "reconnect_observed_at")
    recorded = parse_timestamp(data["recorded_at"], "recorded_at")
    if disconnect >= reconnect:
        fail("disconnect_observed_at must precede reconnect_observed_at")
    if reconnect > recorded:
        fail("reconnect_observed_at must not follow recorded_at")
    if reconnect - disconnect > MAX_RECONNECT_DURATION:
        fail("the observed VDI reconnect exceeds the five-minute evidence window")
    if recorded - disconnect > MAX_SCENARIO_DURATION:
        fail("the VDI reconnect scenario exceeds the ten-minute evidence window")
    current = now or datetime.now(timezone.utc)
    age = current - recorded
    if age < -timedelta(minutes=5) or age > MAX_EVIDENCE_AGE:
        fail("recorded_at is stale or implausibly in the future")

    captures = data["captures"]
    if not isinstance(captures, list) or len(captures) != 4:
        fail("captures must contain exactly playback/capture before and after recovery")
    observed_matrix: set[tuple[str, str]] = set()
    artifact_paths: set[Path] = set()
    artifact_digests: set[str] = set()
    results: list[dict[str, Any]] = []
    playback_samples = 0
    capture_samples = 0
    pcm_bytes = 0

    for index, capture in enumerate(captures):
        if not isinstance(capture, dict) or frozenset(capture) != CAPTURE_FIELDS:
            fail(f"captures[{index}] has missing or unexpected fields")
        phase = capture["phase"]
        direction = capture["direction"]
        key = (phase, direction)
        if key not in EXPECTED_MATRIX or key in observed_matrix:
            fail(f"captures[{index}] is duplicate or has an invalid phase/direction")
        observed_matrix.add(key)
        if capture["capture_point"] != EXPECTED_CAPTURE_POINTS[direction]:
            fail(f"captures[{index}].capture_point does not prove the {direction} path")
        tone = capture["expected_tone_hz"]
        if isinstance(tone, bool) or not isinstance(tone, int) or not 200 <= tone <= 3500:
            fail(f"captures[{index}].expected_tone_hz is outside the acceptance range")
        expected_tone = EXPECTED_TONES[key]
        if tone != expected_tone:
            fail(
                f"captures[{index}].expected_tone_hz must be the gate-owned "
                f"{expected_tone} Hz stimulus for {phase}/{direction}"
            )
        captured_at = parse_timestamp(capture["captured_at"], f"captures[{index}].captured_at")
        if phase == "before-recovery" and captured_at >= disconnect:
            fail(f"captures[{index}] was not recorded before the disconnect")
        if phase == "after-recovery" and captured_at <= reconnect:
            fail(f"captures[{index}] was not recorded after the reconnect")
        if captured_at > recorded:
            fail(f"captures[{index}] follows recorded_at")
        if recorded - captured_at > MAX_SCENARIO_DURATION:
            fail(f"captures[{index}] is outside the ten-minute evidence window")

        path, artifact = read_artifact(artifact_root, capture, index)
        digest = capture["sha256"]
        if path in artifact_paths or digest in artifact_digests:
            fail("each recovery phase and direction requires distinct sample bytes")
        artifact_paths.add(path)
        artifact_digests.add(digest)
        metrics = analyze_wav(artifact, tone, f"captures[{index}]")
        sample_count = metrics["sample_frames"]
        if direction == "playback":
            playback_samples += sample_count
        else:
            capture_samples += sample_count
        pcm_bytes += metrics["pcm_bytes"]
        results.append(
            {
                "phase": phase,
                "direction": direction,
                "capture_point": capture["capture_point"],
                "sha256": digest,
                "expected_tone_hz": tone,
                **metrics,
            }
        )

    if observed_matrix != EXPECTED_MATRIX:
        fail("sample evidence does not cover the required recovery matrix")
    return {
        "status": "validated",
        "evidence_class": "browser_vm_sample_backed_audio",
        "profile": data["profile"],
        "source_commit": source_commit,
        "image_digest": image_digest,
        "transport": data["transport"],
        "claims": {
            "playback": "sample-backed",
            "capture": "sample-backed",
            "recovery": "sample-backed-after-observed-reconnect",
            "scope": "digital-pcm-path-only",
            "physical_audibility": "operator-confirmation-required",
            "production_audio_acceptance": "not-proven-by-this-validator",
        },
        "pcm_bytes": pcm_bytes,
        "playback_samples": playback_samples,
        "capture_samples": capture_samples,
        "dropouts": 0,
        "recovery_observed": True,
        "captures": results,
        "recorded_at": data["recorded_at"],
    }


def write_tone(
    path: Path,
    frequency: int,
    *,
    duration_seconds: int = 1,
    silent: bool = False,
    clipped_frames: int = 0,
    dropout_frames: int = 0,
    dropout_start_frame: int | None = None,
    dropout_fill_hz: int | None = None,
    dropout_noise_amplitude: int = 0,
    channels: int = 1,
    clipped_channel: int | None = None,
    silent_channel: int | None = None,
) -> str:
    sample_rate = 48_000
    frame_count = sample_rate * duration_seconds
    if dropout_start_frame is None:
        dropout_start = (frame_count - dropout_frames) // 2
    else:
        dropout_start = dropout_start_frame
    dropout_end = dropout_start + dropout_frames
    if (
        duration_seconds < 1
        or dropout_start < 0
        or dropout_end > frame_count
        or dropout_noise_amplitude < 0
        or channels not in (1, 2)
        or (clipped_channel is not None and not 0 <= clipped_channel < channels)
        or (silent_channel is not None and not 0 <= silent_channel < channels)
    ):
        raise ValueError("invalid synthetic WAV fixture bounds")
    samples = array("h")
    for frame in range(frame_count):
        in_dropout = dropout_start <= frame < dropout_end
        for channel in range(channels):
            if silent or channel == silent_channel:
                value = 0
            elif in_dropout and dropout_fill_hz is not None:
                value = round(
                    9000
                    * math.sin(
                        2.0 * math.pi * dropout_fill_hz * frame / sample_rate
                    )
                )
            elif in_dropout and dropout_noise_amplitude:
                span = 2 * dropout_noise_amplitude + 1
                value = (
                    (frame * 1_103_515_245 + 12_345) % span
                ) - dropout_noise_amplitude
            elif in_dropout:
                value = 0
            elif frame < clipped_frames and (
                clipped_channel is None or channel == clipped_channel
            ):
                value = 32767
            else:
                value = round(
                    9000
                    * math.sin(2.0 * math.pi * frequency * frame / sample_rate)
                )
            samples.append(value)
    with wave.open(str(path), "wb") as destination:
        destination.setnchannels(channels)
        destination.setsampwidth(2)
        destination.setframerate(sample_rate)
        destination.writeframes(samples.tobytes())
    path.chmod(0o600)
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fixture(root: Path, now: datetime) -> dict[str, Any]:
    matrix = (
        ("before-recovery", "playback", 523, -5),
        ("before-recovery", "capture", 719, -4),
        ("after-recovery", "playback", 977, 2),
        ("after-recovery", "capture", 1301, 3),
    )
    captures = []
    for index, (phase, direction, tone, offset) in enumerate(matrix):
        relative = f"samples/{index}-{phase}-{direction}.wav"
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        digest = write_tone(path, tone)
        captures.append(
            {
                "phase": phase,
                "direction": direction,
                "capture_point": EXPECTED_CAPTURE_POINTS[direction],
                "path": relative,
                "sha256": digest,
                "captured_at": (now + timedelta(seconds=offset)).strftime("%Y-%m-%dT%H:%M:%SZ"),
                "expected_tone_hz": tone,
            }
        )
    return {
        "schema_version": 1,
        "kind": "browser_vm_live_audio_samples",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "source_commit": "0123456789abcdef0123456789abcdef01234567",
        "image_digest": "sha256:" + "a" * 64,
        "status": "observed",
        "source": "live-browser-vm-audio-capture",
        "transport": "rdp",
        "disconnect_observed_at": (now - timedelta(seconds=3)).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "reconnect_observed_at": (now + timedelta(seconds=1)).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "captures": captures,
        "recorded_at": (now + timedelta(seconds=4)).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }


def self_test() -> None:
    positive = 0
    negative = 0

    def expect_rejected(action: Any, needle: str, label: str) -> None:
        nonlocal negative
        try:
            action()
        except EvidenceError as exc:
            assert needle in str(exc), (label, needle, exc)
            negative += 1
        else:
            raise AssertionError(f"accepted invalid audio evidence: {label}")

    now = datetime.now(timezone.utc).replace(microsecond=0)
    with tempfile.TemporaryDirectory(prefix="browser-vm-live-audio-") as temporary:
        root = Path(temporary)
        valid = fixture(root, now)
        manifest_path = root / "audio-evidence.json"
        manifest_path.write_text(json.dumps(valid, sort_keys=True) + "\n", encoding="utf-8")
        manifest_path.chmod(0o600)
        result = validate_document(
            read_manifest(manifest_path), root, now=now + timedelta(seconds=4)
        )
        assert result["claims"]["playback"] == "sample-backed"
        assert result["claims"]["capture"] == "sample-backed"
        assert result["claims"]["scope"] == "digital-pcm-path-only"
        assert result["claims"]["physical_audibility"] == "operator-confirmation-required"
        assert (
            result["claims"]["production_audio_acceptance"]
            == "not-proven-by-this-validator"
        )
        positive += 1

        duplicate_manifest = root / "duplicate-manifest.json"
        duplicate_manifest.write_text(
            '{"schema_version": 1, "schema_version": 1}\n', encoding="utf-8"
        )
        duplicate_manifest.chmod(0o600)
        expect_rejected(
            lambda: read_manifest(duplicate_manifest),
            "duplicate JSON field: schema_version",
            "duplicate manifest field",
        )

        non_finite_manifest = root / "non-finite-manifest.json"
        non_finite_manifest.write_text(
            '{"schema_version": NaN}\n', encoding="utf-8"
        )
        non_finite_manifest.chmod(0o600)
        expect_rejected(
            lambda: read_manifest(non_finite_manifest),
            "non-finite JSON number",
            "non-finite manifest number",
        )

        public_manifest = root / "public-manifest.json"
        public_manifest.write_text(
            json.dumps(valid, sort_keys=True) + "\n", encoding="utf-8"
        )
        public_manifest.chmod(0o644)
        expect_rejected(
            lambda: read_manifest(public_manifest),
            "private and non-executable",
            "public manifest permissions",
        )

        symlink_manifest = root / "symlink-manifest.json"
        symlink_manifest.symlink_to(manifest_path)
        expect_rejected(
            lambda: read_manifest(symlink_manifest),
            "regular non-symlink",
            "symlinked manifest",
        )

        endpoint_only = {
            "schema_version": 1,
            "playback_endpoint_count": 1,
            "capture_endpoint_count": 1,
            "recovery_observed": True,
        }
        try:
            validate_document(endpoint_only, root, now=now)
        except EvidenceError as exc:
            assert "missing or unexpected" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted endpoint-only audio evidence")

        cases: list[tuple[str, Any]] = [
            ("sha256", lambda value: value["captures"][0].update({"sha256": "0" * 64})),
            ("distinct sample bytes", lambda value: value["captures"][3].update({
                "path": value["captures"][0]["path"],
                "sha256": value["captures"][0]["sha256"],
            })),
            ("before the disconnect", lambda value: value["captures"][0].update({
                "captured_at": value["reconnect_observed_at"]
            })),
            ("gate-owned 523 Hz", lambda value: value["captures"][0].update({
                "expected_tone_hz": 524
            })),
            ("ten-minute evidence window", lambda value: value["captures"][0].update({
                "captured_at": (now - timedelta(minutes=11)).strftime(
                    "%Y-%m-%dT%H:%M:%SZ"
                )
            })),
            ("stay within", lambda value: value["captures"][3].update({
                "path": "../audio-evidence.json"
            })),
            ("invalid phase/direction", lambda value: value["captures"][3].update({
                "phase": "before-recovery"
            })),
        ]
        for needle, mutate in cases:
            candidate = json.loads(json.dumps(valid))
            mutate(candidate)
            try:
                validate_document(candidate, root, now=now + timedelta(seconds=4))
            except EvidenceError as exc:
                assert needle in str(exc), (needle, exc)
                negative += 1
            else:
                raise AssertionError(f"accepted invalid audio evidence: {needle}")

        silent = root / "samples/silent.wav"
        silent_digest = write_tone(silent, 1301, silent=True)
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/silent.wav",
                "sha256": silent_digest,
                "expected_tone_hz": 1301,
            }
        )
        try:
            validate_document(candidate, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "non-silent" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted silent audio samples")

        physical_overclaim = json.loads(json.dumps(valid))
        physical_overclaim["physical_audibility"] = "observed"
        try:
            validate_document(physical_overclaim, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "missing or unexpected" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted a claimant-authored physical audibility claim")

        clipped = root / "samples/clipped.wav"
        clipped_digest = write_tone(clipped, 1301, clipped_frames=1_000)
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/clipped.wav",
                "sha256": clipped_digest,
                "expected_tone_hz": 1301,
            }
        )
        try:
            validate_document(candidate, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "clipped" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted clipped audio samples")

        stereo = root / "samples/stereo-valid.wav"
        stereo_digest = write_tone(stereo, 1301, channels=2)
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/stereo-valid.wav",
                "sha256": stereo_digest,
            }
        )
        stereo_result = validate_document(
            candidate, root, now=now + timedelta(seconds=4)
        )
        assert stereo_result["captures"][3]["channels"] == 2
        assert len(stereo_result["captures"][3]["channel_metrics"]) == 2
        positive += 1

        stereo_dead_channel = root / "samples/stereo-dead-channel.wav"
        stereo_dead_channel_digest = write_tone(
            stereo_dead_channel,
            1301,
            channels=2,
            silent_channel=1,
        )
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/stereo-dead-channel.wav",
                "sha256": stereo_dead_channel_digest,
            }
        )
        try:
            validate_document(candidate, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "channel 2 contains no intentional non-silent" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted a dead stereo channel after reconnect")

        stereo_clipped = root / "samples/stereo-one-channel-clipped.wav"
        stereo_clipped_digest = write_tone(
            stereo_clipped,
            1301,
            clipped_frames=1_200,
            channels=2,
            clipped_channel=0,
        )
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/stereo-one-channel-clipped.wav",
                "sha256": stereo_clipped_digest,
            }
        )
        try:
            validate_document(candidate, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "clipped" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted clipping hidden in one stereo channel")

        malformed = root / "samples/malformed.wav"
        malformed.write_bytes(b"not-a-pcm-wave" + bytes(64))
        malformed.chmod(0o600)
        malformed_digest = hashlib.sha256(malformed.read_bytes()).hexdigest()
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/malformed.wav",
                "sha256": malformed_digest,
            }
        )
        try:
            validate_document(candidate, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "readable PCM WAV" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted a malformed WAV artifact")

        executable = root / "samples/executable.wav"
        executable.write_bytes((root / valid["captures"][3]["path"]).read_bytes())
        executable.chmod(0o700)
        executable_digest = hashlib.sha256(executable.read_bytes()).hexdigest()
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/executable.wav",
                "sha256": executable_digest,
            }
        )
        try:
            validate_document(candidate, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "private and non-executable" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted an executable WAV artifact")

        dropout = root / "samples/dropout-50ms.wav"
        dropout_digest = write_tone(
            dropout,
            1301,
            dropout_frames=48_000 // 20,
        )
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/dropout-50ms.wav",
                "sha256": dropout_digest,
                "expected_tone_hz": 1301,
            }
        )
        try:
            validate_document(candidate, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "digital-silence dropout" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted a 50 ms digital-silence dropout")

        noisy_dropout = root / "samples/noisy-dropout-50ms.wav"
        noisy_dropout_digest = write_tone(
            noisy_dropout,
            1301,
            dropout_frames=48_000 // 20,
            dropout_noise_amplitude=32,
        )
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/noisy-dropout-50ms.wav",
                "sha256": noisy_dropout_digest,
            }
        )
        try:
            validate_document(candidate, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "near-silent acceptance-tone dropout" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted a noisy 50 ms acceptance-tone dropout")

        late_wrong = root / "samples/late-wrong-tone.wav"
        late_wrong_digest = write_tone(
            late_wrong,
            1301,
            duration_seconds=3,
            dropout_frames=48_000 // 10,
            dropout_start_frame=48_000 * 5 // 2,
            dropout_fill_hz=2203,
        )
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/late-wrong-tone.wav",
                "sha256": late_wrong_digest,
            }
        )
        try:
            validate_document(candidate, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "wrong tone or noisy dropout" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted a wrong-tone dropout after two seconds")

        wrong = root / "samples/wrong-tone.wav"
        wrong_digest = write_tone(wrong, 1901)
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/wrong-tone.wav",
                "sha256": wrong_digest,
                "expected_tone_hz": 1301,
            }
        )
        try:
            validate_document(candidate, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "declared 1301 Hz" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted samples without the declared stimulus")

        symlink = root / "samples/symlink.wav"
        symlink.symlink_to(root / valid["captures"][3]["path"])
        candidate = json.loads(json.dumps(valid))
        candidate["captures"][3].update(
            {
                "path": "samples/symlink.wav",
                "sha256": valid["captures"][3]["sha256"],
            }
        )
        try:
            validate_document(candidate, root, now=now + timedelta(seconds=4))
        except EvidenceError as exc:
            assert "non-symlink" in str(exc)
            negative += 1
        else:
            raise AssertionError("accepted a symlinked audio artifact")

    print(
        "verify-browser-vm-live-audio: self-test passed "
        f"({positive} positive, {negative} negative cases)"
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", choices=("validate",))
    parser.add_argument("path", nargs="?", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.self_test:
            if args.command is not None or args.path is not None:
                parser.error("--self-test does not accept a command or path")
            self_test()
            return 0
        if args.command != "validate" or args.path is None:
            parser.error("use validate evidence.json or --self-test")
        result = validate_document(read_manifest(args.path), args.path.parent)
        print(json.dumps(result, sort_keys=True))
        return 0
    except EvidenceError as exc:
        print(f"verify-browser-vm-live-audio: rejected: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
