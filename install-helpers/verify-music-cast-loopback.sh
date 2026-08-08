#!/usr/bin/env bash
# Bounded loopback HTTP renderer proof for the Music cast boundary.
#
# This is an independent protocol exchange, not a claim about a physical DLNA
# renderer, Chromecast, mesh owner, or seat handoff.  The disposable renderer
# binds only to 127.0.0.1 and is torn down before this helper exits.
set -euo pipefail

PROGRAM_NAME=verify-music-cast-loopback
EXCHANGE_TIMEOUT_SECONDS="${MUSIC_CAST_LOOPBACK_TIMEOUT_SECONDS:-15}"
RUNTIME_PROBE_SECONDS="${MUSIC_CAST_RUNTIME_PROBE_SECONDS:-5}"

usage() {
    cat >&2 <<'EOF'
usage: verify-music-cast-loopback.sh [--self-test|--runtime-probe]

Starts a disposable 127.0.0.1 HTTP renderer and performs a bounded discovery /
device-description / SetAVTransportURI / Play / Seek exchange. It also sends
malformed and non-finite Seek requests and requires HTTP refusal for both.
No physical DLNA, Chromecast, mesh-owner, or seat-handoff claim is made.

--runtime-probe sends one read-only SSDP MediaRenderer query and one read-only
mDNS _googlecast._tcp query, then reports bounded packets and answer records.
It never sends a renderer control request or CASTV2 command. Zero answers are
a completed observation, not a pass for physical renderer acceptance.

Environment: MUSIC_CAST_LOOPBACK_TIMEOUT_SECONDS (5..60),
MUSIC_CAST_RUNTIME_PROBE_SECONDS (3..15).
EOF
}

fail() {
    printf '%s: %s\n' "$PROGRAM_NAME" "$1" >&2
    exit 2
}

[[ "$EXCHANGE_TIMEOUT_SECONDS" =~ ^[0-9]+$ ]] ||
    fail 'MUSIC_CAST_LOOPBACK_TIMEOUT_SECONDS must be an integer'
(( EXCHANGE_TIMEOUT_SECONDS >= 5 && EXCHANGE_TIMEOUT_SECONDS <= 60 )) ||
    fail 'MUSIC_CAST_LOOPBACK_TIMEOUT_SECONDS must be 5..60'
[[ "$RUNTIME_PROBE_SECONDS" =~ ^[0-9]+$ ]] ||
    fail 'MUSIC_CAST_RUNTIME_PROBE_SECONDS must be an integer'
(( RUNTIME_PROBE_SECONDS >= 3 && RUNTIME_PROBE_SECONDS <= 15 )) ||
    fail 'MUSIC_CAST_RUNTIME_PROBE_SECONDS must be 3..15'
command -v python3 >/dev/null 2>&1 || fail 'python3 is required'
command -v timeout >/dev/null 2>&1 || fail 'timeout is required'

MODE=run
case "${1:-}" in
    '') ;;
    --self-test)
        [[ "$#" -eq 1 ]] || fail '--self-test takes no additional arguments'
        MODE=self-test
        ;;
    --runtime-probe)
        [[ "$#" -eq 1 ]] || fail '--runtime-probe takes no additional arguments'
        MODE=runtime-probe
        ;;
    --help|-h)
        usage
        exit 0
        ;;
    *)
        usage
        fail "unknown argument: $1"
        ;;
esac

# The Python process owns the disposable listener. timeout supplies the outer
# bound; the Python finally block supplies normal-path cleanup and proof.
result="$(timeout --signal=TERM --kill-after=2s "${EXCHANGE_TIMEOUT_SECONDS}s" \
    python3 - "$MODE" "$RUNTIME_PROBE_SECONDS" <<'PY'
from __future__ import annotations

import http.client
import json
import re
import socket
import struct
import sys
import threading
import time
import urllib.parse
import xml.etree.ElementTree as ET
from http.server import BaseHTTPRequestHandler, HTTPServer


MAX_BODY = 64 * 1024
MAX_RUNTIME_RECORDS = 64
MEDIA_URL = "http://127.0.0.1:9/loopback-track.mp3"
SOAP_NS = "urn:schemas-upnp-org:service:AVTransport:1"
ACTION_RE = re.compile(r"#([A-Za-z][A-Za-z0-9]+)")
TIME_RE = re.compile(r"^(\d{2}):(\d{2}):(\d{2})$")
SSDP_TARGET = ("239.255.255.250", 1900)
MDNS_TARGET = ("224.0.0.251", 5353)
DNS_TYPE_NAMES = {1: "A", 12: "PTR", 16: "TXT", 33: "SRV"}


def local_name(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def element_text(root: ET.Element, name: str) -> str | None:
    for element in root.iter():
        if local_name(element.tag) == name:
            return element.text or ""
    return None


class Renderer(HTTPServer):
    allow_reuse_address = True

    def __init__(self, address):
        super().__init__(address, Handler)
        self.events: list[str] = []
        self.statuses: dict[str, int] = {}
        self.failure: str | None = None


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _format, *_args):
        return

    @property
    def renderer(self) -> Renderer:
        return self.server  # type: ignore[return-value]

    def reply(self, status: int, body: bytes, content_type: str = "text/plain"):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True

    def do_GET(self):
        if self.path == "/discover":
            location = (
                f"http://127.0.0.1:{self.renderer.server_port}/description.xml"
            )
            body = json.dumps(
                {
                    "renderers": [
                        {
                            "id": "loopback-renderer-1",
                            "kind": "dlna_upnp",
                            "location": location,
                        }
                    ]
                },
                separators=(",", ":"),
            ).encode()
            self.renderer.events.append("discovery")
            self.renderer.statuses["discovery"] = 200
            self.reply(200, body, "application/json")
            return

        if self.path == "/description.xml":
            body = f"""<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <device><deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
    <friendlyName>Loopback Music Renderer</friendlyName>
    <serviceList><service>
      <serviceType>{SOAP_NS}</serviceType>
      <controlURL>/control</controlURL>
    </service></serviceList>
  </device>
</root>""".encode()
            self.renderer.events.append("description")
            self.renderer.statuses["description"] = 200
            self.reply(200, body, "text/xml; charset=utf-8")
            return

        self.reply(404, b"not found")

    def do_POST(self):
        if self.path != "/control":
            self.reply(404, b"not found")
            return

        try:
            length = int(self.headers.get("Content-Length", "-1"))
        except ValueError:
            length = -1
        if length < 0 or length > MAX_BODY:
            self.reply(413, b"invalid body length")
            return
        body = self.rfile.read(length)
        action_header = self.headers.get("SOAPACTION", "")
        match = ACTION_RE.search(action_header)
        action = match.group(1) if match else ""

        if action == "SetAVTransportURI":
            key = "SetAVTransportURI"
            try:
                root = ET.fromstring(body)
                uri = element_text(root, "CurrentURI")
                if not uri or not uri.startswith(("http://", "https://")):
                    raise ValueError("missing finite media URL")
            except (ET.ParseError, ValueError):
                self.renderer.failure = "valid SetAVTransportURI was refused"
                self.renderer.statuses[key] = 400
                self.reply(400, b"bad SetAVTransportURI")
                return
            self.renderer.events.append(key)
            self.renderer.statuses[key] = 200
            self.reply(200, b"<SetAVTransportURIResponse/>", "text/xml")
            return

        if action == "Play":
            key = "Play"
            try:
                root = ET.fromstring(body)
                if element_text(root, "Speed") != "1":
                    raise ValueError("unsupported speed")
            except (ET.ParseError, ValueError):
                self.renderer.failure = "valid Play was refused"
                self.renderer.statuses[key] = 400
                self.reply(400, b"bad Play")
                return
            self.renderer.events.append(key)
            self.renderer.statuses[key] = 200
            self.reply(200, b"<PlayResponse/>", "text/xml")
            return

        if action == "Seek":
            try:
                root = ET.fromstring(body)
            except ET.ParseError:
                self.renderer.events.append("malformed_refused")
                self.renderer.statuses["malformed_refused"] = 400
                self.reply(400, b"malformed SOAP")
                return
            target = element_text(root, "Target")
            if target is None:
                self.renderer.events.append("malformed_refused")
                self.renderer.statuses["malformed_refused"] = 400
                self.reply(400, b"missing Target")
                return
            if target.strip().lower() in {"nan", "+nan", "-nan", "inf", "+inf", "-inf", "infinity", "+infinity", "-infinity"}:
                self.renderer.events.append("nonfinite_refused")
                self.renderer.statuses["nonfinite_refused"] = 400
                self.reply(400, b"non-finite Target refused")
                return
            match = TIME_RE.fullmatch(target.strip())
            if not match or int(match.group(2)) > 59 or int(match.group(3)) > 59:
                self.renderer.events.append("malformed_refused")
                self.renderer.statuses["malformed_refused"] = 400
                self.reply(400, b"malformed Target")
                return
            self.renderer.events.append("Seek")
            self.renderer.statuses["Seek"] = 200
            self.reply(200, b"<SeekResponse/>", "text/xml")
            return

        self.renderer.events.append("unknown_refused")
        self.renderer.statuses["unknown_refused"] = 400
        self.reply(400, b"unknown action")


def soap(action: str, inner: str) -> bytes:
    return (
        '<?xml version="1.0"?>'
        '<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">'
        f'<s:Body><u:{action} xmlns:u="{SOAP_NS}">{inner}</u:{action}>'
        "</s:Body></s:Envelope>"
    ).encode()


def request(port: int, method: str, path: str, body: bytes = b"", action: str | None = None):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=2.0)
    headers = {"Connection": "close"}
    if body:
        headers["Content-Type"] = 'text/xml; charset="utf-8"'
        headers["Content-Length"] = str(len(body))
    if action:
        headers["SOAPACTION"] = f'"{SOAP_NS}#{action}"'
    try:
        connection.request(method, path, body=body, headers=headers)
        reply = connection.getresponse()
        payload = reply.read(MAX_BODY + 1)
        return reply.status, payload
    finally:
        connection.close()


def require_status(actual: int, expected: int, label: str):
    if actual != expected:
        raise RuntimeError(f"{label}: HTTP {actual}, expected HTTP {expected}")


def loopback_path(url: str, port: int, label: str) -> str:
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1":
        raise RuntimeError(f"{label} was not an HTTP loopback URL")
    if parsed.port != port:
        raise RuntimeError(f"{label} used the wrong loopback port")
    return parsed.path or "/"


def ssdp_record(payload: bytes, source: tuple[str, int]) -> dict:
    first_line = payload.split(b"\r\n", 1)[0].decode("latin-1", "replace")
    return {
        "source": source[0],
        "bytes": len(payload),
        "first_line": first_line[:160],
        "http_response": first_line.startswith("HTTP/"),
    }


def dns_name(payload: bytes, offset: int) -> tuple[str, int]:
    labels: list[str] = []
    next_offset = offset
    jumped = False
    seen: set[int] = set()
    while True:
        if offset >= len(payload):
            raise ValueError("DNS name exceeds packet")
        length = payload[offset]
        if length == 0:
            if not jumped:
                next_offset = offset + 1
            return ".".join(labels).lower(), next_offset
        if length & 0xC0 == 0xC0:
            if offset + 1 >= len(payload):
                raise ValueError("truncated DNS compression pointer")
            pointer = ((length & 0x3F) << 8) | payload[offset + 1]
            if pointer in seen:
                raise ValueError("recursive DNS compression pointer")
            seen.add(pointer)
            if not jumped:
                next_offset = offset + 2
                jumped = True
            offset = pointer
            continue
        if length > 63 or offset + 1 + length > len(payload):
            raise ValueError("invalid DNS label")
        label = payload[offset + 1 : offset + 1 + length]
        labels.append(label.decode("idna", "replace"))
        offset += 1 + length


def mdns_resource_records(payload: bytes) -> list[dict]:
    if len(payload) < 12:
        raise ValueError("truncated DNS header")
    _, _, questions, answers, authority, additional = struct.unpack(
        "!HHHHHH", payload[:12]
    )
    offset = 12
    for _ in range(questions):
        _, offset = dns_name(payload, offset)
        if offset + 4 > len(payload):
            raise ValueError("truncated DNS question")
        offset += 4

    records: list[dict] = []
    for section, count in (
        ("answer", answers),
        ("authority", authority),
        ("additional", additional),
    ):
        for _ in range(min(count, MAX_RUNTIME_RECORDS - len(records))):
            name, offset = dns_name(payload, offset)
            if offset + 10 > len(payload):
                raise ValueError("truncated DNS resource record")
            rr_type, rr_class, _, data_length = struct.unpack(
                "!HHIH", payload[offset : offset + 10]
            )
            offset += 10
            data_end = offset + data_length
            if data_end > len(payload):
                raise ValueError("truncated DNS resource data")
            data: str | None = None
            if rr_type == 12:
                data, _ = dns_name(payload, offset)
            elif rr_type == 33 and data_length >= 6:
                _, _, port = struct.unpack("!HHH", payload[offset : offset + 6])
                target, _ = dns_name(payload, offset + 6)
                data = f"{target}:{port}"
            elif rr_type == 1 and data_length == 4:
                data = socket.inet_ntoa(payload[offset:data_end])
            records.append(
                {
                    "section": section,
                    "name": name,
                    "type": DNS_TYPE_NAMES.get(rr_type, str(rr_type)),
                    "class": rr_class & 0x7FFF,
                    "data": data,
                }
            )
            offset = data_end
    return records


def mdns_record(payload: bytes, source: tuple[str, int]) -> dict:
    if len(payload) < 12:
        return {"source": source[0], "bytes": len(payload), "malformed": True}
    _, flags, questions, answers, authority, additional = struct.unpack(
        "!HHHHHH", payload[:12]
    )
    record = {
        "source": source[0],
        "bytes": len(payload),
        "flags": f"0x{flags:04x}",
        "qr": bool(flags & 0x8000),
        "questions": questions,
        "answers": answers,
        "authority": authority,
        "additional": additional,
    }
    try:
        resource_records = mdns_resource_records(payload)
    except ValueError as exc:
        record["resource_records"] = []
        record["parse_error"] = str(exc)
    else:
        record["resource_records"] = resource_records
        record["googlecast_records"] = [
            item
            for item in resource_records
            if "googlecast" in (item["name"] or "")
            or "googlecast" in (item["data"] or "")
        ]
    return record


def bounded_multicast_probe(
    target: tuple[str, int], payload: bytes, seconds: int, protocol: str
) -> dict:
    packets = 0
    records: list[dict] = []
    errors: list[str] = []
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 1)
        sock.settimeout(0.25)
        listener_port = 0
        if protocol == "mdns":
            # mDNS answers are multicast to UDP/5353. Reuse the responder's
            # port and join the group so a real answer cannot be missed merely
            # because this diagnostic chose an ephemeral source port.
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            if hasattr(socket, "SO_REUSEPORT"):
                sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
            listener_port = MDNS_TARGET[1]
            sock.bind(("", listener_port))
            membership = socket.inet_aton(target[0]) + socket.inet_aton("0.0.0.0")
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, membership)
        else:
            sock.bind(("", 0))
        sock.sendto(payload, target)
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                packet, source = sock.recvfrom(8192)
            except socket.timeout:
                continue
            except OSError as exc:
                errors.append(str(exc))
                break
            packets += 1
            if len(records) < MAX_RUNTIME_RECORDS:
                if protocol == "ssdp":
                    records.append(ssdp_record(packet, source))
                else:
                    records.append(mdns_record(packet, source))
    except OSError as exc:
        errors.append(str(exc))
    finally:
        sock.close()
    return {
        "query_sent": not errors,
        "target": f"{target[0]}:{target[1]}",
        "listener_port": listener_port,
        "listen_seconds": seconds,
        "packets": packets,
        "records": records,
        "errors": errors,
    }


def mdns_googlecast_query() -> bytes:
    name = b"\x09_googlecast\x04_tcp\x05local\x00"
    return struct.pack("!HHHHHH", 0x4D43, 0, 1, 0, 0, 0) + name + struct.pack("!HH", 12, 1)


def run_runtime_probe(seconds: int) -> dict:
    ssdp = bounded_multicast_probe(
        SSDP_TARGET,
        (
            b"M-SEARCH * HTTP/1.1\r\n"
            b"HOST: 239.255.255.250:1900\r\n"
            b'MAN: "ssdp:discover"\r\n'
            b"MX: 1\r\n"
            b"ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n"
        ),
        seconds,
        "ssdp",
    )
    mdns = bounded_multicast_probe(MDNS_TARGET, mdns_googlecast_query(), seconds, "mdns")
    errors = ssdp["errors"] + mdns["errors"]
    mdns["googlecast_records"] = [
        item
        for packet in mdns["records"]
        for item in packet.get("googlecast_records", [])
    ]
    return {
        "status": "completed" if not errors else "inconclusive",
        "evidence_class": "read_only_multicast_discovery",
        "claims": {
            "physical_dlna_control": "not_proven",
            "chromecast_castv2_control": "not_proven",
            "mesh_owner": "not_proven",
            "seat_handoff": "not_proven",
        },
        "safety": {
            "mutations": [],
            "ssdp_query_count": 1,
            "mdns_query_count": 1,
            "control_requests": 0,
        },
        "ssdp_media_renderer": ssdp,
        "mdns_googlecast": mdns,
        "errors": errors,
    }


def av_transport_control_path(description: bytes, port: int) -> str:
    try:
        root = ET.fromstring(description)
    except ET.ParseError as exc:
        raise RuntimeError(f"description XML is malformed: {exc}") from exc
    for service in root.iter():
        if local_name(service.tag) != "service":
            continue
        service_type = element_text(service, "serviceType")
        control_url = element_text(service, "controlURL")
        if service_type != SOAP_NS or not control_url:
            continue
        if control_url.startswith("http://"):
            return loopback_path(control_url, port, "AVTransport controlURL")
        if control_url.startswith("/"):
            return control_url
        return "/" + control_url
    raise RuntimeError("description omitted AVTransport controlURL")


def run_exchange() -> dict:
    renderer = Renderer(("127.0.0.1", 0))
    thread = threading.Thread(target=renderer.serve_forever, name="loopback-renderer")
    thread.daemon = True
    thread.start()
    port = renderer.server_port
    try:
        status, payload = request(port, "GET", "/discover")
        require_status(status, 200, "discovery")
        document = json.loads(payload)
        records = document.get("renderers")
        if not isinstance(records, list) or len(records) != 1:
            raise RuntimeError("discovery did not return exactly one renderer")
        location = records[0].get("location")
        if not isinstance(location, str):
            raise RuntimeError("discovery returned no description URL")
        description_path = loopback_path(location, port, "description URL")

        status, description = request(port, "GET", description_path)
        require_status(status, 200, "description")
        if b"AVTransport" not in description or b"/control" not in description:
            raise RuntimeError("description omitted AVTransport control URL")
        control_path = av_transport_control_path(description, port)

        set_body = soap(
            "SetAVTransportURI",
            f"<InstanceID>0</InstanceID><CurrentURI>{MEDIA_URL}</CurrentURI>"
            "<CurrentURIMetaData>Loopback &amp; proof</CurrentURIMetaData>",
        )
        status, _ = request(port, "POST", control_path, set_body, "SetAVTransportURI")
        require_status(status, 200, "SetAVTransportURI")

        status, _ = request(
            port,
            "POST",
            control_path,
            soap("Play", "<InstanceID>0</InstanceID><Speed>1</Speed>"),
            "Play",
        )
        require_status(status, 200, "Play")

        status, _ = request(
            port,
            "POST",
            control_path,
            b"<not-xml",
            "Seek",
        )
        require_status(status, 400, "malformed Seek refusal")

        status, _ = request(
            port,
            "POST",
            control_path,
            soap("Seek", "<InstanceID>0</InstanceID><Unit>REL_TIME</Unit><Target>NaN</Target>"),
            "Seek",
        )
        require_status(status, 400, "non-finite Seek refusal")

        status, _ = request(
            port,
            "POST",
            control_path,
            soap("Seek", "<InstanceID>0</InstanceID><Unit>REL_TIME</Unit><Target>00:00:07</Target>"),
            "Seek",
        )
        require_status(status, 200, "Seek")

        expected = [
            "discovery",
            "description",
            "SetAVTransportURI",
            "Play",
            "malformed_refused",
            "nonfinite_refused",
            "Seek",
        ]
        if renderer.events != expected:
            raise RuntimeError(f"renderer event order was {renderer.events!r}, expected {expected!r}")
        return {
            "status": "passed",
            "evidence_class": "loopback_http_renderer_exchange",
            "claims": {
                "loopback_only": True,
                "physical_dlna": "not_proven",
                "chromecast": "not_proven",
                "mesh_owner": "not_proven",
                "seat_handoff": "not_proven",
            },
            "exchange": {
                "bind": "127.0.0.1",
                "discovery": "GET /discover",
                "description": "GET /description.xml",
                "control": "POST /control",
                "actions": ["SetAVTransportURI", "Play", "Seek"],
                "refusals": ["malformed Seek", "non-finite Seek"],
                "events": renderer.events,
                "statuses": renderer.statuses,
            },
        }
    finally:
        renderer.shutdown()
        renderer.server_close()
        try:
            probe = socket.create_connection(("127.0.0.1", port), timeout=0.2)
        except OSError:
            listener_closed = True
        else:
            probe.close()
            listener_closed = False
        thread.join(timeout=2.0)
        if not listener_closed or thread.is_alive():
            raise RuntimeError("renderer listener or server thread survived cleanup")


def main() -> int:
    mode = sys.argv[1] if len(sys.argv) >= 2 else "run"
    if mode not in {"run", "self-test", "runtime-probe"}:
        raise RuntimeError("invalid mode")
    if mode == "runtime-probe":
        result = run_runtime_probe(int(sys.argv[2])) if len(sys.argv) == 3 else None
        if result is None:
            raise RuntimeError("runtime probe duration was not provided")
        result["cleanup"] = {"sockets_closed": True}
        print(json.dumps(result, sort_keys=True))
        if result["status"] != "completed":
            raise RuntimeError("runtime probe could not complete both multicast queries")
        return 0
    result = run_exchange()
    result["cleanup"] = {"listener_closed": True, "server_thread_stopped": True}
    if mode == "self-test":
        exchange = result["exchange"]
        if exchange["statuses"].get("malformed_refused") != 400:
            raise RuntimeError("self-test did not observe malformed refusal")
        if exchange["statuses"].get("nonfinite_refused") != 400:
            raise RuntimeError("self-test did not observe non-finite refusal")
        synthetic_ssdp = ssdp_record(b"HTTP/1.1 200 OK\r\n\r\n", ("127.0.0.1", 1900))
        if not synthetic_ssdp["http_response"]:
            raise RuntimeError("self-test did not recognize SSDP response framing")
        synthetic_mdns = mdns_record(
            struct.pack("!HHHHHH", 0x4D43, 0x8400, 1, 1, 0, 0),
            ("127.0.0.1", 5353),
        )
        if not synthetic_mdns["qr"] or synthetic_mdns["answers"] != 1:
            raise RuntimeError("self-test did not recognize mDNS answer framing")
        print("verify-music-cast-loopback: self-test passed")
    else:
        print(json.dumps(result, sort_keys=True))
    return 0


try:
    raise SystemExit(main())
except Exception as exc:
    print(f"verify-music-cast-loopback: {exc}", file=sys.stderr)
    raise SystemExit(1)
PY
)" || {
    rc=$?
    fail "bounded renderer exchange failed (exit $rc)"
}

printf '%s\n' "$result"
