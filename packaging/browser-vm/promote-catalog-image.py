#!/usr/bin/env python3
"""Atomically import one admitted Browser VM pair into a new isolated catalog."""

from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile


TOKEN = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
MAX_MANIFEST_BYTES = 64 * 1024


def die(message: str) -> "NoReturn":
    raise SystemExit(f"promote-catalog-image: {message}")


def regular_nonsymlink(path: Path, label: str) -> os.stat_result:
    try:
        metadata = path.lstat()
    except OSError as error:
        die(f"cannot inspect {label} {path}: {error}")
    if path.is_symlink() or not path.is_file():
        die(f"{label} must be a regular non-symlink file: {path}")
    if metadata.st_mode & 0o022:
        die(f"{label} must not be writable by group or other: {path}")
    return metadata


def reject_symlinked_components(path: Path) -> None:
    current = Path(path.anchor)
    for component in path.parts[1:]:
        current /= component
        if not current.exists():
            return
        if current.is_symlink():
            die(f"path component must not be a symlink: {current}")
        if not current.is_dir():
            die(f"path component must be a directory: {current}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def same_file_version(left: os.stat_result, right: os.stat_result) -> bool:
    return (
        left.st_dev,
        left.st_ino,
        left.st_mode,
        left.st_size,
        left.st_mtime_ns,
        left.st_ctime_ns,
    ) == (
        right.st_dev,
        right.st_ino,
        right.st_mode,
        right.st_size,
        right.st_mtime_ns,
        right.st_ctime_ns,
    )


def copy_admitted(
    source: Path,
    destination: Path,
    label: str,
    admitted: os.stat_result,
    expected_digest: str,
) -> None:
    """Copy admitted bytes without retaining authority through a source inode."""
    source_descriptor = os.open(source, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    try:
        opened = os.fstat(source_descriptor)
        if not stat.S_ISREG(opened.st_mode) or not same_file_version(opened, admitted):
            die(f"{label} identity changed before isolated catalog copy")

        destination_descriptor = os.open(
            destination,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW,
            0o644,
        )
        digest = hashlib.sha256()
        try:
            while chunk := os.read(source_descriptor, 1024 * 1024):
                digest.update(chunk)
                view = memoryview(chunk)
                while view:
                    written = os.write(destination_descriptor, view)
                    if written == 0:
                        raise OSError(errno.EIO, "short catalog write", destination)
                    view = view[written:]
            os.fsync(destination_descriptor)
            copied = os.fstat(destination_descriptor)
        except BaseException:
            os.close(destination_descriptor)
            destination.unlink(missing_ok=True)
            raise
        else:
            os.close(destination_descriptor)

        finished = os.fstat(source_descriptor)
        if not same_file_version(opened, finished):
            destination.unlink(missing_ok=True)
            die(f"{label} changed during isolated catalog copy")
        if copied.st_ino == opened.st_ino and copied.st_dev == opened.st_dev:
            destination.unlink(missing_ok=True)
            die(f"{label} catalog copy retained the source inode")
        if copied.st_nlink != 1 or not stat.S_ISREG(copied.st_mode):
            destination.unlink(missing_ok=True)
            die(f"{label} catalog copy is not an isolated regular file")
        if digest.hexdigest() != expected_digest:
            destination.unlink(missing_ok=True)
            die(f"{label} bytes changed during isolated catalog copy")
    finally:
        os.close(source_descriptor)


def write_synced(path: Path, body: bytes, mode: int = 0o644) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    with os.fdopen(descriptor, "wb") as output:
        output.write(body)
        output.flush()
        os.fsync(output.fileno())


def fsync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def rename_noreplace(source: Path, destination: Path) -> None:
    """Publish a staged catalog without a check-then-replace race."""
    libc = ctypes.CDLL(None, use_errno=True)
    renameat2 = libc.renameat2
    renameat2.argtypes = [
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_uint,
    ]
    renameat2.restype = ctypes.c_int
    if renameat2(
        -100,
        os.fsencode(source),
        -100,
        os.fsencode(destination),
        1,
    ) == 0:
        return
    error = ctypes.get_errno()
    if error == errno.EEXIST:
        die(f"refusing to replace an existing catalog root: {destination}")
    raise OSError(error, os.strerror(error), destination)


def checked_json(path: Path) -> dict[str, object]:
    metadata = regular_nonsymlink(path, "identity manifest")
    if metadata.st_size > MAX_MANIFEST_BYTES:
        die(f"identity manifest exceeds {MAX_MANIFEST_BYTES} bytes")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        die(f"identity manifest is unreadable: {error}")
    if not isinstance(value, dict):
        die("identity manifest root must be an object")
    return value


def hostile_regression_source_alias_cannot_mutate_promoted_guest() -> None:
    with tempfile.TemporaryDirectory(prefix="browser-vm-promotion-alias-") as raw:
        root = Path(raw)
        source = root / "browser-vm.qcow2"
        promoted = root / "catalog" / "browser-vm.qcow2"
        promoted.parent.mkdir()
        admitted_bytes = b"admitted-browser-vm-generation\n"
        source.write_bytes(admitted_bytes)
        source.chmod(0o644)
        admitted = regular_nonsymlink(source, "hostile fixture image")
        copy_admitted(
            source,
            promoted,
            "image",
            admitted,
            hashlib.sha256(admitted_bytes).hexdigest(),
        )

        source.write_bytes(b"replacement-browser-vm-generation\n")
        if promoted.read_bytes() != admitted_bytes:
            raise AssertionError("source alias changed the promoted Browser VM generation")
        if source.stat().st_ino == promoted.stat().st_ino:
            raise AssertionError("promotion retained source inode authority")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Promote an exact Browser VM artifact+manifest into a new isolated catalog"
    )
    parser.add_argument("--catalog-root", type=Path)
    parser.add_argument("--image", type=Path)
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--name", default="browser-vm-chromium")
    parser.add_argument("--version")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        if any((args.catalog_root, args.image, args.manifest, args.version)):
            die("--self-test does not accept promotion arguments")
        hostile_regression_source_alias_cannot_mutate_promoted_guest()
        print("Browser VM catalog source-alias hostile regression passed")
        return 0
    if any(value is None for value in (args.catalog_root, args.image, args.manifest, args.version)):
        die("--catalog-root, --image, --manifest, and --version are required")

    if not TOKEN.fullmatch(args.name):
        die("name must be a bounded lowercase catalog token")
    if not TOKEN.fullmatch(args.version):
        die("version must be a bounded lowercase catalog token")
    for value, label in ((args.catalog_root, "catalog root"), (args.image, "image"), (args.manifest, "manifest")):
        if not value.is_absolute():
            die(f"{label} must be an absolute path")

    catalog_root = args.catalog_root
    reject_symlinked_components(catalog_root.parent)
    reject_symlinked_components(args.image.parent)
    reject_symlinked_components(args.manifest.parent)
    if not catalog_root.parent.is_dir():
        die(f"catalog parent does not exist: {catalog_root.parent}")
    if catalog_root.exists() or catalog_root.is_symlink():
        die(f"refusing to replace an existing catalog root: {catalog_root}")

    image_before = regular_nonsymlink(args.image, "image")
    manifest_before = regular_nonsymlink(args.manifest, "identity manifest")
    manifest = checked_json(args.manifest)

    script_dir = Path(__file__).resolve().parent
    verifier = script_dir / "verify-image.sh"
    verification = subprocess.run(
        [str(verifier), "--artifact", str(args.image), str(args.manifest)],
        check=False,
    )
    if verification.returncode != 0:
        die("artifact identity verification failed; catalog remains unchanged")

    artifact = manifest.get("artifact")
    profile = manifest.get("profile")
    if not isinstance(artifact, dict) or not isinstance(profile, dict):
        die("identity manifest omits artifact/profile objects")
    manifest_digest = artifact.get("sha256")
    manifest_bytes = artifact.get("bytes")
    profile_id = profile.get("id")
    if not isinstance(manifest_digest, str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", manifest_digest):
        die("identity manifest artifact digest is malformed")
    if manifest_bytes != image_before.st_size:
        die("identity manifest artifact byte count changed before promotion")
    if profile_id != "browser-vm-chromium":
        die("identity manifest is not the Browser VM profile")

    image_digest = sha256(args.image)
    if f"sha256:{image_digest}" != manifest_digest:
        die("image digest does not match the admitted identity manifest")
    identity_digest = sha256(args.manifest)

    image_info = json.loads(
        subprocess.run(
            ["qemu-img", "info", "--output=json", "--", str(args.image)],
            check=True,
            stdout=subprocess.PIPE,
            text=True,
        ).stdout
    )
    if image_info.get("format") != "qcow2" or image_info.get("virtual-size") != 64 * 1024**3:
        die("admitted image is not an exact 64-GiB qcow2")
    subprocess.run(["qemu-img", "check", "--", str(args.image)], check=True, stdout=subprocess.DEVNULL)

    stage = Path(tempfile.mkdtemp(prefix=f".{catalog_root.name}.stage.", dir=catalog_root.parent))
    try:
        stage.chmod(0o755)
        version_dir = stage / "images" / args.name / args.version
        version_dir.mkdir(parents=True, mode=0o755)

        # Copy the admitted source pair onto catalog-owned inodes. The source
        # may remain writable by its owner after promotion, so retaining its
        # inode here would let that external authority mutate the guest image.
        # The resolver alias may share the catalog-owned image inode.
        preserved_image = version_dir / args.image.name
        preserved_manifest = version_dir / args.manifest.name
        workload_image = version_dir / f"{args.name}.img"
        copy_admitted(args.image, preserved_image, "image", image_before, image_digest)
        copy_admitted(
            args.manifest,
            preserved_manifest,
            "identity manifest",
            manifest_before,
            identity_digest,
        )
        if workload_image != preserved_image:
            os.link(preserved_image, workload_image, follow_symlinks=False)

        if sha256(workload_image) != image_digest or sha256(preserved_manifest) != identity_digest:
            die("catalog bytes changed while staging promotion")
        if not same_file_version(args.image.stat(), image_before) or not same_file_version(
            args.manifest.stat(), manifest_before
        ):
            die("source identity changed while staging promotion")

        built_at_ms = int(manifest.get("created_unix_ms", 0))
        manifest_toml = (
            f'name = "{args.name}"\n'
            'kind = "vm"\n'
            f'version = "{args.version}"\n'
            f"built_at_ms = {built_at_ms}\n"
            f"size_bytes = {image_before.st_size}\n"
            'profile = "browser-vm-chromium"\n'
        ).encode()
        write_synced(version_dir / "manifest.toml", manifest_toml)
        write_synced(version_dir / "image.sha256", f"{image_digest}\n".encode())
        admission = {
            "schema_version": 1,
            "kind": "browser_vm_catalog_admission",
            "name": args.name,
            "version": args.version,
            "artifact": f"images/{args.name}/{args.version}/{args.name}.img",
            "artifact_bytes": image_before.st_size,
            "artifact_sha256": f"sha256:{image_digest}",
            "identity_manifest": f"images/{args.name}/{args.version}/{args.manifest.name}",
            "identity_manifest_sha256": f"sha256:{identity_digest}",
            "profile": "browser-vm-chromium",
        }
        write_synced(
            version_dir / "catalog-admission.json",
            (json.dumps(admission, sort_keys=True, separators=(",", ":")) + "\n").encode(),
        )
        write_synced(stage / "images" / args.name / "PROMOTED", f"{args.version}\n".encode())

        for directory in (version_dir, version_dir.parent, version_dir.parent.parent, stage):
            fsync_directory(directory)
        rename_noreplace(stage, catalog_root)
        fsync_directory(catalog_root.parent)
    except BaseException:
        if stage.exists():
            for root, directories, files in os.walk(stage, topdown=False):
                for filename in files:
                    Path(root, filename).unlink(missing_ok=True)
                for directory in directories:
                    Path(root, directory).rmdir()
            stage.rmdir()
        raise

    final_image = catalog_root / "images" / args.name / args.version / f"{args.name}.img"
    print(f"catalog promotion passed: {args.name}:{args.version}")
    print(f"artifact: {final_image}")
    print(f"sha256:{image_digest}")
    print(f"identity-manifest-sha256:{identity_digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
