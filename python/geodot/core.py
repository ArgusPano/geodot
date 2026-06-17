from __future__ import annotations

import json
import math
import os
import random
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import asdict, dataclass
from pathlib import Path

TILE_SIZE = 256

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
    zoom: int = 18
    cols: int = 3
    rows: int = 3
    out: str | Path = "data"
    jobs: int = 16


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
    center = latlon_to_tile(lat, lon, zoom)
    return [Tile(center.x + col, center.y + row, zoom) for row in range(rows) for col in range(cols)]


def tile_grid_between(
    top_left_lat: float,
    top_left_lon: float,
    bottom_right_lat: float,
    bottom_right_lon: float,
    zoom: int,
) -> list[Tile]:
    first = latlon_to_tile(top_left_lat, top_left_lon, zoom)
    second = latlon_to_tile(bottom_right_lat, bottom_right_lon, zoom)
    return _tiles_in_range(
        min(first.x, second.x), max(first.x, second.x), min(first.y, second.y), max(first.y, second.y), zoom
    )


def tile_grid_for_polygon(points: list[Coordinate], zoom: int) -> list[Tile]:
    if len(points) < 3:
        return []
    min_lat = min(point.lat for point in points)
    max_lat = max(point.lat for point in points)
    min_lon = min(point.lon for point in points)
    max_lon = max(point.lon for point in points)
    return [
        tile
        for tile in tile_grid_between(max_lat, min_lon, min_lat, max_lon, zoom)
        if _tile_intersects_polygon(tile, points)
    ]


def tiles_for_options(options: DownloadOptions) -> list[Tile]:
    if options.polygon and len(options.polygon) >= 3:
        return tile_grid_for_polygon(options.polygon, options.zoom)
    if options.bottom_right_lat is not None and options.bottom_right_lon is not None:
        return tile_grid_between(
            options.lat, options.lon, options.bottom_right_lat, options.bottom_right_lon, options.zoom
        )
    return tile_grid(options.lat, options.lon, options.zoom, options.cols, options.rows)


def tile_path(out: str | Path, tile: Tile) -> Path:
    return Path(out) / "tiles" / str(tile.z) / str(tile.x) / f"{tile.y}.jpg"


def download(options: DownloadOptions | None = None) -> DownloadReport:
    options = options or DownloadOptions()
    center = latlon_to_tile(options.lat, options.lon, options.zoom)
    tiles = tiles_for_options(options)
    downloaded: list[DownloadedTile] = []
    failed: list[Tile] = []

    with ThreadPoolExecutor(max_workers=max(1, options.jobs)) as executor:
        futures = {executor.submit(_download_tile, tile): tile for tile in tiles}
        for future in as_completed(futures):
            tile = futures[future]
            data = future.result()
            if data is None:
                failed.append(tile)
                continue
            path = tile_path(options.out, tile)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            downloaded.append(DownloadedTile(tile=tile, bounds=tile_bounds(tile), path=str(path), bytes=len(data)))

    report = DownloadReport(center=center, tiles=downloaded, failed=failed)
    _write_manifest(Path(options.out), report)
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


def _tile_url(subdomain: str, tile: Tile) -> str:
    template = os.environ.get(TILE_URL_TEMPLATE_ENV)
    if template:
        return template.format(sub=subdomain, x=tile.x, y=tile.y, z=tile.z)
    return f"https://{subdomain}.google.com/vt/lyrs=s&x={tile.x}&y={tile.y}&z={tile.z}"


def _tiles_in_range(min_x: int, max_x: int, min_y: int, max_y: int, z: int) -> list[Tile]:
    return [Tile(x, y, z) for y in range(min_y, max_y + 1) for x in range(min_x, max_x + 1)]


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
