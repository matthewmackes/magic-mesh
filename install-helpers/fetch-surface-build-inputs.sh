#!/usr/bin/env bash
# Fetch the complete immutable source set for the Fedora 44 Surface RPM lane.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_LOCK="$ROOT/packaging/surface/surface-build-inputs.f44.json"
LOCK="$DEFAULT_LOCK"
OUTPUT=""
SELF_TEST=0

usage() {
    echo "Usage: $0 [--lock PATH] [--output NEW-DIR] [--self-test]"
    echo "Without --output, validates the immutable build-input lock only."
}

while (($#)); do
    case "$1" in
        --lock)
            (($# >= 2)) || { usage >&2; exit 2; }
            LOCK=$2
            shift 2
            ;;
        --output)
            (($# >= 2)) || { usage >&2; exit 2; }
            OUTPUT=$2
            shift 2
            ;;
        --self-test)
            SELF_TEST=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
done

validate_lock() {
    python3 - "$1" <<'PY'
import json
import re
import sys
from pathlib import Path
from urllib.parse import urlparse

EXPECTED_PACKAGES = {
    "kernel-surface": ["linux-surface", "kernel-ark", "surface-certificate"],
    "iptsd": ["iptsd"],
    "libwacom-surface": ["libwacom-surface", "libwacom-upstream"],
    "surface-control": ["surface-control"],
    "surface-secureboot": ["secureboot-mok", "surface-certificate"],
}
EXPECTED_INPUTS = {
    "linux-surface", "kernel-ark", "iptsd", "libwacom-surface",
    "libwacom-upstream", "surface-control", "secureboot-mok",
    "surface-certificate",
}
ALLOWED_HOSTS = {"github.com", "gitlab.com", "raw.githubusercontent.com"}
HEX40 = re.compile(r"[0-9a-f]{40}")
HEX64 = re.compile(r"[0-9a-f]{64}")
FILENAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+-]{0,159}")

def reject_duplicates(pairs):
    out = {}
    for key, value in pairs:
        if key in out:
            raise ValueError(f"duplicate JSON key: {key}")
        out[key] = value
    return out

def exact(value, keys, label):
    if not isinstance(value, dict) or set(value) != set(keys):
        raise ValueError(f"{label} has unknown or missing fields")

try:
    raw = Path(sys.argv[1]).read_bytes()
    if not raw or len(raw) > 128 * 1024:
        raise ValueError("lock size is empty or exceeds 128 KiB")
    doc = json.loads(raw, object_pairs_hook=reject_duplicates)
    exact(doc, ["schema_version", "kind", "target", "builder_image", "inputs", "packages"], "root")
    if doc["schema_version"] != 1 or doc["kind"] != "mcnf-surface-build-inputs":
        raise ValueError("unsupported build-input schema")
    exact(doc["target"], ["os", "release", "arch"], "target")
    if doc["target"] != {"os": "fedora", "release": 44, "arch": "x86_64"}:
        raise ValueError("target must be Fedora 44 x86_64")
    if not re.fullmatch(r"registry\.fedoraproject\.org/fedora@sha256:[0-9a-f]{64}", doc["builder_image"]):
        raise ValueError("builder image must be the digest-pinned Fedora registry image")
    if not isinstance(doc["inputs"], list) or len(doc["inputs"]) != len(EXPECTED_INPUTS):
        raise ValueError("exactly eight build inputs are required")
    ids = set()
    filenames = set()
    for item in doc["inputs"]:
        exact(item, ["id", "filename", "url", "ref", "commit", "sha256", "license"], "input")
        ident = item["id"]
        if ident not in EXPECTED_INPUTS or ident in ids:
            raise ValueError("unknown or duplicate input id")
        ids.add(ident)
        if not isinstance(item["filename"], str) or not FILENAME.fullmatch(item["filename"]):
            raise ValueError(f"invalid filename for {ident}")
        if item["filename"] in filenames:
            raise ValueError("duplicate output filename")
        filenames.add(item["filename"])
        parsed = urlparse(item["url"])
        if parsed.scheme != "https" or parsed.hostname not in ALLOWED_HOSTS or parsed.username or parsed.password:
            raise ValueError(f"untrusted source URL for {ident}")
        if not isinstance(item["ref"], str) or not item["ref"] or len(item["ref"]) > 160:
            raise ValueError(f"invalid immutable ref for {ident}")
        if not isinstance(item["commit"], str) or not HEX40.fullmatch(item["commit"]):
            raise ValueError(f"invalid commit for {ident}")
        if item["commit"] not in item["url"] and ident not in {"libwacom-upstream"}:
            raise ValueError(f"URL is not bound to the commit for {ident}")
        if not isinstance(item["sha256"], str) or not HEX64.fullmatch(item["sha256"]):
            raise ValueError(f"invalid SHA-256 for {ident}")
        if not isinstance(item["license"], str) or not item["license"] or len(item["license"]) > 96:
            raise ValueError(f"invalid license for {ident}")
    if ids != EXPECTED_INPUTS:
        raise ValueError("missing build input")
    if not isinstance(doc["packages"], list) or len(doc["packages"]) != 5:
        raise ValueError("exactly five Surface package mappings are required")
    mappings = {}
    for package in doc["packages"]:
        exact(package, ["name", "input_ids"], "package")
        name = package["name"]
        if name in mappings or name not in EXPECTED_PACKAGES:
            raise ValueError("unknown or duplicate package mapping")
        if package["input_ids"] != EXPECTED_PACKAGES[name]:
            raise ValueError(f"incorrect ordered source mapping for {name}")
        mappings[name] = package["input_ids"]
    if set(mappings) != set(EXPECTED_PACKAGES):
        raise ValueError("missing package mapping")
except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as error:
    print(f"INVALID: {error}", file=sys.stderr)
    raise SystemExit(1)
PY
}

self_test() {
    python3 - "$DEFAULT_LOCK" <<'PY'
import json
import subprocess
import sys
import tempfile
from pathlib import Path

script = Path(sys.argv[0]).resolve() if False else Path("install-helpers/fetch-surface-build-inputs.sh").resolve()
source = Path(sys.argv[1]).read_text()
base = json.loads(source)

def rejected(name, mutate=None, raw=None):
    with tempfile.TemporaryDirectory(prefix="surface-input-selftest-") as directory:
        path = Path(directory) / "lock.json"
        if raw is None:
            value = json.loads(json.dumps(base))
            mutate(value)
            raw = json.dumps(value)
        path.write_text(raw)
        result = subprocess.run([str(script), "--lock", str(path)], capture_output=True, text=True)
        if result.returncode == 0:
            raise SystemExit(f"self-test accepted hostile fixture: {name}")

rejected("unknown root", lambda x: x.update({"surprise": True}))
rejected("wrong release", lambda x: x["target"].update({"release": 43}))
rejected("floating image", lambda x: x.update({"builder_image": "registry.fedoraproject.org/fedora:44"}))
rejected("http source", lambda x: x["inputs"][0].update({"url": x["inputs"][0]["url"].replace("https://", "http://")}))
rejected("foreign source", lambda x: x["inputs"][0].update({"url": "https://example.com/" + x["inputs"][0]["commit"]}))
rejected("upper hash", lambda x: x["inputs"][0].update({"sha256": "A" * 64}))
rejected("path filename", lambda x: x["inputs"][0].update({"filename": "../escape"}))
rejected("duplicate input", lambda x: x["inputs"].__setitem__(1, x["inputs"][0]))
rejected("wrong mapping", lambda x: x["packages"][0].update({"input_ids": ["linux-surface"]}))
rejected("duplicate JSON key", raw=source.replace('"schema_version": 1,', '"schema_version": 1, "schema_version": 1,', 1))
print("Surface build-input lock self-test passed (10 hostile fixtures rejected)")
PY
}

if ((SELF_TEST)); then
    self_test
    exit 0
fi

validate_lock "$LOCK"

if [[ -z "$OUTPUT" ]]; then
    echo "Surface build-input lock is valid"
    exit 0
fi

command -v curl >/dev/null || { echo "curl is required" >&2; exit 2; }
command -v sha256sum >/dev/null || { echo "sha256sum is required" >&2; exit 2; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 2; }

if [[ -e "$OUTPUT" ]]; then
    echo "refusing to overwrite existing output: $OUTPUT" >&2
    exit 2
fi
output_parent="$(dirname "$OUTPUT")"
[[ -d "$output_parent" ]] || { echo "output parent does not exist: $output_parent" >&2; exit 2; }
output_name="$(basename "$OUTPUT")"
[[ "$output_name" != "." && "$output_name" != ".." && -n "$output_name" ]] \
    || { echo "unsafe output path" >&2; exit 2; }

stage="$(mktemp -d "$output_parent/.surface-build-inputs.XXXXXX")"
chmod 0700 "$stage"
cleanup_stage() {
    if [[ -n "${stage:-}" && -d "$stage" ]]; then
        find "$stage" -depth -delete
    fi
}
trap cleanup_stage EXIT

python3 - "$LOCK" <<'PY' | while IFS=$'\t' read -r filename url sha256; do
import json
import sys
for item in json.load(open(sys.argv[1], encoding="utf-8"))["inputs"]:
    print(item["filename"], item["url"], item["sha256"], sep="\t")
PY
    destination="$stage/$filename"
    curl --proto '=https' --tlsv1.2 --location --fail --silent --show-error \
        --connect-timeout 20 --max-time 1800 --output "$destination" "$url"
    actual="$(sha256sum "$destination" | awk '{print $1}')"
    if [[ "$actual" != "$sha256" ]]; then
        echo "SHA-256 mismatch for $filename" >&2
        exit 1
    fi
done

cp "$LOCK" "$stage/build-input-lock.json"
(
    cd "$stage"
    mapfile -t source_files < <(
        find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%f\n' | LC_ALL=C sort
    )
    sha256sum "${source_files[@]}" > SHA256SUMS
)
chmod 0600 "$stage"/*
mv "$stage" "$OUTPUT"
stage=""
trap - EXIT
echo "Surface build inputs fetched and verified: $OUTPUT"
