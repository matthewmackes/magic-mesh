#!/usr/bin/env python3
"""Guest-user controller for the live Browser VM performance harness.

The system service runs this process as ``mcnf-browser``.  A trusted host writes
one root-owned, non-writable JSON request through QEMU Guest Agent.  The helper
then replaces the ordinary browser in the already-authenticated RDP desktop
with a disposable Chromium instance that owns its profile and loopback CDP
listener.  A source-restricted socat listener exposes CDP only to the libvirt
host bridge.  No metric, frame counter, or pass/fail value crosses this control
file; the host reads all measurements from Chromium and the real RDP stream.
"""

from __future__ import annotations

import hashlib
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import signal
import socket
import stat
import subprocess
import sys
import threading
import time
import urllib.parse
from typing import Any, NoReturn


REQUEST_PATH = Path("/tmp/mcnf-browser-performance-control/request.json")
EXPECTED_USER = "mcnf-browser"
EXPECTED_UID = 1000
HOST_BRIDGE = "192.168.122.1"
MEDIA_ORIGIN = "http://192.168.122.1:9081"
STATUS_PORT = 41_880
RUN_ID_RE = re.compile(r"^[0-9a-f]{16}$")
PROFILE_RE = re.compile(
    r"^/var/lib/mcnf-browser/performance-chromium-[0-9a-f]{16}$"
)


class GuestHarnessError(Exception):
    """The requested guest operation could not be performed truthfully."""


def fail(message: str) -> NoReturn:
    raise GuestHarnessError(message)


def utc_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def own_sha256() -> str:
    digest = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    return "sha256:" + digest


_response_lock = threading.Lock()
_response: dict[str, Any] | None = None


def publish_response(record: dict[str, Any]) -> None:
    payload = json.dumps(record, sort_keys=True, separators=(",", ":")).encode()
    if len(payload) > 16 * 1024:
        fail("guest response exceeded 16 KiB")
    with _response_lock:
        global _response
        _response = dict(record)


class StatusHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        if self.client_address[0] != HOST_BRIDGE:
            self.send_error(HTTPStatus.FORBIDDEN)
            return
        parsed = urllib.parse.urlsplit(self.path)
        query = urllib.parse.parse_qs(parsed.query, strict_parsing=True)
        run_ids = query.get("run_id", [])
        if (
            parsed.path != "/v1/status"
            or len(run_ids) != 1
            or RUN_ID_RE.fullmatch(run_ids[0]) is None
        ):
            self.send_error(HTTPStatus.BAD_REQUEST)
            return
        with _response_lock:
            record = dict(_response) if _response is not None else None
        if record is None or record.get("run_id") != run_ids[0]:
            self.send_error(HTTPStatus.NOT_FOUND)
            return
        body = json.dumps(record, sort_keys=True, separators=(",", ":")).encode() + b"\n"
        self.send_response(HTTPStatus.OK)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *_args: Any) -> None:
        return


class StatusServer(ThreadingHTTPServer):
    allow_reuse_address = True
    daemon_threads = True


def read_request() -> tuple[str, dict[str, Any]] | None:
    try:
        metadata = REQUEST_PATH.lstat()
    except FileNotFoundError:
        return None
    if metadata.st_uid != 0:
        fail("guest request is not root-owned")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail("guest request is not a regular non-symlink file")
    if metadata.st_mode & 0o022:
        fail("guest request grants group/other write access")
    raw = REQUEST_PATH.read_bytes()
    if not raw or len(raw) > 16 * 1024:
        fail("guest request is empty or oversized")
    digest = hashlib.sha256(raw).hexdigest()
    try:
        value = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError):
        fail("guest request is not valid JSON")
    if not isinstance(value, dict):
        fail("guest request is not an object")
    return digest, value


def validate_common(request: dict[str, Any]) -> tuple[str, str]:
    expected = {"schema_version", "action", "run_id"}
    action = request.get("action")
    if action == "start":
        expected |= {
            "guest_ip",
            "media_origin",
            "cdp_internal_port",
            "cdp_proxy_port",
        }
    if set(request) != expected:
        fail("guest request has an unexpected shape")
    if request.get("schema_version") != 1:
        fail("guest request schema is unsupported")
    if action not in {"start", "stop"}:
        fail("guest request action is unsupported")
    run_id = request.get("run_id")
    if not isinstance(run_id, str) or RUN_ID_RE.fullmatch(run_id) is None:
        fail("guest request run identity is malformed")
    return action, run_id


def validate_start(request: dict[str, Any]) -> tuple[str, int, int]:
    guest_ip = request.get("guest_ip")
    media_origin = request.get("media_origin")
    internal = request.get("cdp_internal_port")
    proxy = request.get("cdp_proxy_port")
    if not isinstance(guest_ip, str) or not re.fullmatch(r"192\.168\.122\.[0-9]{1,3}", guest_ip):
        fail("guest request IP is outside the Browser VM subnet")
    if media_origin != MEDIA_ORIGIN:
        fail("guest request media origin is not the controlled host origin")
    for value, lower, upper, label in (
        (internal, 39_000, 39_999, "internal CDP port"),
        (proxy, 40_000, 40_999, "proxied CDP port"),
    ):
        if isinstance(value, bool) or not isinstance(value, int) or not lower <= value <= upper:
            fail(f"guest request {label} is outside its admitted range")
    if internal == proxy:
        fail("guest request CDP ports collide")
    return guest_ip, internal, proxy


def display_facts(timeout: float = 90.0) -> tuple[str, str]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        waylands = sorted(Path(f"/run/user/{EXPECTED_UID}").glob("wayland-*"))
        wayland = next((path.name for path in waylands if path.is_socket()), None)
        displays = sorted(Path("/tmp/.X11-unix").glob("X*"))
        display_path = next((path for path in reversed(displays) if path.is_socket()), None)
        if wayland is not None and display_path is not None:
            return wayland, ":" + display_path.name.removeprefix("X")
        time.sleep(0.25)
    fail("authenticated RDP desktop sockets did not appear")


def chromium_binary() -> str:
    for candidate in ("/usr/bin/chromium", "/usr/bin/chromium-browser"):
        if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return candidate
    fail("guest Chromium binary is unavailable")


def stop_process(process: subprocess.Popen[bytes] | None, label: str) -> None:
    if process is None or process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=10)
    except (ProcessLookupError, subprocess.TimeoutExpired):
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            print(f"serve-browser-vm-performance-guest: {label} did not stop", file=sys.stderr)


def wait_cdp(port: int, chromium: subprocess.Popen[bytes], timeout: float = 45.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if chromium.poll() is not None:
            fail(f"controlled Chromium exited with status {chromium.returncode}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                return
        except OSError:
            time.sleep(0.1)
    fail("controlled Chromium CDP listener did not become reachable")


class ActiveRun:
    def __init__(self, request: dict[str, Any], run_id: str) -> None:
        self.run_id = run_id
        self.chromium: subprocess.Popen[bytes] | None = None
        self.proxy: subprocess.Popen[bytes] | None = None
        self.profile = Path(f"/var/lib/mcnf-browser/performance-chromium-{run_id}")
        if PROFILE_RE.fullmatch(str(self.profile)) is None:
            fail("guest controlled profile path did not validate")
        guest_ip, internal_port, proxy_port = validate_start(request)
        self.internal_port = internal_port
        self.proxy_port = proxy_port
        self.guest_ip = guest_ip
        self._last_cpu_ticks: dict[int, int] = {}
        self._last_cpu_at: float | None = None
        self._metrics_sequence = 0
        try:
            self._start()
        except Exception:
            self.stop()
            raise

    def _start(self) -> None:
        wayland, display = display_facts()
        subprocess.run(
            ["/usr/bin/pkill", "-TERM", "-u", str(EXPECTED_UID), "-x", "chromium"],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        time.sleep(2)
        self.profile.mkdir(mode=0o700, parents=False, exist_ok=False)
        environment = os.environ.copy()
        environment.update(
            {
                "HOME": pwd.getpwuid(EXPECTED_UID).pw_dir,
                "XDG_RUNTIME_DIR": f"/run/user/{EXPECTED_UID}",
                "WAYLAND_DISPLAY": wayland,
                "DISPLAY": display,
                "DBUS_SESSION_BUS_ADDRESS": f"unix:path=/run/user/{EXPECTED_UID}/bus",
            }
        )
        self.proxy = subprocess.Popen(
            [
                "/usr/bin/socat",
                (
                    f"TCP4-LISTEN:{self.proxy_port},bind={self.guest_ip},reuseaddr,fork,"
                    f"range={HOST_BRIDGE}/32"
                ),
                f"TCP4:127.0.0.1:{self.internal_port}",
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        media_urls = [
            f"{MEDIA_ORIGIN}/media.html?tab={index}&run={self.run_id}"
            for index in range(1, 6)
        ]
        self.chromium = subprocess.Popen(
            [
                chromium_binary(),
                "--ozone-platform=wayland",
                "--enable-features=UseOzonePlatform",
                "--start-maximized",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-session-crashed-bubble",
                "--disable-infobars",
                "--autoplay-policy=no-user-gesture-required",
                "--disable-background-timer-throttling",
                "--disable-backgrounding-occluded-windows",
                "--disable-renderer-backgrounding",
                "--disable-background-media-suspend",
                "--remote-allow-origins=*",
                "--remote-debugging-address=127.0.0.1",
                f"--remote-debugging-port={self.internal_port}",
                f"--user-data-dir={self.profile}",
                "--window-size=1920,1080",
                "--force-device-scale-factor=1",
                *media_urls,
            ],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        wait_cdp(self.internal_port, self.chromium)

    def chromium_stats(self) -> dict[str, Any]:
        """Read CPU and RSS for the controlled Chromium process session.

        The system QEMU guest-agent domain is intentionally unable to inspect
        this user's processes.  This helper already owns the controlled
        browser and can therefore measure its dedicated POSIX session directly
        from procfs without crossing that confinement boundary.
        """
        assert self.chromium is not None
        clock_ticks = int(os.sysconf("SC_CLK_TCK"))
        page_size = int(os.sysconf("SC_PAGE_SIZE"))
        if clock_ticks <= 0 or page_size <= 0:
            fail("guest procfs clock/page geometry is invalid")
        session_id = self.chromium.pid
        current_ticks: dict[int, int] = {}
        rss_kib = 0
        oldest_seconds = 0
        observed_at = time.monotonic()
        for path in Path("/proc").iterdir():
            if not path.name.isdecimal():
                continue
            pid = int(path.name)
            try:
                if path.stat().st_uid != EXPECTED_UID or os.getsid(pid) != session_id:
                    continue
                raw = (path / "stat").read_text(encoding="utf-8")
                fields = raw[raw.rfind(")") + 2 :].split()
                if len(fields) < 22:
                    continue
                ticks = int(fields[11]) + int(fields[12])
                started_ticks = int(fields[19])
                rss_pages = int(fields[21])
            except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError):
                continue
            if ticks < 0 or started_ticks < 0 or rss_pages < 0:
                continue
            current_ticks[pid] = ticks
            rss_kib += rss_pages * page_size // 1024
            oldest_seconds = max(
                oldest_seconds,
                max(0, int(observed_at - started_ticks / clock_ticks)),
            )
        if self.chromium.pid not in current_ticks or not current_ticks:
            fail("controlled Chromium procfs session disappeared")

        cpu_permille = 0
        if self._last_cpu_at is not None and observed_at > self._last_cpu_at:
            delta_ticks = sum(
                max(0, ticks - self._last_cpu_ticks.get(pid, ticks))
                for pid, ticks in current_ticks.items()
            )
            cpu_permille = round(
                delta_ticks
                * 1_000
                / clock_ticks
                / (observed_at - self._last_cpu_at)
            )
        self._last_cpu_ticks = current_ticks
        self._last_cpu_at = observed_at
        self._metrics_sequence += 1
        return {
            "process_count": len(current_ticks),
            "pids": sorted(current_ticks),
            "oldest_process_seconds": oldest_seconds,
            "rss_kib": rss_kib,
            "cpu_permille_one_cpu": cpu_permille,
            "metrics_sequence": self._metrics_sequence,
            "source": "guest-user-procfs-controlled-session",
        }

    def response(self) -> dict[str, Any]:
        assert self.chromium is not None and self.proxy is not None
        return {
            "schema_version": 1,
            "status": "ready",
            "run_id": self.run_id,
            "profile": str(self.profile),
            "chromium_pid": self.chromium.pid,
            "proxy_pid": self.proxy.pid,
            "cdp_internal_port": self.internal_port,
            "cdp_proxy_port": self.proxy_port,
            "helper_sha256": own_sha256(),
            "chromium": self.chromium_stats(),
            "recorded_at": utc_now(),
        }

    def healthy(self) -> bool:
        return (
            self.chromium is not None
            and self.proxy is not None
            and self.chromium.poll() is None
            and self.proxy.poll() is None
        )

    def stop(self) -> None:
        stop_process(self.chromium, "Chromium")
        stop_process(self.proxy, "CDP proxy")
        if PROFILE_RE.fullmatch(str(self.profile)) is not None:
            shutil.rmtree(self.profile, ignore_errors=True)


def self_test() -> None:
    action, run_id = validate_common(
        {
            "schema_version": 1,
            "action": "start",
            "run_id": "a" * 16,
            "guest_ip": "192.168.122.58",
            "media_origin": MEDIA_ORIGIN,
            "cdp_internal_port": 39_123,
            "cdp_proxy_port": 40_123,
        }
    )
    assert action == "start" and run_id == "a" * 16
    assert validate_start(
        {
            "guest_ip": "192.168.122.58",
            "media_origin": MEDIA_ORIGIN,
            "cdp_internal_port": 39_123,
            "cdp_proxy_port": 40_123,
        }
    ) == ("192.168.122.58", 39_123, 40_123)
    assert PROFILE_RE.fullmatch("/var/lib/mcnf-browser/performance-chromium-" + "b" * 16)
    print("serve-browser-vm-performance-guest.py: self-test passed")


def run() -> int:
    account = pwd.getpwnam(EXPECTED_USER)
    if os.getuid() != EXPECTED_UID or account.pw_uid != EXPECTED_UID:
        fail("guest helper must run as the immutable mcnf-browser account")
    processed: str | None = None
    active: ActiveRun | None = None
    next_metrics_refresh = 0.0
    status_server = StatusServer(("0.0.0.0", STATUS_PORT), StatusHandler)
    threading.Thread(target=status_server.serve_forever, daemon=True).start()

    def shutdown(_signum: int, _frame: Any) -> None:
        if active is not None:
            active.stop()
        status_server.shutdown()
        status_server.server_close()
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)
    while True:
        if active is not None and not active.healthy():
            run_id = active.run_id
            active.stop()
            active = None
            publish_response(
                {
                    "schema_version": 1,
                    "status": "failed",
                    "run_id": run_id,
                    "reason": "controlled Chromium or CDP proxy exited",
                    "helper_sha256": own_sha256(),
                    "recorded_at": utc_now(),
                }
            )
        if active is not None and time.monotonic() >= next_metrics_refresh:
            try:
                publish_response(active.response())
                next_metrics_refresh = time.monotonic() + 1.0
            except Exception as exc:
                run_id = active.run_id
                active.stop()
                active = None
                publish_response(
                    {
                        "schema_version": 1,
                        "status": "failed",
                        "run_id": run_id,
                        "reason": str(exc)[:512],
                        "helper_sha256": own_sha256(),
                        "recorded_at": utc_now(),
                    }
                )
        try:
            request_record = read_request()
        except GuestHarnessError as exc:
            print(f"serve-browser-vm-performance-guest: rejected request: {exc}", file=sys.stderr)
            time.sleep(0.25)
            continue
        if request_record is None or request_record[0] == processed:
            time.sleep(0.2)
            continue
        digest, request = request_record
        processed = digest
        run_id = request.get("run_id") if isinstance(request.get("run_id"), str) else "invalid"
        try:
            action, run_id = validate_common(request)
            if action == "start":
                if active is not None:
                    fail("another controlled Chromium run is already active")
                active = ActiveRun(request, run_id)
                publish_response(active.response())
                next_metrics_refresh = time.monotonic() + 1.0
            else:
                if active is None or active.run_id != run_id:
                    fail("stop request does not identify the active run")
                active.stop()
                active = None
                publish_response(
                    {
                        "schema_version": 1,
                        "status": "stopped",
                        "run_id": run_id,
                        "helper_sha256": own_sha256(),
                        "recorded_at": utc_now(),
                    }
                )
        except Exception as exc:
            if active is not None and active.run_id == run_id:
                active.stop()
                active = None
            publish_response(
                {
                    "schema_version": 1,
                    "status": "failed",
                    "run_id": run_id,
                    "reason": str(exc)[:512],
                    "helper_sha256": own_sha256(),
                    "recorded_at": utc_now(),
                }
            )


def main() -> int:
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        return 0
    if sys.argv[1:]:
        fail("unexpected guest helper arguments")
    return run()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except GuestHarnessError as exc:
        print(f"serve-browser-vm-performance-guest: rejected: {exc}", file=sys.stderr)
        raise SystemExit(2)
