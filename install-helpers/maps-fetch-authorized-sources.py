#!/usr/bin/env python3
"""Fetch only the operator-locked Maps source bytes (Geofabrik PBF + TIGER zip).

This helper never fetches public OSM tile CDNs and never marks
production_admitted. Bytes land at an operator-supplied destination with
no-replace mode-0400. The post-fetch sha256 sidecar is not a production
Maps MBTiles receipt.

The GET seam is injectable so tests never hit the network. The default
HTTPS getter streams after URL admission only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import sys
import tempfile
from collections.abc import Callable, Iterable
from pathlib import Path, PurePosixPath
from typing import Union
from urllib.error import URLError
from urllib.parse import urlparse
from urllib.request import Request, urlopen

EXIT_REFUSED = 2
SOURCES_KIND = "mcnf-maps-authorized-sources"
FETCH_KIND = "mcnf-maps-authorized-source-fetch"
PRODUCTION_RECEIPT_KIND = "mcnf-maps-mbtiles-receipt"
LOCKED_SOURCE_IDS = ("pbf", "geometry")
REGION_ID = "buffalo-niagara"
OPERATOR_AUTHORIZATION = "2026-08-22-survey"
MAX_SOURCES_BYTES = 64 * 1024
MAX_SIDECAR_BYTES = 16 * 1024
STREAM_CHUNK = 1024 * 1024
GET_TIMEOUT_SECONDS = 60
TILE_CDN_MARKERS = (
    "tile.openstreetmap.org",
    "tiles.openstreetmap.org",
    "tile.osm.org",
    "mapbox.com",
    "googleapis.com",
    "google.com/maps",
    "hereapi.com",
    "arcgisonline.com",
)
SIDECAR_KEYS = {
    "schema_version",
    "kind",
    "source_id",
    "url",
    "upstream",
    "license",
    "region_id",
    "operator_authorization",
    "sha256",
    "bytes",
    "destination",
    "production_admitted",
}

GetFn = Callable[[str], Union[bytes, Iterable[bytes]]]


class Refusal(ValueError):
    pass


def canonical(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def exact_keys(value: object, expected: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != expected:
        raise Refusal(f"{label} fields are not exact")
    return value


def real_directory(path: Path, label: str) -> Path:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise Refusal(f"{label} is missing or inaccessible") from error
    if stat.S_ISLNK(metadata.st_mode):
        raise Refusal(f"path substitution refused: {label} is a symlink")
    if not stat.S_ISDIR(metadata.st_mode):
        raise Refusal(f"{label} must be a real directory")
    return path


def relative_leaf(value: str, label: str) -> PurePosixPath:
    path = PurePosixPath(value)
    if (
        not value
        or path.is_absolute()
        or "\\" in value
        or any(part in ("", ".", "..") for part in path.parts)
    ):
        raise Refusal(f"path substitution refused: {label} is not a safe relative path")
    return path


def resolve_beneath(root: Path, relative: PurePosixPath, label: str) -> Path:
    real_directory(root, f"{label} dest-root")
    current = root
    for component in relative.parts[:-1]:
        current /= component
        real_directory(current, f"{label} parent")
    candidate = current / relative.parts[-1]
    try:
        candidate.relative_to(root)
    except ValueError as error:
        raise Refusal(f"path substitution refused: {label} escapes dest-root") from error
    if candidate.exists() or candidate.is_symlink():
        try:
            if candidate.lstat() and stat.S_ISLNK(candidate.lstat().st_mode):
                raise Refusal(f"path substitution refused: {label} is a symlink")
        except FileNotFoundError:
            pass
    return candidate


def immutable_json(path: Path, maximum: int, label: str) -> dict:
    try:
        before = path.lstat()
    except OSError as error:
        raise Refusal(f"{label} is missing or inaccessible") from error
    if stat.S_ISLNK(before.st_mode):
        raise Refusal(f"path substitution refused: {label} is a symlink")
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise Refusal(f"{label} must be a singly-linked regular file")
    if before.st_size <= 0 or before.st_size > maximum:
        raise Refusal(f"{label} size is outside its bound")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        data = b""
        while len(data) <= maximum:
            chunk = os.read(descriptor, min(65536, maximum + 1 - len(data)))
            if not chunk:
                break
            data += chunk
    finally:
        os.close(descriptor)
    try:
        value = json.loads(data)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise Refusal(f"{label} is not valid JSON: {error}") from error
    if not isinstance(value, dict):
        raise Refusal(f"{label} root must be an object")
    return value


def canonical_https_url(url: str, label: str) -> str:
    if not isinstance(url, str) or not url or len(url) > 2048:
        raise Refusal(f"{label} is missing or oversized")
    parsed = urlparse(url)
    if parsed.scheme != "https":
        raise Refusal(f"wrong URL refused: {label} must be https")
    host = (parsed.hostname or "").lower()
    if not host or parsed.username or parsed.password or parsed.port:
        raise Refusal(f"wrong URL refused: {label} host is not a locked https host")
    if parsed.params or parsed.query or parsed.fragment:
        raise Refusal(f"wrong URL refused: {label} carries query, params, or fragment")
    if not parsed.path or "\\" in parsed.path or any(part == ".." for part in parsed.path.split("/")):
        raise Refusal(f"path substitution refused: {label} path is unsafe")
    return f"https://{host}{parsed.path}"


def refuse_tile_cdn(url: str) -> None:
    lowered = url.lower()
    if any(marker in lowered for marker in TILE_CDN_MARKERS):
        raise Refusal("public OSM tile CDN refused")


def refuse_never_fetch(url: str, prefixes: object) -> None:
    if not isinstance(prefixes, list):
        raise Refusal("authorized sources never_fetch list is malformed")
    for prefix in prefixes:
        if not isinstance(prefix, str) or not prefix:
            raise Refusal("authorized sources never_fetch entry is malformed")
        locked_prefix = prefix.rstrip("/")
        if url == locked_prefix or url.startswith(locked_prefix + "/"):
            raise Refusal("public OSM tile CDN refused")


def load_authorized_sources(path: Path) -> dict[str, object]:
    sources = immutable_json(path, MAX_SOURCES_BYTES, "authorized sources")
    if sources.get("kind") != SOURCES_KIND or sources.get("schema_version") != 1:
        raise Refusal("authorized sources kind or schema is unsupported")
    if sources.get("region_id") != REGION_ID:
        raise Refusal("path substitution refused: region is not buffalo-niagara")
    if sources.get("operator_authorization") != OPERATOR_AUTHORIZATION:
        raise Refusal("authorized sources operator authorization is not the 2026-08-22 lock")
    if sources.get("license") != "ODbL-1.0":
        raise Refusal("authorized sources license must be ODbL-1.0")
    if not isinstance(sources.get("never_fetch"), list) or not sources["never_fetch"]:
        raise Refusal("authorized sources never_fetch list is missing")
    for prefix in sources["never_fetch"]:
        if not isinstance(prefix, str) or not prefix:
            raise Refusal("authorized sources never_fetch entry is malformed")
    for source_id in LOCKED_SOURCE_IDS:
        entry = sources.get(source_id)
        if not isinstance(entry, dict) or not isinstance(entry.get("url"), str):
            raise Refusal(f"authorized sources {source_id} URL is missing")
        refuse_tile_cdn(entry["url"])
        canonical_https_url(entry["url"], f"locked {source_id} URL")
    return sources


def locked_url(sources: dict[str, object], source_id: str) -> tuple[str, dict[str, object]]:
    if source_id not in LOCKED_SOURCE_IDS:
        raise Refusal("source id is not a locked Maps source")
    entry = sources[source_id]
    if not isinstance(entry, dict):
        raise Refusal(f"authorized sources {source_id} entry is malformed")
    return canonical_https_url(str(entry["url"]), f"locked {source_id} URL"), entry


def admit_url(sources: dict[str, object], source_id: str, requested: str | None) -> tuple[str, dict[str, object]]:
    locked, entry = locked_url(sources, source_id)
    url = locked if requested is None else canonical_https_url(requested, "requested URL")
    refuse_tile_cdn(url)
    refuse_never_fetch(url, sources.get("never_fetch"))
    if url != locked:
        raise Refusal("wrong URL refused: requested URL is not the locked authorized source")
    return url, entry


def chunks_from_get(body: bytes | Iterable[bytes]) -> Iterable[bytes]:
    if isinstance(body, (bytes, bytearray)):
        if not body:
            raise Refusal("injected GET returned no bytes")
        yield bytes(body)
        return
    yielded = False
    for chunk in body:
        if not isinstance(chunk, (bytes, bytearray)) or not chunk:
            raise Refusal("injected GET yielded an empty or non-byte chunk")
        yielded = True
        yield bytes(chunk)
    if not yielded:
        raise Refusal("injected GET returned no bytes")


def default_https_get(url: str) -> Iterable[bytes]:
    request = Request(url, method="GET", headers={"User-Agent": "mcnf-maps-authorized-source-fetch/1"})
    try:
        with urlopen(request, timeout=GET_TIMEOUT_SECONDS) as response:
            status = getattr(response, "status", 200)
            if status != 200:
                raise Refusal(f"authorized source GET refused HTTP {status}")
            while True:
                chunk = response.read(STREAM_CHUNK)
                if not chunk:
                    break
                yield chunk
    except Refusal:
        raise
    except (URLError, TimeoutError, OSError) as error:
        raise Refusal(f"authorized source GET failed: {error}") from error


def atomic_write_stream(
    path: Path,
    chunks: Iterable[bytes],
    *,
    label: str,
) -> tuple[str, int]:
    if path.exists() or path.is_symlink():
        raise Refusal(f"{label} already exists; publication is no-replace")
    parent = path.parent
    real_directory(parent, f"{label} parent")
    parent = parent.resolve(strict=True)
    fd, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=parent)
    temporary = Path(name)
    hasher = hashlib.sha256()
    size = 0
    try:
        os.fchmod(fd, 0o400)
        for chunk in chunks:
            hasher.update(chunk)
            view = memoryview(chunk)
            while view:
                written = os.write(fd, view)
                if written <= 0:
                    raise Refusal(f"{label} write made no progress")
                view = view[written:]
            size += len(chunk)
        if size <= 0:
            raise Refusal(f"{label} fetch produced no bytes")
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
        raise Refusal(f"{label} appeared during publication: {path}") from error
    finally:
        if fd >= 0:
            os.close(fd)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass
    return hasher.hexdigest(), size


def atomic_write_bytes(path: Path, body: bytes, *, label: str) -> None:
    atomic_write_stream(path, (body,), label=label)


def bind_sidecar(
    *,
    source_id: str,
    url: str,
    entry: dict[str, object],
    destination: str,
    sha256: str,
    size: int,
    default_license: object,
) -> dict[str, object]:
    upstream = entry.get("upstream")
    license_value = entry.get("license", default_license)
    if not isinstance(upstream, str) or not upstream:
        raise Refusal(f"authorized sources {source_id} upstream is missing")
    if not isinstance(license_value, str) or not license_value:
        raise Refusal(f"authorized sources {source_id} license is missing")
    sidecar = {
        "schema_version": 1,
        "kind": FETCH_KIND,
        "source_id": source_id,
        "url": url,
        "upstream": upstream,
        "license": license_value,
        "region_id": REGION_ID,
        "operator_authorization": OPERATOR_AUTHORIZATION,
        "sha256": sha256,
        "bytes": size,
        "destination": destination,
        # Fetched upstream bytes are not a production Maps MBTiles receipt
        # and never close the production Maps gate.
        "production_admitted": False,
    }
    if sidecar["kind"] == PRODUCTION_RECEIPT_KIND:
        raise Refusal("fetch sidecar must not be a production Maps receipt")
    if sidecar["production_admitted"] is not False:
        raise Refusal("fetch sidecar must never mark production_admitted")
    exact_keys(sidecar, SIDECAR_KEYS, "fetch sidecar")
    return sidecar


def fetch_authorized_source(
    *,
    sources_path: Path,
    source_id: str,
    dest_root: Path,
    destination: str,
    sidecar: str,
    url: str | None = None,
    get: GetFn | None = None,
) -> dict[str, object]:
    sources = load_authorized_sources(sources_path)
    admitted, entry = admit_url(sources, source_id, url)
    dest_rel = relative_leaf(destination, "destination")
    sidecar_rel = relative_leaf(sidecar, "sidecar")
    dest_path = resolve_beneath(dest_root, dest_rel, "destination")
    sidecar_path = resolve_beneath(dest_root, sidecar_rel, "sidecar")
    if dest_path == sidecar_path:
        raise Refusal("path substitution refused: destination and sidecar are the same path")
    if dest_path.exists() or dest_path.is_symlink():
        raise Refusal("destination already exists; publication is no-replace")
    if sidecar_path.exists() or sidecar_path.is_symlink():
        raise Refusal("sidecar already exists; publication is no-replace")
    getter = get if get is not None else default_https_get
    sha256, size = atomic_write_stream(dest_path, chunks_from_get(getter(admitted)), label="destination")
    record = bind_sidecar(
        source_id=source_id,
        url=admitted,
        entry=entry,
        destination=str(dest_rel),
        sha256=sha256,
        size=size,
        default_license=sources.get("license"),
    )
    body = canonical(record)
    if len(body) > MAX_SIDECAR_BYTES:
        raise Refusal("fetch sidecar exceeds its bound")
    atomic_write_bytes(sidecar_path, body, label="sidecar")
    return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sources",
        type=Path,
        default=Path(__file__).resolve().with_name("maps-authorized-sources.json"),
    )
    parser.add_argument("--source", required=True, choices=LOCKED_SOURCE_IDS)
    parser.add_argument("--url", default=None, help="must match the locked URL for --source")
    parser.add_argument("--dest-root", type=Path, required=True)
    parser.add_argument("--destination", required=True)
    parser.add_argument("--sidecar", required=True)
    args = parser.parse_args()
    try:
        value = fetch_authorized_source(
            sources_path=args.sources,
            source_id=args.source,
            dest_root=args.dest_root,
            destination=args.destination,
            sidecar=args.sidecar,
            url=args.url,
        )
    except (Refusal, OSError, UnicodeError, ValueError) as error:
        print(f"maps-fetch-authorized-sources: refusal: {error}", file=sys.stderr)
        return EXIT_REFUSED
    print(canonical(value).decode("ascii"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
