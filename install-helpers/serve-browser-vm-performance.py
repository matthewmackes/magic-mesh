#!/usr/bin/env python3
"""Serve challenge-bound live Browser VM performance telemetry.

This is the missing producer for collect-browser-vm-performance.py's schema-v4
NDJSON contract.  It prepares one controlled five-tab Chromium session inside
the already-running Browser VM, observes that session through the production
mde-vdi-rdp stack, reads every tab through Chromium DevTools, and samples the
actual RDP observer, QEMU, Chromium, and DRM processes/devices.

The public endpoint binds only to loopback.  It emits no fixture mode and has no
option for supplying counter values.  Extended QEMU/Chromium memory and CPU
measurements go to a private sidecar because the collector's exact v4 sample
shape has no fields for them.

Typical Dell invocation (run as root after the seat warning):

  serve-browser-vm-performance.py serve \
    --domain browser-vm --guest-ip 192.168.122.58 \
    --source-commit <40-hex> --image-digest sha256:<64-hex> \
    --rdp-user mcnf-browser --credential-file /run/mcnf/browser-vm-rdp.secret \
    --rdp-probe /usr/local/libexec/serve-browser-vm-performance-rdp \
    --sidecar-out /var/lib/mcnf-browser-vm/performance-sidecar.ndjson

Then, from the same host:

  collect-browser-vm-performance.py collect \
    --endpoint http://127.0.0.1:9080/v1/browser-vm/performance \
    --source-commit <40-hex> --image-digest sha256:<64-hex> \
    --transport rdp --out /path/to/private-evidence.json
"""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
from fractions import Fraction
import glob
import hashlib
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer, ThreadingHTTPServer
import ipaddress
import json
import math
import os
from pathlib import Path
import queue
import re
import secrets
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any, BinaryIO, NoReturn
import urllib.error
import urllib.parse
import urllib.request
import uuid


STREAM_SCHEMA_VERSION = 4
SAMPLE_INTERVAL_SECONDS = 5.0
COLLECTION_SECONDS = 905.0
HIDE_AT_SECONDS = 600.0
MIN_TABS = 5
WIDTH = 1920
HEIGHT = 1080
TARGET_FPS = 30
TAB_RATE_PROBE_SECONDS = 4.0
MEDIA_DURATION_SECONDS = 8
MAX_MEDIA_BYTES = 64 * 1024 * 1024
SOURCE_COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
IMAGE_DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
NONCE_RE = re.compile(r"^[0-9a-f]{64}$")
SESSION_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$")
UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
)
NONCE_HEADER = "X-MCNF-Collection-Nonce"
ENDPOINT_PATH = "/v1/browser-vm/performance"
SHELL_METRICS_SOURCE = "mde-shell-egui-vdi"
GUEST_METRICS_SOURCE = "chromium-devtools"
MAX_QGA_OUTPUT = 256 * 1024
GUEST_CONTROL_REQUEST = "/tmp/mcnf-browser-performance-control/request.json"
GUEST_CONTROL_STATUS_PORT = 41_880
GUEST_HELPER_DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
GUEST_CHROMIUM_METRICS_SOURCE = "guest-user-procfs-controlled-session"
GUEST_CHROMIUM_METRIC_FIELDS = {
    "process_count",
    "pids",
    "oldest_process_seconds",
    "rss_kib",
    "cpu_permille_one_cpu",
    "metrics_sequence",
    "source",
}
GUEST_READY_FIELDS = {
    "schema_version",
    "status",
    "run_id",
    "profile",
    "chromium_pid",
    "proxy_pid",
    "cdp_internal_port",
    "cdp_proxy_port",
    "helper_sha256",
    "chromium",
    "recorded_at",
}


class HarnessError(Exception):
    """A live prerequisite or measured operation failed closed."""


def fail(message: str) -> NoReturn:
    raise HarnessError(message)


def compact_json(value: Any) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")


def utc_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def validate_source(source_commit: str, image_digest: str) -> None:
    if SOURCE_COMMIT_RE.fullmatch(source_commit) is None or source_commit == "0" * 40:
        fail("source commit must be a full non-null lowercase Git revision")
    if (
        IMAGE_DIGEST_RE.fullmatch(image_digest) is None
        or image_digest == "sha256:" + "0" * 64
    ):
        fail("image digest must be a full non-null lowercase sha256 reference")


def validate_domain(value: str) -> str:
    if not value or len(value) > 128 or re.fullmatch(r"[A-Za-z0-9_.-]+", value) is None:
        fail("domain name is malformed")
    return value


def validate_ip(value: str) -> str:
    try:
        parsed = ipaddress.ip_address(value)
    except ValueError:
        fail("guest IP is malformed")
    if parsed.version != 4 or parsed.is_unspecified or parsed.is_multicast:
        fail("guest IP must be one explicit IPv4 unicast address")
    return value


def validate_regular_private(path: Path, label: str, *, executable: bool = False) -> None:
    try:
        metadata = path.lstat()
    except OSError as exc:
        fail(f"{label} is unavailable: {exc}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a regular non-symlink file")
    if metadata.st_mode & 0o077:
        fail(f"{label} must not grant group/other permissions")
    if executable and not os.access(path, os.X_OK):
        fail(f"{label} is not executable")


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


@dataclass(frozen=True)
class MediaAsset:
    data: bytes
    sha256: str
    codec: str
    width: int
    height: int
    fps: int
    duration_ms: int
    generator_sha256: str
    generator_version: str


def generate_media_asset(runtime_root: Path) -> MediaAsset:
    """Generate and inspect the real encoded 1080p/36 source served to Chromium."""
    ffmpeg = Path("/usr/bin/ffmpeg")
    ffprobe = Path("/usr/bin/ffprobe")
    for path, label in ((ffmpeg, "ffmpeg"), (ffprobe, "ffprobe")):
        if not path.is_file() or not os.access(path, os.X_OK):
            fail(f"{label} is unavailable for the live encoded media source")
    output = runtime_root / "browser-vm-performance-source.webm"
    command = [
        str(ffmpeg),
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        (
            f"testsrc2=size={WIDTH}x{HEIGHT}:rate=36:"
            f"duration={MEDIA_DURATION_SECONDS}"
        ),
        "-an",
        "-c:v",
        "libvpx-vp9",
        "-deadline",
        "realtime",
        "-cpu-used",
        "8",
        "-row-mt",
        "1",
        "-threads",
        "8",
        "-b:v",
        "1200k",
        "-g",
        "72",
        "-pix_fmt",
        "yuv420p",
        "-f",
        "webm",
        "-y",
        str(output),
    ]
    try:
        generated = subprocess.run(
            command,
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        fail(f"live 1080p media generation failed: {exc}")
    if generated.returncode != 0:
        fail(f"live 1080p media generation was rejected: {generated.stderr[:512]}")
    try:
        metadata = output.lstat()
    except OSError as exc:
        fail(f"live 1080p media output is unavailable: {exc}")
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISREG(metadata.st_mode)
        or not 0 < metadata.st_size <= MAX_MEDIA_BYTES
    ):
        fail("live 1080p media output is not one bounded regular file")
    try:
        inspected = subprocess.run(
            [
                str(ffprobe),
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,width,height,avg_frame_rate",
                "-show_entries",
                "format=duration",
                "-of",
                "json",
                str(output),
            ],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=20,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        fail(f"live 1080p media inspection failed: {exc}")
    if inspected.returncode != 0:
        fail(f"live 1080p media inspection was rejected: {inspected.stderr[:512]}")
    try:
        facts = json.loads(inspected.stdout)
        stream = facts["streams"][0]
        codec = stream["codec_name"]
        width = int(stream["width"])
        height = int(stream["height"])
        frame_rate = Fraction(stream["avg_frame_rate"])
        duration = float(facts["format"]["duration"])
    except (KeyError, IndexError, TypeError, ValueError, ZeroDivisionError, json.JSONDecodeError):
        fail("live 1080p media inspection returned malformed facts")
    if (
        codec != "vp9"
        or width != WIDTH
        or height != HEIGHT
        or frame_rate != Fraction(36, 1)
        or not math.isfinite(duration)
        or duration < MEDIA_DURATION_SECONDS - 0.1
    ):
        fail("encoded media did not verify as VP9 1920x1080 at 36 fps")
    try:
        version = subprocess.run(
            [str(ffmpeg), "-version"],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=10,
        ).stdout.splitlines()[0][:256]
        data = output.read_bytes()
    except (OSError, subprocess.SubprocessError, IndexError) as exc:
        fail(f"encoded media provenance could not be recorded: {exc}")
    finally:
        try:
            output.unlink()
        except FileNotFoundError:
            pass
    return MediaAsset(
        data=data,
        sha256="sha256:" + hashlib.sha256(data).hexdigest(),
        codec=codec,
        width=width,
        height=height,
        fps=frame_rate.numerator,
        duration_ms=round(duration * 1_000),
        generator_sha256=file_sha256(ffmpeg),
        generator_version=version,
    )


def materialize_rdp_password(
    credential_path: Path,
    expected_username: str,
    *,
    runtime_parent: Path = Path("/run"),
) -> tuple[Path, Path]:
    """Decrypt the host-bound JSON credential into one private runtime leaf."""
    validate_regular_private(credential_path, "Browser VM RDP credential")
    try:
        raw = credential_path.read_bytes()
    except OSError as exc:
        fail(f"Browser VM RDP credential is unreadable: {exc}")
    if not raw.lstrip().startswith(b"{"):
        try:
            completed = subprocess.run(
                [
                    "/usr/bin/systemd-creds",
                    "decrypt",
                    "--name=browser-vm-rdp",
                    str(credential_path),
                    "-",
                ],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=15,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            fail(f"host-bound Browser VM RDP credential could not be decrypted: {exc}")
        if completed.returncode != 0:
            fail("host-bound Browser VM RDP credential decryption was rejected")
        raw = completed.stdout
    if len(raw) > 4 * 1024:
        fail("decrypted Browser VM RDP credential exceeds 4 KiB")
    try:
        record = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError):
        fail("decrypted Browser VM RDP credential is not valid JSON")
    if not isinstance(record, dict) or set(record) != {
        "schema_version",
        "username",
        "password",
    }:
        fail("decrypted Browser VM RDP credential has an unexpected shape")
    if record["schema_version"] != 1 or record["username"] != expected_username:
        fail("decrypted Browser VM RDP credential identity does not match the guest account")
    password = record["password"]
    if (
        not isinstance(password, str)
        or not password
        or len(password) > 512
        or any(character.isspace() or ord(character) < 32 for character in password)
    ):
        fail("decrypted Browser VM RDP credential contains an invalid password")
    runtime_root = Path(
        tempfile.mkdtemp(
            prefix="serve-browser-vm-performance-", dir=runtime_parent
        )
    )
    os.chmod(runtime_root, 0o700)
    password_path = runtime_root / "rdp-password"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    flags |= getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(password_path, flags, 0o600)
        with os.fdopen(fd, "wb") as destination:
            destination.write(password.encode("utf-8") + b"\n")
            destination.flush()
            os.fsync(destination.fileno())
    except OSError as exc:
        try:
            runtime_root.rmdir()
        except OSError:
            pass
        fail(f"private RDP runtime credential could not be materialized: {exc}")
    return runtime_root, password_path


def remove_runtime_password(runtime_root: Path) -> None:
    password_path = runtime_root / "rdp-password"
    try:
        password_path.unlink()
    except FileNotFoundError:
        pass
    try:
        runtime_root.rmdir()
    except FileNotFoundError:
        pass


class SidecarWriter:
    """Append-only private extended telemetry, fsynced at terminal state."""

    def __init__(self, path: Path) -> None:
        self.path = path
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
        flags |= getattr(os, "O_NOFOLLOW", 0)
        try:
            fd = os.open(path, flags, 0o600)
        except OSError as exc:
            fail(f"private sidecar cannot be created exclusively: {exc}")
        self.handle = os.fdopen(fd, "wb", buffering=0)

    def write(self, record: dict[str, Any]) -> None:
        self.handle.write(compact_json(record) + b"\n")

    def close(self) -> None:
        if not self.handle.closed:
            self.handle.flush()
            os.fsync(self.handle.fileno())
            self.handle.close()


@dataclass
class GuestExecResult:
    exitcode: int
    stdout: str
    stderr: str


class GuestAgent:
    """Bounded qemu-guest-agent execution through the selected libvirt domain."""

    def __init__(self, domain: str, guest_user: str) -> None:
        self.domain = validate_domain(domain)
        if re.fullmatch(r"[a-z_][a-z0-9_-]{0,31}", guest_user) is None:
            fail("RDP/guest account name is malformed")
        self.guest_user = guest_user

    def _qga(self, request: dict[str, Any], timeout: float = 15.0) -> dict[str, Any]:
        encoded = json.dumps(request, separators=(",", ":"))
        try:
            completed = subprocess.run(
                [
                    "/usr/bin/virsh",
                    "-c",
                    "qemu:///system",
                    "qemu-agent-command",
                    self.domain,
                    encoded,
                ],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=timeout,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            fail(f"qemu guest-agent command failed: {exc}")
        if completed.returncode != 0:
            diagnostic = completed.stderr.strip()[:512]
            fail(f"qemu guest-agent command was rejected: {diagnostic}")
        try:
            value = json.loads(completed.stdout)
        except json.JSONDecodeError as exc:
            fail(f"qemu guest-agent returned malformed JSON: {exc}")
        if not isinstance(value, dict) or "return" not in value:
            fail("qemu guest-agent response has no return object")
        return value

    def launch(self, path: str, arguments: list[str], *, capture: bool = False) -> int:
        if not path.startswith("/") or any("\x00" in item for item in [path, *arguments]):
            fail("guest execution path/arguments are malformed")
        response = self._qga(
            {
                "execute": "guest-exec",
                "arguments": {
                    "path": path,
                    "arg": arguments,
                    "capture-output": capture,
                },
            }
        )["return"]
        if not isinstance(response, dict) or not isinstance(response.get("pid"), int):
            fail("qemu guest-agent did not return a guest process identity")
        return response["pid"]

    def wait(self, pid: int, timeout: float = 20.0) -> GuestExecResult:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = self._qga(
                {
                    "execute": "guest-exec-status",
                    "arguments": {"pid": pid},
                }
            )["return"]
            if not isinstance(result, dict):
                fail("qemu guest-agent process status is malformed")
            if not result.get("exited", False):
                time.sleep(0.1)
                continue
            stdout = self._decode_output(result.get("out-data"), "guest stdout")
            stderr = self._decode_output(result.get("err-data"), "guest stderr")
            exitcode = result.get("exitcode")
            if not isinstance(exitcode, int):
                fail("qemu guest-agent process status has no integer exit code")
            return GuestExecResult(exitcode, stdout, stderr)
        fail("qemu guest-agent process exceeded its bounded execution window")

    @staticmethod
    def _decode_output(value: Any, label: str) -> str:
        if value is None:
            return ""
        if not isinstance(value, str) or len(value) > MAX_QGA_OUTPUT * 2:
            fail(f"{label} is malformed or oversized")
        try:
            decoded = base64.b64decode(value, validate=True)
        except ValueError:
            fail(f"{label} is not canonical base64")
        if len(decoded) > MAX_QGA_OUTPUT:
            fail(f"{label} exceeds {MAX_QGA_OUTPUT} bytes")
        return decoded.decode("utf-8", errors="replace")

    def command(
        self, path: str, arguments: list[str], *, timeout: float = 20.0
    ) -> GuestExecResult:
        return self.wait(self.launch(path, arguments, capture=True), timeout=timeout)

    def shell(self, script: str, *, timeout: float = 20.0) -> GuestExecResult:
        if len(script.encode("utf-8")) > 32 * 1024 or "\x00" in script:
            fail("guest shell probe is malformed or oversized")
        return self.command("/usr/bin/bash", ["-lc", script], timeout=timeout)

    def require_shell(self, script: str, label: str, *, timeout: float = 20.0) -> str:
        result = self.shell(script, timeout=timeout)
        if result.exitcode != 0:
            diagnostic = result.stderr.strip()[:512]
            fail(f"{label} failed in the guest: {diagnostic}")
        return result.stdout

    def remove_control_file(self, path: str) -> None:
        if path != GUEST_CONTROL_REQUEST:
            fail("guest control cleanup path is not admitted")
        result = self.command("/usr/bin/rm", ["-f", "--", path])
        if result.exitcode != 0:
            fail(f"guest control cleanup failed for {path}: {result.stderr[:256]}")

    def write_control_request(self, record: dict[str, Any]) -> None:
        payload = compact_json(record) + b"\n"
        if len(payload) > 16 * 1024:
            fail("guest control request exceeds 16 KiB")
        response = self._qga(
            {
                "execute": "guest-file-open",
                "arguments": {"path": GUEST_CONTROL_REQUEST, "mode": "w"},
            }
        )["return"]
        if not isinstance(response, int) or response < 0:
            fail("qemu guest-agent did not open the guest control request")
        handle = response
        try:
            written = self._qga(
                {
                    "execute": "guest-file-write",
                    "arguments": {
                        "handle": handle,
                        "buf-b64": base64.b64encode(payload).decode("ascii"),
                    },
                }
            )["return"]
            if not isinstance(written, dict) or written.get("count") != len(payload):
                fail("qemu guest-agent did not write the complete guest control request")
            self._qga(
                {"execute": "guest-file-flush", "arguments": {"handle": handle}}
            )
        finally:
            self._qga(
                {"execute": "guest-file-close", "arguments": {"handle": handle}}
            )
        mode = self.command("/usr/bin/chmod", ["0644", GUEST_CONTROL_REQUEST])
        if mode.exitcode != 0:
            fail("guest control request permissions could not be fixed")

    def facts(self) -> dict[str, str]:
        script = r"""
set -eu
input=/etc/mcnf-browser-vm
printf 'guest_boot_id=%s\n' "$(cat /proc/sys/kernel/random/boot_id)"
printf 'source_commit=%s\n' "$(cat /usr/share/mcnf/browser-vm/source-commit)"
printf 'image_digest=%s\n' "$(cat "$input/image-digest")"
printf 'session_id=%s\n' "$(cat "$input/session-id")"
printf 'transport=%s\n' "$(cat "$input/transport")"
printf 'guest_user=%s\n' "@GUEST_USER@"
printf 'guest_uid=%s\n' "$(id -u @GUEST_USER@)"
printf 'guest_gid=%s\n' "$(id -g @GUEST_USER@)"
printf 'guest_home=%s\n' "$(getent passwd @GUEST_USER@ | cut -d: -f6)"
runtime=/run/user/$(id -u @GUEST_USER@)
wayland=
for candidate in "$runtime"/wayland-*; do
    if [ -S "$candidate" ]; then wayland=${candidate##*/}; break; fi
done
printf 'wayland_display=%s\n' "$wayland"
display=
for candidate in /tmp/.X11-unix/X*; do
    if [ -S "$candidate" ]; then display=:${candidate##*X}; fi
done
printf 'display=%s\n' "$display"
printf 'chromium_bin=%s\n' "$(command -v chromium || command -v chromium-browser)"
""".replace("@GUEST_USER@", self.guest_user)
        raw = self.require_shell(script, "guest runtime fact probe")
        facts: dict[str, str] = {}
        for line in raw.splitlines():
            if "=" not in line:
                continue
            key, value = line.split("=", 1)
            if key in facts:
                fail(f"guest runtime fact is duplicated: {key}")
            facts[key] = value
        required = {
            "guest_boot_id",
            "source_commit",
            "image_digest",
            "session_id",
            "transport",
            "guest_user",
            "guest_uid",
            "guest_gid",
            "guest_home",
            "wayland_display",
            "display",
            "chromium_bin",
        }
        if required - facts.keys():
            fail("guest runtime fact probe omitted required values")
        return facts

def wait_for_guest_session_facts(
    guest: GuestAgent, timeout: float = 60.0
) -> dict[str, str]:
    deadline = time.monotonic() + timeout
    last = "guest session has not published display sockets"
    while time.monotonic() < deadline:
        try:
            facts = guest.facts()
            if facts["wayland_display"] and facts["display"]:
                return facts
            last = "authenticated xrdp session has no Wayland/X11 socket yet"
        except HarnessError as exc:
            last = str(exc)
        time.sleep(0.5)
    fail(f"Browser VM desktop session did not become ready: {last}")


class ProcSampler:
    """Interval CPU and RSS for one real host process."""

    def __init__(self, pid: int) -> None:
        if pid <= 0:
            fail("process sampler requires a positive PID")
        self.pid = pid
        self.previous_process: int | None = None
        self.previous_total: int | None = None

    @staticmethod
    def _total_ticks() -> int:
        with open("/proc/stat", encoding="utf-8") as source:
            fields = source.readline().split()
        if not fields or fields[0] != "cpu":
            fail("host /proc/stat has no aggregate CPU row")
        return sum(int(field) for field in fields[1:])

    def sample(self) -> dict[str, Any] | None:
        try:
            stat_line = Path(f"/proc/{self.pid}/stat").read_text(encoding="utf-8")
            status = Path(f"/proc/{self.pid}/status").read_text(encoding="utf-8")
        except OSError:
            return None
        suffix = stat_line.rsplit(")", 1)
        if len(suffix) != 2:
            return None
        fields = suffix[1].split()
        if len(fields) <= 12:
            return None
        try:
            process_ticks = int(fields[11]) + int(fields[12])
            total_ticks = self._total_ticks()
        except (ValueError, OSError):
            return None
        rss_kib = None
        for line in status.splitlines():
            if line.startswith("VmRSS:"):
                try:
                    rss_kib = int(line.split()[1])
                except (IndexError, ValueError):
                    pass
                break
        cpu = None
        if self.previous_process is not None and self.previous_total is not None:
            process_delta = max(0, process_ticks - self.previous_process)
            total_delta = max(0, total_ticks - self.previous_total)
            if total_delta:
                cpu = min(100_000, process_delta * 100_000 // total_delta)
        self.previous_process = process_ticks
        self.previous_total = total_ticks
        return {
            "pid": self.pid,
            "cpu_permille_host": cpu,
            "rss_kib": rss_kib,
            "source": "host-procfs-interval",
        }


def parse_i915_engine_runtimes(raw: str) -> dict[str, int]:
    runtimes: dict[str, int] = {}
    engine: str | None = None
    for line in raw.splitlines():
        if line and not line[0].isspace() and re.fullmatch(r"[A-Za-z0-9_.:-]+", line):
            engine = line
            continue
        match = re.match(r"\s*Runtime:\s*([0-9]+)ms\s*$", line)
        if match is not None and engine is not None:
            runtimes[engine] = int(match.group(1))
    return runtimes


class DrmGpuSampler:
    """Use sysfs busy percent or measured i915 engine-runtime deltas."""

    def __init__(self) -> None:
        self.previous: dict[str, int] | None = None
        self.previous_at: float | None = None
        self.source = "unavailable"

    def sample(self) -> int | None:
        for path in sorted(glob.glob("/sys/class/drm/card*/device/gpu_busy_percent")):
            try:
                percent = int(Path(path).read_text(encoding="utf-8").strip())
            except (OSError, ValueError):
                continue
            if 0 <= percent <= 100:
                self.source = path
                return percent * 1_000
        for path in sorted(glob.glob("/sys/kernel/debug/dri/*/i915_engine_info")):
            try:
                current = parse_i915_engine_runtimes(
                    Path(path).read_text(encoding="utf-8", errors="replace")
                )
            except OSError:
                continue
            if not current:
                continue
            now = time.monotonic()
            result = None
            if self.previous is not None and self.previous_at is not None:
                elapsed_ms = max(1.0, (now - self.previous_at) * 1_000.0)
                deltas = [
                    max(0, runtime - self.previous.get(engine, runtime))
                    for engine, runtime in current.items()
                ]
                # Engines can execute concurrently.  The maximum individual
                # engine duty cycle is a real lower bound on whole-GPU busy and
                # does not double-count overlapping engine time.
                if deltas:
                    result = min(100_000, round(max(deltas) * 100_000 / elapsed_ms))
            self.previous = current
            self.previous_at = now
            self.source = path + ":max-engine-runtime-delta"
            return result
        return None


class RdpProbe:
    def __init__(
        self,
        binary: Path,
        host: str,
        port: int,
        username: str,
        credential_file: Path,
    ) -> None:
        if not binary.is_absolute():
            fail("RDP probe path must be absolute")
        if not binary.is_file() or not os.access(binary, os.X_OK):
            fail("RDP probe binary is unavailable or not executable")
        validate_regular_private(credential_file, "RDP credential file")
        self.process = subprocess.Popen(
            [
                str(binary),
                "--host",
                host,
                "--port",
                str(port),
                "--username",
                username,
                "--credential-file",
                str(credential_file),
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self.binary = binary
        self.ready = threading.Event()
        self.lock = threading.Lock()
        self.latest: dict[str, Any] | None = None
        self.snapshot_sequence = 0
        self.returned_snapshot_sequence = 0
        self.ready_record: dict[str, Any] | None = None
        self.error: str | None = None
        self.latencies: list[int] = []
        self.stderr_lines: queue.Queue[str] = queue.Queue(maxsize=128)
        threading.Thread(target=self._read_stdout, daemon=True).start()
        threading.Thread(target=self._read_stderr, daemon=True).start()

    def _read_stdout(self) -> None:
        assert self.process.stdout is not None
        for raw in self.process.stdout:
            try:
                record = json.loads(raw)
            except json.JSONDecodeError:
                with self.lock:
                    self.error = "RDP observer emitted malformed JSON"
                self.ready.set()
                continue
            if not isinstance(record, dict):
                continue
            kind = record.get("type")
            with self.lock:
                if kind == "rdp_ready":
                    self.ready_record = record
                    self.ready.set()
                elif kind == "rdp_snapshot":
                    values = record.get("session_latencies_ms", [])
                    if isinstance(values, list):
                        self.latencies.extend(
                            value
                            for value in values
                            if isinstance(value, int) and not isinstance(value, bool)
                        )
                    self.latest = record
                    self.snapshot_sequence += 1
                elif kind == "rdp_error":
                    self.error = str(record.get("reason", "RDP observer failed"))[:1024]
                    self.ready.set()
        if self.process.poll() not in (None, 0):
            with self.lock:
                self.error = self.error or "RDP observer exited unexpectedly"
            self.ready.set()

    def _read_stderr(self) -> None:
        assert self.process.stderr is not None
        for raw in self.process.stderr:
            try:
                self.stderr_lines.put_nowait(raw.rstrip()[:1024])
            except queue.Full:
                pass

    def wait_ready(self, timeout: float = 90.0) -> dict[str, Any]:
        if not self.ready.wait(timeout):
            fail("RDP observer did not reach a connected 1920x1080 session")
        with self.lock:
            if self.error is not None:
                fail(f"RDP observer failed: {self.error}")
            record = dict(self.ready_record or {})
        if record.get("width") != WIDTH or record.get("height") != HEIGHT:
            fail("RDP observer did not negotiate the required 1920x1080 desktop")
        identity = record.get("source_instance_id")
        if not isinstance(identity, str) or UUID_RE.fullmatch(identity) is None:
            fail("RDP observer returned a malformed source instance identity")
        return record

    def _send_control(self, control: str) -> None:
        if self.process.stdin is None:
            fail("RDP observer control pipe is unavailable")
        try:
            self.process.stdin.write(control + "\n")
            self.process.stdin.flush()
        except (BrokenPipeError, OSError) as exc:
            fail(f"RDP observer rejected the {control!r} control: {exc}")

    def begin(self) -> None:
        self._send_control("begin")

    def mark_hidden(self) -> None:
        self._send_control("hidden")

    def snapshot(
        self,
        timeout: float = 10.0,
        *,
        expected_browser_visible: bool | None = None,
    ) -> tuple[dict[str, Any], list[int]]:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.lock:
                if self.error is not None:
                    fail(f"RDP observer failed: {self.error}")
                if (
                    self.latest is not None
                    and self.snapshot_sequence > self.returned_snapshot_sequence
                    and (
                        expected_browser_visible is None
                        or self.latest.get("browser_visible")
                        is expected_browser_visible
                    )
                ):
                    latest = dict(self.latest)
                    self.returned_snapshot_sequence = self.snapshot_sequence
                    latencies = self.latencies
                    self.latencies = []
                    return latest, latencies
            time.sleep(0.02)
        fail("RDP observer did not publish a fresh bounded snapshot")

    def stop(self) -> None:
        if self.process.poll() is None and self.process.stdin is not None:
            try:
                self.process.stdin.write("stop\n")
                self.process.stdin.flush()
                self.process.wait(timeout=10)
            except (BrokenPipeError, OSError, subprocess.TimeoutExpired):
                self.process.terminate()
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.process.kill()


class WebSocketClient:
    """Minimal bounded RFC6455 client sufficient for Chromium DevTools."""

    def __init__(self, url: str, timeout: float = 5.0) -> None:
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme != "ws" or not parsed.hostname or parsed.port is None:
            fail("Chromium returned a non-local or malformed DevTools WebSocket URL")
        self.socket = socket.create_connection((parsed.hostname, parsed.port), timeout=timeout)
        self.socket.settimeout(timeout)
        key = base64.b64encode(secrets.token_bytes(16)).decode("ascii")
        path = parsed.path or "/"
        if parsed.query:
            path += "?" + parsed.query
        request = (
            f"GET {path} HTTP/1.1\r\n"
            f"Host: {parsed.hostname}:{parsed.port}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n"
            "Origin: http://127.0.0.1\r\n\r\n"
        ).encode("ascii")
        self.socket.sendall(request)
        response = self._read_headers()
        status = response.split(b"\r\n", 1)[0]
        if b" 101 " not in status:
            self.close()
            fail(f"Chromium rejected the DevTools WebSocket handshake: {status!r}")
        expected = base64.b64encode(
            hashlib.sha1(
                (key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode("ascii")
            ).digest()
        ).lower()
        headers = response.lower()
        if b"sec-websocket-accept: " + expected not in headers:
            self.close()
            fail("Chromium DevTools WebSocket accept digest did not match")
        self.next_id = 1
        self.lock = threading.Lock()

    def _read_headers(self) -> bytes:
        data = bytearray()
        while b"\r\n\r\n" not in data:
            chunk = self.socket.recv(4096)
            if not chunk:
                fail("Chromium closed the DevTools handshake")
            data.extend(chunk)
            if len(data) > 64 * 1024:
                fail("Chromium DevTools handshake exceeded 64 KiB")
        return bytes(data)

    def _recv_exact(self, size: int) -> bytes:
        data = bytearray()
        while len(data) < size:
            chunk = self.socket.recv(size - len(data))
            if not chunk:
                fail("Chromium closed the DevTools WebSocket")
            data.extend(chunk)
        return bytes(data)

    def _send_frame(self, opcode: int, payload: bytes) -> None:
        mask = secrets.token_bytes(4)
        length = len(payload)
        if length < 126:
            header = bytes([0x80 | opcode, 0x80 | length])
        elif length <= 0xFFFF:
            header = bytes([0x80 | opcode, 0x80 | 126]) + struct.pack("!H", length)
        else:
            header = bytes([0x80 | opcode, 0x80 | 127]) + struct.pack("!Q", length)
        masked = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
        self.socket.sendall(header + mask + masked)

    def _recv_message(self) -> bytes:
        fragments = bytearray()
        expected_opcode = None
        while True:
            first = self._recv_exact(2)
            final = bool(first[0] & 0x80)
            opcode = first[0] & 0x0F
            length = first[1] & 0x7F
            masked = bool(first[1] & 0x80)
            if length == 126:
                length = struct.unpack("!H", self._recv_exact(2))[0]
            elif length == 127:
                length = struct.unpack("!Q", self._recv_exact(8))[0]
            if length > 4 * 1024 * 1024:
                fail("Chromium DevTools message exceeded 4 MiB")
            mask = self._recv_exact(4) if masked else None
            payload = self._recv_exact(length)
            if mask is not None:
                payload = bytes(
                    value ^ mask[index % 4] for index, value in enumerate(payload)
                )
            if opcode == 0x8:
                fail("Chromium closed the DevTools WebSocket")
            if opcode == 0x9:
                self._send_frame(0xA, payload)
                continue
            if opcode == 0xA:
                continue
            if opcode in (0x1, 0x2):
                expected_opcode = opcode
                fragments.extend(payload)
            elif opcode == 0x0 and expected_opcode is not None:
                fragments.extend(payload)
            else:
                fail("Chromium emitted an unsupported DevTools WebSocket frame")
            if final:
                return bytes(fragments)

    def call(self, method: str, params: dict[str, Any] | None = None) -> Any:
        with self.lock:
            call_id = self.next_id
            self.next_id += 1
            self._send_frame(
                0x1,
                compact_json(
                    {"id": call_id, "method": method, "params": params or {}}
                ),
            )
            while True:
                try:
                    response = json.loads(self._recv_message())
                except json.JSONDecodeError as exc:
                    fail(f"Chromium emitted malformed DevTools JSON: {exc}")
                if not isinstance(response, dict) or response.get("id") != call_id:
                    continue
                if "error" in response:
                    fail(f"Chromium DevTools {method} failed: {response['error']}")
                return response.get("result")

    def close(self) -> None:
        try:
            self._send_frame(0x8, b"")
        except (OSError, AttributeError):
            pass
        try:
            self.socket.close()
        except (OSError, AttributeError):
            pass


class CdpTab:
    def __init__(self, target: dict[str, Any]) -> None:
        tab_id = target.get("id")
        url = target.get("url")
        websocket = target.get("webSocketDebuggerUrl")
        if (
            not isinstance(tab_id, str)
            or SESSION_RE.fullmatch(tab_id) is None
            or not isinstance(url, str)
            or not isinstance(websocket, str)
        ):
            fail("Chromium DevTools target identity is malformed")
        self.id = tab_id
        self.url = url
        self.socket = WebSocketClient(websocket)
        self.socket.call("Runtime.enable")

    def evaluate(self, expression: str) -> Any:
        result = self.socket.call(
            "Runtime.evaluate",
            {
                "expression": expression,
                "returnByValue": True,
                "awaitPromise": True,
                "timeout": 5_000,
            },
        )
        if not isinstance(result, dict):
            fail("Chromium Runtime.evaluate returned no result")
        remote = result.get("result")
        if not isinstance(remote, dict) or "value" not in remote:
            description = remote.get("description") if isinstance(remote, dict) else None
            fail(f"Chromium page evaluation failed: {description}")
        return remote["value"]

    def begin(self) -> None:
        value = self.evaluate("window.__mcnfPerformance.begin()")
        if value != "begun":
            fail("Chromium media page refused the challenge-bound begin signal")

    def initialization_status(self) -> dict[str, Any]:
        value = self.evaluate(
            "({readyState:document.readyState,"
            "harness:typeof window.__mcnfPerformance === 'object',"
            "error:window.__mcnfPerformanceError || null,"
            "videoWidth:document.getElementById('video')?.videoWidth || 0,"
            "videoHeight:document.getElementById('video')?.videoHeight || 0})"
        )
        if not isinstance(value, dict):
            fail("Chromium media page initialization status is malformed")
        return value

    def snapshot(self) -> dict[str, Any]:
        value = self.evaluate("window.__mcnfPerformance.snapshot()")
        if not isinstance(value, dict):
            fail("Chromium media page returned a malformed snapshot")
        required = {
            "framesPresented",
            "sourceTick",
            "width",
            "height",
            "readyState",
            "navigationLatenciesMs",
            "tab",
        }
        if required - value.keys():
            fail("Chromium media page snapshot omitted required values")
        for field in (
            "framesPresented",
            "sourceTick",
            "width",
            "height",
            "readyState",
        ):
            if isinstance(value[field], bool) or not isinstance(value[field], int):
                fail(f"Chromium media page {field} is not an integer measurement")
        latencies = value["navigationLatenciesMs"]
        if not isinstance(latencies, list) or any(
            isinstance(item, bool) or not isinstance(item, int) or item <= 0
            for item in latencies
        ):
            fail("Chromium media page navigation latencies are malformed")
        return value

    def window_id(self) -> int:
        value = self.socket.call("Browser.getWindowForTarget", {"targetId": self.id})
        if not isinstance(value, dict):
            fail("Chromium did not return a browser window identity")
        window_id = value.get("windowId")
        if isinstance(window_id, bool) or not isinstance(window_id, int) or window_id < 0:
            fail("Chromium returned a malformed browser window identity")
        return window_id

    def minimize_window(self, window_id: int) -> None:
        self.socket.call(
            "Browser.setWindowBounds",
            {"windowId": window_id, "bounds": {"windowState": "minimized"}},
        )
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            value = self.socket.call("Browser.getWindowBounds", {"windowId": window_id})
            bounds = value.get("bounds") if isinstance(value, dict) else None
            if isinstance(bounds, dict) and bounds.get("windowState") == "minimized":
                return
            time.sleep(0.1)
        fail(f"Chromium window {window_id} did not enter a verified minimized state")

    def set_normal_window_bounds(
        self, window_id: int, *, left: int, top: int, width: int, height: int
    ) -> None:
        self.socket.call(
            "Browser.setWindowBounds",
            {"windowId": window_id, "bounds": {"windowState": "normal"}},
        )
        self.socket.call(
            "Browser.setWindowBounds",
            {
                "windowId": window_id,
                "bounds": {
                    "left": left,
                    "top": top,
                    "width": width,
                    "height": height,
                },
            },
        )
        expected = {"left": left, "top": top, "width": width, "height": height}
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            value = self.socket.call("Browser.getWindowBounds", {"windowId": window_id})
            bounds = value.get("bounds") if isinstance(value, dict) else None
            if (
                isinstance(bounds, dict)
                and bounds.get("windowState") == "normal"
                and all(bounds.get(field) == number for field, number in expected.items())
            ):
                return
            time.sleep(0.1)
        fail(f"Chromium window {window_id} did not enter its verified tiled bounds")

    def close(self) -> None:
        self.socket.close()


def spread_tabs_across_windows(tabs: list[CdpTab]) -> bool:
    """Give every measured page an active tab in its own Chromium window.

    Chromium does not submit video frames for inactive tabs, even when its
    renderer/timer background throttles are disabled.  Keeping one measured
    tab per real browser window preserves five concurrent, headful video
    players; ``--disable-backgrounding-occluded-windows`` then keeps the
    covered windows rendering while RDP observes the foreground window.

    Return ``True`` after replacing duplicate-window tabs.  The caller must
    rediscover targets because both the old and new CDP identities change the
    target list while this operation is in flight.
    """
    if len(tabs) != MIN_TABS:
        fail("Chromium window spreading requires the exact measured tab set")
    owners: dict[int, CdpTab] = {}
    duplicates: list[CdpTab] = []
    for tab in tabs:
        window_id = tab.window_id()
        if window_id in owners:
            duplicates.append(tab)
        else:
            owners[window_id] = tab
    if not duplicates:
        return False

    controller = tabs[0]
    created: list[str] = []
    try:
        for tab in duplicates:
            result = controller.socket.call(
                "Target.createTarget",
                {
                    "url": tab.url,
                    "newWindow": True,
                    "background": False,
                },
            )
            target_id = result.get("targetId") if isinstance(result, dict) else None
            if (
                not isinstance(target_id, str)
                or SESSION_RE.fullmatch(target_id) is None
                or target_id in created
            ):
                fail("Chromium returned a malformed replacement-window target")
            created.append(target_id)
    except Exception:
        for target_id in created:
            try:
                controller.socket.call("Target.closeTarget", {"targetId": target_id})
            except (HarnessError, OSError):
                pass
        raise

    for tab in duplicates:
        result = controller.socket.call("Target.closeTarget", {"targetId": tab.id})
        if not isinstance(result, dict) or result.get("success") is not True:
            fail("Chromium did not close a replaced background media tab")
    return True


def tile_tab_windows(tabs: list[CdpTab]) -> list[int]:
    """Keep every 1080p source visibly scheduled on the 1920x1080 desktop."""
    layout = (
        (0, 0, 640, 520),
        (640, 0, 640, 520),
        (1280, 0, 640, 520),
        (320, 540, 640, 520),
        (960, 540, 640, 520),
    )
    windows: list[int] = []
    for tab, (left, top, width, height) in zip(tabs, layout):
        window_id = tab.window_id()
        if window_id in windows:
            fail("Chromium media tabs did not retain independent windows")
        tab.set_normal_window_bounds(
            window_id,
            left=left,
            top=top,
            width=width,
            height=height,
        )
        windows.append(window_id)
    return windows


MEDIA_PAGE = r"""<!doctype html>
<html><head><meta charset="utf-8"><title>MCNF Browser VM performance media</title>
<style>
html,body{margin:0;width:100%;height:100%;overflow:hidden;background:#07111f}
#video{position:fixed;inset:0;width:100%;height:100%;object-fit:cover;background:#07111f}
#beacon{position:fixed;left:24px;top:48px;width:160px;height:96px;border:0;border-radius:12px;
background:#c62828;color:white;font:700 17px system-ui;z-index:4;box-shadow:0 3px 18px #0008}
#nav{position:fixed;width:1px;height:1px;left:-10px;top:-10px;border:0}
</style></head><body>
<video id="video" muted autoplay loop playsinline preload="auto"></video>
<button id="beacon" type="button">INPUT 0</button><iframe id="nav"></iframe>
<script>
(() => {
  'use strict';
  window.__mcnfPerformanceError = null;
  try {
  const query = new URLSearchParams(location.search);
  const tab = Number(query.get('tab'));
  const run = query.get('run');
  const mediaOrigin = location.origin;
  const video = document.getElementById('video');
  const beacon = document.getElementById('beacon');
  const nav = document.getElementById('nav');
  let sourceTick = 0, framesPresented = 0, beaconEpoch = 0;
  let begun = false, navigationSequence = 0, navigationLatencies = [], navigationTimer;
  video.src = `${mediaOrigin}/video.webm?run=${encodeURIComponent(run)}&tab=${tab}`;
  video.play().catch(() => {});
  function presented() {
    framesPresented++;
    sourceTick++;
    video.requestVideoFrameCallback(presented);
  }
  video.requestVideoFrameCallback(presented);
  beacon.addEventListener('click', () => {
    beaconEpoch++;
    beacon.textContent = `INPUT ${beaconEpoch}`;
    beacon.style.background = beaconEpoch % 2 ? '#1565c0' : '#c62828';
  });
  function navigate() {
    if (!begun) return;
    const started = performance.now();
    const sequence = ++navigationSequence;
    const listener = () => {
      nav.removeEventListener('load', listener);
      const elapsed = Math.max(1, Math.round(performance.now() - started));
      navigationLatencies.push(elapsed);
    };
    nav.addEventListener('load', listener);
    nav.src = `${mediaOrigin}/nav?run=${encodeURIComponent(run)}&tab=${tab}&n=${sequence}`;
  }
  window.__mcnfPerformance = {
    begin() {
      if (begun) return 'begun';
      begun = true; navigationLatencies = [];
      setTimeout(navigate, 10000);
      navigationTimer = setInterval(navigate, 45000);
      return 'begun';
    },
    snapshot() {
      const latencies = navigationLatencies.splice(0, navigationLatencies.length);
      return {
        framesPresented,
        width: video.videoWidth,
        height: video.videoHeight,
        readyState: video.readyState,
        navigationLatenciesMs: latencies,
        beaconEpoch,
        sourceTick,
        tab
      };
    }
  };
  } catch (error) {
    window.__mcnfPerformanceError = String(error && (error.stack || error.message) || error);
  }
})();
</script></body></html>""".encode("utf-8")


class MediaHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        parsed = urllib.parse.urlsplit(self.path)
        if parsed.path == "/media.html":
            body = MEDIA_PAGE
            content_type = "text/html; charset=utf-8"
        elif parsed.path == "/video.webm":
            asset = getattr(self.server, "media_asset", None)
            if not isinstance(asset, MediaAsset):
                self.send_error(HTTPStatus.SERVICE_UNAVAILABLE)
                return
            body = asset.data
            content_type = "video/webm"
        elif parsed.path == "/nav":
            body = b"<!doctype html><meta charset=utf-8><title>navigation receipt</title>ok"
            content_type = "text/html; charset=utf-8"
        elif parsed.path == "/health":
            body = b"ok\n"
            content_type = "text/plain; charset=utf-8"
        else:
            self.send_error(HTTPStatus.NOT_FOUND)
            return
        self.send_response(HTTPStatus.OK)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *_args: Any) -> None:
        return


class ReusableThreadingHTTPServer(ThreadingHTTPServer):
    allow_reuse_address = True
    daemon_threads = True


class MediaServer:
    def __init__(self, bind: str, port: int, media_asset: MediaAsset) -> None:
        self.server = ReusableThreadingHTTPServer((bind, port), MediaHandler)
        self.server.media_asset = media_asset  # type: ignore[attr-defined]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def start(self) -> None:
        self.thread.start()

    def stop(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


class FirewallLease:
    """Own one runtime-only guest-to-host media-origin firewalld rule."""

    def __init__(self, guest_ip: str, port: int) -> None:
        if port != 9081:
            fail("firewall lease only admits the controlled media port")
        self.rule = (
            'rule family="ipv4" '
            f'source address="{validate_ip(guest_ip)}/32" '
            f'port port="{port}" protocol="tcp" accept'
        )
        self.active = False
        query = self._command("--query-rich-rule=" + self.rule)
        if query.returncode == 0:
            fail("controlled media firewall rule already exists outside this harness")
        if query.returncode != 1:
            fail("firewalld could not query the controlled media rule")
        added = self._command(
            "--add-rich-rule=" + self.rule,
            "--timeout=1200",
        )
        if added.returncode != 0:
            fail(f"firewalld rejected the controlled media rule: {added.stderr[:256]}")
        self.active = True
        if self._command("--query-rich-rule=" + self.rule).returncode != 0:
            self.close()
            fail("controlled media firewall rule was not observable after creation")

    @staticmethod
    def _command(*arguments: str) -> subprocess.CompletedProcess[str]:
        try:
            return subprocess.run(
                ["/usr/bin/firewall-cmd", "--zone=libvirt", *arguments],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=15,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            fail(f"firewalld command failed: {exc}")

    def close(self) -> None:
        if not self.active:
            return
        removed = self._command("--remove-rich-rule=" + self.rule)
        self.active = False
        if removed.returncode not in (0, 1):
            fail(f"controlled media firewall cleanup failed: {removed.stderr[:256]}")


def fetch_json(
    url: str,
    timeout: float = 5.0,
    *,
    headers: dict[str, str] | None = None,
) -> Any:
    request_headers = {"User-Agent": "mcnf-performance-harness/1"}
    request_headers.update(headers or {})
    request = urllib.request.Request(url, headers=request_headers)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        if response.status != HTTPStatus.OK:
            fail(f"Chromium DevTools endpoint returned HTTP {response.status}")
        payload = response.read(4 * 1024 * 1024 + 1)
    if len(payload) > 4 * 1024 * 1024:
        fail("Chromium DevTools target list exceeds 4 MiB")
    try:
        return json.loads(payload)
    except json.JSONDecodeError as exc:
        fail(f"Chromium DevTools target list is malformed: {exc}")


def wait_for_tabs(
    guest_ip: str,
    proxy_port: int,
    internal_port: int,
    run_id: str,
    timeout: float = 60.0,
) -> list[CdpTab]:
    endpoint = f"http://{guest_ip}:{proxy_port}/json/list"
    deadline = time.monotonic() + timeout
    last_error = "not reachable"
    fatal_window_error: str | None = None
    while time.monotonic() < deadline:
        try:
            targets = fetch_json(
                endpoint,
                headers={"Host": f"127.0.0.1:{internal_port}"},
            )
            if not isinstance(targets, list):
                fail("Chromium DevTools target list is not an array")
            prefix = f"http://192.168.122.1:9081/media.html?"
            matches = [
                target
                for target in targets
                if isinstance(target, dict)
                and target.get("type") == "page"
                and isinstance(target.get("url"), str)
                and target["url"].startswith(prefix)
                and f"run={run_id}" in target["url"]
            ]
            if len(matches) == MIN_TABS:
                matches.sort(key=lambda target: target["url"])
                rewritten: list[dict[str, Any]] = []
                for target in matches:
                    websocket = urllib.parse.urlsplit(target.get("webSocketDebuggerUrl", ""))
                    if (
                        websocket.scheme != "ws"
                        or websocket.hostname not in {"127.0.0.1", "localhost"}
                        or websocket.port != internal_port
                    ):
                        fail("Chromium DevTools target escaped its guest-loopback listener")
                    target = dict(target)
                    target["webSocketDebuggerUrl"] = urllib.parse.urlunsplit(
                        (
                            "ws",
                            f"{guest_ip}:{proxy_port}",
                            websocket.path,
                            websocket.query,
                            "",
                        )
                    )
                    rewritten.append(target)
                tabs: list[CdpTab] = []
                accepted = False
                try:
                    tabs = [CdpTab(target) for target in rewritten]
                    try:
                        windows_changed = spread_tabs_across_windows(tabs)
                    except (HarnessError, OSError) as exc:
                        fatal_window_error = str(exc)
                        raise
                    if windows_changed:
                        last_error = "rediscovering five independent Chromium windows"
                        continue
                    try:
                        tile_tab_windows(tabs)
                    except (HarnessError, OSError) as exc:
                        fatal_window_error = str(exc)
                        raise
                    statuses = [tab.initialization_status() for tab in tabs]
                    if not all(status.get("harness") is True for status in statuses):
                        errors = sorted(
                            {
                                str(status.get("error"))[:512]
                                for status in statuses
                                if status.get("error")
                            }
                        )
                        last_error = (
                            "media script initialization failed: "
                            + ("; ".join(errors) if errors else "no browser exception was exposed")
                        )
                    else:
                        snapshots = [tab.snapshot() for tab in tabs]
                        if all(
                            snapshot["width"] == WIDTH
                            and snapshot["height"] == HEIGHT
                            and snapshot["readyState"] >= 2
                            and snapshot["framesPresented"] > 0
                            for snapshot in snapshots
                        ):
                            rate_started = time.monotonic()
                            frame_baselines = [
                                snapshot["framesPresented"] for snapshot in snapshots
                            ]
                            time.sleep(TAB_RATE_PROBE_SECONDS)
                            rate_snapshots = [tab.snapshot() for tab in tabs]
                            rate_elapsed = time.monotonic() - rate_started
                            rates = [
                                (snapshot["framesPresented"] - baseline) / rate_elapsed
                                for baseline, snapshot in zip(
                                    frame_baselines, rate_snapshots
                                )
                            ]
                            if all(
                                snapshot["width"] == WIDTH
                                and snapshot["height"] == HEIGHT
                                and snapshot["readyState"] >= 2
                                for snapshot in rate_snapshots
                            ) and all(rate >= TARGET_FPS for rate in rates):
                                accepted = True
                                return tabs
                            last_error = (
                                "five media tabs did not sustain 30 fps during "
                                f"the {rate_elapsed:.3f}s live probe: "
                                + json.dumps(
                                    [round(rate, 3) for rate in rates],
                                    separators=(",", ":"),
                                )
                            )
                            continue
                        measured = [
                            {
                                "tab": snapshot.get("tab"),
                                "frames": snapshot.get("framesPresented"),
                                "source_ticks": snapshot.get("sourceTick"),
                                "width": snapshot.get("width"),
                                "height": snapshot.get("height"),
                                "ready_state": snapshot.get("readyState"),
                            }
                            for snapshot in snapshots
                        ]
                        last_error = (
                            "five media tabs are not rendering required frames: "
                            + json.dumps(measured, sort_keys=True, separators=(",", ":"))
                        )
                finally:
                    if not accepted:
                        for tab in tabs:
                            tab.close()
            else:
                last_error = f"observed {len(matches)} controlled media tabs"
        except (HarnessError, OSError, urllib.error.URLError) as exc:
            last_error = str(exc)
        if fatal_window_error is not None:
            break
        time.sleep(0.5)
    fail(f"Chromium did not expose five ready CDP media tabs: {last_error}")


def activate_tab(guest_ip: str, port: int, tab_id: str) -> None:
    url = f"http://{guest_ip}:{port}/json/activate/{urllib.parse.quote(tab_id, safe='')}"
    try:
        with urllib.request.urlopen(url, timeout=5) as response:
            if response.status != HTTPStatus.OK:
                fail("Chromium refused to activate the measured visible tab")
            response.read(1024)
    except (OSError, urllib.error.URLError) as exc:
        fail(f"Chromium visible-tab activation failed: {exc}")


def fetch_guest_control_status(
    guest_ip: str, run_id: str
) -> dict[str, Any] | None:
    url = (
        f"http://{guest_ip}:{GUEST_CONTROL_STATUS_PORT}/v1/status?"
        + urllib.parse.urlencode({"run_id": run_id})
    )
    try:
        value = fetch_json(url, timeout=2.0)
    except urllib.error.HTTPError as exc:
        if exc.code == HTTPStatus.NOT_FOUND:
            return None
        fail(f"guest performance status endpoint returned HTTP {exc.code}")
    except (OSError, urllib.error.URLError):
        return None
    if not isinstance(value, dict):
        fail("guest performance status endpoint returned a non-object")
    return value


def validate_guest_chromium_stats(
    response: dict[str, Any], chromium_pid: int
) -> dict[str, Any]:
    stats = response.get("chromium")
    if not isinstance(stats, dict) or set(stats) != GUEST_CHROMIUM_METRIC_FIELDS:
        fail("guest Chromium process metrics have an unexpected shape")
    limits = {
        "process_count": 4_096,
        "oldest_process_seconds": 365 * 24 * 60 * 60,
        "rss_kib": 64 * 1024 * 1024,
        "cpu_permille_one_cpu": 1_000_000,
        "metrics_sequence": 1_000_000_000,
    }
    for field, maximum in limits.items():
        value = stats.get(field)
        minimum = 1 if field in {"process_count", "metrics_sequence"} else 0
        if (
            isinstance(value, bool)
            or not isinstance(value, int)
            or not minimum <= value <= maximum
        ):
            fail(f"guest Chromium {field} is not a bounded live measurement")
    pids = stats.get("pids")
    if (
        not isinstance(pids, list)
        or len(pids) != stats["process_count"]
        or len(set(pids)) != len(pids)
        or any(isinstance(pid, bool) or not isinstance(pid, int) or pid <= 0 for pid in pids)
        or chromium_pid not in pids
    ):
        fail("guest Chromium process identities are malformed or incomplete")
    if stats.get("source") != GUEST_CHROMIUM_METRICS_SOURCE:
        fail("guest Chromium process metrics do not identify live procfs provenance")
    return dict(stats)


def current_guest_chromium_stats(
    guest_ip: str, run_id: str, helper_sha256: str
) -> dict[str, Any]:
    response = fetch_guest_control_status(guest_ip, run_id)
    if response is None:
        fail("guest Chromium process metrics endpoint disappeared")
    if response.get("status") == "failed":
        fail(
            "guest Chromium process metrics failed: "
            + str(response.get("reason", "unknown guest helper failure"))[:512]
        )
    if (
        set(response) != GUEST_READY_FIELDS
        or response.get("schema_version") != 1
        or response.get("status") != "ready"
        or response.get("run_id") != run_id
        or response.get("helper_sha256") != helper_sha256
    ):
        fail("guest Chromium process metrics lost their ready/run/helper binding")
    chromium_pid = response.get("chromium_pid")
    if isinstance(chromium_pid, bool) or not isinstance(chromium_pid, int):
        fail("guest Chromium process metric owner is malformed")
    return validate_guest_chromium_stats(response, chromium_pid)


def prepare_controlled_chromium(
    guest: GuestAgent,
    facts: dict[str, str],
    guest_ip: str,
    cdp_internal_port: int,
    cdp_proxy_port: int,
    run_id: str,
) -> tuple[list[CdpTab], str, str, str, int]:
    if not facts["wayland_display"] or not facts["display"]:
        fail("authenticated RDP session has no guest Wayland/X11 display")
    if facts["guest_uid"] != "1000":
        fail("Browser VM account UID does not match the immutable guest layout")
    request = {
        "schema_version": 1,
        "action": "start",
        "run_id": run_id,
        "guest_ip": guest_ip,
        "media_origin": "http://192.168.122.1:9081",
        "cdp_internal_port": cdp_internal_port,
        "cdp_proxy_port": cdp_proxy_port,
    }
    guest.remove_control_file(GUEST_CONTROL_REQUEST)
    guest.write_control_request(request)
    response: dict[str, Any] | None = None
    deadline = time.monotonic() + 90.0
    while time.monotonic() < deadline:
        response = fetch_guest_control_status(guest_ip, run_id)
        if response is not None and response.get("run_id") == run_id:
            break
        time.sleep(0.2)
    if response is None or response.get("run_id") != run_id:
        fail("guest performance controller did not answer the start request")
    if response.get("status") == "failed":
        reason = str(response.get("reason", "guest controller failed"))[:512]
        fail(f"guest performance controller rejected Chromium setup: {reason}")
    if set(response) != GUEST_READY_FIELDS or response.get("schema_version") != 1:
        fail("guest performance controller returned an unexpected ready shape")
    if response.get("status") != "ready":
        fail("guest performance controller did not report ready")
    if response.get("cdp_internal_port") != cdp_internal_port or response.get(
        "cdp_proxy_port"
    ) != cdp_proxy_port:
        fail("guest performance controller changed the requested CDP ports")
    for field in ("chromium_pid", "proxy_pid"):
        value = response.get(field)
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            fail(f"guest performance controller returned a malformed {field}")
    profile = response.get("profile")
    if not isinstance(profile, str) or re.fullmatch(
        r"/var/lib/mcnf-browser/performance-chromium-[0-9a-f]{16}", profile
    ) is None:
        fail("guest performance controller returned a malformed profile path")
    helper_sha256 = response.get("helper_sha256")
    if not isinstance(helper_sha256, str) or GUEST_HELPER_DIGEST_RE.fullmatch(
        helper_sha256
    ) is None:
        fail("guest performance controller returned a malformed helper digest")
    chromium_stats = validate_guest_chromium_stats(
        response, int(response["chromium_pid"])
    )
    try:
        tabs = wait_for_tabs(
            guest_ip, cdp_proxy_port, cdp_internal_port, run_id
        )
        activate_tab(guest_ip, cdp_proxy_port, tabs[0].id)
        browser_identity_material = (
            f"{facts['guest_boot_id']}:{run_id}:{min(chromium_stats['pids'])}:"
            f"{chromium_stats['oldest_process_seconds']}"
        )
        browser_instance_id = str(
            uuid.uuid5(
                uuid.NAMESPACE_URL,
                "mcnf-browser-instance:" + browser_identity_material,
            )
        )
        return (
            tabs,
            browser_instance_id,
            profile,
            helper_sha256,
            int(chromium_stats["metrics_sequence"]),
        )
    except Exception:
        stop_guest_controlled_chromium(
            guest, guest_ip, run_id, required=False
        )
        raise


def stop_guest_controlled_chromium(
    guest: GuestAgent,
    guest_ip: str,
    run_id: str,
    *,
    required: bool = True,
) -> None:
    if re.fullmatch(r"[0-9a-f]{16}", run_id) is None:
        if required:
            fail("guest controlled Chromium stop identity is malformed")
        return
    try:
        guest.write_control_request(
            {"schema_version": 1, "action": "stop", "run_id": run_id}
        )
        deadline = time.monotonic() + 20.0
        while time.monotonic() < deadline:
            response = fetch_guest_control_status(guest_ip, run_id)
            if response is not None and response.get("run_id") == run_id:
                if response.get("status") == "stopped":
                    return
                if response.get("status") == "failed":
                    break
            time.sleep(0.2)
        if required:
            fail("guest performance controller did not confirm Chromium cleanup")
    except HarnessError:
        if required:
            raise
    finally:
        try:
            guest.remove_control_file(GUEST_CONTROL_REQUEST)
        except HarnessError:
            if required:
                raise


def libvirt_domain_uuid(domain: str) -> str:
    try:
        completed = subprocess.run(
            ["/usr/bin/virsh", "-c", "qemu:///system", "domuuid", domain],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        fail(f"cannot resolve libvirt domain UUID: {exc}")
    value = completed.stdout.strip().lower()
    if completed.returncode != 0 or UUID_RE.fullmatch(value) is None:
        fail("libvirt domain did not return a canonical UUID")
    return value


def qemu_pid(domain: str) -> int:
    candidates = [
        Path(f"/run/libvirt/qemu/{domain}.pid"),
        Path(f"/var/run/libvirt/qemu/{domain}.pid"),
    ]
    for path in candidates:
        try:
            value = int(path.read_text(encoding="utf-8").strip())
        except (OSError, ValueError):
            continue
        if value > 0 and Path(f"/proc/{value}").is_dir():
            return value
    fail("cannot resolve the live Browser VM QEMU process")


@dataclass
class RuntimeIdentity:
    host_boot_id: str
    guest_boot_id: str
    domain_uuid: str
    browser_instance_id: str
    workload_instance_id: str
    source_instance_id: str
    session_id: str
    tab_ids: list[str]

    def validate(self) -> None:
        values = [
            self.host_boot_id,
            self.guest_boot_id,
            self.domain_uuid,
            self.browser_instance_id,
            self.workload_instance_id,
            self.source_instance_id,
        ]
        if any(UUID_RE.fullmatch(value) is None for value in values):
            fail("one or more runtime identities are not canonical UUIDs")
        if len(set(values)) != len(values):
            fail("runtime identities are not distinct")
        if SESSION_RE.fullmatch(self.session_id) is None:
            fail("Browser VM session identity is malformed")
        if len(self.tab_ids) < MIN_TABS or len(set(self.tab_ids)) != len(self.tab_ids):
            fail("CDP tab identities are missing or duplicated")


@dataclass
class HarnessContext:
    source_commit: str
    image_digest: str
    domain: str
    guest_ip: str
    identity: RuntimeIdentity
    guest: GuestAgent
    tabs: list[CdpTab]
    probe: RdpProbe
    sidecar: SidecarWriter
    qemu: ProcSampler
    gpu: DrmGpuSampler
    probe_sha256: str
    server_sha256: str
    guest_run_id: str
    guest_helper_sha256: str
    guest_profile: str
    last_guest_metrics_sequence: int
    media_asset: MediaAsset
    firewall: FirewallLease
    credential_runtime_root: Path


def write_stream_record(
    output: BinaryIO, digest: hashlib._Hash, record: dict[str, Any]
) -> None:
    encoded = compact_json(record) + b"\n"
    output.write(encoded)
    output.flush()
    digest.update(encoded)


def collect_tab_snapshots(tabs: list[CdpTab]) -> list[dict[str, Any]]:
    snapshots: list[dict[str, Any]] = []
    for tab in tabs:
        snapshots.append(tab.snapshot())
    return snapshots


def minimize_chromium_windows(tabs: list[CdpTab]) -> list[int]:
    """Minimize every actual Chromium window and return verified CDP identities."""
    windows: dict[int, CdpTab] = {}
    for tab in tabs:
        windows.setdefault(tab.window_id(), tab)
    if not windows:
        fail("no Chromium window was available for the measured hidden phase")
    for window_id, tab in windows.items():
        tab.minimize_window(window_id)
    return sorted(windows)


def make_header(context: HarnessContext, nonce: str) -> dict[str, Any]:
    identity = context.identity
    return {
        "schema_version": STREAM_SCHEMA_VERSION,
        "kind": "browser_vm_performance_stream",
        "source": "live-browser-vm-session-endpoint",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "workload": "browser-vm",
        "session_id": identity.session_id,
        "source_commit": context.source_commit,
        "image_digest": context.image_digest,
        "transport": "rdp",
        "collection_nonce": nonce,
        "shell_metrics_source": SHELL_METRICS_SOURCE,
        "guest_metrics_source": GUEST_METRICS_SOURCE,
        "host_boot_id": identity.host_boot_id,
        "guest_boot_id": identity.guest_boot_id,
        "domain_uuid": identity.domain_uuid,
        "browser_instance_id": identity.browser_instance_id,
        "workload_instance_id": identity.workload_instance_id,
        "source_instance_id": identity.source_instance_id,
        "tab_ids": identity.tab_ids,
        "supported_target_fps": TARGET_FPS,
    }


def run_live_stream(context: HarnessContext, nonce: str, output: BinaryIO) -> None:
    if NONCE_RE.fullmatch(nonce) is None:
        fail("collector challenge is malformed")
    digest = hashlib.sha256()
    sample_count = 0
    completion_status = "completed"
    header = make_header(context, nonce)
    context.sidecar.write(
        {
            "type": "header",
            "recorded_at": utc_now(),
            "stream_header": header,
            "server_sha256": context.server_sha256,
            "rdp_probe_sha256": context.probe_sha256,
            "guest_helper_sha256": context.guest_helper_sha256,
            "media_asset": {
                "sha256": context.media_asset.sha256,
                "codec": context.media_asset.codec,
                "width": context.media_asset.width,
                "height": context.media_asset.height,
                "fps": context.media_asset.fps,
                "duration_ms": context.media_asset.duration_ms,
                "generator_sha256": context.media_asset.generator_sha256,
                "generator_version": context.media_asset.generator_version,
            },
            "media_firewall_rule": context.firewall.rule,
            "qemu_pid": context.qemu.pid,
        }
    )
    write_stream_record(output, digest, header)

    for tab in context.tabs:
        tab.begin()
    context.probe.begin()
    started = time.monotonic()
    initial_tab_snapshots = collect_tab_snapshots(context.tabs)
    frame_baselines = {
        tab.id: snapshot["framesPresented"]
        for tab, snapshot in zip(context.tabs, initial_tab_snapshots)
    }
    previous_tab_frames = {tab.id: 0 for tab in context.tabs}
    pending_session_latencies: list[int] = []
    pending_navigation_latencies: list[int] = []
    sample_index = 0
    browser_hidden = False

    try:
        while True:
            due = started + sample_index * SAMPLE_INTERVAL_SECONDS
            remaining = due - time.monotonic()
            if remaining > 0:
                time.sleep(remaining)
            observed_at = time.monotonic()
            elapsed_ms = max(0, int((observed_at - started) * 1_000))
            if not browser_hidden and elapsed_ms >= int(HIDE_AT_SECONDS * 1_000):
                minimized_windows = minimize_chromium_windows(context.tabs)
                context.probe.mark_hidden()
                browser_hidden = True
                context.sidecar.write(
                    {
                        "type": "visibility_transition",
                        "recorded_at": utc_now(),
                        "elapsed_ms": elapsed_ms,
                        "browser_visible": False,
                        "mechanism": "cdp-verified-window-minimize",
                        "window_ids": minimized_windows,
                    }
                )
            rdp, new_session_latencies = context.probe.snapshot(
                expected_browser_visible=not browser_hidden
            )
            pending_session_latencies.extend(new_session_latencies)
            tab_snapshots = collect_tab_snapshots(context.tabs)
            tab_source_frames: dict[str, dict[str, int]] = {}
            for tab, snapshot in zip(context.tabs, tab_snapshots):
                measured = snapshot["framesPresented"] - frame_baselines[tab.id]
                if measured < previous_tab_frames[tab.id]:
                    fail(f"CDP source-frame counter decreased for immutable tab {tab.id}")
                previous_tab_frames[tab.id] = measured
                if snapshot["width"] != WIDTH or snapshot["height"] != HEIGHT:
                    fail(f"CDP media source lost 1920x1080 geometry for tab {tab.id}")
                tab_source_frames[tab.id] = {
                    "frames_presented": measured,
                    "width": snapshot["width"],
                    "height": snapshot["height"],
                }
                pending_navigation_latencies.extend(snapshot["navigationLatenciesMs"])

            if sample_index == 0:
                # The collector requires the first challenged sample to exclude
                # all setup/pre-challenge events.  They are discarded, never
                # shifted into a later interval.
                pending_navigation_latencies.clear()
                pending_session_latencies.clear()
            navigation_latencies = pending_navigation_latencies[:128]
            del pending_navigation_latencies[: len(navigation_latencies)]
            session_latencies = pending_session_latencies[:128]
            del pending_session_latencies[: len(session_latencies)]

            host_gpu_busy = context.gpu.sample()
            chromium_stats = current_guest_chromium_stats(
                context.guest_ip,
                context.guest_run_id,
                context.guest_helper_sha256,
            )
            metrics_sequence = int(chromium_stats["metrics_sequence"])
            if (
                sample_index > 0
                and metrics_sequence <= context.last_guest_metrics_sequence
            ):
                fail("guest Chromium CPU/RSS metrics stopped advancing")
            if metrics_sequence < context.last_guest_metrics_sequence:
                fail("guest Chromium CPU/RSS metric sequence decreased")
            context.last_guest_metrics_sequence = metrics_sequence
            browser_visible = bool(rdp.get("browser_visible"))
            visible_tab_id = context.identity.tab_ids[0] if browser_visible else None
            sample = {
                "type": "sample",
                "elapsed_ms": elapsed_ms,
                "tab_count": len(context.tabs),
                "viewport_width": WIDTH,
                "viewport_height": HEIGHT,
                "frames_received": int(rdp["frames_received"]),
                "max_frame_gap_ms": min(60_000, int(rdp["max_frame_gap_ms"])),
                "pointer_updates": int(rdp["pointer_updates"]),
                "navigation_latencies_ms": navigation_latencies,
                "session_latencies_ms": session_latencies,
                "reconnects": int(rdp["reconnects"]),
                "connection_state": rdp["connection_state"],
                "tab_source_frames": tab_source_frames,
                "visible_tab_id": visible_tab_id,
                "browser_visible": browser_visible,
                "pointer_x": int(rdp["pointer_x"]),
                "pointer_y": int(rdp["pointer_y"]),
                "full_uploads": int(rdp["full_uploads"]),
                "partial_uploads": int(rdp["partial_uploads"]),
                "partial_rects": int(rdp["partial_rects"]),
                "surface_repaints": int(rdp["surface_repaints"]),
                "host_process_cpu_permille": rdp.get("host_process_cpu_permille"),
                "host_gpu_busy_permille": host_gpu_busy,
            }
            write_stream_record(output, digest, sample)
            sample_count += 1

            context.sidecar.write(
                {
                    "type": "sample",
                    "recorded_at": utc_now(),
                    "elapsed_ms": elapsed_ms,
                    "stream_sample_sha256": "sha256:"
                    + hashlib.sha256(compact_json(sample)).hexdigest(),
                    "rdp_observer": {
                        "pid": context.probe.process.pid,
                        "cpu_permille_host": rdp.get("host_process_cpu_permille"),
                        "rss_kib": rdp.get("host_process_rss_kib"),
                        "frames_received": sample["frames_received"],
                        "connection_state": sample["connection_state"],
                    },
                    "qemu": context.qemu.sample(),
                    "chromium": chromium_stats,
                    "drm_gpu": {
                        "busy_permille": host_gpu_busy,
                        "source": context.gpu.source,
                    },
                    "tabs": {
                        tab.id: {
                            "url_sha256": "sha256:"
                            + hashlib.sha256(tab.url.encode("utf-8")).hexdigest(),
                            "frames_presented": tab_source_frames[tab.id]["frames_presented"],
                            "source_tick": snapshot["sourceTick"],
                            "width": tab_source_frames[tab.id]["width"],
                            "height": tab_source_frames[tab.id]["height"],
                            "ready_state": snapshot["readyState"],
                        }
                        for tab, snapshot in zip(context.tabs, tab_snapshots)
                    },
                }
            )

            if elapsed_ms >= int(COLLECTION_SECONDS * 1_000):
                break
            sample_index += 1
    except Exception:
        completion_status = "failed"
        raise
    finally:
        complete = {
            "type": "complete",
            "status": completion_status,
            "collection_nonce": nonce,
            "sample_count": sample_count,
            "stream_sha256": "sha256:" + digest.hexdigest(),
        }
        # The complete record binds the exact header+sample prefix and therefore
        # is intentionally not included in its own digest.
        output.write(compact_json(complete) + b"\n")
        output.flush()
        context.sidecar.write(
            {
                "type": "complete",
                "recorded_at": utc_now(),
                "status": completion_status,
                "sample_count": sample_count,
                "stream_sha256": complete["stream_sha256"],
            }
        )


class PerformanceHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    context: HarnessContext
    request_claimed = False
    claim_lock = threading.Lock()

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        if urllib.parse.urlsplit(self.path).path != ENDPOINT_PATH:
            self.send_error(HTTPStatus.NOT_FOUND)
            return
        nonce = self.headers.get(NONCE_HEADER, "")
        if NONCE_RE.fullmatch(nonce) is None:
            self.send_error(HTTPStatus.BAD_REQUEST, "fresh collection nonce required")
            return
        with self.claim_lock:
            if self.request_claimed:
                self.send_error(HTTPStatus.CONFLICT, "collection already claimed")
                return
            type(self).request_claimed = True
        self.send_response(HTTPStatus.OK)
        self.send_header("Content-Type", "application/x-ndjson")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            run_live_stream(self.context, nonce, self.wfile)
        except (BrokenPipeError, ConnectionResetError):
            return
        except Exception as exc:
            print(f"serve-browser-vm-performance: live stream failed: {exc}", file=sys.stderr)

    def log_message(self, _format: str, *_args: Any) -> None:
        return


def build_context(args: argparse.Namespace) -> tuple[HarnessContext, MediaServer]:
    validate_source(args.source_commit, args.image_digest)
    domain = validate_domain(args.domain)
    guest_ip = validate_ip(args.guest_ip)
    if args.bind != "127.0.0.1":
        fail("the public performance endpoint must bind exactly to 127.0.0.1")
    if args.media_bind != "192.168.122.1" or args.media_port != 9081:
        fail("the controlled media origin must remain the Dell libvirt bridge endpoint")
    validate_regular_private(args.credential_file, "RDP credential file")
    if not args.rdp_probe.is_absolute() or not args.rdp_probe.is_file():
        fail("RDP probe must be one absolute regular file")

    credential_runtime_root, password_path = materialize_rdp_password(
        args.credential_file, args.rdp_user
    )
    sidecar: SidecarWriter | None = None
    media: MediaServer | None = None
    firewall: FirewallLease | None = None
    probe: RdpProbe | None = None
    guest = GuestAgent(domain, args.rdp_user)
    guest_run_id: str | None = None
    try:
        sidecar = SidecarWriter(args.sidecar_out)
        firewall = FirewallLease(guest_ip, args.media_port)
        media_asset = generate_media_asset(credential_runtime_root)
        media = MediaServer(args.media_bind, args.media_port, media_asset)
        media.start()
        probe = RdpProbe(
            args.rdp_probe,
            guest_ip,
            args.rdp_port,
            args.rdp_user,
            password_path,
        )
        ready = probe.wait_ready()
        facts = wait_for_guest_session_facts(guest)
        if facts["source_commit"] != args.source_commit:
            fail("guest source commit does not match the requested immutable source")
        if facts["image_digest"].lower() != args.image_digest:
            fail("guest image digest does not match the requested immutable image")
        if facts["transport"] != "rdp":
            fail("guest transport is not the collector's requested RDP path")
        if facts["guest_user"] != args.rdp_user:
            fail("guest runtime account does not match the RDP credential account")
        run_id = secrets.token_hex(8)
        guest_run_id = run_id
        cdp_internal_port = secrets.randbelow(1_000) + 39_000
        cdp_proxy_port = secrets.randbelow(1_000) + 40_000
        (
            tabs,
            browser_instance_id,
            guest_profile,
            guest_helper_sha256,
            guest_metrics_sequence,
        ) = (
            prepare_controlled_chromium(
                guest,
                facts,
                guest_ip,
                cdp_internal_port,
                cdp_proxy_port,
                run_id,
            )
        )
        host_boot_id = Path("/proc/sys/kernel/random/boot_id").read_text(encoding="utf-8").strip()
        domain_id = libvirt_domain_uuid(domain)
        workload_instance_id = str(
            uuid.uuid5(
                uuid.NAMESPACE_URL,
                f"mcnf-workload:{domain_id}:{facts['guest_boot_id']}:{facts['session_id']}",
            )
        )
        identity = RuntimeIdentity(
            host_boot_id=host_boot_id,
            guest_boot_id=facts["guest_boot_id"],
            domain_uuid=domain_id,
            browser_instance_id=browser_instance_id,
            workload_instance_id=workload_instance_id,
            source_instance_id=str(ready["source_instance_id"]),
            session_id=facts["session_id"],
            tab_ids=[tab.id for tab in tabs],
        )
        identity.validate()
        qemu = ProcSampler(qemu_pid(domain))
        context = HarnessContext(
            source_commit=args.source_commit,
            image_digest=args.image_digest,
            domain=domain,
            guest_ip=guest_ip,
            identity=identity,
            guest=guest,
            tabs=tabs,
            probe=probe,
            sidecar=sidecar,
            qemu=qemu,
            gpu=DrmGpuSampler(),
            probe_sha256=file_sha256(args.rdp_probe),
            server_sha256=file_sha256(Path(__file__).resolve()),
            guest_run_id=run_id,
            guest_helper_sha256=guest_helper_sha256,
            guest_profile=guest_profile,
            last_guest_metrics_sequence=guest_metrics_sequence,
            media_asset=media_asset,
            firewall=firewall,
            credential_runtime_root=credential_runtime_root,
        )
        return context, media
    except Exception:
        if guest_run_id is not None:
            stop_guest_controlled_chromium(
                guest, guest_ip, guest_run_id, required=False
            )
        if probe is not None:
            probe.stop()
        if media is not None:
            media.stop()
        if firewall is not None:
            firewall.close()
        if sidecar is not None:
            sidecar.close()
        remove_runtime_password(credential_runtime_root)
        raise


def serve(args: argparse.Namespace) -> int:
    context, media = build_context(args)
    PerformanceHandler.context = context
    PerformanceHandler.request_claimed = False
    # This endpoint is deliberately single-claim and synchronous: handle_request
    # must not return (and trigger cleanup) until the 15-minute stream completes.
    server = HTTPServer((args.bind, args.port), PerformanceHandler)
    server.timeout = args.accept_timeout_seconds
    readiness = {
        "status": "ready",
        "endpoint": f"http://{args.bind}:{args.port}{ENDPOINT_PATH}",
        "domain": context.domain,
        "guest_ip": context.guest_ip,
        "session_id": context.identity.session_id,
        "tab_ids": context.identity.tab_ids,
        "sidecar": str(args.sidecar_out),
        "source_commit": context.source_commit,
        "image_digest": context.image_digest,
    }
    print(json.dumps(readiness, sort_keys=True), flush=True)
    try:
        server.handle_request()
        if not PerformanceHandler.request_claimed:
            fail("no collector claimed the live endpoint before its accept timeout")
        return 0
    finally:
        server.server_close()
        for tab in context.tabs:
            tab.close()
        context.probe.stop()
        stop_guest_controlled_chromium(
            context.guest,
            context.guest_ip,
            context.guest_run_id,
            required=True,
        )
        media.stop()
        context.firewall.close()
        context.sidecar.close()
        remove_runtime_password(context.credential_runtime_root)


def percentile(values: list[int], fraction: float) -> int | None:
    if not values:
        return None
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, int(len(ordered) * fraction + 0.999999) - 1))
    return ordered[index]


def summarize(args: argparse.Namespace) -> int:
    try:
        metadata = args.sidecar.lstat()
    except OSError as exc:
        fail(f"sidecar is unavailable: {exc}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail("sidecar must be a regular non-symlink file")
    records: list[dict[str, Any]] = []
    for number, raw in enumerate(args.sidecar.read_bytes().splitlines(), start=1):
        try:
            record = json.loads(raw)
        except json.JSONDecodeError as exc:
            fail(f"sidecar record {number} is malformed: {exc}")
        if not isinstance(record, dict):
            fail(f"sidecar record {number} is not an object")
        records.append(record)
    samples = [record for record in records if record.get("type") == "sample"]
    if not samples:
        fail("sidecar contains no live samples")

    def observed(path: tuple[str, ...]) -> list[int]:
        values: list[int] = []
        for sample in samples:
            value: Any = sample
            for key in path:
                if not isinstance(value, dict):
                    value = None
                    break
                value = value.get(key)
            if isinstance(value, int) and not isinstance(value, bool):
                values.append(value)
        return values

    qemu_cpu = observed(("qemu", "cpu_permille_host"))
    qemu_rss = observed(("qemu", "rss_kib"))
    chromium_cpu = observed(("chromium", "cpu_permille_one_cpu"))
    chromium_rss = observed(("chromium", "rss_kib"))
    gpu = observed(("drm_gpu", "busy_permille"))
    result = {
        "sidecar": str(args.sidecar),
        "sample_count": len(samples),
        "elapsed_ms": samples[-1].get("elapsed_ms"),
        "qemu": {
            "cpu_p95_permille_host": percentile(qemu_cpu, 0.95),
            "cpu_max_permille_host": max(qemu_cpu, default=None),
            "rss_p95_kib": percentile(qemu_rss, 0.95),
            "rss_max_kib": max(qemu_rss, default=None),
        },
        "chromium": {
            "cpu_p95_permille_one_cpu": percentile(chromium_cpu, 0.95),
            "cpu_max_permille_one_cpu": max(chromium_cpu, default=None),
            "rss_p95_kib": percentile(chromium_rss, 0.95),
            "rss_max_kib": max(chromium_rss, default=None),
        },
        "drm_gpu": {
            "busy_p95_permille": percentile(gpu, 0.95),
            "busy_max_permille": max(gpu, default=None),
            "coverage_samples": len(gpu),
        },
        "terminal": records[-1] if records[-1].get("type") == "complete" else None,
    }
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


def self_test() -> None:
    assert compact_json({"b": 2, "a": 1}) == b'{"a":1,"b":2}'
    assert parse_i915_engine_runtimes("rcs0\n\tRuntime: 12ms\nvcs0\n  Runtime: 9ms\n") == {
        "rcs0": 12,
        "vcs0": 9,
    }
    validate_source("1" * 40, "sha256:" + "a" * 64)
    assert b"/video.webm" in MEDIA_PAGE
    assert b"requestVideoFrameCallback" in MEDIA_PAGE
    identities = RuntimeIdentity(
        host_boot_id="11111111-1111-4111-8111-111111111111",
        guest_boot_id="22222222-2222-4222-8222-222222222222",
        domain_uuid="33333333-3333-4333-8333-333333333333",
        browser_instance_id="44444444-4444-4444-8444-444444444444",
        workload_instance_id="55555555-5555-4555-8555-555555555555",
        source_instance_id="66666666-6666-4666-8666-666666666666",
        session_id="session:self-test",
        tab_ids=[f"cdp-tab-{index}" for index in range(1, 6)],
    )
    identities.validate()
    with tempfile.TemporaryDirectory(prefix="serve-browser-vm-performance-") as root:
        credential = Path(root) / "browser-vm-rdp.json"
        credential.write_text(
            '{"schema_version":1,"username":"mcnf-browser","password":"selftest-hex"}\n',
            encoding="utf-8",
        )
        credential.chmod(0o600)
        runtime_root, password_path = materialize_rdp_password(
            credential, "mcnf-browser", runtime_parent=Path(root)
        )
        assert password_path.read_text(encoding="utf-8") == "selftest-hex\n"
        remove_runtime_password(runtime_root)
        sidecar_path = Path(root) / "private" / "sidecar.ndjson"
        sidecar = SidecarWriter(sidecar_path)
        sidecar.write({"type": "self-test", "fixture_status": "never-live"})
        sidecar.close()
        assert stat.S_IMODE(sidecar_path.stat().st_mode) == 0o600
        media_asset = MediaAsset(
            data=b"self-test-webm",
            sha256="sha256:" + hashlib.sha256(b"self-test-webm").hexdigest(),
            codec="vp9",
            width=WIDTH,
            height=HEIGHT,
            fps=36,
            duration_ms=MEDIA_DURATION_SECONDS * 1_000,
            generator_sha256="sha256:" + "a" * 64,
            generator_version="self-test only",
        )
        media = MediaServer("127.0.0.1", 0, media_asset)
        media.start()
        port = media.server.server_address[1]
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/media.html", timeout=5) as response:
            assert response.status == HTTPStatus.OK
            assert b"__mcnfPerformance" in response.read()
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/video.webm", timeout=5) as response:
            assert response.status == HTTPStatus.OK
            assert response.headers.get_content_type() == "video/webm"
            assert response.read() == media_asset.data
        media.stop()
    print("serve-browser-vm-performance.py: self-test passed")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    subparsers = parser.add_subparsers(dest="command")

    serve_parser = subparsers.add_parser("serve", help="prepare and serve one live collection")
    serve_parser.add_argument("--bind", default="127.0.0.1")
    serve_parser.add_argument("--port", type=int, default=9080)
    serve_parser.add_argument("--media-bind", default="192.168.122.1")
    serve_parser.add_argument("--media-port", type=int, default=9081)
    serve_parser.add_argument("--domain", required=True)
    serve_parser.add_argument("--guest-ip", required=True)
    serve_parser.add_argument("--source-commit", required=True)
    serve_parser.add_argument("--image-digest", required=True)
    serve_parser.add_argument("--rdp-user", required=True)
    serve_parser.add_argument("--rdp-port", type=int, default=3389)
    serve_parser.add_argument("--credential-file", required=True, type=Path)
    serve_parser.add_argument("--rdp-probe", required=True, type=Path)
    serve_parser.add_argument("--sidecar-out", required=True, type=Path)
    serve_parser.add_argument("--accept-timeout-seconds", type=int, default=300)

    summarize_parser = subparsers.add_parser(
        "summarize", help="summarize the private extended live sidecar"
    )
    summarize_parser.add_argument("--sidecar", required=True, type=Path)

    args = parser.parse_args(argv)
    try:
        if args.self_test:
            if args.command is not None:
                parser.error("--self-test does not accept a subcommand")
            self_test()
            return 0
        if args.command == "serve":
            if not 1 <= args.port <= 65535 or not 1 <= args.rdp_port <= 65535:
                fail("ports must be between 1 and 65535")
            if not 60 <= args.accept_timeout_seconds <= 3_600:
                fail("accept timeout must be between 60 and 3600 seconds")
            return serve(args)
        if args.command == "summarize":
            return summarize(args)
        parser.error("choose serve, summarize, or --self-test")
    except HarnessError as exc:
        print(f"serve-browser-vm-performance: rejected: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
