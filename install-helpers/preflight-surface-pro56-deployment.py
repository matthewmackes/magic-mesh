#!/usr/bin/python3
"""Read-only, fail-closed deployment/access preflight for Surface Pro 5/6."""

from __future__ import annotations

import argparse
import ast
import base64
import hashlib
import ipaddress
import json
import os
import re
import selectors
import signal
import stat
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


SCHEMA_VERSION = 1
MAX_OUTPUT_BYTES = 64 * 1024
MAX_INPUT_BYTES = 512 * 1024
COMMAND_TIMEOUT_SECONDS = 8
MAX_COMMAND_CAPTURE_BYTES = 64 * 1024
PRO6_ENDPOINTS = (("lan", "172.20.146.79"), ("overlay", "10.42.0.7"))
PRO5_SKUS = {"Surface_Pro_1796", "Surface_Pro_1807"}
REVISION = re.compile(r"^[0-9a-f]{40}$")
SHORT_REVISION = re.compile(r"^[0-9a-f]{7,40}$")
SAFE_REASON = re.compile(r"^[A-Za-z0-9][A-Za-z0-9 ._:/+(),'-]{0,511}$")
EXPECTED_PACKAGES = {
    "kernel-surface",
    "iptsd",
    "libwacom-surface",
    "surface-control",
    "surface-secureboot",
}


class PreflightError(RuntimeError):
    pass


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def bounded_regular(path: Path, maximum: int = MAX_INPUT_BYTES) -> bytes:
    metadata = path.lstat()
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise PreflightError(f"required input is not a regular non-symlink file: {path.name}")
    if metadata.st_size <= 0 or metadata.st_size > maximum:
        raise PreflightError(f"required input has an invalid bounded size: {path.name}")
    with path.open("rb") as stream:
        data = stream.read(maximum + 1)
    if len(data) > maximum:
        raise PreflightError(f"required input exceeds {maximum} bytes: {path.name}")
    return data


def safe_reason(value: Any, fallback: str) -> str:
    if not isinstance(value, str):
        return fallback
    value = re.sub(r"(?i)\b(?:bearer|token|password|secret|private[-_ ]?key)\s*[:=]\s*\S+", "credential=[REDACTED]", value)
    value = re.sub(r"(?<![0-9])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![0-9])", "[REDACTED-IP]", value)
    value = re.sub(r"(?i)\b(?:[0-9a-f]{2}:){5}[0-9a-f]{2}\b", "[REDACTED-MAC]", value)
    value = re.sub(r"[\x00-\x1f\x7f]", "?", value).strip()[:512]
    return value if SAFE_REASON.fullmatch(value) else fallback


def _kill_and_reap(process: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    for stream in (process.stdout, process.stderr):
        if stream is not None:
            stream.close()
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def run(
    argv: list[str],
    timeout: int = COMMAND_TIMEOUT_SECONDS,
    maximum: int = MAX_COMMAND_CAPTURE_BYTES,
) -> subprocess.CompletedProcess[bytes]:
    process = subprocess.Popen(
        argv,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env={"PATH": "/usr/sbin:/usr/bin", "LANG": "C", "LC_ALL": "C"},
        start_new_session=True,
    )
    assert process.stdout is not None and process.stderr is not None
    streams = {process.stdout: bytearray(), process.stderr: bytearray()}
    deadline = time.monotonic() + timeout
    selector = selectors.DefaultSelector()
    try:
        for stream in streams:
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ)
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                _kill_and_reap(process)
                raise subprocess.TimeoutExpired(argv, timeout)
            events = selector.select(min(remaining, 0.25))
            for key, _mask in events:
                stream = key.fileobj
                chunk = os.read(stream.fileno(), 65536)
                if not chunk:
                    selector.unregister(stream)
                    continue
                streams[stream].extend(chunk)
                if sum(len(value) for value in streams.values()) > maximum:
                    _kill_and_reap(process)
                    raise PreflightError("command-output-exceeded-bounded-capture")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            _kill_and_reap(process)
            raise subprocess.TimeoutExpired(argv, timeout)
        returncode = process.wait(timeout=remaining)
    except subprocess.TimeoutExpired:
        if process.poll() is None:
            _kill_and_reap(process)
        raise
    finally:
        selector.close()
    return subprocess.CompletedProcess(
        argv,
        returncode,
        bytes(streams[process.stdout]),
        bytes(streams[process.stderr]),
    )


def local_revision(repo: Path) -> dict[str, Any]:
    try:
        admission = f"safe.directory={repo}"
        result = run(["/usr/bin/git", "-c", admission, "-C", str(repo), "rev-parse", "HEAD"])
        status = run(["/usr/bin/git", "-c", admission, "-C", str(repo), "status", "--porcelain=v1", "--untracked-files=no"])
    except (OSError, subprocess.TimeoutExpired, PreflightError):
        return {"status": "blocked", "revision": None, "tracked_checkout_clean": None, "reason": "local-git-probe-failed"}
    revision = result.stdout.decode("ascii", errors="ignore").strip().lower()
    if result.returncode != 0 or REVISION.fullmatch(revision) is None:
        return {"status": "blocked", "revision": None, "tracked_checkout_clean": None, "reason": "local-revision-unavailable"}
    return classify_local_revision(revision, status)


def classify_local_revision(
    revision: str, status: subprocess.CompletedProcess[bytes]
) -> dict[str, Any]:
    clean = status.returncode == 0 and not status.stdout
    return {
        "status": "ready" if clean else "blocked",
        "revision": revision,
        "tracked_checkout_clean": clean,
        "reason": None if clean else ("local-checkout-not-clean" if status.returncode == 0 else "local-git-status-failed"),
    }


def artifact_manifest(repo: Path, manifest: Path) -> dict[str, Any]:
    verifier = repo / "install-helpers/verify-surface-stack.sh"
    artifact_dir = repo / "packaging/surface/artifacts"
    blockers: list[str] = []
    manifest_status = "invalid"
    package_names: set[str] = set()
    try:
        raw = bounded_regular(manifest)
        value = json.loads(raw, object_pairs_hook=strict_object)
        if not isinstance(value, dict):
            raise PreflightError("manifest root is not an object")
        manifest_status = value.get("status") if value.get("status") in {"ready", "blocked"} else "invalid"
        packages = value.get("packages")
        if isinstance(packages, list):
            package_names = {row.get("name") for row in packages if isinstance(row, dict) and isinstance(row.get("name"), str)}
        raw_blockers = value.get("blockers")
        if isinstance(raw_blockers, list):
            blockers = [safe_reason(item, "manifest-blocker-redacted") for item in raw_blockers[:16]]
    except (OSError, UnicodeError, json.JSONDecodeError, PreflightError, ValueError) as exc:
        return {
            "status": "blocked",
            "manifest_status": manifest_status,
            "manifest_sha256": None,
            "package_set_complete": False,
            "signature_verification": "not-run",
            "blockers": [safe_reason(str(exc), "manifest-read-failed")],
        }
    if not verifier.is_file() or verifier.is_symlink():
        verification = "verifier-unavailable"
        return {
            "status": "blocked",
            "manifest_status": manifest_status,
            "manifest_sha256": sha256(manifest),
            "package_set_complete": package_names == EXPECTED_PACKAGES,
            "signature_verification": verification,
            "blockers": blockers + [verification],
        }
    try:
        verified = run([str(verifier), "--manifest", str(manifest)], timeout=30)
        verification = "passed" if verified.returncode == 0 else ("blocked" if verified.returncode == 3 else "failed")
    except (OSError, subprocess.TimeoutExpired, PreflightError):
        verification = "failed"
    ready = manifest_status == "ready" and package_names == EXPECTED_PACKAGES and verification == "passed"
    if not ready and not blockers:
        blockers.append("complete signed Surface artifact manifest is unavailable")
    return {
        "status": "ready" if ready else "blocked",
        "manifest_status": manifest_status,
        "manifest_sha256": sha256(manifest),
        "package_set_complete": package_names == EXPECTED_PACKAGES,
        "signature_verification": verification,
        "blockers": blockers,
    }


def strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"duplicate JSON field: {key}")
        value[key] = item
    return value


def collector_contract(repo: Path) -> dict[str, Any]:
    collector = repo / "install-helpers/collect-surface-acceptance.py"
    doc = repo / "docs/ops/surface-pro56-acceptance-collection.md"
    try:
        source = bounded_regular(collector)
        bounded_regular(doc)
        tree = ast.parse(source.decode("utf-8"), filename=collector.name)
        manual: tuple[str, ...] | None = None
        for node in tree.body:
            if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "MANUAL_CHECKS" for target in node.targets):
                candidate = ast.literal_eval(node.value)
                if isinstance(candidate, tuple) and all(isinstance(item, str) for item in candidate):
                    manual = candidate
                break
        if manual is None or len(manual) < 9 or len(set(manual)) != len(manual):
            raise PreflightError("collector manual-proof checklist is missing or incomplete")
        required_terms = ("touch", "pen", "Type Cover", "rotation", "camera", "audio", "suspend", "boot")
        joined = " ".join(manual)
        if any(term.lower() not in joined.lower() for term in required_terms):
            raise PreflightError("collector manual-proof checklist omits a required category")
        return {
            "status": "ready",
            "collector_sha256": hashlib.sha256(source).hexdigest(),
            "manual_checks_declared": len(manual),
            "physical_acceptance_claimed": False,
            "reason": None,
        }
    except (OSError, UnicodeError, SyntaxError, ValueError, PreflightError) as exc:
        return {
            "status": "blocked",
            "collector_sha256": None,
            "manual_checks_declared": 0,
            "physical_acceptance_claimed": False,
            "reason": safe_reason(str(exc), "collector-contract-invalid"),
        }


REMOTE_SOURCE = r'''
import hashlib, json, os, re, stat, subprocess
def read(path, limit=256):
    try:
        with open(path, "rb") as stream: data=stream.read(limit+1)
        if len(data)>limit: return None
        return data.decode("utf-8").strip()
    except (OSError, UnicodeError): return None
version=None
try:
    result=subprocess.run(["/usr/bin/mackesd","--version"],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,env={"PATH":"/usr/sbin:/usr/bin","LANG":"C","LC_ALL":"C"},timeout=3,check=False)
    text=result.stdout[:4096].decode("utf-8",errors="ignore")
    match=re.search(r"[· ]([0-9a-f]{7,40})[ ·]",text.lower())
    if result.returncode==0 and match: version=match.group(1)
except (OSError,subprocess.TimeoutExpired): pass
def bounded_sha256(path, limit=524288):
    try:
        fd=os.open(path,os.O_RDONLY|os.O_CLOEXEC|os.O_NOFOLLOW)
        try:
            info=os.fstat(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_size<=0 or info.st_size>limit or not info.st_mode&0o111: return None
            digest=hashlib.sha256(); total=0
            while True:
                chunk=os.read(fd,min(65536,limit+1-total))
                if not chunk: break
                total+=len(chunk)
                if total>limit: return None
                digest.update(chunk)
            return digest.hexdigest()
        finally: os.close(fd)
    except OSError: return None
collectors=("/usr/libexec/mackesd/collect-surface-acceptance","/opt/mcnf/install-helpers/collect-surface-acceptance.py")
collector_sha256=next((value for value in (bounded_sha256(path) for path in collectors) if value is not None),None)
print(json.dumps({"uid_root":os.geteuid()==0,"vendor":read("/sys/class/dmi/id/sys_vendor"),"product":read("/sys/class/dmi/id/product_name"),"sku":read("/sys/class/dmi/id/product_sku"),"build_revision":version,"collector_sha256":collector_sha256},sort_keys=True,separators=(",",":")))
'''
REMOTE_COMMAND = "/usr/bin/python3 -I -S -c 'import base64;exec(base64.b64decode(\"" + base64.b64encode(REMOTE_SOURCE.encode()).decode("ascii") + "\"))'"


def parse_remote(raw: bytes, generation: int, revision: str, collector_sha256: str) -> dict[str, Any]:
    if len(raw) > 16 * 1024:
        raise PreflightError("remote-probe-output-oversized")
    value = json.loads(raw, object_pairs_hook=strict_object)
    expected = {"uid_root", "vendor", "product", "sku", "build_revision", "collector_sha256"}
    if not isinstance(value, dict) or set(value) != expected:
        raise PreflightError("remote-probe-schema-invalid")
    identity = value["vendor"] == "Microsoft Corporation"
    if generation == 6:
        identity = identity and value["product"] == "Surface Pro 6"
    else:
        identity = identity and value["product"] == "Surface Pro" and value["sku"] in PRO5_SKUS
    remote_revision = value["build_revision"]
    revision_matches = isinstance(remote_revision, str) and SHORT_REVISION.fullmatch(remote_revision) is not None and revision.startswith(remote_revision)
    collector_matches = value["collector_sha256"] == collector_sha256
    ready = value["uid_root"] is True and identity and revision_matches and collector_matches
    blockers = []
    if value["uid_root"] is not True: blockers.append("root-ssh-admission-required")
    if not identity: blockers.append("surface-identity-mismatch")
    if not revision_matches: blockers.append("remote-revision-does-not-match-local-head")
    if not collector_matches: blockers.append("remote-acceptance-collector-hash-mismatch")
    return {
        "status": "ready" if ready else "blocked",
        "root_admitted": value["uid_root"] is True,
        "surface_identity_exact": identity,
        "revision_matches_local": revision_matches,
        "collector_hash_matches_local": collector_matches,
        "blockers": blockers,
    }


def endpoint_probe(label: str, address: str, generation: int, revision: str, collector_sha256: str) -> dict[str, Any]:
    try:
        ping = run(["/usr/bin/ping", "-n", "-c", "1", "-W", "2", address], timeout=4)
        reachable = ping.returncode == 0
    except (OSError, subprocess.TimeoutExpired, PreflightError):
        reachable = False
    destination = f"root@{address}"
    ssh_argv = fixed_ssh_argv(destination)
    try:
        admitted = run(ssh_argv, timeout=COMMAND_TIMEOUT_SECONDS)
    except (OSError, subprocess.TimeoutExpired, PreflightError):
        admitted = None
    output = {
        "endpoint": label,
        "address": "[REDACTED-IP]",
        "icmp_reachable": reachable,
        "ssh_admission": "admitted" if admitted is not None and admitted.returncode == 0 else "refused",
        "remote": None,
    }
    if admitted is not None and admitted.returncode == 0:
        try:
            output["remote"] = parse_remote(admitted.stdout, generation, revision, collector_sha256)
        except (UnicodeError, json.JSONDecodeError, ValueError, PreflightError):
            output["remote"] = {"status": "blocked", "blockers": ["remote-probe-invalid"]}
    return output


def fixed_ssh_argv(destination: str) -> list[str]:
    return [
        "/usr/bin/ssh", "-F", "/dev/null", "-o", "BatchMode=yes", "-o", "PasswordAuthentication=no",
        "-o", "KbdInteractiveAuthentication=no", "-o", "PreferredAuthentications=publickey",
        "-o", "StrictHostKeyChecking=yes", "-o", "UpdateHostKeys=no", "-o", "ConnectTimeout=5",
        "-o", "CheckHostIP=yes", "-o", "UserKnownHostsFile=/root/.ssh/known_hosts",
        "-o", "GlobalKnownHostsFile=/etc/ssh/ssh_known_hosts", "-o", "ProxyCommand=none",
        "-o", "ProxyJump=none", "-o", "ControlMaster=no", "-o", "ControlPath=none",
        "-o", "ClearAllForwardings=yes", "-o", "ForwardAgent=no", "-o", "ForwardX11=no",
        "-o", "PermitLocalCommand=no", "-o", "LocalCommand=none", "-o", "RequestTTY=no",
        "-o", "ConnectionAttempts=1", "-o", "LogLevel=ERROR", destination, REMOTE_COMMAND,
    ]


def resolve_target(generation: int, pro5_address: str | None) -> tuple[str, tuple[tuple[str, str], ...]]:
    if generation == 6:
        if pro5_address is not None:
            raise PreflightError("--pro5-address is invalid for the canonical Pro 6 seat")
        return "Surface", PRO6_ENDPOINTS
    if pro5_address is None:
        raise PreflightError("Surface Pro 5 requires an explicit --pro5-address")
    try:
        address = ipaddress.ip_address(pro5_address)
    except ValueError as exc:
        raise PreflightError("Surface Pro 5 address must be a numeric IP address") from exc
    if address.version != 4 or not address.is_private or address.is_loopback or address.is_unspecified or address.is_multicast:
        raise PreflightError("Surface Pro 5 address must be an explicit private unicast IPv4 address")
    return "Surface-Pro-5", (("explicit", str(address)),)


def self_test() -> int:
    assert resolve_target(6, None) == ("Surface", PRO6_ENDPOINTS)
    for hostile in (None, "example.com", "8.8.8.8", "127.0.0.1", "0.0.0.0"):
        try: resolve_target(5, hostile)
        except PreflightError: pass
        else: raise AssertionError(f"accepted hostile Pro 5 target: {hostile}")
    assert resolve_target(5, "192.168.44.5")[1][0][1] == "192.168.44.5"
    assert "172.20.146.79" not in safe_reason("target 172.20.146.79", "redacted")
    clean_status = subprocess.CompletedProcess([], 0, b"", b"")
    dirty_status = subprocess.CompletedProcess([], 0, b" M tracked-file\n", b"")
    failed_status = subprocess.CompletedProcess([], 1, b"", b"hostile detail")
    assert classify_local_revision("a" * 40, clean_status)["status"] == "ready"
    assert classify_local_revision("a" * 40, dirty_status)["status"] == "blocked"
    assert classify_local_revision("a" * 40, failed_status)["status"] == "blocked"
    collector_hash = "b" * 64
    remote = json.dumps({"uid_root": True, "vendor": "Microsoft Corporation", "product": "Surface Pro 6", "sku": "x", "build_revision": "abcdef1", "collector_sha256": collector_hash}).encode()
    assert parse_remote(remote, 6, "abcdef1" + "0" * 33, collector_hash)["status"] == "ready"
    assert parse_remote(remote, 6, "abcdef1" + "0" * 33, "c" * 64)["status"] == "blocked"
    hostile_remote = b'{"uid_root":true,"uid_root":true}'
    try: parse_remote(hostile_remote, 6, "a" * 40, collector_hash)
    except (ValueError, PreflightError): pass
    else: raise AssertionError("accepted duplicate remote field")
    ssh = fixed_ssh_argv("root@192.0.2.1")
    joined_ssh = " ".join(ssh)
    for fixed_option in (
        "-F /dev/null", "ProxyCommand=none", "ProxyJump=none", "ControlMaster=no",
        "ClearAllForwardings=yes", "UserKnownHostsFile=/root/.ssh/known_hosts",
        "GlobalKnownHostsFile=/etc/ssh/ssh_known_hosts",
    ):
        assert fixed_option in joined_ssh
    try:
        run(
            ["/usr/bin/python3", "-I", "-S", "-c", "import os;os.write(1,b'x'*65537)"],
            maximum=MAX_COMMAND_CAPTURE_BYTES,
        )
    except PreflightError as exc:
        assert str(exc) == "command-output-exceeded-bounded-capture"
    else:
        raise AssertionError("accepted oversized command output")
    sample = {"schema_version": SCHEMA_VERSION, "kind": "mcnf-surface-pro56-deployment-preflight"}
    assert len(json.dumps(sample).encode()) < MAX_OUTPUT_BYTES
    print("preflight-surface-pro56-deployment: self-test passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--generation", type=int, choices=(5, 6), default=6)
    parser.add_argument("--pro5-address")
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    repo = Path(__file__).resolve().parent.parent
    seat, endpoints = resolve_target(args.generation, args.pro5_address)
    revision = local_revision(repo)
    manifest = args.manifest or repo / "packaging/surface/surface-stack.f44.json"
    artifacts = artifact_manifest(repo, manifest)
    collector = collector_contract(repo)
    endpoint_results = []
    if revision["revision"] is not None and collector["collector_sha256"] is not None:
        endpoint_results = [endpoint_probe(label, address, args.generation, revision["revision"], collector["collector_sha256"]) for label, address in endpoints]
    remote_ready = any(item.get("remote", {}).get("status") == "ready" for item in endpoint_results if isinstance(item.get("remote"), dict))
    blockers = []
    if revision["status"] != "ready": blockers.append("local-current-revision-unavailable")
    if artifacts["status"] != "ready": blockers.append("complete-signed-surface-artifact-manifest-unavailable")
    if collector["status"] != "ready": blockers.append("acceptance-collector-contract-unavailable")
    if not remote_ready: blockers.append("no-endpoint-passed-root-ssh-identity-revision-collector-admission")
    output = {
        "schema_version": SCHEMA_VERSION,
        "kind": "mcnf-surface-pro56-deployment-preflight",
        "verdict": "ready" if not blockers else "blocked",
        "read_only": True,
        "target": {"seat": seat, "expected_generation": args.generation, "addresses_redacted": True},
        "local_revision": revision,
        "artifact_manifest": artifacts,
        "collector_and_physical_proof": collector,
        "access": endpoint_results,
        "physical_proof_performed": False,
        "blockers": blockers,
    }
    encoded = (json.dumps(output, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n").encode("ascii")
    if len(encoded) > MAX_OUTPUT_BYTES:
        raise PreflightError("preflight output exceeded the 64 KiB contract")
    sys.stdout.buffer.write(encoded)
    return 0 if not blockers else 3


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PreflightError as exc:
        error = {"schema_version": SCHEMA_VERSION, "kind": "mcnf-surface-pro56-deployment-preflight", "verdict": "invalid", "read_only": True, "reason": safe_reason(str(exc), "invalid-preflight-request")}
        sys.stdout.write(json.dumps(error, sort_keys=True, separators=(",", ":")) + "\n")
        raise SystemExit(2)
