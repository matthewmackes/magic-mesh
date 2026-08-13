#!/usr/bin/env python3
"""Verify the bounded, provenance-bound UX-014 Kiron asset package.

The validator is intentionally independent of the desktop renderer.  It is a
package admission gate: every authored scene must be licensed, size bounded,
content-addressed, and covered by a complete live/pre-rendered/static fallback
ladder before a package can be consumed by a renderer.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import tempfile
import wave

SCHEMA_VERSION = 2
KIND = "mcnf-kiron-asset-manifest"
MAX_MANIFEST_BYTES = 128 * 1024
MAX_ASSET_BYTES = 32 * 1024 * 1024
HASH_CHUNK = 1024 * 1024
GRADES = tuple("ABCDEF")
MODES = ("live-3d", "pre-rendered", "static")
LICENSES = {"Apache-2.0", "CC0-1.0", "CC-BY-4.0", "MIT"}
MAX_AUDIO_SECONDS = 15
SAMPLE_RATES = {44_100, 48_000}


class ManifestError(ValueError):
    pass


def fail(message: str) -> None:
    raise ManifestError(message)


def exact_keys(value: object, keys: set[str], label: str) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != keys:
        fail(f"{label} fields do not match the schema")
    return value


def no_duplicate_json(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            fail(f"manifest contains duplicate JSON field: {key}")
        value[key] = item
    return value


def regular_asset(root: Path, relative: object, label: str) -> tuple[bytes, int]:
    if not isinstance(relative, str) or not relative or "\\" in relative:
        fail(f"{label} path is malformed")
    path = Path(relative)
    if path.is_absolute() or ".." in path.parts or any(part == "" for part in path.parts):
        fail(f"{label} path escapes the package root")
    return read_regular_file(root / path, label, MAX_ASSET_BYTES)


def read_regular_file(path: Path, label: str, maximum: int) -> tuple[bytes, int]:
    """Read one immutable package inode without reopening its mutable path."""
    try:
        before = path.lstat()
        flags = os.O_RDONLY | os.O_CLOEXEC
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        descriptor = os.open(path, flags)
    except OSError as exc:
        fail(f"{label} is unavailable: {exc}")

    try:
        opened = os.fstat(descriptor)
        if (before.st_dev, before.st_ino) != (opened.st_dev, opened.st_ino):
            fail(f"{label} changed identity while it was opened")
        if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(opened.st_mode):
            fail(f"{label} must be a regular non-symlink file")
        if opened.st_nlink != 1:
            fail(f"{label} must have exactly one package link")
        if opened.st_size <= 0 or opened.st_size > maximum:
            fail(f"{label} size is outside the bounded contract")
        if opened.st_mode & 0o022:
            fail(f"{label} must not be writable by group or other")

        with os.fdopen(descriptor, "rb", closefd=False) as handle:
            contents = handle.read(maximum + 1)
        after = os.fstat(descriptor)
        stable_fields = ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns")
        if any(getattr(opened, field) != getattr(after, field) for field in stable_fields):
            fail(f"{label} changed while it was read")
        if len(contents) != opened.st_size:
            fail(f"{label} read did not match its admitted size")
        return contents, opened.st_size
    finally:
        os.close(descriptor)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(HASH_CHUNK):
            digest.update(chunk)
    return digest.hexdigest()


def digest(value: object, label: str) -> str:
    if not isinstance(value, str) or len(value) != 64 or value != value.lower():
        fail(f"{label} is not a lowercase SHA-256 digest")
    try:
        int(value, 16)
    except ValueError:
        fail(f"{label} is not a hexadecimal SHA-256 digest")
    if set(value) == {"0"}:
        fail(f"{label} must not be all zeroes")
    return value


def provenance(value: object, label: str) -> None:
    record = exact_keys(value, {"origin", "creator", "source_revision"}, label)
    if record["origin"] not in {"original", "third-party"}:
        fail(f"{label}.origin is unsupported")
    if not isinstance(record["creator"], str) or not record["creator"].strip():
        fail(f"{label}.creator is required")
    digest(record["source_revision"], f"{label}.source_revision")


def verify_wave(contents: bytes, row: dict[str, object], label: str) -> None:
    import io

    try:
        with wave.open(io.BytesIO(contents), "rb") as source:
            channels = source.getnchannels()
            sample_rate = source.getframerate()
            frames = source.getnframes()
            sample_width = source.getsampwidth()
            compression = source.getcomptype()
    except (EOFError, wave.Error) as exc:
        fail(f"{label} is not a valid WAV file: {exc}")
    if compression != "NONE" or sample_width not in {2, 3}:
        fail(f"{label} must be 16-bit or 24-bit uncompressed PCM")
    if channels not in {1, 2} or sample_rate not in SAMPLE_RATES:
        fail(f"{label} has unsupported channels or sample rate")
    if frames <= 0 or frames > sample_rate * MAX_AUDIO_SECONDS:
        fail(f"{label} duration is outside the bounded contract")
    if (row["channels"], row["sample_rate_hz"], row["frames"]) != (channels, sample_rate, frames):
        fail(f"{label} waveform metadata does not match the file")


def verify_manifest(root: Path, manifest: Path) -> None:
    try:
        manifest_bytes, _ = read_regular_file(manifest, "manifest", MAX_MANIFEST_BYTES)
        value = json.loads(manifest_bytes.decode("utf-8"), object_pairs_hook=no_duplicate_json)
    except (UnicodeError, json.JSONDecodeError) as exc:
        fail(f"manifest is malformed: {exc}")

    document = exact_keys(value, {"kind", "schema_version", "scenes", "audio"}, "manifest")
    if document["kind"] != KIND or document["schema_version"] != SCHEMA_VERSION:
        fail("manifest identity/version is unsupported")
    assets = document["scenes"]
    if not isinstance(assets, list) or len(assets) != len(GRADES) * len(MODES):
        fail("manifest must contain exactly one asset for each grade and fallback mode")

    seen: set[tuple[str, str]] = set()
    for index, raw in enumerate(assets):
        row = exact_keys(raw, {"grade", "mode", "path", "bytes", "sha256", "license", "provenance"}, f"scenes[{index}]")
        grade, mode = row["grade"], row["mode"]
        if grade not in GRADES or mode not in MODES:
            fail(f"assets[{index}] has an unsupported grade or fallback mode")
        identity = (grade, mode)
        if identity in seen:
            fail(f"duplicate asset identity: {grade}/{mode}")
        seen.add(identity)
        if row["license"] not in LICENSES:
            fail(f"assets[{index}] has no approved SPDX license")
        provenance(row["provenance"], f"scenes[{index}].provenance")
        if not isinstance(row["bytes"], int) or isinstance(row["bytes"], bool) or row["bytes"] <= 0 or row["bytes"] > MAX_ASSET_BYTES:
            fail(f"assets[{index}].bytes is outside the bounded contract")
        expected = digest(row["sha256"], f"assets[{index}].sha256")
        contents, size = regular_asset(root, row["path"], f"assets[{index}]")
        if size != row["bytes"]:
            fail(f"assets[{index}] byte count does not match the file")
        if hashlib.sha256(contents).hexdigest() != expected:
            fail(f"assets[{index}] digest does not match the file")

    expected_identities = {(grade, mode) for grade in GRADES for mode in MODES}
    if seen != expected_identities:
        fail("fallback ladder is incomplete: every A-F grade needs live, pre-rendered, and static assets")

    audio = document["audio"]
    if not isinstance(audio, list) or len(audio) != len(GRADES):
        fail("manifest must contain exactly one audio cue for each A-F grade")
    audio_grades: set[str] = set()
    audio_paths: set[object] = set()
    for index, raw in enumerate(audio):
        row = exact_keys(raw, {"grade", "path", "bytes", "sha256", "license", "provenance", "channels", "sample_rate_hz", "frames"}, f"audio[{index}]")
        grade = row["grade"]
        if grade not in GRADES or grade in audio_grades:
            fail(f"audio[{index}] has an unsupported or duplicate grade")
        if row["path"] in audio_paths:
            fail("each grade must have a distinct governed audio path")
        audio_grades.add(grade)
        audio_paths.add(row["path"])
        if row["license"] not in LICENSES:
            fail(f"audio[{index}] has no approved SPDX license")
        provenance(row["provenance"], f"audio[{index}].provenance")
        if not isinstance(row["bytes"], int) or isinstance(row["bytes"], bool) or row["bytes"] <= 0 or row["bytes"] > MAX_ASSET_BYTES:
            fail(f"audio[{index}].bytes is outside the bounded contract")
        expected = digest(row["sha256"], f"audio[{index}].sha256")
        contents, size = regular_asset(root, row["path"], f"audio[{index}]")
        if size != row["bytes"] or hashlib.sha256(contents).hexdigest() != expected:
            fail(f"audio[{index}] identity does not match the file")
        verify_wave(contents, row, f"audio[{index}]")
    if audio_grades != set(GRADES):
        fail("audio coverage is incomplete: every A-F grade needs one cue")


def write_fixture(root: Path, manifest: Path) -> None:
    assets = []
    fixture_revision = hashlib.sha256(b"self-test governed source revision").hexdigest()
    fixture_provenance = {"origin": "original", "creator": "MCNF self-test", "source_revision": fixture_revision}
    for grade in GRADES:
        for mode in MODES:
            relative = f"assets/{grade.lower()}-{mode}.bin"
            target = root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(f"{grade}:{mode}\n".encode())
            os.chmod(target, 0o644)
            assets.append({
                "grade": grade,
                "mode": mode,
                "path": relative,
                "bytes": target.stat().st_size,
                "sha256": sha256(target),
                "license": "CC0-1.0",
                "provenance": fixture_provenance,
            })
    audio = []
    for index, grade in enumerate(GRADES):
        relative = f"audio/{grade.lower()}-cue.wav"
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        with wave.open(str(target), "wb") as output:
            output.setnchannels(1)
            output.setsampwidth(2)
            output.setframerate(48_000)
            output.writeframes(bytes([index + 1, 0]) * 480)
        os.chmod(target, 0o644)
        audio.append({
            "grade": grade, "path": relative, "bytes": target.stat().st_size,
            "sha256": sha256(target), "license": "CC0-1.0",
            "provenance": fixture_provenance, "channels": 1,
            "sample_rate_hz": 48_000, "frames": 480,
        })
    manifest.write_text(json.dumps({"kind": KIND, "schema_version": SCHEMA_VERSION, "scenes": assets, "audio": audio}, separators=(",", ":")), encoding="utf-8")


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="mcnf-kiron-assets-") as directory:
        root = Path(directory)
        manifest = root / "manifest.json"
        write_fixture(root, manifest)
        verify_manifest(root, manifest)

        tampered = json.loads(manifest.read_text(encoding="utf-8"))
        tampered["scenes"][0]["sha256"] = "1" * 64
        manifest.write_text(json.dumps(tampered, separators=(",", ":")), encoding="utf-8")
        try:
            verify_manifest(root, manifest)
        except ManifestError:
            pass
        else:
            fail("self-test accepted a digest-mismatched asset")

        write_fixture(root, manifest)
        incomplete = json.loads(manifest.read_text(encoding="utf-8"))
        incomplete["scenes"] = [row for row in incomplete["scenes"] if row["mode"] != "static" or row["grade"] != "F"]
        manifest.write_text(json.dumps(incomplete, separators=(",", ":")), encoding="utf-8")
        try:
            verify_manifest(root, manifest)
        except ManifestError:
            pass
        else:
            fail("self-test accepted an incomplete static fallback")

        # A package admitted before restart must not let an external alias mutate
        # the grade-F scene bytes while retaining the old governed manifest.
        write_fixture(root, manifest)
        f_static = root / "assets/f-static.bin"
        alias = root / "f-static-restart-alias.bin"
        os.link(f_static, alias)
        try:
            verify_manifest(root, manifest)
        except ManifestError:
            pass
        else:
            fail("self-test accepted a multiply-linked grade-F restart scene")

        write_fixture(root, manifest)
        missing_audio = json.loads(manifest.read_text(encoding="utf-8"))
        missing_audio["audio"].pop()
        manifest.write_text(json.dumps(missing_audio, separators=(",", ":")), encoding="utf-8")
        try:
            verify_manifest(root, manifest)
        except ManifestError:
            pass
        else:
            fail("self-test accepted an incomplete A-F audio package")

        write_fixture(root, manifest)
        false_waveform = json.loads(manifest.read_text(encoding="utf-8"))
        false_waveform["audio"][0]["frames"] += 1
        manifest.write_text(json.dumps(false_waveform, separators=(",", ":")), encoding="utf-8")
        try:
            verify_manifest(root, manifest)
        except ManifestError:
            pass
        else:
            fail("self-test accepted false waveform metadata")
    print("Kiron asset manifest verification self-tests passed")


def main() -> int:
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, help="package root containing the admitted assets")
    parser.add_argument("manifest", nargs="?", type=Path, help="manifest JSON path")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    try:
        if args.self_test:
            if args.root or args.manifest:
                parser.error("--self-test does not accept a manifest")
            self_test()
        elif args.root and args.manifest:
            verify_manifest(args.root, args.manifest)
            print(f"OK: {args.manifest} — six-grade fallback ladder and hashes verified")
        else:
            parser.error("provide --root ROOT MANIFEST or --self-test")
    except (ManifestError, OSError, UnicodeError, TypeError) as exc:
        parser.error(str(exc))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
