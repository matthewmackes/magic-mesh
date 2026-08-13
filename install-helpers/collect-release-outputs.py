#!/usr/bin/env python3
"""Collect already-verified first-release outputs into one immutable manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile

ROLES = {
    "workstation-rpm": "application/x-rpm",
    "server-rpm": "application/x-rpm",
    "lighthouse-rpm": "application/x-rpm",
    "browser-vm": "application/x-qemu-disk",
    "app-vm": "application/x-qemu-disk",
    "cuttlefish-image": "application/vnd.mcnf.cuttlefish-image",
    "bootc-image": "application/vnd.mcnf.bootc-image-receipt+json",
}
REVISION = re.compile(r"[0-9a-f]{40}\Z")
FINGERPRINT = re.compile(r"[0-9A-F]{40,64}\Z")
DIGEST = re.compile(r"sha256:[0-9a-f]{64}\Z")
MAX_PLAN = 1024 * 1024
MAX_ARTIFACT = 256 * 1024**3
MAX_VERIFIER_OUTPUT = 1024 * 1024
MAX_MANIFEST = 1024 * 1024
REPO = Path(__file__).resolve().parent.parent


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            refuse(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def regular(path: Path, label: str, maximum: int) -> os.stat_result:
    try:
        metadata = path.lstat()
    except OSError as error:
        refuse(f"{label} metadata unavailable: {error}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        refuse(f"{label} must be a regular non-symlink file")
    if not 0 < metadata.st_size <= maximum:
        refuse(f"{label} exceeds its bounded size contract")
    if metadata.st_mode & 0o022:
        refuse(f"{label} must not be group/other writable")
    return metadata


def digest_fd(fd: int, size: int) -> str:
    value = hashlib.sha256()
    os.lseek(fd, 0, os.SEEK_SET)
    remaining = size
    while remaining:
        chunk = os.read(fd, min(1024 * 1024, remaining))
        if not chunk:
            refuse("artifact shrank while being measured")
        value.update(chunk)
        remaining -= len(chunk)
    if os.read(fd, 1):
        refuse("artifact grew while being measured")
    return "sha256:" + value.hexdigest()


def identity(value: os.stat_result) -> tuple[int, int, int, int]:
    return value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns


def media_magic(fd: int, media_type: str) -> None:
    os.lseek(fd, 0, os.SEEK_SET)
    prefix = os.read(fd, 4)
    if media_type == "application/x-rpm" and prefix != b"\xed\xab\xee\xdb":
        refuse("RPM output has invalid file magic")
    if media_type == "application/x-qemu-disk" and prefix != b"QFI\xfb":
        refuse("qcow2 output has invalid file magic")


def load_plan(path: Path) -> dict[str, object]:
    regular(path, "collection plan", MAX_PLAN)
    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique_object,
                           parse_constant=lambda item: refuse(f"non-finite JSON number: {item}"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        refuse(f"collection plan is invalid: {error}")
    if not isinstance(value, dict) or set(value) != {"schema_version", "kind", "source_revision", "outputs"}:
        refuse("collection plan has an unsupported shape")
    if value["schema_version"] != 1 or value["kind"] != "mcnf-release-output-collection-plan":
        refuse("collection plan schema is unsupported")
    if not isinstance(value["source_revision"], str) or not REVISION.fullmatch(value["source_revision"]):
        refuse("source revision must be one full lowercase Git object ID")
    if value["source_revision"] == "0" * 40:
        refuse("null source revision is forbidden")
    return value


def verifier_argv(raw: object, artifact: Path, companions: dict[str, Path],
                  revision: str, signer: str, require_signer: bool) -> list[str]:
    if not isinstance(raw, list) or not 1 <= len(raw) <= 64:
        refuse("verifier must be a bounded non-empty argv array")
    result: list[str] = []
    saw_artifact = False
    saw_revision = False
    saw_signer = False
    for item in raw:
        if not isinstance(item, str) or not item or len(item) > 4096 or "\x00" in item:
            refuse("verifier argv contains a malformed value")
        if item == "{artifact}":
            result.append(str(artifact)); saw_artifact = True
        elif item == "{source_revision}":
            result.append(revision); saw_revision = True
        elif item == "{signing_identity}":
            result.append(signer); saw_signer = True
        elif item.startswith("{companion:") and item.endswith("}"):
            name = item[11:-1]
            if name not in companions:
                refuse(f"verifier references unknown companion: {name}")
            result.append(str(companions[name]))
        elif "{" in item or "}" in item:
            refuse("verifier argv contains an unsupported substitution")
        else:
            result.append(item)
    if not saw_artifact:
        refuse("owning verifier must receive the exact artifact path")
    if not saw_revision:
        refuse("owning verifier must receive the exact source revision")
    if require_signer and not saw_signer:
        refuse("RPM owning verifier must receive the exact signing identity")
    executable = Path(result[0])
    regular(executable, "owning verifier", 16 * 1024**2)
    if not os.access(executable, os.X_OK):
        refuse("owning verifier is not executable")
    try:
        resolved = executable.resolve(strict=True)
        relative = resolved.relative_to(REPO)
    except (OSError, ValueError):
        refuse("owning verifier must come from the pinned source checkout")
    if ".git" in relative.parts:
        refuse("owning verifier cannot come from Git metadata")
    try:
        head = subprocess.run(
            ["git", "-C", str(REPO), "rev-parse", "--verify", "HEAD^{commit}"],
            text=True, capture_output=True, timeout=15, check=False,
        )
        tracked = subprocess.run(
            ["git", "-C", str(REPO), "ls-tree", revision, "--", str(relative)],
            text=True, capture_output=True, timeout=15, check=False,
        )
        unchanged = subprocess.run(
            ["git", "-C", str(REPO), "diff", "--quiet", revision, "--", str(relative)],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, timeout=15, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        refuse(f"owning verifier source identity could not be checked: {error}")
    if head.returncode or head.stdout.strip() != revision:
        refuse("collector checkout does not match the pinned source revision")
    fields = tracked.stdout.split()
    if tracked.returncode or len(fields) != 4 or fields[1] != "blob" or not fields[0].endswith("755"):
        refuse("owning verifier is not an executable tracked source file")
    if unchanged.returncode != 0:
        refuse("owning verifier differs from the pinned source revision")
    return result


def collect(plan_path: Path, output: Path) -> dict[str, object]:
    plan = load_plan(plan_path)
    rows = plan["outputs"]
    if not isinstance(rows, list) or len(rows) != len(ROLES):
        refuse("plan must contain exactly one output for every required release role")
    if output.exists() or output.is_symlink():
        refuse("output manifest already exists or is substituted")
    parent = output.parent
    if not parent.is_dir() or parent.is_symlink() or parent.stat().st_mode & 0o022:
        refuse("output parent must be a private real directory")

    admitted: list[dict[str, object]] = []
    seen_roles: set[str] = set()
    seen_files: set[tuple[int, int]] = set()
    seen_paths: set[Path] = set()
    for index, row in enumerate(rows):
        if not isinstance(row, dict) or set(row) != {"role", "path", "media_type", "source_revision", "signing_identity", "companions", "verifier"}:
            refuse(f"output[{index}] has an unsupported shape")
        role = row["role"]
        if not isinstance(role, str) or role not in ROLES or role in seen_roles:
            refuse(f"output[{index}] has an unknown or duplicate role")
        seen_roles.add(role)
        if row["media_type"] != ROLES[role]:
            refuse(f"{role} media type does not match its canonical role")
        if row["source_revision"] != plan["source_revision"]:
            refuse(f"{role} source revision differs from the collection")
        signer = row["signing_identity"]
        if not isinstance(signer, str) or not FINGERPRINT.fullmatch(signer) or set(signer) == {"0"}:
            refuse(f"{role} signing identity is malformed or null")
        if not isinstance(row["path"], str) or not row["path"]:
            refuse(f"{role} path is malformed")
        artifact = Path(row["path"]).resolve(strict=True)
        before = regular(artifact, role, MAX_ARTIFACT)
        file_key = (before.st_dev, before.st_ino)
        if artifact in seen_paths or file_key in seen_files:
            refuse(f"{role} duplicates another output")
        seen_paths.add(artifact); seen_files.add(file_key)
        raw_companions = row["companions"]
        if not isinstance(raw_companions, dict) or len(raw_companions) > 16:
            refuse(f"{role} companions must be a bounded object")
        companions: dict[str, Path] = {}
        for name, raw_path in raw_companions.items():
            if not isinstance(name, str) or not re.fullmatch(r"[a-z][a-z0-9_-]{0,63}", name) or not isinstance(raw_path, str):
                refuse(f"{role} companion descriptor is malformed")
            companion = Path(raw_path).resolve(strict=True)
            regular(companion, f"{role} companion {name}", MAX_ARTIFACT)
            companions[name] = companion
        argv = verifier_argv(row["verifier"], artifact, companions,
                             str(plan["source_revision"]), signer,
                             row["media_type"] == "application/x-rpm")
        try:
            result = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                    stderr=subprocess.STDOUT, timeout=300, check=False)
        except (OSError, subprocess.TimeoutExpired) as error:
            refuse(f"{role} owning verifier could not complete: {error}")
        if len(result.stdout) > MAX_VERIFIER_OUTPUT:
            refuse(f"{role} owning verifier exceeded its output bound")
        if result.returncode != 0:
            refuse(f"{role} owning verifier rejected the output")
        after_verify = artifact.lstat()
        if identity(after_verify) != identity(before):
            refuse(f"{role} changed while its owning verifier ran")
        fd = os.open(artifact, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
        try:
            opened = os.fstat(fd)
            if identity(opened) != identity(before):
                refuse(f"{role} identity changed before measurement")
            media_magic(fd, row["media_type"])
            sha256 = digest_fd(fd, before.st_size)
            final = os.fstat(fd)
        finally:
            os.close(fd)
        if identity(final) != identity(before) or identity(artifact.lstat()) != identity(before):
            refuse(f"{role} changed while being measured")
        if not DIGEST.fullmatch(sha256) or sha256 == "sha256:" + "0" * 64:
            refuse(f"{role} produced a null or malformed digest")
        admitted.append({"media_type": row["media_type"], "path": str(artifact), "role": role,
                         "sha256": sha256, "signing_identity": signer,
                         "size": before.st_size, "source_revision": plan["source_revision"]})
    if seen_roles != set(ROLES):
        refuse("required output roles are missing")
    document = {"schema_version": 1, "kind": "mcnf-immutable-release-output-manifest",
                "source_revision": plan["source_revision"], "promotion": "forbidden",
                "outputs": sorted(admitted, key=lambda item: str(item["role"]))}
    encoded = (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode()
    if len(encoded) > MAX_MANIFEST:
        refuse("output manifest exceeds its bounded contract")
    temporary = Path(tempfile.mkdtemp(prefix=".release-output.", dir=parent))
    try:
        os.chmod(temporary, 0o700)
        staged = temporary / "manifest.json"
        fd = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
        with os.fdopen(fd, "wb") as stream:
            stream.write(encoded); stream.flush(); os.fsync(stream.fileno())
        # A same-filesystem hard-link publication supplies O_EXCL/no-replace
        # semantics that os.rename() cannot provide portably.
        os.link(staged, output, follow_symlinks=False)
        staged.unlink()
        directory_fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(directory_fd)
        finally: os.close(directory_fd)
    finally:
        try: temporary.rmdir()
        except OSError: pass
    return document


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--plan", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        document = collect(args.plan, args.output)
    except (Refusal, OSError) as error:
        print(f"release-output-collector: REFUSED: {error}", file=sys.stderr)
        return 2
    print(f"release-output-collector: PASS: {len(document['outputs'])} verified outputs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
