#!/usr/bin/env python3
"""Finalize a complete Fedora 44 Surface stack from already-signed RPMs.

This helper never signs.  It consumes the immutable producer outputs and the
result of the operator-only ``sign-release.sh`` flow, verifies every binding,
then atomically emits a ready schema-v2 candidate plus its exact artifact set.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import resource
import shutil
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCK = ROOT / "packaging/surface/surface-build-inputs.f44.json"
VERIFIER = ROOT / "install-helpers/verify-surface-stack.sh"
DEFAULT_KEY = ROOT / "packaging/repo/RPM-GPG-KEY-magic-mesh"
PACKAGES = ("kernel-surface", "iptsd", "libwacom-surface", "surface-control", "surface-secureboot")
BASE_RE = re.compile(r"quay\.io/fedora/fedora-bootc:44@sha256:[0-9a-f]{64}\Z")
SAFE_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+~-]{0,190}\Z")
SUM_LINE = re.compile(r"([0-9a-f]{64})  ([A-Za-z0-9][A-Za-z0-9._+~-]{0,190})\Z")
MAX_JSON = 2 * 1024 * 1024
MAX_RPM = 2 * 1024 * 1024 * 1024
MAX_MODULE = 64 * 1024 * 1024
MAX_MODULES = 4096
MAX_TOOL_OUTPUT = 4 * 1024 * 1024
USERSPACE_RPMS = {
    "iptsd": {"iptsd-3.1.0-1.fc44.src.rpm", "iptsd-3.1.0-1.fc44.x86_64.rpm"},
    "libwacom-surface": {
        "libwacom-surface-2.17.0-1.fc44.src.rpm",
        "libwacom-surface-2.17.0-1.fc44.x86_64.rpm",
        "libwacom-surface-data-2.17.0-1.fc44.noarch.rpm",
        "libwacom-surface-devel-2.17.0-1.fc44.x86_64.rpm",
        "libwacom-surface-utils-2.17.0-1.fc44.x86_64.rpm",
    },
    "surface-control": {"surface-control-0.5.0-1.fc44.src.rpm", "surface-control-0.5.0-1.fc44.x86_64.rpm"},
    "surface-secureboot": {
        "surface-secureboot-20251230-1.fc44.noarch.rpm",
        "surface-secureboot-20251230-1.fc44.src.rpm",
    },
}


class Refusal(RuntimeError):
    """A fail-closed contract refusal."""


def refuse(message: str) -> None:
    raise Refusal(message)


def strict_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            refuse(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def load_json(path: Path) -> dict:
    regular(path, max_bytes=MAX_JSON)
    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=strict_object)
    except (UnicodeError, json.JSONDecodeError) as error:
        refuse(f"invalid JSON {path.name}: {error}")
    if not isinstance(value, dict):
        refuse(f"JSON root is not an object: {path.name}")
    return value


def regular(path: Path, *, max_bytes: int | None = None) -> Path:
    try:
        info = path.lstat()
    except OSError as error:
        refuse(f"required file is unavailable: {path}: {error}")
    if path.is_symlink() or not path.is_file() or info.st_size == 0:
        refuse(f"required artifact is not a non-empty regular file: {path}")
    if max_bytes is not None and info.st_size > max_bytes:
        refuse(f"artifact exceeds its size bound: {path.name}")
    return path


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with regular(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def limit_tool_output() -> None:
    resource.setrlimit(resource.RLIMIT_FSIZE, (MAX_TOOL_OUTPUT, MAX_TOOL_OUTPUT))


def limit_module_output() -> None:
    resource.setrlimit(resource.RLIMIT_FSIZE, (MAX_MODULE, MAX_MODULE))


def run(argv: list[str], what: str, *, env: dict[str, str] | None = None) -> str:
    process = None
    try:
        with tempfile.TemporaryFile() as capture:
            process = subprocess.Popen(
                argv, stdin=subprocess.DEVNULL, stdout=capture,
                stderr=subprocess.STDOUT,
                env=env or {"PATH": "/usr/sbin:/usr/bin", "LANG": "C", "LC_ALL": "C"},
                start_new_session=True, preexec_fn=limit_tool_output,
            )
            try:
                returncode = process.wait(timeout=60)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=10)
                refuse(f"{what} exceeded its 60-second time bound")
            size = capture.tell()
            if size >= MAX_TOOL_OUTPUT:
                refuse(f"{what} exceeded its output-size bound")
            capture.seek(0)
            raw = capture.read()
    except (OSError, subprocess.TimeoutExpired) as error:
        if process is not None and process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=10)
        refuse(f"{what} could not run: {error}")
    try:
        output = raw.decode("utf-8", errors="strict").strip()
    except UnicodeDecodeError:
        refuse(f"{what} emitted non-UTF-8 output")
    if returncode != 0:
        refuse(f"{what} failed: {output[:2000]}")
    return output


def exact_directory(root: Path, expected: set[str]) -> None:
    if not root.is_dir() or root.is_symlink():
        refuse(f"input is not a real directory: {root}")
    actual = set()
    for path in root.iterdir():
        if not SAFE_NAME.fullmatch(path.name):
            refuse(f"unsafe directory entry: {path.name!r}")
        regular(path)
        actual.add(path.name)
    if actual != expected:
        refuse(f"directory set differs for {root} (missing={sorted(expected-actual)}, unknown={sorted(actual-expected)})")


def checksum_map(path: Path, expected: set[str]) -> dict[str, str]:
    regular(path, max_bytes=MAX_JSON)
    rows: dict[str, str] = {}
    for line in path.read_text(encoding="ascii").splitlines():
        match = SUM_LINE.fullmatch(line)
        if not match or match.group(2) in rows:
            refuse(f"malformed or duplicate checksum row in {path.name}")
        rows[match.group(2)] = match.group(1)
    if set(rows) != expected:
        refuse(f"checksum set differs in {path.name}")
    return rows


def verify_checksums(root: Path, expected: set[str]) -> None:
    rows = checksum_map(root / "SHA256SUMS", expected)
    for name, wanted in rows.items():
        if digest(root / name) != wanted:
            refuse(f"SHA-256 mismatch: {name}")


def verify_source_bundle(root: Path, lock: dict) -> dict[str, Path]:
    lock_sha = digest(LOCK)
    inputs = {row["id"]: row for row in lock["inputs"]}
    expected = {row["filename"] for row in inputs.values()} | {"build-input-lock.json", "SHA256SUMS"}
    exact_directory(root, expected)
    if digest(root / "build-input-lock.json") != lock_sha:
        refuse("source bundle lock does not match the governed repository lock")
    verify_checksums(root, expected - {"SHA256SUMS"})
    for row in inputs.values():
        if digest(root / row["filename"]) != row["sha256"]:
            refuse(f"locked source digest mismatch: {row['id']}")
    return {name: root / row["filename"] for name, row in inputs.items()}


def verify_producer(root: Path, package: str, lock: dict) -> tuple[dict, list[Path]]:
    manifest = load_json(root / "build-manifest.json")
    kind = "mcnf-surface-kernel-build" if package == "kernel-surface" else "mcnf-surface-userspace-build"
    if manifest.get("schema_version") != 1 or manifest.get("kind") != kind or manifest.get("package") != package:
        refuse(f"{package} producer manifest identity differs")
    if manifest.get("target") != {"os": "fedora", "release": 44, "arch": "x86_64"}:
        refuse(f"{package} producer target differs")
    if manifest.get("builder_image") != lock["builder_image"]:
        refuse(f"{package} producer builder digest differs")
    mapping = next(row for row in lock["packages"] if row["name"] == package)
    locked = {row["id"]: row for row in lock["inputs"]}
    expected_sources = [
        {key: locked[item][key] for key in ("id", "filename", "commit", "sha256")}
        if package == "kernel-surface"
        else {key: locked[item][key] for key in ("id", "filename", "commit")}
        for item in mapping["input_ids"]
    ]
    if manifest.get("source_inputs") != expected_sources:
        refuse(f"{package} producer source binding differs")
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        refuse(f"{package} producer artifact set is empty")
    names = [row.get("filename") for row in artifacts] if package == "kernel-surface" else artifacts
    if any(not isinstance(name, str) or not name.endswith(".rpm") or not SAFE_NAME.fullmatch(name) for name in names):
        refuse(f"{package} producer artifact name is unsafe")
    if len(names) != len(set(names)):
        refuse(f"{package} producer artifact set contains duplicates")
    if package == "kernel-surface":
        if len(names) > 64:
            refuse("kernel producer RPM count exceeds its contract")
    elif set(names) != USERSPACE_RPMS[package]:
        refuse(f"{package} producer RPM set differs from its locked build contract")
    metadata = {"build-manifest.json", "build-environment-rpm-nevra.txt", "SHA256SUMS"}
    if package == "kernel-surface":
        metadata.add("signing-manifest.json")
    exact_directory(root, set(names) | metadata)
    verify_checksums(root, (set(names) | metadata) - {"SHA256SUMS"})
    for path in (root / name for name in names):
        regular(path, max_bytes=MAX_RPM)
    if package != "kernel-surface" and manifest.get("signed") is not False:
        refuse(f"{package} producer must be the explicit unsigned build output")
    if package == "kernel-surface":
        for row in artifacts:
            if row.get("rpm_signature") != "unsigned" or digest(root / row["filename"]) != row.get("sha256"):
                refuse("kernel producer artifact digest/signature assertion differs")
        signing = load_json(root / "signing-manifest.json")
        if signing.get("kind") != "mcnf-surface-kernel-signing" or signing.get("kernel_modules", {}).get("signer_asserted") is not False:
            refuse("kernel producer signing boundary is not the expected verification-pending state")
    return manifest, [root / name for name in names]


def key_fingerprints(key: Path) -> tuple[str, set[str]]:
    output = run(["gpg", "--batch", "--show-keys", "--with-colons", str(key)], "release public-key inspection")
    primary, admitted, pending, pub_count = None, set(), None, 0
    for line in output.splitlines():
        fields = line.split(":")
        if fields[0] in {"pub", "sub"}:
            pending = (fields[0], len(fields) > 11 and "s" in fields[11].lower())
            if fields[0] == "pub":
                pub_count += 1
        elif fields[0] == "fpr" and pending is not None:
            if len(fields) <= 9:
                refuse("release key contains a malformed fingerprint record")
            fingerprint = fields[9].upper()
            if not re.fullmatch(r"[0-9A-F]{40}(?:[0-9A-F]{24})?", fingerprint):
                refuse("release key contains a malformed fingerprint")
            if pending[0] == "pub":
                primary = fingerprint
            if pending[1]:
                admitted.add(fingerprint)
            pending = None
    if pub_count != 1 or primary is None or primary not in admitted:
        refuse("release key artifact must contain one signing-capable primary key")
    return primary, admitted


def require_admitted_rpm_signature(signature: str, admitted: set[str], name: str) -> str:
    ids = re.findall(r"(?i)signature.*key ID ([0-9a-f]{8,16}).*: OK", signature)
    matches = []
    for key_id in ids:
        found = [item for item in admitted if item.endswith(key_id.upper())]
        if len(found) != 1:
            refuse(f"RPM signature is not uniquely bound to the governed release key or signing subkey: {name}")
        matches.append(found[0])
    if not matches or len(set(matches)) != 1:
        refuse(f"RPM signatures do not have one consistent governed signer: {name}")
    return matches[0]


def require_same_payload(unsigned_value: str, signed_value: str, name: str) -> None:
    pattern = re.compile(r"[A-Za-z0-9_-]+:[0-9A-Fa-f]{64}\Z")
    if unsigned_value != signed_value or not pattern.fullmatch(unsigned_value):
        refuse(f"release signing changed or obscured the producer RPM payload: {name}")


def verify_signed_bundle(root: Path, unsigned: list[Path], key: Path) -> tuple[dict[str, Path], str, set[str], dict[str, str]]:
    unsigned_by_name = {path.name: path for path in unsigned}
    if len(unsigned_by_name) != len(unsigned):
        refuse("producer RPM basenames collide")
    expected = set(unsigned_by_name)
    exact_directory(root, expected | {"SHA256SUMS", "SHA256SUMS.asc"})
    verify_checksums(root, expected)
    fingerprint, admitted_fingerprints = key_fingerprints(key)
    rpm_signers = {}
    with tempfile.TemporaryDirectory(prefix="surface-finalize-gpg-") as home, tempfile.TemporaryDirectory(prefix="surface-finalize-rpmdb-") as rpmdb:
        os.chmod(home, 0o700)
        env = {"PATH": "/usr/sbin:/usr/bin", "LANG": "C", "LC_ALL": "C", "GNUPGHOME": home}
        run(["gpg", "--batch", "--import", str(key)], "release key import", env=env)
        envelope = run(
            ["gpg", "--batch", "--status-fd=1", "--verify", str(root / "SHA256SUMS.asc"), str(root / "SHA256SUMS")],
            "signed checksum-envelope verification", env=env,
        )
        valid = [line.split() for line in envelope.splitlines() if line.startswith("[GNUPG:] VALIDSIG ")]
        envelope_signer = valid[0][2].upper() if len(valid) == 1 and len(valid[0]) >= 3 else ""
        if (envelope_signer not in admitted_fingerprints
                or (envelope_signer != fingerprint and valid[0][-1].upper() != fingerprint)):
            refuse("checksum envelope is not signed by the governed release key")
        run(["rpm", "--dbpath", rpmdb, "--initdb"], "temporary RPM database initialization")
        run(["rpm", "--dbpath", rpmdb, "--import", str(key)], "release RPM key import")
        for name, old in unsigned_by_name.items():
            signed = root / name
            signature = run(["rpmkeys", "--dbpath", rpmdb, "--checksig", "--verbose", str(signed)], f"RPM signature verification for {name}")
            rpm_signers[name] = require_admitted_rpm_signature(signature, admitted_fingerprints, name)
            old_payload = run(["rpm", "-qp", "--qf", "%{PAYLOADDIGESTALGO}:%{PAYLOADDIGEST}", str(old)], f"unsigned payload identity for {name}")
            new_payload = run(["rpm", "-qp", "--qf", "%{PAYLOADDIGESTALGO}:%{PAYLOADDIGEST}", str(signed)], f"signed payload identity for {name}")
            require_same_payload(old_payload, new_payload, name)
    return {name: root / name for name in expected}, fingerprint, admitted_fingerprints, rpm_signers


def rpm_name(path: Path) -> str:
    return run(["rpm", "-qp", "--qf", "%{NAME}", str(path)], f"RPM package-name inspection for {path.name}")


def rpm_arch(path: Path) -> str:
    return run(["rpm", "-qp", "--qf", "%{ARCH}", str(path)], f"RPM architecture inspection for {path.name}")


def rpm_nevra(path: Path) -> str:
    return run(["rpm", "-qp", "--qf", "%{NAME}-%|EPOCH?{%{EPOCH}:}:{}|%{VERSION}-%{RELEASE}.%{ARCH}", str(path)], f"RPM NEVRA inspection for {path.name}")


def selected_rpms(signed: dict[str, Path]) -> dict[str, Path]:
    rows: dict[str, Path] = {}
    for path in signed.values():
        name = rpm_name(path)
        if name in PACKAGES and rpm_arch(path) != "src":
            if name in rows:
                refuse(f"more than one exact install RPM exists for {name}")
            rows[name] = path
    if set(rows) != set(PACKAGES):
        refuse(f"exact install RPM set differs (missing={sorted(set(PACKAGES)-set(rows))})")
    return rows


def certificate_details(certificate: Path) -> tuple[str, str, str, str, str]:
    regular(certificate, max_bytes=1024 * 1024)
    cert_sha = digest(certificate)
    subject = run(["openssl", "x509", "-in", str(certificate), "-noout", "-subject", "-nameopt", "RFC2253"], "Surface certificate subject")
    subject = subject.removeprefix("subject=")
    text = run(["openssl", "x509", "-in", str(certificate), "-noout", "-text"], "Surface certificate key identifier")
    match = re.search(r"Subject Key Identifier:\s*\n\s*([0-9A-Fa-f:]+)", text)
    if not match:
        refuse("Surface certificate has no subject key identifier")
    key_id = re.sub(r"[^0-9A-F]", "", match.group(1).upper())
    public = run(["openssl", "x509", "-in", str(certificate), "-pubkey", "-noout"], "Surface certificate public key")
    try:
        converted = subprocess.run(
            ["openssl", "pkey", "-pubin", "-outform", "DER"],
            input=(public + "\n").encode(), stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, check=False,
            env={"PATH": "/usr/sbin:/usr/bin", "LANG": "C", "LC_ALL": "C"}, timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        refuse(f"Surface certificate public-key conversion could not run: {error}")
    if converted.returncode != 0 or not converted.stdout:
        refuse("Surface certificate public-key conversion failed")
    public_sha = hashlib.sha256(converted.stdout).hexdigest()
    # DER is binary, so obtain its digest directly rather than from the text helper.
    try:
        der_result = subprocess.run(
            ["openssl", "x509", "-in", str(certificate), "-outform", "DER"],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False,
            env={"PATH": "/usr/sbin:/usr/bin", "LANG": "C", "LC_ALL": "C"}, timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        refuse(f"Surface certificate DER conversion could not run: {error}")
    if der_result.returncode != 0 or not der_result.stdout:
        refuse("Surface certificate DER conversion failed")
    der_sha = hashlib.sha256(der_result.stdout).hexdigest()
    return cert_sha, der_sha, subject, key_id, public_sha


def extract_member(rpm: Path, member: str, output: Path) -> None:
    if not member.startswith("/usr/") or ".." in Path(member).parts:
        refuse(f"unsafe RPM member path: {member}")
    fixed_env = {"PATH": "/usr/sbin:/usr/bin", "LANG": "C", "LC_ALL": "C"}
    first = None
    try:
        with output.open("wb") as stream:
            first = subprocess.Popen(
                ["rpm2cpio", str(rpm)], stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=fixed_env,
                start_new_session=True,
            )
            assert first.stdout is not None
            try:
                second = subprocess.run(
                    ["cpio", "--quiet", "-i", "--to-stdout", f".{member}"],
                    stdin=first.stdout, stdout=stream, stderr=subprocess.DEVNULL,
                    timeout=60, check=False, env=fixed_env,
                    start_new_session=True, preexec_fn=limit_module_output,
                )
            finally:
                first.stdout.close()
            first_rc = first.wait(timeout=10)
    except (OSError, subprocess.TimeoutExpired) as error:
        if first is not None and first.poll() is None:
            first.kill()
            first.wait()
        output.unlink(missing_ok=True)
        refuse(f"bounded RPM member extraction failed for {member}: {error}")
    if first_rc != 0 or second.returncode != 0 or not output.is_file() or output.stat().st_size == 0:
        refuse(f"could not extract required RPM member: {member}")


def require_module_signature(signer: str, sig_key: str, sig_id: str, cert_key_id: str, member: str) -> None:
    normalized = re.sub(r"[^0-9A-F]", "", sig_key.upper())
    if (not signer or sig_id != "PKCS#7" or not normalized
            or not (cert_key_id.endswith(normalized) or normalized.endswith(cert_key_id))):
        refuse(f"module signature is not bound to the supplied Surface certificate: {member}")


def verify_module_binding(kernel_rpms: list[Path], certificate: Path, kernel_manifest: dict) -> tuple[str, str]:
    cert_sha, der_sha, subject, cert_key_id, public_sha = certificate_details(certificate)
    signing = load_json(kernel_manifest["root"] / "signing-manifest.json")
    cert_row = signing.get("certificate", {})
    cert_fingerprint = ":".join(der_sha[index:index + 2] for index in range(0, 64, 2)).upper()
    if (cert_row.get("file_sha256") != cert_sha
            or cert_row.get("public_key_sha256") != public_sha
            or cert_row.get("subject") != subject
            or cert_row.get("sha256_fingerprint") != cert_fingerprint):
        refuse("kernel producer certificate/public-key binding differs")
    signers: set[str] = set()
    module_count = 0
    with tempfile.TemporaryDirectory(prefix="surface-finalize-modules-") as temp:
        temp_root = Path(temp)
        for rpm in kernel_rpms:
            members = [line for line in run(["rpm", "-qpl", str(rpm)], f"kernel payload listing for {rpm.name}").splitlines() if re.search(r"\.ko(?:\.(?:xz|zst|gz))?\Z", line)]
            if len(members) > 4096:
                refuse("kernel module count exceeds bound")
            for member in members:
                module_count += 1
                if module_count > MAX_MODULES:
                    refuse("total kernel module count exceeds bound")
                target = temp_root / f"module-{module_count}.ko{''.join(Path(member).suffixes[1:])}"
                extract_member(rpm, member, target)
                if target.stat().st_size > MAX_MODULE:
                    refuse("kernel module exceeds inspection bound")
                signer = run(["modinfo", "-F", "signer", str(target)], f"module signer inspection for {member}")
                sig_key = run(["modinfo", "-F", "sig_key", str(target)], f"module key inspection for {member}")
                sig_id = run(["modinfo", "-F", "sig_id", str(target)], f"module signature-format inspection for {member}")
                require_module_signature(signer, sig_key, sig_id, cert_key_id, member)
                signers.add(signer)
    if module_count == 0 or len(signers) != 1:
        refuse("kernel output has no modules or inconsistent module signers")
    signer = next(iter(signers))
    if f"CN={signer}" not in subject.split(","):
        refuse("module signer does not equal the Surface certificate common name")
    return signer, der_sha


def verify_packaged_certificate(rpm: Path, certificate: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="surface-finalize-cert-") as temp:
        extracted = Path(temp) / "surface.cer"
        extract_member(rpm, "/usr/share/surface-secureboot/surface.cer", extracted)
        if digest(extracted) != digest(certificate):
            refuse("surface-secureboot payload certificate differs from the kernel signing certificate")


def finalize(args) -> None:
    if not BASE_RE.fullmatch(args.bootc_base) or args.bootc_base.endswith("0" * 64):
        refuse("--bootc-base must be a non-zero digest-pinned Fedora 44 bootc reference")
    output = args.output.resolve(strict=False)
    if output.exists() or not output.parent.is_dir():
        refuse("--output must name a new directory under an existing parent")
    lock = load_json(LOCK)
    sources = verify_source_bundle(args.source_bundle, lock)
    regular(args.release_key, max_bytes=1024 * 1024)
    regular(args.certificate, max_bytes=1024 * 1024)
    if digest(args.certificate) != digest(sources["surface-certificate"]):
        refuse("supplied Surface certificate differs from the locked source bundle")
    outputs = {"kernel-surface": args.kernel_output, "iptsd": args.iptsd_output, "libwacom-surface": args.libwacom_output, "surface-control": args.surface_control_output, "surface-secureboot": args.surface_secureboot_output}
    all_unsigned: list[Path] = []
    manifests = {}
    for package, root in outputs.items():
        manifest, artifacts = verify_producer(root, package, lock)
        manifests[package] = {"value": manifest, "root": root}
        all_unsigned.extend(artifacts)
    signed, fingerprint, admitted_fingerprints, rpm_signers = verify_signed_bundle(args.signed_dir, all_unsigned, args.release_key)
    chosen = selected_rpms(signed)
    kernel_signed = [path for path in signed.values() if rpm_name(path).startswith("kernel-surface")]
    signer, cert_sha = verify_module_binding(kernel_signed, args.certificate, manifests["kernel-surface"])
    verify_packaged_certificate(chosen["surface-secureboot"], args.certificate)
    lock_inputs = {row["id"]: row for row in lock["inputs"]}
    package_map = {row["name"]: row["input_ids"][0] for row in lock["packages"]}
    stage = Path(tempfile.mkdtemp(prefix=".surface-finalize-", dir=output.parent))
    try:
        artifacts = stage / "artifacts"
        artifacts.mkdir(mode=0o700)
        key_name = "RPM-GPG-KEY-magic-mesh.asc"
        shutil.copyfile(args.release_key, artifacts / key_name)
        rows = []
        used_sources = set()
        for package in PACKAGES:
            source = lock_inputs[package_map[package]]
            source_name = source["filename"]
            if source_name in used_sources:
                refuse("candidate source filenames are not unique")
            used_sources.add(source_name)
            shutil.copyfile(sources[package_map[package]], artifacts / source_name)
            shutil.copyfile(chosen[package], artifacts / chosen[package].name)
            required = package in {"kernel-surface", "surface-secureboot"}
            rows.append({
                "name": package, "availability": "ready", "blocker": None,
                "source": {"filename": source_name, "url": source["url"], "ref": source["ref"], "sha256": source["sha256"], "license": source["license"]},
                "rpm": {"filename": chosen[package].name, "nevra": rpm_nevra(chosen[package]), "sha256": digest(chosen[package]), "signing_fingerprint": rpm_signers[chosen[package].name]},
                "kernel_module_signing": {"applicability": "required" if required else "not-applicable", "signer": signer if required else None, "certificate_sha256": cert_sha if required else None},
            })
        candidate = {
            "schema_version": 2, "kind": "mcnf-surface-stack-provenance",
            "target": {"os": "fedora", "release": 44, "arch": "x86_64", "profile": "workstation-bootc", "bootc_base": args.bootc_base},
            "signing_key": {"filename": key_name, "sha256": digest(artifacts / key_name), "fingerprint": fingerprint, "rpm_signing_fingerprints": sorted(admitted_fingerprints)},
            "status": "ready", "blockers": [], "packages": rows,
        }
        manifest = stage / "surface-stack.f44.json"
        manifest.write_text(json.dumps(candidate, indent=2) + "\n", encoding="utf-8")
        run([str(VERIFIER), "--manifest", str(manifest), "--artifact-dir", str(artifacts), "--emit-lock", str(stage / "surface-stack.install.lock")], "final Surface candidate verification")
        os.rename(stage, output)
    except Exception:
        shutil.rmtree(stage, ignore_errors=True)
        raise
    print(f"READY: verified Surface stack candidate emitted at {output}")


def self_test() -> None:
    failures = 0
    def rejected(callable_):
        nonlocal failures
        try:
            callable_()
        except Refusal:
            failures += 1
            return
        raise AssertionError("hostile fixture was accepted")
    with tempfile.TemporaryDirectory(prefix="surface-finalizer-self-test-") as temp:
        root = Path(temp)
        (root / "a").write_bytes(b"a")
        (root / "SHA256SUMS").write_text(f"{digest(root / 'a')}  a\n", encoding="ascii")
        verify_checksums(root, {"a"})
        (root / "a").write_bytes(b"tamper")
        rejected(lambda: verify_checksums(root, {"a"}))
        (root / "SHA256SUMS").write_text("bad\n", encoding="ascii")
        rejected(lambda: checksum_map(root / "SHA256SUMS", {"a"}))
        (root / "duplicate.json").write_text('{"a":1,"a":2}', encoding="utf-8")
        rejected(lambda: load_json(root / "duplicate.json"))
        (root / "link").symlink_to("a")
        rejected(lambda: regular(root / "link"))
        rejected(lambda: exact_directory(root, {"a"}))
        require_same_payload("8:" + "a" * 64, "8:" + "a" * 64, "good.rpm")
        rejected(lambda: require_same_payload("8:" + "a" * 64, "8:" + "b" * 64, "changed.rpm"))
        rejected(lambda: require_same_payload("(none):(none)", "(none):(none)", "unsigned.rpm"))
        require_module_signature("Surface Secure Boot", "AA:BB", "PKCS#7", "AABB", "good.ko")
        rejected(lambda: require_module_signature("", "AA:BB", "PKCS#7", "AABB", "empty-signer.ko"))
        rejected(lambda: require_module_signature("Surface Secure Boot", "CCDD", "PKCS#7", "AABB", "wrong-key.ko"))
        rejected(lambda: require_module_signature("Surface Secure Boot", "AABB", "unknown", "AABB", "wrong-format.ko"))
        rejected(lambda: run(["python3", "-c", "import os; os.write(1, b'x' * (4 * 1024 * 1024 + 1))"], "oversize fixture"))
        rejected(lambda: run(["python3", "-c", "import os; os.write(1, b'\\xff')"], "binary fixture"))
        primary = "B54633C4C87F83987A80B660C709EE87704EB07A"
        subkey = "7D1D17102507AA33167D14C5E8EAC651D0921C73"
        require_admitted_rpm_signature(
            "Header V4 RSA/SHA256 Signature, key ID E8EAC651D0921C73: OK",
            {primary, subkey}, "subkey-signed.rpm",
        )
        rejected(lambda: require_admitted_rpm_signature(
            "Header V4 RSA/SHA256 Signature, key ID DEADBEEF: OK",
            {primary, subkey}, "wrong-key.rpm",
        ))
    if failures != 13:
        raise AssertionError(f"expected 13 hostile refusals, saw {failures}")
    print("Surface stack finalizer self-test passed (13 hostile fixtures rejected)")


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(description=__doc__)
    value.add_argument("--self-test", action="store_true")
    value.add_argument("--kernel-output", type=Path)
    value.add_argument("--iptsd-output", type=Path)
    value.add_argument("--libwacom-output", type=Path)
    value.add_argument("--surface-control-output", type=Path)
    value.add_argument("--surface-secureboot-output", type=Path)
    value.add_argument("--source-bundle", type=Path)
    value.add_argument("--signed-dir", type=Path)
    value.add_argument("--release-key", type=Path, default=DEFAULT_KEY)
    value.add_argument("--certificate", type=Path)
    value.add_argument("--bootc-base")
    value.add_argument("--output", type=Path)
    return value


def main() -> int:
    args = parser().parse_args()
    if args.self_test:
        self_test()
        return 0
    required = ("kernel_output", "iptsd_output", "libwacom_output", "surface_control_output", "surface_secureboot_output", "source_bundle", "signed_dir", "certificate", "bootc_base", "output")
    missing = ["--" + name.replace("_", "-") for name in required if getattr(args, name) is None]
    if missing:
        parser().error("required arguments missing: " + ", ".join(missing))
    finalize(args)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Refusal as error:
        print(f"REFUSED: {error}", file=sys.stderr)
        raise SystemExit(1)
