#!/usr/bin/env python3
"""Admit the App VM's candidate-bound curated Flatpak catalog."""
import argparse, hashlib, json, os, re, stat, sys
from pathlib import Path

class Refusal(ValueError): pass

IMMUTABLE_REF = re.compile(r"[A-Za-z0-9][A-Za-z0-9._/-]*@sha256:[0-9a-f]{64}\Z")
REVISION = re.compile(r"[0-9a-f]{40}\Z")

def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--catalog", type=Path, required=True)
    p.add_argument("--source-revision", required=True)
    p.add_argument("--source-epoch", type=int, required=True)
    p.add_argument("--output", type=Path, required=True)
    a = p.parse_args()
    try:
        if REVISION.fullmatch(a.source_revision) is None or a.source_revision == "0" * 40:
            raise Refusal("source revision must be one non-null 40-character lowercase Git object ID")
        if a.source_epoch <= 0:
            raise Refusal("source epoch must be positive")
        info = a.catalog.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_mode & 0o222:
            raise Refusal("catalog must be an immutable single-link regular file")
        value = json.loads(a.catalog.read_text(encoding="utf-8"))
        if set(value) != {"schema_version", "remote", "refs"} or value["schema_version"] != 1:
            raise Refusal("catalog schema is unsupported")
        if value["remote"] != "curated" or not isinstance(value["refs"], list) or not value["refs"]:
            raise Refusal("catalog must declare non-empty curated refs")
        if any(not isinstance(ref, str) or IMMUTABLE_REF.fullmatch(ref) is None for ref in value["refs"]):
            raise Refusal("catalog refs must be fully qualified sha256-pinned refs")
        body = json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n"
        receipt = {"schema_version": 1, "kind": "mcnf-app-vm-curated-catalog", "remote": "curated",
                   "catalog_sha256": hashlib.sha256(body.encode()).hexdigest(), "refs": sorted(value["refs"]),
                   "source_revision": a.source_revision, "source_epoch": a.source_epoch}
        payload = (json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n").encode()
        if a.output.exists() or a.output.is_symlink(): raise Refusal("receipt output already exists")
        if not a.output.parent.is_dir() or a.output.parent.is_symlink() or a.output.parent.stat().st_mode & 0o022:
            raise Refusal("receipt output parent must be a private real directory")
        descriptor = os.open(a.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"produce-catalog-receipt: REFUSED: {error}", file=sys.stderr); return 2
    return 0
if __name__ == "__main__": raise SystemExit(main())
