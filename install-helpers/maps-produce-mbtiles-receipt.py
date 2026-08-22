#!/usr/bin/env python3
"""Bind operator-supplied Maps MBTiles to an immutable non-production receipt.

The producer never fetches OSM tiles and never marks fixture or operator
bytes as production-admitted. Wrong provider, path substitution, and quota
breach refuse before publication.
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("maps_verify_mbtiles", HERE / "maps-verify-mbtiles.py")
verify = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(verify)


def produce(approval_path: Path, source_root: Path, relative_mbtiles: str, output: Path) -> dict:
    approval = verify.load_approval(approval_path)
    mbtiles = verify.resolve_mbtiles(source_root, relative_mbtiles)
    inspected = verify.inspect_mbtiles(mbtiles, approval["quota_bytes"])
    receipt = verify.bind_receipt(approval, inspected)
    atomic_write(output, verify.canonical(receipt))
    return receipt


def atomic_write(path: Path, body: bytes) -> None:
    if path.exists() or path.is_symlink():
        raise verify.Refusal("receipt output already exists; publication is no-replace")
    parent = path.parent.resolve(strict=True)
    fd, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=parent)
    temporary = Path(name)
    try:
        os.fchmod(fd, 0o400)
        view = memoryview(body)
        while view:
            written = os.write(fd, view)
            if written <= 0:
                raise verify.Refusal("receipt write made no progress")
            view = view[written:]
        os.fsync(fd)
        os.close(fd)
        fd = -1
        os.link(temporary, path)
        parent_fd = os.open(parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
    except FileExistsError as error:
        raise verify.Refusal(f"receipt output appeared during publication: {path}") from error
    finally:
        if fd >= 0:
            os.close(fd)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    produce_parser = sub.add_parser("produce")
    produce_parser.add_argument("--approval", type=Path, required=True)
    produce_parser.add_argument("--source-root", type=Path, required=True)
    produce_parser.add_argument("--mbtiles", required=True)
    produce_parser.add_argument("--output", type=Path, required=True)
    verify_parser = sub.add_parser("verify")
    verify_parser.add_argument("--receipt", type=Path, required=True)
    verify_parser.add_argument("--source-root", type=Path, required=True)
    verify_parser.add_argument("--mbtiles", required=True)
    verify_parser.add_argument("--source-revision", required=True)
    verify_parser.add_argument("--source-epoch", type=int, required=True)
    verify_parser.add_argument("--quota-bytes", type=int, required=True)
    args = parser.parse_args()
    try:
        if args.command == "produce":
            value = produce(args.approval, args.source_root, args.mbtiles, args.output)
        else:
            mbtiles = verify.resolve_mbtiles(args.source_root, args.mbtiles)
            value = verify.verify_receipt(
                args.receipt, mbtiles, args.source_revision, args.source_epoch, args.quota_bytes
            )
    except (verify.Refusal, OSError, UnicodeError, ValueError) as error:
        print(f"maps-produce-mbtiles-receipt: refusal: {error}", file=sys.stderr)
        return verify.EXIT_REFUSED
    print(verify.canonical(value).decode("ascii"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
