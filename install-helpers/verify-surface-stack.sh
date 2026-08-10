#!/usr/bin/env bash
# Verify the governed Fedora 44 Microsoft Surface package artifact contract.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
MANIFEST="$ROOT/packaging/surface/surface-stack.f44.json"
ARTIFACT_DIR="$ROOT/packaging/surface/artifacts"
EMIT_LOCK=""
SELF_TEST=false

usage() {
    cat <<'EOF'
Usage: install-helpers/verify-surface-stack.sh [--manifest PATH]
       [--artifact-dir DIR] [--emit-lock PATH] [--self-test]

Exit 0: manifest and exact local artifacts are ready and verified
Exit 1: manifest/artifacts are malformed, hostile, ambiguous, or contradictory
Exit 2: usage/runtime prerequisite failure
Exit 3: manifest is valid but explicitly blocked (artifacts are not inspected)
EOF
}

while (($#)); do
    case "$1" in
        --manifest) (($# >= 2)) || { echo "FATAL: --manifest requires a path" >&2; exit 2; }; MANIFEST=$2; shift 2 ;;
        --artifact-dir) (($# >= 2)) || { echo "FATAL: --artifact-dir requires a path" >&2; exit 2; }; ARTIFACT_DIR=$2; shift 2 ;;
        --emit-lock) (($# >= 2)) || { echo "FATAL: --emit-lock requires a path" >&2; exit 2; }; EMIT_LOCK=$2; shift 2 ;;
        --self-test) SELF_TEST=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "FATAL: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

command -v python3 >/dev/null 2>&1 || { echo "FATAL: python3 is required" >&2; exit 2; }
if [[ $SELF_TEST == false && ! -f $MANIFEST ]]; then
    echo "FATAL: Surface provenance manifest is missing: $MANIFEST" >&2
    exit 1
fi

python3 - "$MANIFEST" "$ARTIFACT_DIR" "$EMIT_LOCK" "$SELF_TEST" <<'PY'
import copy
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

EXPECTED = ("kernel-surface", "iptsd", "libwacom-surface", "surface-control", "surface-secureboot")
SIGNING_REQUIRED = {"kernel-surface", "surface-secureboot"}
ROOT_KEYS = {"schema_version", "kind", "target", "signing_key", "status", "blockers", "packages"}
TARGET_KEYS = {"os", "release", "arch", "profile", "bootc_base"}
KEY_KEYS_BLOCKED = {"filename", "sha256", "fingerprint"}
KEY_KEYS_READY = KEY_KEYS_BLOCKED | {"rpm_signing_fingerprints"}
PACKAGE_KEYS = {"name", "availability", "blocker", "source", "rpm", "kernel_module_signing"}
SOURCE_KEYS = {"filename", "url", "ref", "sha256", "license"}
RPM_KEYS = {"filename", "nevra", "sha256", "signing_fingerprint"}
SIGNING_KEYS = {"applicability", "signer", "certificate_sha256"}
HEX64 = re.compile(r"^[0-9a-f]{64}$")
FINGERPRINT = re.compile(r"^[0-9A-F]{40}(?:[0-9A-F]{24})?$")
PINNED_REF = re.compile(r"^(?:[0-9a-f]{40,64}|refs/tags/[A-Za-z0-9][A-Za-z0-9._+/-]{0,126})$")
LICENSE = re.compile(r"^[A-Za-z0-9.+()-]+(?: (?:AND|OR|WITH) [A-Za-z0-9.+()-]+)*$")
FILENAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+~-]{0,190}$")
BASE = re.compile(r"^quay\.io/fedora/fedora-bootc:44@sha256:[0-9a-f]{64}$")

class Invalid(Exception): pass
def fail(message): raise Invalid(message)

def strict_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value: fail(f"duplicate JSON object key: {key}")
        value[key] = item
    return value

def exact_keys(value, expected, where):
    if not isinstance(value, dict): fail(f"{where} must be an object")
    if set(value) != expected:
        fail(f"{where} keys differ (missing={sorted(expected-set(value))}, unknown={sorted(set(value)-expected)})")

def nonempty(value, where):
    if not isinstance(value, str) or not value.strip() or value != value.strip():
        fail(f"{where} must be a non-empty, trimmed string")

def pinned_sha(value, where):
    if not isinstance(value, str) or not HEX64.fullmatch(value) or value == "0" * 64:
        fail(f"{where} must be a non-null lowercase SHA-256")

def fingerprint(value, where):
    if not isinstance(value, str) or not FINGERPRINT.fullmatch(value) or set(value) == {"0"}:
        fail(f"{where} must be a full uppercase 40- or 64-hex fingerprint")

def filename(value, where, suffix=None):
    if not isinstance(value, str) or not FILENAME.fullmatch(value) or (suffix and not value.endswith(suffix)):
        fail(f"{where} must be a safe artifact basename{f' ending in {suffix}' if suffix else ''}")

def validate(document):
    exact_keys(document, ROOT_KEYS, "manifest")
    if document["schema_version"] != 2: fail("schema_version must be integer 2")
    if document["kind"] != "mcnf-surface-stack-provenance": fail("kind is unknown")
    target = document["target"]
    exact_keys(target, TARGET_KEYS, "target")
    fixed_target = {"os": "fedora", "release": 44, "arch": "x86_64", "profile": "workstation-bootc"}
    if {k: target[k] for k in fixed_target} != fixed_target: fail("target must be Fedora 44 x86_64 workstation-bootc")
    if not isinstance(document["signing_key"], dict): fail("signing_key must be an object")
    status = document["status"]
    if status not in {"ready", "blocked"}: fail("status is unknown")
    blockers = document["blockers"]
    if not isinstance(blockers, list) or any(not isinstance(x, str) or not x.strip() for x in blockers):
        fail("blockers must be an array of non-empty strings")
    if len(blockers) != len(set(blockers)): fail("blockers contains duplicates")
    packages = document["packages"]
    if not isinstance(packages, list) or len(packages) != len(EXPECTED): fail("packages must contain exactly five rows")
    names = [row.get("name") if isinstance(row, dict) else None for row in packages]
    if len(names) != len(set(names)) or set(names) != set(EXPECTED): fail("package set must contain each governed name exactly once")

    row_blockers, ready_count, filenames = [], 0, []
    for index, row in enumerate(packages):
        where = f"packages[{index}]"
        exact_keys(row, PACKAGE_KEYS, where)
        name, availability = row["name"], row["availability"]
        if availability not in {"ready", "unavailable"}: fail(f"{where}.availability is unknown")
        exact_keys(row["source"], SOURCE_KEYS, f"{where}.source")
        exact_keys(row["rpm"], RPM_KEYS, f"{where}.rpm")
        exact_keys(row["kernel_module_signing"], SIGNING_KEYS, f"{where}.kernel_module_signing")
        signing = row["kernel_module_signing"]
        required = "required" if name in SIGNING_REQUIRED else "not-applicable"
        if signing["applicability"] != required: fail(f"{where}.kernel_module_signing.applicability contradicts package contract")
        immutable = list(row["source"].values()) + list(row["rpm"].values()) + [signing["signer"], signing["certificate_sha256"]]
        if availability == "unavailable":
            nonempty(row["blocker"], f"{where}.blocker")
            if any(value is not None for value in immutable): fail(f"{where} unavailable row must not carry guessed provenance")
            row_blockers.append(row["blocker"]); continue
        ready_count += 1
        if row["blocker"] is not None: fail(f"{where} ready row must have a null blocker")
        source, rpm = row["source"], row["rpm"]
        if not isinstance(source["url"], str) or not re.fullmatch(r"https://[^\s?#]+", source["url"]): fail(f"{where}.source.url must be stable HTTPS")
        if not isinstance(source["ref"], str) or not PINNED_REF.fullmatch(source["ref"]): fail(f"{where}.source.ref must be immutable")
        filename(source["filename"], f"{where}.source.filename"); filenames.append(source["filename"])
        pinned_sha(source["sha256"], f"{where}.source.sha256")
        if not isinstance(source["license"], str) or not LICENSE.fullmatch(source["license"]): fail(f"{where}.source.license must be SPDX")
        filename(rpm["filename"], f"{where}.rpm.filename", ".rpm"); filenames.append(rpm["filename"])
        if not isinstance(rpm["nevra"], str) or not re.fullmatch(re.escape(name) + r"-(?:[0-9]+:)?[0-9][A-Za-z0-9._+~^-]*-[0-9][A-Za-z0-9._+~^-]*\.(?:x86_64|noarch)", rpm["nevra"]):
            fail(f"{where}.rpm.nevra is malformed or does not match package")
        pinned_sha(rpm["sha256"], f"{where}.rpm.sha256"); fingerprint(rpm["signing_fingerprint"], f"{where}.rpm.signing_fingerprint")
        if required == "required":
            nonempty(signing["signer"], f"{where}.kernel_module_signing.signer")
            pinned_sha(signing["certificate_sha256"], f"{where}.kernel_module_signing.certificate_sha256")
        elif signing["signer"] is not None or signing["certificate_sha256"] is not None:
            fail(f"{where} not-applicable kernel signing fields must be null")
    if len(filenames) != len(set(filenames)): fail("source/RPM artifact filenames must be unique")
    if sorted(blockers) != sorted(row_blockers): fail("top-level blockers must exactly match unavailable package blockers")
    if ready_count == len(EXPECTED):
        if status != "ready" or blockers: fail("a complete manifest must be ready with no blockers")
        if not isinstance(target["bootc_base"], str) or not BASE.fullmatch(target["bootc_base"]) or target["bootc_base"].endswith("0" * 64): fail("ready manifest requires non-null digest-pinned quay.io Fedora 44 bootc base")
        key = document["signing_key"]
        exact_keys(key, KEY_KEYS_READY, "signing_key")
        filename(key["filename"], "signing_key.filename", ".asc"); pinned_sha(key["sha256"], "signing_key.sha256"); fingerprint(key["fingerprint"], "signing_key.fingerprint")
        admitted = key["rpm_signing_fingerprints"]
        if (not isinstance(admitted, list) or not admitted or admitted != sorted(set(admitted))):
            fail("signing_key.rpm_signing_fingerprints must be a non-empty sorted unique array")
        for index, item in enumerate(admitted): fingerprint(item, f"signing_key.rpm_signing_fingerprints[{index}]")
        if key["fingerprint"] not in admitted: fail("primary signing-key fingerprint must be admitted for RPM signing")
        for row in packages:
            if row["rpm"]["signing_fingerprint"] not in admitted: fail(f"{row['name']} signing fingerprint is not admitted by the governed key")
        return "ready"
    if ready_count != 0: fail("partial package provenance is not admissible; manifest must be wholly ready or wholly unavailable")
    if status != "blocked" or not blockers: fail("an incomplete manifest must be blocked with exact blockers")
    exact_keys(document["signing_key"], KEY_KEYS_BLOCKED, "signing_key")
    if target["bootc_base"] is not None or any(v is not None for v in document["signing_key"].values()):
        fail("blocked manifest must not guess a base digest or signing key")
    return "blocked"

def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""): digest.update(chunk)
    return digest.hexdigest()

def safe_artifact(root, name):
    path = root / name
    if path.is_symlink() or not path.is_file(): fail(f"artifact must be a regular non-symlink file: {name}")
    if path.parent.resolve() != root.resolve(): fail(f"artifact escapes artifact directory: {name}")
    return path

def run(command, what):
    try: result = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
    except OSError as exc: fail(f"cannot execute {command[0]} for {what}: {exc}")
    if result.returncode != 0: fail(f"{what} failed: {result.stdout.strip()}")
    return result.stdout.strip()

def signing_key_fingerprints(output):
    primary, admitted, pending, pub_count = None, set(), None, 0
    for line in output.splitlines():
        fields = line.split(":")
        if fields[0] in {"pub", "sub"}:
            pending = (fields[0], len(fields) > 11 and "s" in fields[11].lower())
            if fields[0] == "pub": pub_count += 1
        elif fields[0] == "fpr" and pending is not None:
            if len(fields) <= 9: fail("signing key artifact has a malformed fingerprint record")
            value = fields[9].upper()
            if pending[0] == "pub": primary = value
            if pending[1]: admitted.add(value)
            pending = None
    if pub_count != 1 or primary is None or primary not in admitted:
        fail("signing key artifact must contain one signing-capable primary key")
    return primary, admitted

def admitted_signature_fingerprint(output, expected_fingerprints):
    key_ids = []
    for line in output.splitlines():
        if "signature" not in line.lower() or not line.rstrip().endswith(": OK"):
            continue
        match = re.search(r"(?i)key ID ([0-9a-f]{8,16})", line)
        if match: key_ids.append(match.group(1).upper())
    if not key_ids: fail("RPM signature verification did not report a valid signature key ID")
    matches = []
    for key_id in key_ids:
        found = [item for item in expected_fingerprints if item.endswith(key_id)]
        if len(found) != 1: fail("RPM signature key ID does not uniquely match a governed fingerprint")
        matches.append(found[0])
    if len(set(matches)) != 1: fail("RPM carries signatures from inconsistent governed keys")
    return matches[0]

def verify_artifacts(document, artifact_dir, inspector=None):
    root = Path(artifact_dir)
    if not root.is_dir(): fail(f"artifact directory is missing: {root}")
    expected_files = {document["signing_key"]["filename"]}
    expected_files |= {r["rpm"]["filename"] for r in document["packages"]}
    expected_files |= {r["source"]["filename"] for r in document["packages"]}
    actual_rpms = {p.name for p in root.iterdir() if p.name != ".gitkeep" and p.is_file() and not p.is_symlink()}
    if actual_rpms != expected_files: fail(f"artifact directory set differs (missing={sorted(expected_files-actual_rpms)}, unknown={sorted(actual_rpms-expected_files)})")
    key = safe_artifact(root, document["signing_key"]["filename"])
    if sha256(key) != document["signing_key"]["sha256"]: fail("signing key artifact SHA-256 mismatch")
    rows, sources = [], []
    if inspector is None:
        primary, admitted = signing_key_fingerprints(run(["gpg", "--batch", "--show-keys", "--with-colons", str(key)], "signing key inspection"))
        if primary != document["signing_key"]["fingerprint"]: fail("signing key artifact primary fingerprint mismatch")
        if admitted != set(document["signing_key"]["rpm_signing_fingerprints"]): fail("signing key artifact admitted RPM fingerprints differ")
        rpmdb = tempfile.TemporaryDirectory(prefix="mcnf-surface-rpmdb-")
        run(["rpm", "--dbpath", rpmdb.name, "--initdb"], "temporary RPM database initialization")
        run(["rpm", "--dbpath", rpmdb.name, "--import", str(key)], "governed signing key import")
        def inspector(path):
            nevra = run(["rpm", "-qp", "--qf", "%{NAME}-%|EPOCH?{%{EPOCH}:}:{}|%{VERSION}-%{RELEASE}.%{ARCH}", str(path)], f"NEVRA inspection for {path.name}")
            signature = run(["rpmkeys", "--dbpath", rpmdb.name, "--checksig", "--verbose", str(path)], f"signature verification for {path.name}")
            signer = admitted_signature_fingerprint(signature, admitted)
            return nevra, signer
    for row in sorted(document["packages"], key=lambda item: item["name"]):
        source = row["source"]; source_path = safe_artifact(root, source["filename"])
        if sha256(source_path) != source["sha256"]: fail(f"source artifact SHA-256 mismatch: {source['filename']}")
        sources.append((source["sha256"], source["filename"]))
        rpm = row["rpm"]; path = safe_artifact(root, rpm["filename"])
        if sha256(path) != rpm["sha256"]: fail(f"RPM artifact SHA-256 mismatch: {rpm['filename']}")
        nevra, signer = inspector(path)
        if nevra != rpm["nevra"]: fail(f"RPM NEVRA mismatch for {rpm['filename']}: got {nevra}")
        if signer != rpm["signing_fingerprint"]: fail(f"RPM signer mismatch for {rpm['filename']}")
        rows.append((rpm["sha256"], rpm["nevra"], signer, rpm["filename"]))
    return rows, sources

def install_lock_lines(document, rows, sources, manifest_sha):
    lines = [f"MANIFEST\t{manifest_sha}", f"BASE\t{document['target']['bootc_base']}",
             "KEY\t" + "\t".join((document["signing_key"]["sha256"], document["signing_key"]["fingerprint"], document["signing_key"]["filename"]))]
    lines += ["SIGNER\t" + value for value in document["signing_key"]["rpm_signing_fingerprints"]]
    lines += ["SOURCE\t" + "\t".join(source) for source in sources]
    lines += ["RPM\t" + "\t".join(row) for row in rows]
    return lines

def fixture():
    sha, fpr = "a" * 64, "B" * 40
    rows = []
    for name in EXPECTED:
        required = name in SIGNING_REQUIRED
        rows.append({"name": name, "availability": "ready", "blocker": None,
          "source": {"filename": f"{name}-source.tar.zst", "url": f"https://fixtures.invalid/{name}.tar.zst", "ref": "refs/tags/v1.0.0", "sha256": sha, "license": "GPL-2.0-only"},
          "rpm": {"filename": f"{name}-1.0.0-1.fc44.x86_64.rpm", "nevra": f"{name}-1.0.0-1.fc44.x86_64", "sha256": sha, "signing_fingerprint": fpr},
          "kernel_module_signing": {"applicability": "required" if required else "not-applicable", "signer": "SELF-TEST" if required else None, "certificate_sha256": sha if required else None}})
    return {"schema_version": 2, "kind": "mcnf-surface-stack-provenance",
      "target": {"os": "fedora", "release": 44, "arch": "x86_64", "profile": "workstation-bootc", "bootc_base": "quay.io/fedora/fedora-bootc:44@sha256:" + "c" * 64},
      "signing_key": {"filename": "surface-signing-key.asc", "sha256": sha, "fingerprint": fpr, "rpm_signing_fingerprints": [fpr]}, "status": "ready", "blockers": [], "packages": rows}

def expect_invalid(name, function):
    try: function()
    except Invalid: return
    fail(f"self-test accepted hostile fixture: {name}")

def self_test():
    base = fixture(); validate(base)
    primary, admitted = signing_key_fingerprints(
        "pub:u:255:22:AAAA:1:2::u:::scSC:::::ed25519:::0:\n"
        + "fpr:::::::::" + "B"*40 + ":\n"
        + "sub:u:4096:1:CCCC:1:2:::::s::::::23:\n"
        + "fpr:::::::::" + "C"*40 + ":"
    )
    assert primary == "B"*40 and admitted == {"B"*40, "C"*40}
    assert [line for line in install_lock_lines(base, [], [], "d"*64) if line.startswith("SIGNER\t")] == ["SIGNER\t" + "B"*40]
    mutations = []
    def mutated(name, edit):
        value = copy.deepcopy(base); edit(value); mutations.append((name, value))
    mutated("mutable base tag", lambda x: x["target"].update({"bootc_base": "quay.io/fedora/fedora-bootc:44"}))
    mutated("wrong Fedora base", lambda x: x["target"].update({"bootc_base": "quay.io/fedora/fedora-bootc:45@sha256:" + "c"*64}))
    mutated("artifact traversal", lambda x: x["packages"][0]["rpm"].update({"filename": "../escape.rpm"}))
    mutated("duplicate artifact", lambda x: x["packages"][1]["rpm"].update({"filename": x["packages"][0]["rpm"]["filename"]}))
    mutated("duplicate source artifact", lambda x: x["packages"][1]["source"].update({"filename": x["packages"][0]["source"]["filename"]}))
    mutated("unconstrained package", lambda x: x["packages"][0]["rpm"].update({"nevra": "kernel-surface-latest.x86_64"}))
    mutated("different package signer", lambda x: x["packages"][0]["rpm"].update({"signing_fingerprint": "C"*40}))
    mutated("missing primary signer", lambda x: x["signing_key"].update({"rpm_signing_fingerprints": ["C"*40]}))
    mutated("lowercase fingerprint", lambda x: x["signing_key"].update({"fingerprint": "b"*40}))
    mutated("zero base digest", lambda x: x["target"].update({"bootc_base": "quay.io/fedora/fedora-bootc:44@sha256:" + "0"*64}))
    for name, value in mutations: expect_invalid(name, lambda value=value: validate(value))
    blocked = copy.deepcopy(base); blocked["blockers"] = []
    for row in blocked["packages"]:
        row["availability"], row["blocker"] = "unavailable", f"{row['name']} fixture unavailable"
        for section in (row["source"], row["rpm"]):
            for key in section: section[key] = None
        row["kernel_module_signing"]["signer"] = row["kernel_module_signing"]["certificate_sha256"] = None
        blocked["blockers"].append(row["blocker"])
    blocked["status"] = "blocked"
    blocked["target"]["bootc_base"] = None; blocked["signing_key"] = {"filename": None, "sha256": None, "fingerprint": None}
    validate(blocked)
    with tempfile.TemporaryDirectory(prefix="surface-hostile-") as temp:
        root = Path(temp); key = root / base["signing_key"]["filename"]; key.write_bytes(b"key")
        for row in base["packages"]:
            (root / row["rpm"]["filename"]).write_bytes(row["name"].encode())
            (root / row["source"]["filename"]).write_bytes((row["name"] + "-source").encode())
        measured = copy.deepcopy(base); measured["signing_key"]["sha256"] = sha256(key)
        for row in measured["packages"]:
            row["rpm"]["sha256"] = sha256(root / row["rpm"]["filename"])
            row["source"]["sha256"] = sha256(root / row["source"]["filename"])
        fake = lambda path: (next(r["rpm"]["nevra"] for r in measured["packages"] if r["rpm"]["filename"] == path.name), measured["signing_key"]["fingerprint"])
        verify_artifacts(measured, root, fake)
        (root / measured["packages"][0]["rpm"]["filename"]).write_bytes(b"tampered")
        expect_invalid("tampered RPM bytes", lambda: verify_artifacts(measured, root, fake))
        (root / measured["packages"][0]["rpm"]["filename"]).write_bytes(measured["packages"][0]["name"].encode())
        wrong_nevra = lambda path: ("wrong-1-1.x86_64", measured["signing_key"]["fingerprint"])
        expect_invalid("RPM identity mismatch", lambda: verify_artifacts(measured, root, wrong_nevra))
        wrong_signer = lambda path: (next(r["rpm"]["nevra"] for r in measured["packages"] if r["rpm"]["filename"] == path.name), "C" * 40)
        expect_invalid("RPM signer mismatch", lambda: verify_artifacts(measured, root, wrong_signer))
        (root / measured["packages"][0]["source"]["filename"]).write_bytes(b"tampered-source")
        expect_invalid("tampered source bytes", lambda: verify_artifacts(measured, root, fake))
        (root / measured["packages"][0]["source"]["filename"]).write_bytes((measured["packages"][0]["name"] + "-source").encode())
        assert admitted_signature_fingerprint(
            "Header V4 RSA/SHA256 Signature, key ID " + measured["signing_key"]["fingerprint"][-16:] + ": OK",
            set(measured["signing_key"]["rpm_signing_fingerprints"]),
        ) == measured["signing_key"]["fingerprint"]
        subkey = "C" * 40
        assert admitted_signature_fingerprint(
            "Header V4 RSA/SHA256 Signature, key ID " + subkey[-16:] + ": OK",
            {measured["signing_key"]["fingerprint"], subkey},
        ) == subkey
        expect_invalid("unsigned RPM output", lambda: admitted_signature_fingerprint("Header SHA256 digest: OK", set(measured["signing_key"]["rpm_signing_fingerprints"])))
        expect_invalid("wrong RPM key ID", lambda: admitted_signature_fingerprint("Header V4 RSA/SHA256 Signature, key ID DEADBEEF: OK", set(measured["signing_key"]["rpm_signing_fingerprints"])))
        key.unlink(); key.symlink_to("outside.asc")
        expect_invalid("symlink signing key", lambda: verify_artifacts(measured, root, fake))
        key.unlink(); key.write_bytes(b"key")
        (root / "extra.rpm").write_bytes(b"extra")
        expect_invalid("unmanifested artifact", lambda: verify_artifacts(measured, root, fake))
    print(f"Surface artifact provenance self-test passed ({len(mutations) + 9} hostile fixtures rejected)")

try:
    if sys.argv[4] == "true": self_test(); raise SystemExit(0)
    path = Path(sys.argv[1]); document = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=strict_object)
    result = validate(document)
    if result == "blocked":
        print("BLOCKED: Fedora 44 Surface stack release provenance is incomplete", file=sys.stderr)
        for blocker in document["blockers"]: print(f"  - {blocker}", file=sys.stderr)
        raise SystemExit(3)
    rows, sources = verify_artifacts(document, sys.argv[2])
    if sys.argv[3]:
        lock = Path(sys.argv[3]); manifest_sha = sha256(path)
        lines = install_lock_lines(document, rows, sources, manifest_sha)
        lock.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("OK: Fedora 44 Surface stack local artifacts, identities, signatures, and base digest are pinned")
except (OSError, UnicodeError, json.JSONDecodeError) as exc:
    print(f"FATAL: cannot read strict provenance input: {exc}", file=sys.stderr); raise SystemExit(1)
except Invalid as exc:
    print(f"FATAL: {exc}", file=sys.stderr); raise SystemExit(1)
PY
