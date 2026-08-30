#!/usr/bin/env python3
"""Load one private release-input document and exec the canonical preflight."""

from __future__ import annotations

import json
import os
import re
import stat
import sys
from pathlib import Path


MAX_DOCUMENT_BYTES = 64 * 1024
KIND = "mcnf-release-input-argv"
PREFLIGHT = Path(__file__).resolve().with_name("release-input-preflight.sh")
REVISION_RE = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}\Z")
TOKEN_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+:-]{0,254}\Z")
IMAGE_REFERENCE_RE = re.compile(r"[^@\s]+@sha256:[0-9a-f]{64}\Z")
CANONICAL_BOOTC_ROLE = "all-roles"
LEGACY_BOOTC_ROLES = frozenset({"base", "unified-seat-server"})
RAW_BOOTC_DIGEST_FIELD = "bootc_base_digest"
STALE_CUTTLEFISH_FIELDS = frozenset(
    {
        "cuttlefish_declaration",
        "cuttlefish_signature",
        "cuttlefish_readiness_relay",
        "cuttlefish_vdi_agent",
        "cuttlefish_image_receipt",
        "cuttlefish_image_source_kind",
        "cuttlefish_image_original_source",
        "cuttlefish_image_architecture",
        "cuttlefish_provider_identity",
        "cuttlefish_android_release_id",
        "cuttlefish_image_compatibility_id",
        "cuttlefish_image_media_type",
        "cuttlefish_image_artifact_format",
        "cuttlefish_guest_packages",
    }
)

PATH_FIELDS = {
    "maps_approval",
    "maps_tile_source_root",
    "maps_verifier",
    "maps_mbtiles",
    "app_vm_base_image_receipt",
    "app_vm_catalog_receipt",
    "rpm_signing_identity_receipt",
    "bootc_base_digest_receipt",
}
FILE_FIELDS = PATH_FIELDS - {"maps_tile_source_root"}
SCALAR_FIELDS = {
    "source_revision",
    "source_epoch",
    "maps_quota_bytes",
    *PATH_FIELDS,
    "rpm_signing_identity_receipt",
    "bootc_base_image_reference",
    "bootc_base_architecture",
    "bootc_release_role",
    "app_vm_base_image_reference",
    "app_vm_base_architecture",
}
EXPECTED_FIELDS = {"schema_version", "kind", *SCALAR_FIELDS}

ARGUMENTS = (
    ("source_revision", "--source-revision"),
    ("source_epoch", "--source-epoch"),
    ("maps_approval", "--maps-approval"),
    ("maps_tile_source_root", "--maps-tile-source-root"),
    ("maps_quota_bytes", "--maps-quota-bytes"),
    ("maps_verifier", "--maps-verifier"),
    ("maps_mbtiles", "--maps-mbtiles"),
    ("rpm_signing_identity_receipt", "--rpm-signing-identity-receipt"),
    ("bootc_base_digest_receipt", "--bootc-base-digest-receipt"),
    ("bootc_base_image_reference", "--bootc-base-image-reference"),
    ("bootc_base_architecture", "--bootc-base-architecture"),
    ("bootc_release_role", "--bootc-release-role"),
    ("app_vm_base_image_receipt", "--app-vm-base-image-receipt"),
    ("app_vm_base_image_reference", "--app-vm-base-image-reference"),
    ("app_vm_base_architecture", "--app-vm-base-architecture"),
    ("app_vm_catalog_receipt", "--app-vm-catalog-receipt"),
)


class Refusal(RuntimeError):
    pass


def strict_object(items: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in items:
        if key in result:
            raise Refusal(f"document contains duplicate field: {key}")
        result[key] = value
    return result


def read_private_document(path: Path) -> dict[str, object]:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as exc:
        raise Refusal("private argv file is missing, inaccessible, or symlinked") from exc
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
            raise Refusal("private argv file must be a single-link regular file")
        if before.st_uid != os.getuid():
            raise Refusal("private argv file must be owned by the current uid")
        if stat.S_IMODE(before.st_mode) != 0o400:
            raise Refusal("private argv file mode must be exactly 0400")
        if before.st_size <= 0 or before.st_size > MAX_DOCUMENT_BYTES:
            raise Refusal("private argv file size is outside the allowed bound")
        body = b""
        while len(body) <= MAX_DOCUMENT_BYTES:
            chunk = os.read(descriptor, min(65536, MAX_DOCUMENT_BYTES + 1 - len(body)))
            if not chunk:
                break
            body += chunk
        after = os.fstat(descriptor)
        identity = lambda value: (
            value.st_dev,
            value.st_ino,
            value.st_mode,
            value.st_nlink,
            value.st_uid,
            value.st_gid,
            value.st_size,
            value.st_mtime_ns,
            value.st_ctime_ns,
        )
        if identity(before) != identity(after) or len(body) != before.st_size:
            raise Refusal("private argv file changed while being read")
    finally:
        os.close(descriptor)
    try:
        value = json.loads(body.decode("utf-8"), object_pairs_hook=strict_object)
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise Refusal("private argv file is not strict UTF-8 JSON") from exc
    if not isinstance(value, dict):
        raise Refusal("private argv document must be one JSON object")
    return value


def bounded_string(value: object, label: str) -> str:
    if not isinstance(value, str) or not value or len(value) > 4096:
        raise Refusal(f"{label} must be one non-empty bounded string")
    if any(ord(character) < 0x20 or ord(character) == 0x7F for character in value):
        raise Refusal(f"{label} contains control characters")
    return value


def validate_path(value: object, label: str, directory: bool = False) -> str:
    path_text = bounded_string(value, label)
    path = Path(path_text)
    if not path.is_absolute() or os.path.normpath(path_text) != path_text:
        raise Refusal(f"{label} must be a normalized absolute path")
    try:
        info = path.lstat()
    except OSError as exc:
        raise Refusal(f"{label} is missing or inaccessible") from exc
    if stat.S_ISLNK(info.st_mode):
        raise Refusal(f"{label} must not be a symlink")
    expected = stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode)
    if not expected:
        kind = "directory" if directory else "regular file"
        raise Refusal(f"{label} must be a {kind}")
    return path_text


def validate(document: dict[str, object]) -> dict[str, str | list[str]]:
    if RAW_BOOTC_DIGEST_FIELD in document:
        raise Refusal("bootc raw digest is not a release input; supply bootc_base_digest_receipt")
    stale = sorted(set(document) & STALE_CUTTLEFISH_FIELDS)
    if stale:
        raise Refusal("stale Cuttlefish-bearing production object refuses: " + ",".join(stale))
    if set(document) != EXPECTED_FIELDS:
        missing = sorted(EXPECTED_FIELDS - set(document))
        extra = sorted(set(document) - EXPECTED_FIELDS)
        detail = "missing=" + ",".join(missing) + "; extra=" + ",".join(extra)
        raise Refusal(f"private argv field set is not exact ({detail})")
    if document["schema_version"] != 1 or document["kind"] != KIND:
        raise Refusal("private argv schema identity is unsupported")

    values: dict[str, str | list[str]] = {}
    for field in SCALAR_FIELDS:
        values[field] = bounded_string(document[field], field)
        if "REPLACE_" in str(values[field]):
            raise Refusal(
                f"{field} still contains REPLACE_*; dest-operator leftover is not a release input"
            )
    for field in PATH_FIELDS:
        values[field] = validate_path(document[field], field, field == "maps_tile_source_root")

    revision = str(values["source_revision"])
    if not REVISION_RE.fullmatch(revision) or set(revision) == {"0"}:
        raise Refusal("source_revision is malformed or null")
    if not str(values["source_epoch"]).isdigit() or int(str(values["source_epoch"])) <= 0:
        raise Refusal("source_epoch must be a positive integer string")
    if not str(values["maps_quota_bytes"]).isdigit() or int(str(values["maps_quota_bytes"])) <= 0:
        raise Refusal("maps_quota_bytes must be a positive integer string")
    for field in (
        "bootc_base_architecture",
        "app_vm_base_architecture",
        "bootc_release_role",
    ):
        if not TOKEN_RE.fullmatch(str(values[field])):
            raise Refusal(f"{field} is malformed")

    role = str(values["bootc_release_role"])
    if role in LEGACY_BOOTC_ROLES:
        raise Refusal(f"bootc_release_role refuses legacy {role} identity")
    if role != CANONICAL_BOOTC_ROLE:
        raise Refusal("bootc_release_role must be the canonical all-roles identity")

    for field in ("bootc_base_image_reference", "app_vm_base_image_reference"):
        reference = str(values[field])
        if not IMAGE_REFERENCE_RE.fullmatch(reference):
            raise Refusal(f"{field} must be one digest-pinned image reference")

    return values


def canonical_argv(values: dict[str, str | list[str]]) -> list[str]:
    result = [str(PREFLIGHT)]
    for field, option in ARGUMENTS:
        result.extend((option, str(values[field])))
    return result


def driver_argv(values: dict[str, str | list[str]]) -> list[str]:
    """Return the prepare-driver argv derived from the validated object.

    The driver owns source revision and epoch, so those three argv entries are
    deliberately removed from the canonical preflight invocation.
    """
    result = canonical_argv(values)
    del result[1:5]
    return result


def emit_driver_arguments(values: dict[str, str | list[str]], output: Path) -> None:
    if not output.is_absolute() or os.path.normpath(output) != str(output):
        raise Refusal("derived argument output must be one normalized absolute path")
    if output.exists() or output.is_symlink():
        raise Refusal("derived argument output must be absent")
    parent = output.parent
    if not parent.is_dir() or parent.is_symlink():
        raise Refusal("derived argument output parent must be a real directory")
    parent_info = parent.stat()
    if stat.S_IMODE(parent_info.st_mode) & 0o022:
        raise Refusal("derived argument output parent must not be group/other writable")
    payload = json.dumps(driver_argv(values), separators=(",", ":"), ensure_ascii=True).encode("utf-8")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(output, flags, 0o400)
    except OSError as exc:
        raise Refusal("derived argument output could not be created exclusively") from exc
    try:
        os.write(descriptor, payload)
        os.fchmod(descriptor, 0o400)
    finally:
        os.close(descriptor)


def main() -> int:
    emit = len(sys.argv) == 4 and sys.argv[2] == "--emit-driver-arguments"
    if len(sys.argv) not in {2, 4, 6} or (len(sys.argv) == 4 and not emit):
        print(
            f"usage: {Path(sys.argv[0]).name} PRIVATE_ARGV.json "
            "[--expected-source-revision REV --expected-source-epoch EPOCH]",
            file=sys.stderr,
        )
        return 2
    try:
        values = validate(read_private_document(Path(sys.argv[1])))
        if emit:
            emit_driver_arguments(values, Path(sys.argv[3]))
            return 0
        if len(sys.argv) == 6:
            if sys.argv[2] != "--expected-source-revision" or sys.argv[4] != "--expected-source-epoch":
                raise Refusal("expected source identity options are malformed")
            if values["source_revision"] != sys.argv[3] or values["source_epoch"] != sys.argv[5]:
                raise Refusal("private argv source identity does not match the clean checkout")
        if not PREFLIGHT.is_file() or PREFLIGHT.is_symlink():
            raise Refusal("canonical release-input preflight is missing or substituted")
        os.execv(PREFLIGHT, canonical_argv(values))
    except Refusal as exc:
        print(f"release-input-argv: REFUSED: {exc}", file=sys.stderr)
        return 2
    except OSError as exc:
        print(f"release-input-argv: REFUSED: canonical preflight execution failed: {exc}", file=sys.stderr)
        return 2
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
