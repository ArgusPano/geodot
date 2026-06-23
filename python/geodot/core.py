from __future__ import annotations

import json
import math
import os
import random
import urllib.request
from collections.abc import Callable, Iterator
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from dataclasses import asdict, dataclass, replace
from pathlib import Path
from typing import Any

TILE_SIZE = 256
MAX_ZOOM = 30

USER_AGENTS = (
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 "
    "(KHTML, like Gecko) Version/18.5 Safari/605.1.15",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
)

SUBDOMAINS = ("mt0", "mt1", "mt2", "mt3")
TILE_URL_TEMPLATE_ENV = "GEODOT_TILE_URL_TEMPLATE"


@dataclass(frozen=True)
class Tile:
    x: int
    y: int
    z: int


@dataclass(frozen=True)
class TileBounds:
    lat_min: float
    lon_min: float
    lat_max: float
    lon_max: float


@dataclass(frozen=True)
class Coordinate:
    lon: float
    lat: float


@dataclass(frozen=True)
class DownloadOptions:
    lat: float = 55.7303
    lon: float = 37.6504907
    bottom_right_lat: float | None = None
    bottom_right_lon: float | None = None
    polygon: list[Coordinate] | None = None
    geojson: str | Path | None = None
    zoom: int = 18
    cols: int = 3
    rows: int = 3
    out: str | Path = "data"
    jobs: int = 16
    no_manifest: bool = False
    no_demo: bool = False
    on_progress: Callable[[dict[str, Any]], None] | None = None


@dataclass(frozen=True)
class DownloadedTile:
    tile: Tile
    bounds: TileBounds
    path: str
    bytes: int


@dataclass(frozen=True)
class DownloadReport:
    center: Tile
    tiles: list[DownloadedTile]
    failed: list[Tile]


@dataclass(frozen=True)
class PrepareOptions:
    out: str | Path = "data"
    patch_sizes: tuple[int, ...] = (1, 2, 3, 4)
    stride: int = 1
    rotations: tuple[int, ...] = (0, 90, 180, 270)


@dataclass(frozen=True)
class PrepareReport:
    tiles: int
    patches: int
    variants: int
    path: str


def latlon_to_tile(lat: float, lon: float, z: int) -> Tile:
    n = 2**z
    x = math.floor((lon + 180.0) / 360.0 * n)
    lat_rad = math.radians(lat)
    y = math.floor((1.0 - math.log(math.tan(lat_rad) + 1.0 / math.cos(lat_rad)) / math.pi) / 2.0 * n)
    return Tile(x=x, y=y, z=z)


def tile_bounds(tile: Tile) -> TileBounds:
    n = 2**tile.z
    lon_min = tile.x / n * 360.0 - 180.0
    lon_max = (tile.x + 1) / n * 360.0 - 180.0
    lat_max = math.degrees(math.atan(math.sinh(math.pi * (1.0 - 2.0 * tile.y / n))))
    lat_min = math.degrees(math.atan(math.sinh(math.pi * (1.0 - 2.0 * (tile.y + 1) / n))))
    return TileBounds(lat_min=lat_min, lon_min=lon_min, lat_max=lat_max, lon_max=lon_max)


def meters_per_pixel(lat: float, z: int) -> float:
    return 40_075_016.686 / (TILE_SIZE * 2**z) * math.cos(math.radians(lat))


def tile_grid(lat: float, lon: float, zoom: int, cols: int, rows: int) -> list[Tile]:
    return list(_tile_grid_iter(lat, lon, zoom, cols, rows))


def tile_grid_between(
    top_left_lat: float,
    top_left_lon: float,
    bottom_right_lat: float,
    bottom_right_lon: float,
    zoom: int,
) -> list[Tile]:
    first = latlon_to_tile(top_left_lat, top_left_lon, zoom)
    second = latlon_to_tile(bottom_right_lat, bottom_right_lon, zoom)
    return list(
        _tiles_in_range(
            min(first.x, second.x),
            max(first.x, second.x),
            min(first.y, second.y),
            max(first.y, second.y),
            zoom,
        )
    )


def tile_grid_for_polygon(points: list[Coordinate], zoom: int) -> list[Tile]:
    return list(_tile_grid_for_polygon_iter(points, zoom))


def tiles_for_options(options: DownloadOptions) -> list[Tile]:
    return list(tile_iter_for_options(options))


def count_tiles_for_options(
    options: DownloadOptions, on_progress: Callable[[dict[str, Any]], None] | None = None
) -> int:
    return sum(1 for _tile in tile_iter_for_options(options, on_progress))


def tile_iter_for_options(
    options: DownloadOptions, on_progress: Callable[[dict[str, Any]], None] | None = None
) -> Iterator[Tile]:
    if options.polygon and len(options.polygon) >= 3:
        return _tile_grid_for_polygon_iter(options.polygon, options.zoom, on_progress)
    if options.bottom_right_lat is not None and options.bottom_right_lon is not None:
        first = latlon_to_tile(options.lat, options.lon, options.zoom)
        second = latlon_to_tile(options.bottom_right_lat, options.bottom_right_lon, options.zoom)
        return _tiles_in_range(
            min(first.x, second.x),
            max(first.x, second.x),
            min(first.y, second.y),
            max(first.y, second.y),
            options.zoom,
            on_progress,
        )
    return _tile_grid_iter(options.lat, options.lon, options.zoom, options.cols, options.rows, on_progress)


def _tile_grid_for_polygon_iter(
    points: list[Coordinate], zoom: int, on_progress: Callable[[dict[str, Any]], None] | None = None
) -> Iterator[Tile]:
    if len(points) < 3:
        return iter(())
    min_lat = min(point.lat for point in points)
    max_lat = max(point.lat for point in points)
    min_lon = min(point.lon for point in points)
    max_lon = max(point.lon for point in points)
    first = latlon_to_tile(max_lat, min_lon, zoom)
    second = latlon_to_tile(min_lat, max_lon, zoom)
    min_x = min(first.x, second.x)
    max_x = max(first.x, second.x)
    min_y = min(first.y, second.y)
    max_y = max(first.y, second.y)
    total = (max_x - min_x + 1) * (max_y - min_y + 1)

    def iterator() -> Iterator[Tile]:
        scanned = 0
        selected = 0
        for tile in _tiles_in_range(min_x, max_x, min_y, max_y, zoom):
            scanned += 1
            if _tile_intersects_polygon(tile, points):
                selected += 1
                if on_progress:
                    on_progress({"phase": "select", "scanned": scanned, "selected": selected, "total": total})
                yield tile
            elif on_progress:
                on_progress({"phase": "select", "scanned": scanned, "selected": selected, "total": total})

    return iterator()


def resolve_options(options: DownloadOptions) -> DownloadOptions:
    resolved = (
        replace(options, polygon=load_geojson_polygon(options.geojson))
        if options.geojson and not options.polygon
        else options
    )
    validate_options(resolved)
    return resolved


def validate_options(options: DownloadOptions) -> None:
    _validate_finite_number("lat", options.lat)
    _validate_finite_number("lon", options.lon)
    if options.bottom_right_lat is not None:
        _validate_finite_number("bottom_right_lat", options.bottom_right_lat)
    if options.bottom_right_lon is not None:
        _validate_finite_number("bottom_right_lon", options.bottom_right_lon)
    for index, point in enumerate(options.polygon or []):
        _validate_finite_number(f"polygon[{index}].lon", point.lon)
        _validate_finite_number(f"polygon[{index}].lat", point.lat)
    _validate_integer_range("zoom", options.zoom, 0, MAX_ZOOM)
    _validate_integer_range("cols", options.cols, 1, None)
    _validate_integer_range("rows", options.rows, 1, None)
    _validate_integer_range("jobs", options.jobs, 1, None)


def load_geojson_polygon(source: str | Path) -> list[Coordinate]:
    source_text = str(source)
    if source_text.startswith(("http://", "https://")):
        with urllib.request.urlopen(source_text, timeout=15) as response:
            text = response.read().decode("utf-8")
    else:
        text = Path(source).read_text(encoding="utf-8")
    return polygon_from_geojson(json.loads(text))


def polygon_from_geojson(geojson: object) -> list[Coordinate]:
    geometry = _find_polygon_geometry(geojson)
    if geometry is None:
        raise ValueError("GeoJSON does not contain a Polygon geometry")
    coordinates = geometry["coordinates"]
    ring = coordinates[0] if geometry["type"] == "Polygon" else coordinates[0][0]
    points = [Coordinate(lon=float(point[0]), lat=float(point[1])) for point in ring]
    if len(points) < 3:
        raise ValueError("GeoJSON polygon requires at least three lon,lat coordinates")
    return points


def tile_path(out: str | Path, tile: Tile) -> Path:
    return Path(out) / "tiles" / str(tile.z) / str(tile.x) / f"{tile.y}.jpg"


def prepare_dataset(options: PrepareOptions | None = None) -> PrepareReport:
    options = options or PrepareOptions()
    _validate_prepare_options(options)
    out = Path(options.out)
    tiles = _discover_tiles(out)
    tile_ids = {(tile["z"], tile["x"], tile["y"]): tile["tile_id"] for tile in tiles}
    patches = _build_patches(tiles, tile_ids, options)
    variants = [
        {
            "variant_id": f"{patch['patch_id']}_r{rotation}",
            "patch_id": patch["patch_id"],
            "rotation_deg": rotation,
            "descriptor_id": None,
            "index_id": None,
        }
        for patch in patches
        for rotation in options.rotations
    ]
    root = out / "vpr"
    manifest = root / "manifest"
    config = root / "config"
    manifest.mkdir(parents=True, exist_ok=True)
    config.mkdir(parents=True, exist_ok=True)
    (manifest / "tiles.json").write_text(json.dumps(tiles, indent=2), encoding="utf-8")
    (manifest / "patches.json").write_text(json.dumps(patches, indent=2), encoding="utf-8")
    (manifest / "variants.json").write_text(json.dumps(variants, indent=2), encoding="utf-8")
    dataset = {
        "profile": "aerial-vpr-default",
        "tile_root": "tiles/{z}/{x}/{y}.jpg",
        "mode": "virtual",
        "tile_size": TILE_SIZE,
        "patch_sizes": list(options.patch_sizes),
        "stride": options.stride,
        "rotations": list(options.rotations),
        "appearance": [],
        "counts": {"tiles": len(tiles), "patches": len(patches), "variants": len(variants)},
    }
    (config / "dataset.json").write_text(json.dumps(dataset, indent=2), encoding="utf-8")
    return PrepareReport(tiles=len(tiles), patches=len(patches), variants=len(variants), path=str(root))


def download(options: DownloadOptions | None = None) -> DownloadReport:
    options = resolve_options(options or DownloadOptions())
    center = latlon_to_tile(options.lat, options.lon, options.zoom)
    tiles = tile_iter_for_options(options)
    downloaded: list[DownloadedTile] = []
    failed: list[Tile] = []
    completed = 0

    with ThreadPoolExecutor(max_workers=max(1, options.jobs)) as executor:
        tile_iter = iter(tiles)
        futures = {}

        def submit_next() -> None:
            try:
                tile = next(tile_iter)
            except StopIteration:
                return
            futures[executor.submit(_download_tile, tile)] = tile

        for _ in range(max(1, options.jobs)):
            submit_next()

        while futures:
            done, _pending = wait(futures, return_when=FIRST_COMPLETED)
            for future in done:
                tile = futures.pop(future)
                data = future.result()
                if data is None:
                    failed.append(tile)
                else:
                    path = tile_path(options.out, tile)
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(data)
                    downloaded.append(
                        DownloadedTile(tile=tile, bounds=tile_bounds(tile), path=str(path), bytes=len(data))
                    )
                completed += 1
                if options.on_progress:
                    options.on_progress(
                        {
                            "phase": "download",
                            "completed": completed,
                            "downloaded": len(downloaded),
                            "failed": len(failed),
                            "tile": tile,
                        }
                    )
                submit_next()

    report = DownloadReport(center=center, tiles=downloaded, failed=failed)
    out = Path(options.out)
    if not options.no_manifest:
        _write_manifest(out, report)
    if not options.no_demo:
        _write_demo(out, report)
    return report


def _download_tile(tile: Tile) -> bytes | None:
    headers = {
        "Accept": "image/avif,image/webp,image/apng,image/*,*/*;q=0.8",
        "Accept-Language": "en-US,en;q=0.9",
        "Referer": "https://www.google.com/maps",
    }
    for subdomain in SUBDOMAINS:
        url = _tile_url(subdomain, tile)
        request = urllib.request.Request(url, headers={**headers, "User-Agent": random.choice(USER_AGENTS)})
        try:
            with urllib.request.urlopen(request, timeout=15) as response:
                data = response.read()
                if response.status == 200 and len(data) > 100:
                    return data
        except OSError:
            continue
    return None


def _validate_finite_number(name: str, value: float) -> None:
    if not math.isfinite(value):
        raise ValueError(f"{name} must be a finite number")


def _validate_integer_range(name: str, value: int, minimum: int, maximum: int | None) -> None:
    if (
        not isinstance(value, int)
        or isinstance(value, bool)
        or value < minimum
        or (maximum is not None and value > maximum)
    ):
        limit = f"{minimum} to {maximum}" if maximum is not None else f"at least {minimum}"
        raise ValueError(f"{name} must be an integer {limit}")


def _validate_prepare_options(options: PrepareOptions) -> None:
    _validate_integer_range("stride", options.stride, 1, None)
    if not options.patch_sizes:
        raise ValueError("patch_sizes must not be empty")
    if not options.rotations:
        raise ValueError("rotations must not be empty")
    for size in options.patch_sizes:
        _validate_integer_range("patch_sizes", size, 1, None)
    for rotation in options.rotations:
        _validate_integer_range("rotations", rotation, 0, 359)


def _discover_tiles(out: Path) -> list[dict[str, Any]]:
    root = out / "tiles"
    if not root.exists():
        raise FileNotFoundError(f"tile directory not found: {root}")
    tiles: list[dict[str, Any]] = []
    for file in sorted(root.rglob("*.jpg")):
        try:
            z = int(file.parent.parent.name)
            x = int(file.parent.name)
            y = int(file.stem)
        except (ValueError, IndexError):
            continue
        if not (0 <= z <= MAX_ZOOM):
            continue
        max_tile = 2**z
        if not (0 <= x < max_tile and 0 <= y < max_tile):
            continue
        tile = Tile(x=x, y=y, z=z)
        bounds = tile_bounds(tile)
        tile_id = f"z{z}_x{x}_y{y}"
        tiles.append(
            {
                "tile_id": tile_id,
                "z": z,
                "x": x,
                "y": y,
                "path": str(file.relative_to(out)),
                "pixel_width": TILE_SIZE,
                "pixel_height": TILE_SIZE,
                "lon_min": bounds.lon_min,
                "lat_min": bounds.lat_min,
                "lon_max": bounds.lon_max,
                "lat_max": bounds.lat_max,
                "center_lon": (bounds.lon_min + bounds.lon_max) / 2,
                "center_lat": (bounds.lat_min + bounds.lat_max) / 2,
            }
        )
    if not tiles:
        raise ValueError(f"no valid tiles found under {root}")
    return tiles


def _build_patches(
    tiles: list[dict[str, Any]],
    tile_ids: dict[tuple[int, int, int], str],
    options: PrepareOptions,
) -> list[dict[str, Any]]:
    by_zoom: dict[int, list[dict[str, Any]]] = {}
    for tile in tiles:
        by_zoom.setdefault(tile["z"], []).append(tile)
    patches: list[dict[str, Any]] = []
    for z, zoom_tiles in sorted(by_zoom.items()):
        xs = sorted({tile["x"] for tile in zoom_tiles})
        ys = sorted({tile["y"] for tile in zoom_tiles})
        if not xs or not ys:
            continue
        for size in sorted(set(options.patch_sizes)):
            for y in range(min(ys), max(ys) - size + 2, options.stride):
                for x in range(min(xs), max(xs) - size + 2, options.stride):
                    keys = [
                        (z, source_x, source_y)
                        for source_y in range(y, y + size)
                        for source_x in range(x, x + size)
                    ]
                    source_tiles = [tile_ids.get(key) for key in keys]
                    if any(tile_id is None for tile_id in source_tiles):
                        continue
                    top_left = tile_bounds(Tile(x=x, y=y, z=z))
                    bottom_right = tile_bounds(Tile(x=x + size - 1, y=y + size - 1, z=z))
                    lon_min = top_left.lon_min
                    lat_min = bottom_right.lat_min
                    lon_max = bottom_right.lon_max
                    lat_max = top_left.lat_max
                    patch_id = f"z{z}_x{x}-{x + size - 1}_y{y}-{y + size - 1}_s{size}"
                    patches.append(
                        {
                            "patch_id": patch_id,
                            "z": z,
                            "source_x_min": x,
                            "source_x_max": x + size - 1,
                            "source_y_min": y,
                            "source_y_max": y + size - 1,
                            "source_tiles": source_tiles,
                            "pixel_width": TILE_SIZE * size,
                            "pixel_height": TILE_SIZE * size,
                            "lon_min": lon_min,
                            "lat_min": lat_min,
                            "lon_max": lon_max,
                            "lat_max": lat_max,
                            "center_lon": (lon_min + lon_max) / 2,
                            "center_lat": (lat_min + lat_max) / 2,
                            "mosaic_size_tiles": size,
                            "stride_tiles": options.stride,
                            "scale_profile": f"z{z}_{size}x{size}",
                            "image_path_or_virtual_spec": {
                                "type": "virtual_mosaic",
                                "tile_ids": source_tiles,
                                "layout": [size, size],
                            },
                        }
                    )
    return patches


def _tile_url(subdomain: str, tile: Tile) -> str:
    template = os.environ.get(TILE_URL_TEMPLATE_ENV)
    if template:
        return template.format(sub=subdomain, x=tile.x, y=tile.y, z=tile.z)
    return f"https://{subdomain}.google.com/vt/lyrs=s&x={tile.x}&y={tile.y}&z={tile.z}"


def _tile_grid_iter(
    lat: float,
    lon: float,
    zoom: int,
    cols: int,
    rows: int,
    on_progress: Callable[[dict[str, Any]], None] | None = None,
) -> Iterator[Tile]:
    center = latlon_to_tile(lat, lon, zoom)

    def iterator() -> Iterator[Tile]:
        scanned = 0
        total = cols * rows
        for row in range(rows):
            for col in range(cols):
                scanned += 1
                if on_progress:
                    on_progress({"phase": "select", "scanned": scanned, "selected": scanned, "total": total})
                yield Tile(center.x + col, center.y + row, zoom)

    return iterator()


def _tiles_in_range(
    min_x: int,
    max_x: int,
    min_y: int,
    max_y: int,
    z: int,
    on_progress: Callable[[dict[str, Any]], None] | None = None,
) -> Iterator[Tile]:
    def iterator() -> Iterator[Tile]:
        scanned = 0
        total = (max_x - min_x + 1) * (max_y - min_y + 1)
        for y in range(min_y, max_y + 1):
            for x in range(min_x, max_x + 1):
                scanned += 1
                if on_progress:
                    on_progress({"phase": "select", "scanned": scanned, "selected": scanned, "total": total})
                yield Tile(x, y, z)

    return iterator()


def _find_polygon_geometry(value: object) -> dict | None:
    if not isinstance(value, dict):
        return None
    if value.get("type") in {"Polygon", "MultiPolygon"}:
        return value
    if value.get("type") == "Feature":
        return _find_polygon_geometry(value.get("geometry"))
    if value.get("type") == "FeatureCollection":
        for feature in value.get("features", []):
            geometry = _find_polygon_geometry(feature)
            if geometry is not None:
                return geometry
    return None


def _tile_intersects_polygon(tile: Tile, points: list[Coordinate]) -> bool:
    bounds = tile_bounds(tile)
    center = Coordinate(lon=(bounds.lon_min + bounds.lon_max) / 2, lat=(bounds.lat_min + bounds.lat_max) / 2)
    if _point_in_polygon(center, points):
        return True
    corners = [
        Coordinate(bounds.lon_min, bounds.lat_min),
        Coordinate(bounds.lon_min, bounds.lat_max),
        Coordinate(bounds.lon_max, bounds.lat_min),
        Coordinate(bounds.lon_max, bounds.lat_max),
    ]
    if any(_point_in_polygon(corner, points) for corner in corners):
        return True
    return any(
        bounds.lon_min <= point.lon <= bounds.lon_max and bounds.lat_min <= point.lat <= bounds.lat_max
        for point in points
    )


def _point_in_polygon(point: Coordinate, polygon: list[Coordinate]) -> bool:
    inside = False
    previous = polygon[-1]
    for current in polygon:
        if (current.lat > point.lat) != (previous.lat > point.lat):
            lon = (previous.lon - current.lon) * (point.lat - current.lat) / (previous.lat - current.lat) + current.lon
            if point.lon < lon:
                inside = not inside
        previous = current
    return inside


def _write_manifest(out: Path, report: DownloadReport) -> None:
    out.mkdir(parents=True, exist_ok=True)
    (out / "manifest.json").write_text(json.dumps(asdict(report), indent=2), encoding="utf-8")


def _write_demo(out: Path, report: DownloadReport) -> None:
    out.mkdir(parents=True, exist_ok=True)
    tiles = [asdict(item.tile) for item in report.tiles]
    if report.tiles:
        min_lon = min(item.bounds.lon_min for item in report.tiles)
        min_lat = min(item.bounds.lat_min for item in report.tiles)
        max_lon = max(item.bounds.lon_max for item in report.tiles)
        max_lat = max(item.bounds.lat_max for item in report.tiles)
        zoom = report.tiles[0].tile.z
    else:
        bounds = tile_bounds(report.center)
        min_lon, min_lat, max_lon, max_lat = bounds.lon_min, bounds.lat_min, bounds.lon_max, bounds.lat_max
        zoom = report.center.z
    demo_data = json.dumps(
        {
            "tiles": tiles,
            "bounds": [[min_lon, min_lat], [max_lon, max_lat]],
            "mapCenter": [(min_lon + max_lon) / 2, (min_lat + max_lat) / 2],
            "zoom": zoom,
            "center": asdict(report.center),
        },
        separators=(",", ":"),
    )
    (out / "index.html").write_text(_demo_html(demo_data), encoding="utf-8")


def _demo_html(demo_data: str) -> str:
    return f"""<!doctype html>
<html lang=\"en\">
<head>
  <meta charset=\"utf-8\">
  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
  <title>geodot tile overlay demo</title>
  <link href=\"https://unpkg.com/maplibre-gl@5.14.0/dist/maplibre-gl.css\" rel=\"stylesheet\">
  <style>
    html, body, #map {{ height: 100%; margin: 0; }}
    .panel {{
      position: absolute; top: 12px; right: 12px; z-index: 1; display: grid; gap: 8px;
      padding: 10px; border-radius: 10px; background: rgba(255,255,255,.92);
      font: 13px system-ui, sans-serif; box-shadow: 0 6px 24px rgba(0,0,0,.18);
    }}
    .panel button {{
      border: 0; border-radius: 8px; padding: 8px 10px; background: #1f2937; color: white;
      cursor: pointer;
    }}
    .opacity {{ display: grid; gap: 4px; }}
    .warning {{ max-width: 260px; color: #92400e; }}
    .hidden {{ display: none; }}
  </style>
</head>
<body>
  <div id=\"map\"></div>
  <div class=\"panel\">
    <button id=\"toggle\" type=\"button\">Overlay opacity</button>
    <label id=\"opacityPanel\" class=\"opacity hidden\">Transparency
      <input id=\"opacity\" type=\"range\" min=\"0\" max=\"1\" step=\"0.05\" value=\"0.65\">
    </label>
    <div id=\"fileWarning\" class=\"warning hidden\">
      Local file mode cannot load tile files. Run geodot demo and open http://127.0.0.1:8000/.
    </div>
  </div>
  <script src=\"https://unpkg.com/maplibre-gl@5.14.0/dist/maplibre-gl.js\"></script>
  <script>
    const data = {demo_data};
    if (location.protocol === 'file:') {{
      document.getElementById('fileWarning').classList.remove('hidden');
    }}
    const map = new maplibregl.Map({{
      container: 'map',
      style: {{
        version: 8,
        sources: {{
          satellite: {{
            type: 'raster',
            tiles: [
              'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{{z}}/{{y}}/{{x}}'
            ],
            tileSize: 256,
            attribution: 'Sources: Esri, Maxar, Earthstar Geographics, and the GIS User Community'
          }}
        }},
        layers: [{{ id: 'satellite', type: 'raster', source: 'satellite' }}]
      }},
      center: data.mapCenter,
      zoom: data.zoom,
      minZoom: data.zoom,
      maxZoom: data.zoom,
      scrollZoom: false,
      boxZoom: false,
      doubleClickZoom: false,
      touchZoomRotate: false,
      keyboard: false,
      dragRotate: false,
      pitchWithRotate: false
    }});

    map.on('load', () => {{
      map.addSource('geodot-tiles', {{
        type: 'raster',
        tiles: ['./tiles/{{z}}/{{x}}/{{y}}.jpg'],
        tileSize: 256,
        minzoom: data.zoom,
        maxzoom: data.zoom,
        bounds: [data.bounds[0][0], data.bounds[0][1], data.bounds[1][0], data.bounds[1][1]]
      }});
      map.addLayer({{
        id: 'geodot-tiles',
        type: 'raster',
        source: 'geodot-tiles',
        paint: {{ 'raster-opacity': 0.65 }}
      }});
    }});

    document.getElementById('toggle').addEventListener('click', () => {{
      document.getElementById('opacityPanel').classList.toggle('hidden');
    }});
    document.getElementById('opacity').addEventListener('input', (event) => {{
      if (map.getLayer('geodot-tiles')) {{
        map.setPaintProperty('geodot-tiles', 'raster-opacity', Number(event.target.value));
      }}
    }});
  </script>
</body>
</html>
"""
