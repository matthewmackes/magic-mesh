#!/usr/bin/env python3
"""Admit the App VM's candidate-bound curated Flatpak catalog."""
import argparse, hashlib, json, os, stat, sys
from pathlib import Path

class Refusal(ValueError): pass

def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--catalog", type=Path, required=True)
    p.add_argument("--source-revision", required=True)
    p.add_argument("--source-epoch", type=int, required=True)
    p.add_argument("--output", type=Path, required=True)
    a = p.parse_args()
    try:
        info = a.catalog.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_mode & 0o222:
            raise Refusal("catalog must be an immutable single-link regular file")
        value = json.loads(a.catalog.read_text(encoding="utf-8"))
        if set(value) != {"schema_version", "remote", "refs"} or value["schema_version"] != 1:
            raise Refusal("catalog schema is unsupported")
        if value["remote"] != "curated" or not isinstance(value["refs"], list) or not value["refs"]:
            raise Refusal("catalog must declare non-empty curated refs")
        if any(not isinstance(ref, str) or not ref or "@" not in ref for ref in value["refs"]):
            raise Refusal("catalog refs must be fully qualified immutable refs")
        body = json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n"
        receipt = {"schema_version": 1, "kind": "mcnf-app-vm-curated-catalog", "remote": "curated",
                   "catalog_sha256": hashlib.sha256(body.encode()).hexdigest(), "refs": sorted(value["refs"]),
                   "source_revision": a.source_revision, "source_epoch": a.source_epoch}
        payload = (json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n").encode()
        if a.output.exists() or a.output.is_symlink(): raise Refusal("receipt output already exists")
        a.output.write_bytes(payload); a.output.chmod(0o400)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"produce-catalog-receipt: REFUSED: {error}", file=sys.stderr); return 2
    return 0
if __name__ == "__main__": raise SystemExit(main())
