#!/usr/bin/env python3
"""Hostile tests for TIGER Erie+Niagara clip-geometry extract.

Fixtures build the smallest valid DBF/SHP/SHX zip the helper can parse.
Tests never download the Census zip or any public OSM tile CDN. Extract
sidecars are not production Maps receipts and never mark production_admitted.
"""

from __future__ import annotations

import importlib.util
import io
import json
import os
import stat
import struct
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path
from urllib.request import urlopen

HERE = Path(__file__).resolve().parent
EXTRACTOR = HERE / "maps-extract-tiger-clip.py"
LOCK = HERE / "maps-authorized-sources.json"
TIGER_URL = "https://www2.census.gov/geo/tiger/TIGER2024/COUNTY/tl_2024_us_county.zip"
TILE_CDN = "https://tile.openstreetmap.org/0/0/0.png"
TILE_CDN_ALT = "https://tiles.openstreetmap.org/1/0/0.png"
TILE_OSM = "https://tile.osm.org/0/0/0.png"

COUNTIES = (
    ("36029", "Erie", "Erie County", ((-79.12, 42.70), (-78.50, 42.70), (-78.80, 43.00))),
    ("36063", "Niagara", "Niagara County", ((-79.10, 43.00), (-78.80, 43.00), (-78.95, 43.30))),
    ("36001", "Albany", "Albany County", ((-74.10, 42.60), (-73.70, 42.60), (-73.85, 42.85))),
)


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


extract = load("maps_extract_tiger_clip", EXTRACTOR)


def network_get(url: str, *args, **kwargs):
    raise AssertionError(f"test must never download: {url}")


def expect_refusal(label: str, call, needle: str) -> None:
    try:
        call()
    except extract.Refusal as error:
        text = str(error).lower()
        if needle not in text:
            raise AssertionError(f"{label} refusal message drifted: {error}") from error
        return
    raise AssertionError(f"hostile case was accepted: {label}")


def write_lock(path: Path, document: dict) -> Path:
    path.write_text(json.dumps(document, indent=2) + "\n")
    path.chmod(0o444)
    return path


def base_lock() -> dict:
    return json.loads(LOCK.read_text())


def _closed_ring(points: tuple[tuple[float, float], ...]) -> list[tuple[float, float]]:
    ring = list(points)
    ring.append(points[0])
    return ring


def _bbox(points: list[tuple[float, float]]) -> tuple[float, float, float, float]:
    xs = [point[0] for point in points]
    ys = [point[1] for point in points]
    return min(xs), min(ys), max(xs), max(ys)


def encode_dbf(rows: list[tuple[str, str, str]]) -> bytes:
    fields = (("GEOID", "C", 5), ("NAME", "C", 16), ("NAMELSAD", "C", 24))
    rec_len = 1 + sum(length for _name, _typ, length in fields)
    header_len = 32 + 32 * len(fields) + 1
    header = bytearray(32)
    header[0] = 0x03
    struct.pack_into("<IHH", header, 4, len(rows), header_len, rec_len)
    body = bytearray(header)
    for name, typ, length in fields:
        desc = bytearray(32)
        raw = name.encode("ascii")
        desc[0 : len(raw)] = raw
        desc[11] = ord(typ)
        desc[16] = length
        body.extend(desc)
    body.append(0x0D)
    for geoid, name, namelsad in rows:
        record = bytearray(b" ")
        record.extend(geoid.encode("ascii").ljust(5))
        record.extend(name.encode("ascii").ljust(16))
        record.extend(namelsad.encode("ascii").ljust(24))
        if len(record) != rec_len:
            raise AssertionError("fixture DBF record length drifted")
        body.extend(record)
    body.append(0x1A)
    return bytes(body)


def encode_polygon_content(points: list[tuple[float, float]]) -> bytes:
    xmin, ymin, xmax, ymax = _bbox(points)
    content = bytearray()
    content.extend(struct.pack("<i", extract.SHAPE_POLYGON))
    content.extend(struct.pack("<4d", xmin, ymin, xmax, ymax))
    content.extend(struct.pack("<ii", 1, len(points)))
    content.extend(struct.pack("<i", 0))
    for x, y in points:
        content.extend(struct.pack("<2d", x, y))
    return bytes(content)


def encode_shp_shx(contents: list[bytes]) -> tuple[bytes, bytes]:
    records = bytearray()
    index = bytearray()
    cursor = 100
    file_bbox = [None, None, None, None]
    for number, content in enumerate(contents, start=1):
        if len(content) % 2:
            raise AssertionError("fixture SHP content is not word-aligned")
        xmin, ymin, xmax, ymax = struct.unpack_from("<4d", content, 4)
        if file_bbox[0] is None:
            file_bbox = [xmin, ymin, xmax, ymax]
        else:
            file_bbox[0] = min(file_bbox[0], xmin)
            file_bbox[1] = min(file_bbox[1], ymin)
            file_bbox[2] = max(file_bbox[2], xmax)
            file_bbox[3] = max(file_bbox[3], ymax)
        header = struct.pack(">ii", number, len(content) // 2)
        records.extend(header)
        records.extend(content)
        index.extend(struct.pack(">ii", cursor // 2, len(content) // 2))
        cursor += 8 + len(content)

    def file_header(length: int) -> bytes:
        header = bytearray(100)
        struct.pack_into(">i", header, 0, extract.SHP_FILE_CODE)
        struct.pack_into(">i", header, 24, length // 2)
        struct.pack_into("<i", header, 28, extract.SHP_VERSION)
        struct.pack_into("<i", header, 32, extract.SHAPE_POLYGON)
        struct.pack_into("<4d", header, 36, *file_bbox)
        return bytes(header)

    shp = file_header(100 + len(records)) + bytes(records)
    shx = file_header(100 + len(index)) + bytes(index)
    return shp, shx


def make_tiger_shapefile_zip(*geoids: str, include_shx: bool = True) -> bytes:
    selected = []
    for geoid in geoids:
        match = next((row for row in COUNTIES if row[0] == geoid), None)
        if match is None:
            raise AssertionError(f"fixture has no county {geoid}")
        selected.append(match)
    dbf = encode_dbf([(geoid, name, namelsad) for geoid, name, namelsad, _pts in selected])
    contents = [encode_polygon_content(_closed_ring(points)) for _geoid, _name, _namelsad, points in selected]
    shp, shx = encode_shp_shx(contents)
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, mode="w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("tl_2024_us_county.dbf", dbf)
        archive.writestr("tl_2024_us_county.shp", shp)
        if include_shx:
            archive.writestr("tl_2024_us_county.shx", shx)
        archive.writestr("tl_2024_us_county.prj", b"GEOGCS[\"NAD83\"]\n")
    return buffer.getvalue()


def write_sources(root: Path, geometry: bytes) -> Path:
    source_root = root / "source"
    source_root.mkdir(parents=True)
    (source_root / "tl_2024_us_county.zip").write_bytes(geometry)
    return source_root


def extract_ok(*, source_root: Path, dest_root: Path, **overrides):
    args = {
        "sources_path": LOCK,
        "source_root": source_root,
        "geometry": "tl_2024_us_county.zip",
        "dest_root": dest_root,
        "destination": "erie-niagara.geojson",
        "sidecar": "erie-niagara.geojson.sha256.json",
    }
    args.update(overrides)
    return extract.extract_tiger_clip(**args)


def feature_geoids(path: Path) -> list[str]:
    document = json.loads(path.read_bytes())
    assert document["type"] == "FeatureCollection"
    assert document["bbox"]
    return [feature["properties"]["GEOID"] for feature in document["features"]]


def main() -> None:
    original_urlopen = urlopen
    try:
        import urllib.request

        urllib.request.urlopen = network_get  # type: ignore[assignment]
        extract.fetch.default_https_get = network_get  # type: ignore[method-assign]

        lock = base_lock()
        assert lock["kind"] == "mcnf-maps-authorized-sources"
        assert lock["geometry"]["url"] == TIGER_URL
        assert lock["geometry"]["select_geoid"] == ["36029", "36063"]
        assert "tile.openstreetmap.org" in "".join(lock["never_fetch"])

        tiger_zip = make_tiger_shapefile_zip("36029", "36063", "36001")
        collection = extract.extract_clip_collection(tiger_zip)
        assert [feature["properties"]["GEOID"] for feature in collection["features"]] == [
            "36029",
            "36063",
        ]
        assert [feature["properties"]["NAME"] for feature in collection["features"]] == [
            "Erie County",
            "Niagara County",
        ]
        assert collection["type"] == "FeatureCollection"
        assert len(collection["features"]) == 2
        assert len(collection["bbox"]) == 4

        missing_niagara = make_tiger_shapefile_zip("36029", "36001")
        expect_refusal(
            "zip-missing-niagara",
            lambda: extract.extract_clip_collection(missing_niagara),
            "36063",
        )

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            source_root = write_sources(root, tiger_zip)
            dest_root = root / "dest"
            dest_root.mkdir()

            record = extract_ok(source_root=source_root, dest_root=dest_root)
            geojson = dest_root / "erie-niagara.geojson"
            sidecar = dest_root / "erie-niagara.geojson.sha256.json"
            assert geojson.name == "erie-niagara.geojson"
            assert stat.S_IMODE(geojson.stat().st_mode) == 0o400
            assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
            assert record["kind"] == "mcnf-maps-tiger-clip"
            assert record["kind"] != "mcnf-maps-mbtiles-receipt"
            assert record["production_admitted"] is False
            assert record["region_id"] == "buffalo-niagara"
            assert record["provider"] == "openstreetmap-derived"
            assert record["clip_geoids"] == ["36029", "36063"]
            assert record["clip_names"] == ["Erie County", "Niagara County"]
            assert record["geometry_url"] == TIGER_URL
            assert record["geometry_sha256"] == extract.digest(tiger_zip)
            assert record["feature_count"] == 2
            assert record["destination"] == "erie-niagara.geojson"
            stored = json.loads(sidecar.read_bytes())
            assert stored == record
            assert stored["production_admitted"] is False
            assert stored["kind"] != "mcnf-maps-mbtiles-receipt"
            assert feature_geoids(geojson) == ["36029", "36063"]
            parsed = json.loads(geojson.read_bytes())
            assert parsed["type"] == "FeatureCollection"
            assert {feature["properties"]["GEOID"] for feature in parsed["features"]} == {
                "36029",
                "36063",
            }
            assert "36001" not in json.dumps(parsed)

            no_shx = make_tiger_shapefile_zip("36029", "36063", "36001", include_shx=False)
            no_shx_src = write_sources(root / "no-shx-src", no_shx)
            no_shx_dest = root / "no-shx-dest"
            no_shx_dest.mkdir()
            no_shx_record = extract_ok(
                source_root=no_shx_src,
                dest_root=no_shx_dest,
                sidecar="no-shx.json",
            )
            assert feature_geoids(no_shx_dest / "erie-niagara.geojson") == ["36029", "36063"]
            assert no_shx_record["production_admitted"] is False

            expect_refusal(
                "zip-missing-niagara-extract",
                lambda: extract_ok(
                    source_root=write_sources(root / "miss-src", missing_niagara),
                    dest_root=(root / "miss-dest").mkdir() or (root / "miss-dest"),
                    sidecar="miss.json",
                ),
                "36063",
            )

            expect_refusal(
                "overwrite-dest",
                lambda: extract_ok(source_root=source_root, dest_root=dest_root),
                "no-replace",
            )
            assert geojson.read_bytes()
            (dest_root / "second").mkdir()
            expect_refusal(
                "overwrite-sidecar",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=dest_root,
                    destination="second/erie-niagara.geojson",
                    sidecar="erie-niagara.geojson.sha256.json",
                ),
                "no-replace",
            )
            assert not (dest_root / "second" / "erie-niagara.geojson").exists()

            expect_refusal(
                "wrong-filename",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=(root / "name-dest").mkdir() or (root / "name-dest"),
                    destination="albany.geojson",
                    sidecar="albany.json",
                ),
                "path substitution",
            )

            linked_root = root / "linked-root"
            linked_root.symlink_to(dest_root, target_is_directory=True)
            expect_refusal(
                "symlink-dest-root",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=linked_root,
                    destination="via-symlink/erie-niagara.geojson",
                    sidecar="via-symlink/extract.json",
                ),
                "path substitution",
            )
            linked_source = root / "linked-source"
            linked_source.symlink_to(source_root, target_is_directory=True)
            expect_refusal(
                "symlink-source-root",
                lambda: extract_ok(
                    source_root=linked_source,
                    dest_root=(root / "sym-src-dest").mkdir() or (root / "sym-src-dest"),
                    sidecar="sym-src.json",
                ),
                "path substitution",
            )

            expect_refusal(
                "tile-cdn-url",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=(root / "cdn-dest").mkdir() or (root / "cdn-dest"),
                    sidecar="cdn.json",
                    url=TILE_CDN,
                ),
                "tile",
            )
            expect_refusal(
                "tile-cdn-alt",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=(root / "cdn-alt-dest").mkdir() or (root / "cdn-alt-dest"),
                    sidecar="cdn-alt.json",
                    url=TILE_CDN_ALT,
                ),
                "tile",
            )
            expect_refusal(
                "tile-osm",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=(root / "cdn-osm-dest").mkdir() or (root / "cdn-osm-dest"),
                    sidecar="cdn-osm.json",
                    url=TILE_OSM,
                ),
                "tile",
            )

            tiles_lock = base_lock()
            tiles_lock["tiles"] = {"url": TILE_CDN, "upstream": "osm-public-tiles"}
            expect_refusal(
                "tiles-source-kind",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=(root / "tiles-dest").mkdir() or (root / "tiles-dest"),
                    sources_path=write_lock(root / "tiles-lock.json", tiles_lock),
                    sidecar="tiles.json",
                ),
                "source",
            )

            locked_clip = base_lock()
            locked_clip["geometry"]["select_geoid"] = ["36061"]
            expect_refusal(
                "lock-clip-drift",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=(root / "lock-clip-dest").mkdir() or (root / "lock-clip-dest"),
                    sources_path=write_lock(root / "lock-clip.json", locked_clip),
                    sidecar="lock-clip.json",
                ),
                "clip",
            )

            env = os.environ.copy()
            env["PYTHONPATH"] = ""
            cli_dest = root / "cli-dest"
            cli_dest.mkdir()
            cli = subprocess.run(
                [
                    sys.executable,
                    str(EXTRACTOR),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--geometry",
                    "tl_2024_us_county.zip",
                    "--dest-root",
                    str(cli_dest),
                    "--destination",
                    "erie-niagara.geojson",
                    "--sidecar",
                    "cli.json",
                ],
                check=False,
                capture_output=True,
                text=True,
                env=env,
            )
            if cli.returncode != 0:
                raise AssertionError(f"cli extract failed: {cli.stderr}")
            cli_record = json.loads(cli.stdout)
            assert cli_record["clip_geoids"] == ["36029", "36063"]
            assert cli_record["kind"] == "mcnf-maps-tiger-clip"
            assert cli_record["kind"] != "mcnf-maps-mbtiles-receipt"
            assert cli_record["production_admitted"] is False
            assert feature_geoids(cli_dest / "erie-niagara.geojson") == ["36029", "36063"]

            refused = subprocess.run(
                [
                    sys.executable,
                    str(EXTRACTOR),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--geometry",
                    "tl_2024_us_county.zip",
                    "--dest-root",
                    str((root / "cli-cdn").mkdir() or (root / "cli-cdn")),
                    "--destination",
                    "erie-niagara.geojson",
                    "--sidecar",
                    "cli-cdn.json",
                    "--url",
                    TILE_CDN,
                ],
                check=False,
                capture_output=True,
                text=True,
                env=env,
            )
            if refused.returncode != extract.EXIT_REFUSED:
                raise AssertionError(f"cli tile CDN was not refused: {refused.stdout} {refused.stderr}")
            if "tile" not in refused.stderr.lower():
                raise AssertionError(f"cli tile CDN refusal drifted: {refused.stderr}")
            assert not (root / "cli-cdn" / "erie-niagara.geojson").exists()

        print("maps extract tiger clip hostile suite passed")
    finally:
        import urllib.request

        urllib.request.urlopen = original_urlopen  # type: ignore[assignment]


if __name__ == "__main__":
    main()
