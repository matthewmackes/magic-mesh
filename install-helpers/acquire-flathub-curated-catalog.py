#!/usr/bin/env python3
"""Acquire digest-pinned curated Flatpak refs from Flathub.

Operator 2026-08-23: acquire REL-006 catalog refs from the open-source
provider. Default app is LibreOffice (office guest; Construct host has no
office app). Writes dest catalog JSON only. Never invents a commit. Never
marks production_admitted. org.example.* is refused.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path

KIND = "mcnf-flathub-curated-catalog"
DEFAULT_APP = "org.libreoffice.LibreOffice"
DEFAULT_REF_URL = (
    "https://dl.flathub.org/repo/refs/heads/app/"
    f"{DEFAULT_APP}/x86_64/stable"
)
IMMUTABLE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._/-]*@sha256:[0-9a-f]{64}\Z")
COMMIT = re.compile(r"[0-9a-f]{64}\Z")
APP_ID = re.compile(r"\A[A-Za-z0-9]+([._][A-Za-z0-9]+)+\Z")
EXIT_REFUSED = 2


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def dest_resolved(path: Path) -> Path:
    return path.parent.resolve() / path.name


def fetch_commit(url: str) -> str:
    try:
        with urllib.request.urlopen(url, timeout=30) as response:
            body = response.read()
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        refuse(f"flathub ref fetch failed: {error}")
        raise AssertionError
    text = body.decode("ascii", errors="strict").strip()
    if COMMIT.fullmatch(text) is None:
        refuse("flathub ref is not a 64-hex ostree commit")
    return text


def publish(path: Path, body: bytes, mode: int) -> None:
    dest = dest_resolved(path)
    if dest.exists() or dest.is_symlink():
        refuse("catalog dest already exists; refusing replace")
    parent = dest.parent.resolve(strict=True)
    if not parent.is_dir() or parent.stat().st_mode & 0o022:
        refuse("catalog dest parent must be a private real directory")
    directory = Path(tempfile.mkdtemp(prefix=f".{dest.name}.", dir=parent))
    try:
        directory.chmod(0o700)
        staged = directory / "body"
        descriptor = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(body)
            stream.flush()
            os.fsync(stream.fileno())
        os.link(staged, dest, follow_symlinks=False)
        staged.unlink()
    finally:
        try:
            directory.rmdir()
        except OSError:
            pass


def acquire(app_id: str, ref_url: str, output: Path, sidecar: Path) -> dict[str, object]:
    if APP_ID.fullmatch(app_id) is None:
        refuse("app id is malformed")
    if app_id.startswith("org.example.") or app_id.startswith("org.mcnf.test."):
        refuse("fixture catalog ids are not production refs")
    commit = fetch_commit(ref_url)
    ref = f"{app_id}@sha256:{commit}"
    if IMMUTABLE.fullmatch(ref) is None:
        refuse("acquired ref is not a sha256-pinned catalog ref")
    catalog = {"refs": [ref], "remote": "curated", "schema_version": 1}
    body = (json.dumps(catalog, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")
    publish(output, body, 0o444)
    record = {
        "app_id": app_id,
        "catalog_sha256": hashlib.sha256(body).hexdigest(),
        "kind": KIND,
        "production_admitted": False,
        "provider": "flathub",
        "ref_url": ref_url,
        "schema_version": 1,
        "sidecar_path": str(dest_resolved(sidecar)),
        "source": "https://dl.flathub.org/repo/refs/heads/",
    }
    if record["production_admitted"] is not False:
        refuse("helper must never mark production_admitted")
    publish(sidecar, (json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii"), 0o400)
    return record


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app-id", default=DEFAULT_APP)
    parser.add_argument("--ref-url", default=DEFAULT_REF_URL)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--sidecar", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        record = acquire(args.app_id, args.ref_url, args.output, args.sidecar)
        print(
            "acquire-flathub-curated-catalog: wrote dest; "
            f"catalog_sha256={record['catalog_sha256']}; "
            "production_admitted=false"
        )
        return 0
    except (OSError, Refusal, UnicodeError, ValueError) as error:
        print(f"acquire-flathub-curated-catalog: REFUSED: {error}", file=sys.stderr)
        return EXIT_REFUSED


if __name__ == "__main__":
    raise SystemExit(main())
