from __future__ import annotations

import json
import math
import os
import random
import sys
import urllib.request
from collections.abc import Callable, Iterator
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from dataclasses import asdict, dataclass, replace
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

TILE_SIZE = 256
MAX_ZOOM = 30
SUPPORTED_IMAGE_EXTENSIONS = (".jpg", ".jpeg", ".png", ".webp")

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
    patch_sizes: tuple[int, ...] = (1, 2, 4)
    stride: int = 1
    rotations: tuple[int, ...] = (0, 45, 90, 135, 180, 225, 270, 315)
    auto400m: bool = True


@dataclass(frozen=True)
class PrepareReport:
    tiles: int
    patches: int
    variants: int
    path: str


@dataclass(frozen=True)
class RenderReport:
    source_path: str
    output_path: str
    bytes: int


@dataclass(frozen=True)
class ValidationReport:
    valid: bool
    errors: tuple[str, ...]
    warnings: tuple[str, ...]
    counts: dict[str, int]


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
    patch_sizes = _resolve_patch_sizes(tiles, options)
    tile_ids = {(tile["root"], tile["capture_id"], tile["z"], tile["x"], tile["y"]): tile["tile_id"] for tile in tiles}
    patches = _build_patches(tiles, tile_ids, options, patch_sizes)
    places = _build_places(tiles, patches)
    quality = _build_quality(tiles, patches)
    variants = [
        {
            "variant_id": f"{patch['patch_id']}_r{rotation}",
            "patch_id": patch["patch_id"],
            "rotation_degrees": rotation,
            "crop_shape": "square",
            "virtual_only": True,
            "image_written": False,
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
    (manifest / "places.json").write_text(json.dumps(places, indent=2), encoding="utf-8")
    (manifest / "quality.json").write_text(json.dumps(quality, indent=2), encoding="utf-8")
    dataset = {
        "schema_version": "1.0",
        "geodot_version": "unknown",
        "created_at": datetime.now(UTC).isoformat(),
        "command": " ".join(sys.argv),
        "output_directory": str(out),
        "profile": "aerial-vpr-default",
        "tile_roots": [
            f"{root}/{{z}}/{{x}}/{{y}}.{{jpg,jpeg,png,webp}}" for root in sorted({tile["root"] for tile in tiles})
        ],
        "mode": "virtual",
        "tile_size": TILE_SIZE,
        "image_roots_detected": sorted({tile["root"] for tile in tiles}),
        "supported_image_extensions": list(SUPPORTED_IMAGE_EXTENSIONS),
        "zoom_levels_detected": sorted({tile["z"] for tile in tiles}),
        "patch_sizes": patch_sizes,
        "stride": options.stride,
        "rotations": list(options.rotations),
        "auto400m": options.auto400m,
        "circular_crops_virtual": True,
        "images_modified": False,
        "generated_images_default": False,
        "descriptors_computed": False,
        "indexes_built": False,
        "appearance": [],
        "counts": {"tiles": len(tiles), "patches": len(patches), "variants": len(variants), "places": len(places)},
    }
    (config / "dataset.json").write_text(json.dumps(dataset, indent=2), encoding="utf-8")
    return PrepareReport(tiles=len(tiles), patches=len(patches), variants=len(variants), path=str(root))


def render_dataset(
    out: str | Path = "data",
    *,
    patch_id: str | None = None,
    variant_id: str | None = None,
    output: str | Path,
) -> RenderReport:
    if bool(patch_id) == bool(variant_id):
        raise ValueError("provide exactly one of patch_id or variant_id")
    root = Path(out)
    manifest = root / "vpr" / "manifest"
    patches = json.loads((manifest / "patches.json").read_text(encoding="utf-8"))
    tiles = json.loads((manifest / "tiles.json").read_text(encoding="utf-8"))
    if variant_id:
        variants = json.loads((manifest / "variants.json").read_text(encoding="utf-8"))
        variant = next((item for item in variants if item.get("variant_id") == variant_id), None)
        if variant is None:
            raise ValueError(f"variant not found: {variant_id}")
        patch_id = variant["patch_id"]
    patch = next((item for item in patches if item.get("patch_id") == patch_id), None)
    if patch is None:
        raise ValueError(f"patch not found: {patch_id}")
    source_tile_ids = patch.get("source_tile_ids") or patch.get("source_tiles") or []
    if len(source_tile_ids) != 1:
        raise ValueError("render currently supports one-source-tile virtual patches only")
    tile = next((item for item in tiles if item.get("tile_id") == source_tile_ids[0]), None)
    if tile is None:
        raise ValueError(f"source tile not found: {source_tile_ids[0]}")
    source = root / tile["path"]
    target = Path(output)
    target.parent.mkdir(parents=True, exist_ok=True)
    data = source.read_bytes()
    target.write_bytes(data)
    return RenderReport(source_path=str(source), output_path=str(target), bytes=len(data))


def load_dataset(out: str | Path = "data") -> dict[str, Any]:
    root = Path(out)
    manifest = root / "vpr" / "manifest"
    config = root / "vpr" / "config"
    files = {
        "tiles": manifest / "tiles.json",
        "patches": manifest / "patches.json",
        "variants": manifest / "variants.json",
        "places": manifest / "places.json",
        "quality": manifest / "quality.json",
        "dataset": config / "dataset.json",
    }
    missing = [str(path) for path in files.values() if not path.exists()]
    if missing:
        raise FileNotFoundError("missing dataset manifest(s): " + ", ".join(missing))
    dataset = {name: json.loads(path.read_text(encoding="utf-8")) for name, path in files.items()}
    dataset["_root"] = str(root)
    return dataset


def render_variant(dataset: dict[str, Any], variant_id: str) -> bytes:
    variants = dataset["variants"]
    patches = dataset["patches"]
    tiles = dataset["tiles"]
    variant = next((item for item in variants if item.get("variant_id") == variant_id), None)
    if variant is None:
        raise ValueError(f"variant not found: {variant_id}")
    patch = next((item for item in patches if item.get("patch_id") == variant.get("patch_id")), None)
    if patch is None:
        raise ValueError(f"patch not found: {variant.get('patch_id')}")
    source_tile_ids = patch.get("source_tile_ids") or patch.get("source_tiles") or []
    if len(source_tile_ids) != 1:
        raise ValueError("render_variant currently supports one-source-tile virtual patches only")
    tile = next((item for item in tiles if item.get("tile_id") == source_tile_ids[0]), None)
    if tile is None:
        raise ValueError(f"source tile not found: {source_tile_ids[0]}")
    return (Path(dataset.get("_root", ".")) / tile["path"]).read_bytes()


def validate_dataset(out: str | Path = "data", *, strict: bool = False) -> ValidationReport:
    root = Path(out)
    errors: list[str] = []
    warnings: list[str] = []
    try:
        dataset = load_dataset(root)
    except FileNotFoundError:
        raise
    except (json.JSONDecodeError, OSError) as error:
        return ValidationReport(False, (f"failed to parse dataset: {error}",), (), _empty_validation_counts())

    tiles = dataset["tiles"]
    patches = dataset["patches"]
    variants = dataset["variants"]
    places = dataset["places"]
    config = dataset["dataset"]
    tile_ids = _check_unique_ids("tile", tiles, "tile_id", errors)
    patch_ids = _check_unique_ids("patch", patches, "patch_id", errors)
    _check_unique_ids("variant", variants, "variant_id", errors)
    _check_unique_ids("place", places, "place_id", errors)

    for tile in tiles:
        path = tile.get("path")
        if not isinstance(path, str) or not (root / path).exists():
            errors.append(f"missing source image for tile {tile.get('tile_id')}: {path}")
        if not _valid_bbox(tile.get("bbox")):
            errors.append(f"invalid bbox for tile {tile.get('tile_id')}")
        if not _positive_int(tile.get("image_width")) or not _positive_int(tile.get("image_height")):
            errors.append(f"invalid image dimensions for tile {tile.get('tile_id')}")

    for patch in patches:
        if not _valid_bbox(patch.get("bbox")):
            errors.append(f"invalid bbox for patch {patch.get('patch_id')}")
        for tile_id in patch.get("source_tile_ids") or patch.get("source_tiles") or []:
            if tile_id not in tile_ids:
                errors.append(f"patch {patch.get('patch_id')} references missing tile {tile_id}")

    for variant in variants:
        patch_id = variant.get("patch_id")
        if patch_id not in patch_ids:
            errors.append(f"variant {variant.get('variant_id')} references missing patch {patch_id}")

    for place in places:
        for field in ("tile_ids", "reference_tile_ids", "query_tile_ids"):
            for tile_id in place.get(field, []):
                if tile_id not in tile_ids:
                    errors.append(f"place {place.get('place_id')} {field} references missing tile {tile_id}")
        for patch_id in place.get("patch_ids", []):
            if patch_id not in patch_ids:
                errors.append(f"place {place.get('place_id')} references missing patch {patch_id}")

    for field in ("images_modified", "descriptors_computed", "indexes_built", "generated_images_default"):
        if config.get(field) is not False:
            errors.append(f"dataset config {field} must be false")

    generated = [path for path in (root / "vpr").rglob("*") if path.suffix.lower() in SUPPORTED_IMAGE_EXTENSIONS]
    if generated:
        warnings.append(f"found generated image(s) under vpr: {len(generated)}")
    if strict and warnings:
        errors.extend(warnings)
        warnings = []
    counts = {
        "tiles": len(tiles),
        "patches": len(patches),
        "variants": len(variants),
        "places": len(places),
        "query_tiles": sum(1 for tile in tiles if tile.get("role") == "query"),
        "reference_tiles": sum(1 for tile in tiles if tile.get("role") == "reference"),
        "warnings": len(warnings),
        "errors": len(errors),
    }
    return ValidationReport(not errors, tuple(errors), tuple(warnings), counts)


def _empty_validation_counts() -> dict[str, int]:
    return {
        "tiles": 0,
        "patches": 0,
        "variants": 0,
        "places": 0,
        "query_tiles": 0,
        "reference_tiles": 0,
        "warnings": 0,
        "errors": 1,
    }


def _check_unique_ids(kind: str, items: list[dict[str, Any]], field: str, errors: list[str]) -> set[str]:
    seen: set[str] = set()
    for item in items:
        value = item.get(field)
        if not isinstance(value, str) or not value:
            errors.append(f"{kind} missing {field}")
            continue
        if value in seen:
            errors.append(f"duplicate {kind} id: {value}")
        seen.add(value)
    return seen


def _valid_bbox(value: object) -> bool:
    if not isinstance(value, list) or len(value) != 4:
        return False
    if not all(isinstance(item, int | float) and math.isfinite(item) for item in value):
        return False
    lon_min, lat_min, lon_max, lat_max = value
    return -180 <= lon_min < lon_max <= 180 and -90 <= lat_min < lat_max <= 90


def _positive_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


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
    roots = [("tiles", out / "tiles"), ("drone-view", out / "drone-view")]
    if not any(root.exists() for _name, root in roots):
        raise FileNotFoundError(f"tile directory not found: {out / 'tiles'}")
    tiles: list[dict[str, Any]] = []
    for root_name, root in roots:
        if not root.exists():
            continue
        role = "reference" if root_name == "tiles" else "query"
        for file in sorted(item for item in root.rglob("*") if item.suffix.lower() in SUPPORTED_IMAGE_EXTENSIONS):
            parsed = _parse_image_path(root, file)
            if parsed is None:
                continue
            capture_id, z, x, y = parsed
            max_tile = 2**z
            if not (0 <= x < max_tile and 0 <= y < max_tile):
                continue
            tile = Tile(x=x, y=y, z=z)
            bounds = tile_bounds(tile)
            tile_id = f"{root_name}_{capture_id}_z{z}_x{x}_y{y}"
            info = file.stat()
            image_width, image_height, detected_format = _read_image_header(file)
            tiles.append(
                {
                    "tile_id": tile_id,
                    "root": root_name,
                    "capture_id": capture_id,
                    "role": role,
                    "z": z,
                    "x": x,
                    "y": y,
                    "extension": file.suffix.lower().lstrip("."),
                    "detected_format": detected_format,
                    "path": str(file.relative_to(out)),
                    "bbox": [bounds.lon_min, bounds.lat_min, bounds.lon_max, bounds.lat_max],
                    "center_lon": (bounds.lon_min + bounds.lon_max) / 2,
                    "center_lat": (bounds.lat_min + bounds.lat_max) / 2,
                    "image_width": image_width or TILE_SIZE,
                    "image_height": image_height or TILE_SIZE,
                    "pixel_width": image_width or TILE_SIZE,
                    "pixel_height": image_height or TILE_SIZE,
                    "bytes": info.st_size,
                    "valid": image_width is not None and image_height is not None,
                    "lon_min": bounds.lon_min,
                    "lat_min": bounds.lat_min,
                    "lon_max": bounds.lon_max,
                    "lat_max": bounds.lat_max,
                }
            )
    if not tiles:
        raise ValueError(f"no valid tiles found under {out / 'tiles'} or {out / 'drone-view'}")
    return tiles


def _parse_image_path(root: Path, file: Path) -> tuple[str, int, int, int] | None:
    parts = file.relative_to(root).parts
    if len(parts) == 3:
        capture_id = "default"
        z_text, x_text, y_file = parts
    elif len(parts) == 4:
        capture_id, z_text, x_text, y_file = parts
    else:
        return None
    try:
        z = int(z_text)
        x = int(x_text)
        y = int(Path(y_file).stem)
    except ValueError:
        return None
    if not (0 <= z <= MAX_ZOOM):
        return None
    return capture_id, z, x, y


def _read_image_header(file: Path) -> tuple[int | None, int | None, str | None]:
    data = file.read_bytes()[:64]
    if data.startswith(b"\x89PNG\r\n\x1a\n") and len(data) >= 24:
        return int.from_bytes(data[16:20], "big"), int.from_bytes(data[20:24], "big"), "png"
    if data.startswith(b"RIFF") and data[8:12] == b"WEBP":
        return _read_webp_header(file)
    if data.startswith(b"\xff\xd8"):
        return _read_jpeg_header(file)
    return None, None, None


def _read_jpeg_header(file: Path) -> tuple[int | None, int | None, str | None]:
    data = file.read_bytes()
    index = 2
    while index + 9 < len(data):
        if data[index] != 0xFF:
            index += 1
            continue
        marker = data[index + 1]
        if marker in {0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF}:
            return (
                int.from_bytes(data[index + 7 : index + 9], "big"),
                int.from_bytes(data[index + 5 : index + 7], "big"),
                "jpeg",
            )
        if index + 4 > len(data):
            break
        length = int.from_bytes(data[index + 2 : index + 4], "big")
        index += 2 + max(length, 1)
    return None, None, "jpeg"


def _read_webp_header(file: Path) -> tuple[int | None, int | None, str | None]:
    data = file.read_bytes()[:64]
    chunk = data[12:16]
    if chunk == b"VP8X" and len(data) >= 30:
        width = int.from_bytes(data[24:27], "little") + 1
        height = int.from_bytes(data[27:30], "little") + 1
        return width, height, "webp"
    if chunk == b"VP8L" and len(data) >= 25:
        bits = int.from_bytes(data[21:25], "little")
        return (bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1, "webp"
    return None, None, "webp"


def _resolve_patch_sizes(tiles: list[dict[str, Any]], options: PrepareOptions) -> list[int]:
    sizes = set(options.patch_sizes)
    if options.auto400m:
        for tile in tiles:
            tile_width_m = meters_per_pixel(tile["center_lat"], tile["z"]) * TILE_SIZE
            if tile_width_m > 0:
                sizes.add(max(1, min(8, round(400 / tile_width_m))))
    return sorted(sizes)


def _build_patches(
    tiles: list[dict[str, Any]],
    tile_ids: dict[tuple[str, str, int, int, int], str],
    options: PrepareOptions,
    patch_sizes: list[int],
) -> list[dict[str, Any]]:
    by_group: dict[tuple[str, str, int], list[dict[str, Any]]] = {}
    for tile in tiles:
        by_group.setdefault((tile["root"], tile["capture_id"], tile["z"]), []).append(tile)
    patches: list[dict[str, Any]] = []
    for (root, capture_id, z), zoom_tiles in sorted(by_group.items()):
        xs = sorted({tile["x"] for tile in zoom_tiles})
        ys = sorted({tile["y"] for tile in zoom_tiles})
        if not xs or not ys:
            continue
        role = "reference" if root == "tiles" else "query"
        for size in patch_sizes:
            for y in range(min(ys), max(ys) - size + 2, options.stride):
                for x in range(min(xs), max(xs) - size + 2, options.stride):
                    keys = [
                        (root, capture_id, z, source_x, source_y)
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
                    ground_w = meters_per_pixel((lat_min + lat_max) / 2, z) * TILE_SIZE * size
                    patch_id = f"{root}_{capture_id}_z{z}_x{x}-{x + size - 1}_y{y}-{y + size - 1}_s{size}"
                    place_id = f"z{z}_x{x}_y{y}"
                    patches.append(
                        {
                            "patch_id": patch_id,
                            "place_id": place_id,
                            "root": root,
                            "capture_id": capture_id,
                            "role": role,
                            "z": z,
                            "x": x,
                            "y": y,
                            "source_x_min": x,
                            "source_x_max": x + size - 1,
                            "source_y_min": y,
                            "source_y_max": y + size - 1,
                            "source_tiles": source_tiles,
                            "source_tile_ids": source_tiles,
                            "pixel_width": TILE_SIZE * size,
                            "pixel_height": TILE_SIZE * size,
                            "bbox": [lon_min, lat_min, lon_max, lat_max],
                            "lon_min": lon_min,
                            "lat_min": lat_min,
                            "lon_max": lon_max,
                            "lat_max": lat_max,
                            "center_lon": (lon_min + lon_max) / 2,
                            "center_lat": (lat_min + lat_max) / 2,
                            "mosaic_size_tiles": size,
                            "stride_tiles": options.stride,
                            "scale_profile": f"z{z}_{size}x{size}",
                            "ground_width_m_estimate": ground_w,
                            "ground_height_m_estimate": ground_w,
                            "complete": True,
                            "rotation_safe_circle_diameter_px": TILE_SIZE * size,
                            "circular_crop_available": True,
                            "image_written": False,
                            "virtual_compose_spec": {
                                "type": "virtual_mosaic",
                                "tile_ids": source_tiles,
                                "layout": [size, size],
                            },
                            "image_path_or_virtual_spec": {
                                "type": "virtual_mosaic",
                                "tile_ids": source_tiles,
                                "layout": [size, size],
                            },
                        }
                    )
    return patches


def _build_places(tiles: list[dict[str, Any]], patches: list[dict[str, Any]]) -> list[dict[str, Any]]:
    grouped: dict[tuple[int, int, int], dict[str, Any]] = {}
    for tile in tiles:
        key = (tile["z"], tile["x"], tile["y"])
        place = grouped.setdefault(
            key,
            {
                "place_id": f"z{tile['z']}_x{tile['x']}_y{tile['y']}",
                "z": tile["z"],
                "x": tile["x"],
                "y": tile["y"],
                "center_lon": tile["center_lon"],
                "center_lat": tile["center_lat"],
                "bbox": tile["bbox"],
                "available_roots": [],
                "available_captures": [],
                "tile_ids": [],
                "reference_available": False,
                "query_available": False,
                "reference_tile_ids": [],
                "query_tile_ids": [],
                "patch_ids": [],
                "quality_summary": {"recommendation": "keep"},
            },
        )
        place["available_roots"].append(tile["root"])
        place["available_captures"].append(tile["capture_id"])
        place["tile_ids"].append(tile["tile_id"])
        if tile["role"] == "reference":
            place["reference_available"] = True
            place["reference_tile_ids"].append(tile["tile_id"])
        if tile["role"] == "query":
            place["query_available"] = True
            place["query_tile_ids"].append(tile["tile_id"])
    for patch in patches:
        key = (patch["z"], patch["x"], patch["y"])
        if key in grouped:
            grouped[key]["patch_ids"].append(patch["patch_id"])
    for place in grouped.values():
        place["available_roots"] = sorted(set(place["available_roots"]))
        place["available_captures"] = sorted(set(place["available_captures"]))
    return list(grouped.values())


def _build_quality(tiles: list[dict[str, Any]], patches: list[dict[str, Any]]) -> dict[str, Any]:
    tile_quality = []
    for tile in tiles:
        reasons = ["very_small_file"] if tile["bytes"] < 128 else []
        tile_quality.append(
            {
                "id": tile["tile_id"],
                "type": "tile",
                "blank_near_blank_score": 1.0 if reasons else 0.0,
                "mean_brightness": None,
                "contrast": None,
                "blur_estimate": None,
                "edge_density": None,
                "entropy": None,
                "likely_low_information": bool(reasons),
                "recommendation": "reject" if reasons else "keep",
                "reject_reasons": reasons,
            }
        )
    patch_quality = [
        {
            "id": patch["patch_id"],
            "type": "patch",
            "likely_low_information": False,
            "recommendation": "keep",
            "reject_reasons": [],
        }
        for patch in patches
    ]
    return {"tiles": tile_quality, "patches": patch_quality}


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
    html = """<!doctype html>
<html lang=\"en\">
<head>
  <meta charset=\"utf-8\">
  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
  <title>geodot tile overlay demo</title>
  <link href=\"https://unpkg.com/maplibre-gl@5.24.0/dist/maplibre-gl.css\" rel=\"stylesheet\">
  <style>
    :root {
      color-scheme: light; --panel-bg: rgba(255,255,255,.94); --panel-border: rgba(17,24,39,.12);
      --text: #111827; --muted: #6b7280; --button: #111827; --button-text: #fff;
      --secondary: #e5e7eb; --secondary-text: #111827; --input: #fff; --label: #111827;
    }
    body.dark {
      color-scheme: dark; --panel-bg: rgba(17,24,39,.92); --panel-border: rgba(255,255,255,.16);
      --text: #f9fafb; --muted: #9ca3af; --button: #f9fafb; --button-text: #111827;
      --secondary: rgba(255,255,255,.14); --secondary-text: #f9fafb; --input: rgba(17,24,39,.85);
      --label: #f9fafb;
    }
    html, body, #map { height: 100%; margin: 0; }
    body { font: 13px/1.35 system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: var(--text); }
    .panel {
      position: absolute; top: 12px; right: 12px; z-index: 1; display: grid; gap: 12px;
      width: min(360px, calc(100vw - 24px)); padding: 14px; border: 1px solid var(--panel-border);
      border-radius: 18px; background: var(--panel-bg); box-shadow: 0 14px 40px rgba(0,0,0,.24);
      backdrop-filter: blur(12px);
    }
    .panel.collapsed { width: auto; }
    .panel.collapsed .panel-body { display: none; }
    .panel-header { display: flex; align-items: start; justify-content: space-between; gap: 12px; }
    .panel-title { display: grid; gap: 3px; }
    .panel h1 { margin: 0; font-size: 15px; }
    .panel-body { display: grid; gap: 12px; }
    .muted { color: var(--muted); }
    .control { display: grid; gap: 6px; }
    .row { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
    .buttons, .jump { display: grid; grid-template-columns: repeat(3, 1fr); gap: 6px; }
    .jump { grid-template-columns: .8fr 1fr 1fr auto; }
    .toggles { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
    .check {
      display: flex; align-items: center; gap: 8px; padding: 8px; border: 1px solid var(--panel-border);
      border-radius: 12px; background: color-mix(in srgb, var(--input) 85%, transparent);
    }
    button {
      border: 0; border-radius: 10px; padding: 8px 10px; background: var(--button); color: var(--button-text);
      font-weight: 650; cursor: pointer;
    }
    button.secondary { background: var(--secondary); color: var(--secondary-text); }
    input {
      min-width: 0; border: 1px solid #d1d5db; border-radius: 10px; padding: 8px;
      background: var(--input); color: var(--text); font: inherit;
    }
    input[type="checkbox"] { min-width: auto; accent-color: var(--button); }
    input[type="range"] { padding: 0; accent-color: var(--button); }
    .warning { max-width: 100%; color: #92400e; }
    .hidden { display: none; }
    @media (max-width: 640px) { .panel { left: 12px; right: 12px; bottom: 12px; top: auto; width: auto; } }
  </style>
</head>
<body>
  <div id=\"map\"></div>
  <div id=\"panel\" class=\"panel\">
    <div class=\"panel-header\">
      <div class=\"panel-title\">
        <h1>geodot tile demo</h1>
        <div class=\"muted\">Labels show <code>z/x/y</code>. Use <code>#12/2367/1306.jpg</code> to center a tile.</div>
      </div>
      <button id=\"togglePanel\" type=\"button\" class=\"secondary\">Hide</button>
    </div>
    <div class=\"panel-body\">
      <div class=\"toggles\">
        <label class=\"check\"><input id=\"labelsToggle\" type=\"checkbox\" checked> Labels</label>
        <button id=\"themeToggle\" type=\"button\" class=\"secondary\">Dark theme</button>
      </div>
      <div class=\"control\">
        <div class=\"row\">
          <label for=\"opacity\">Overlay transparency</label><strong id=\"opacityValue\">65%</strong>
        </div>
        <input id=\"opacity\" type=\"range\" min=\"0\" max=\"1\" step=\"0.05\" value=\"0.65\">
      </div>
      <div class=\"control\">
        <div class=\"row\"><span>View zoom</span><span id=\"viewZoom\" class=\"muted\"></span></div>
        <div class=\"buttons\">
          <button id=\"zoomOut\" type=\"button\" class=\"secondary\">−</button>
          <button id=\"fitTiles\" type=\"button\" class=\"secondary\">Fit</button>
          <button id=\"zoomIn\" type=\"button\" class=\"secondary\">+</button>
        </div>
      </div>
      <form id=\"jumpForm\" class=\"control\">
        <label>Jump to tile</label>
        <div class=\"jump\">
          <input id=\"jumpZ\" type=\"number\" min=\"0\" step=\"1\" aria-label=\"z\" placeholder=\"z\">
          <input id=\"jumpX\" type=\"number\" min=\"0\" step=\"1\" aria-label=\"x\" placeholder=\"x\">
          <input id=\"jumpY\" type=\"number\" min=\"0\" step=\"1\" aria-label=\"y\" placeholder=\"y\">
          <button type=\"submit\">Go</button>
        </div>
      </form>
      <div id=\"fileWarning\" class=\"warning hidden\">
        Local file mode cannot load tile files. Run geodot demo and open http://127.0.0.1:8000/.
      </div>
    </div>
  </div>
  <script src=\"https://unpkg.com/maplibre-gl@5.24.0/dist/maplibre-gl.js\"></script>
  <script>
    const data = __GEODOT_DEMO_DATA__;
    const opacityInput = document.getElementById('opacity');
    const opacityValue = document.getElementById('opacityValue');
    const viewZoom = document.getElementById('viewZoom');
    const labelsToggle = document.getElementById('labelsToggle');
    const panel = document.getElementById('panel');
    if (location.protocol === 'file:') {
      document.getElementById('fileWarning').classList.remove('hidden');
    }
    const map = new maplibregl.Map({
      container: 'map',
      style: {
        version: 8,
        glyphs: 'https://demotiles.maplibre.org/font/{fontstack}/{range}.pbf',
        sources: {
          satellite: {
            type: 'raster',
            tiles: [
              'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}'
            ],
            tileSize: 256,
            attribution: 'Sources: Esri, Maxar, Earthstar Geographics, and the GIS User Community'
          }
        },
        layers: [{ id: 'satellite', type: 'raster', source: 'satellite' }]
      },
      center: data.mapCenter,
      zoom: data.zoom,
      minZoom: Math.max(0, data.zoom - 8),
      maxZoom: data.zoom + 8,
      scrollZoom: false,
      boxZoom: false,
      doubleClickZoom: true,
      touchZoomRotate: true,
      keyboard: true,
      dragRotate: false,
      pitchWithRotate: false
    });

    function tileBounds(tile) {
      const n = 2 ** tile.z;
      const lonMin = tile.x / n * 360 - 180;
      const lonMax = (tile.x + 1) / n * 360 - 180;
      const latMax = Math.atan(Math.sinh(Math.PI * (1 - 2 * tile.y / n))) * 180 / Math.PI;
      const latMin = Math.atan(Math.sinh(Math.PI * (1 - 2 * (tile.y + 1) / n))) * 180 / Math.PI;
      return { lonMin, latMin, lonMax, latMax };
    }

    function tileCenter(tile) {
      const bounds = tileBounds(tile);
      return [(bounds.lonMin + bounds.lonMax) / 2, (bounds.latMin + bounds.latMax) / 2];
    }

    function tileFromLocation() {
      const value = location.hash.slice(1) || location.pathname.slice(1);
      const parts = value.split('/');
      if (parts.length !== 3) return undefined;
      const y = parts[2].split('.')[0];
      if (![parts[0], parts[1], y].every((part) => (
        part && [...part].every((char) => char >= '0' && char <= '9')
      ))) return undefined;
      return { z: Number(parts[0]), x: Number(parts[1]), y: Number(y) };
    }

    function setOpacity(value) {
      opacityValue.textContent = `${Math.round(value * 100)}%`;
      for (const tile of data.tiles) {
        const layer = `geodot-tile-${tile.z}-${tile.x}-${tile.y}`;
        if (map.getLayer(layer)) map.setPaintProperty(layer, 'raster-opacity', value);
      }
    }

    function updateZoomLabel() {
      viewZoom.textContent = map.getZoom().toFixed(2);
      if (map.getLayer('geodot-labels')) {
        map.setLayoutProperty(
          'geodot-labels',
          'text-size',
          Math.max(10, Math.min(24, 13 + (map.getZoom() - data.zoom) * 2))
        );
      }
    }

    function updateLabelStyle() {
      if (!map.getLayer('geodot-labels')) return;
      map.setLayoutProperty('geodot-labels', 'visibility', labelsToggle.checked ? 'visible' : 'none');
      map.setPaintProperty(
        'geodot-labels',
        'text-color',
        getComputedStyle(document.body).getPropertyValue('--label').trim()
      );
    }

    function fillJump(tile) {
      document.getElementById('jumpZ').value = tile.z;
      document.getElementById('jumpX').value = tile.x;
      document.getElementById('jumpY').value = tile.y;
    }

    function jumpToTile(tile, updateHash = true) {
      fillJump(tile);
      map.easeTo({ center: tileCenter(tile), zoom: Math.max(map.getZoom(), tile.z), duration: 450 });
      if (updateHash) history.replaceState(null, '', `#${tile.z}/${tile.x}/${tile.y}.jpg`);
    }

    map.on('load', () => {
      for (const tile of data.tiles) {
        const bounds = tileBounds(tile);
        const id = `geodot-tile-${tile.z}-${tile.x}-${tile.y}`;
        map.addSource(id, {
          type: 'image',
          url: `./tiles/${tile.z}/${tile.x}/${tile.y}.jpg`,
          coordinates: [
            [bounds.lonMin, bounds.latMax],
            [bounds.lonMax, bounds.latMax],
            [bounds.lonMax, bounds.latMin],
            [bounds.lonMin, bounds.latMin]
          ]
        });
        map.addLayer({ id, type: 'raster', source: id, paint: { 'raster-opacity': Number(opacityInput.value) } });
      }
      map.addSource('geodot-labels', {
        type: 'geojson',
        data: {
          type: 'FeatureCollection',
          features: data.tiles.map((tile) => ({
            type: 'Feature',
            properties: { label: `${tile.z}/${tile.x}/${tile.y}` },
            geometry: { type: 'Point', coordinates: tileCenter(tile) }
          }))
        }
      });
      map.addLayer({
        id: 'geodot-labels',
        type: 'symbol',
        source: 'geodot-labels',
        layout: {
          'text-field': ['get', 'label'],
          'text-size': 13,
          'text-font': ['Open Sans Bold'],
          'text-allow-overlap': true
        },
        paint: { 'text-color': '#111827', 'text-halo-width': 0 }
      });
      updateLabelStyle();
      updateZoomLabel();
      const requestedTile = tileFromLocation();
      if (requestedTile) jumpToTile(requestedTile, false);
    });

    opacityInput.addEventListener('input', (event) => setOpacity(Number(event.target.value)));
    document.getElementById('zoomOut').addEventListener('click', () => map.zoomOut());
    document.getElementById('zoomIn').addEventListener('click', () => map.zoomIn());
    document.getElementById('fitTiles').addEventListener('click', () => {
      map.fitBounds(data.bounds, { padding: 48, duration: 450 });
    });
    document.getElementById('togglePanel').addEventListener('click', (event) => {
      panel.classList.toggle('collapsed');
      event.target.textContent = panel.classList.contains('collapsed') ? 'Show' : 'Hide';
    });
    document.getElementById('themeToggle').addEventListener('click', (event) => {
      document.body.classList.toggle('dark');
      event.target.textContent = document.body.classList.contains('dark') ? 'Light theme' : 'Dark theme';
      updateLabelStyle();
    });
    labelsToggle.addEventListener('change', updateLabelStyle);
    document.getElementById('jumpForm').addEventListener('submit', (event) => {
      event.preventDefault();
      jumpToTile({
        z: Number(document.getElementById('jumpZ').value),
        x: Number(document.getElementById('jumpX').value),
        y: Number(document.getElementById('jumpY').value)
      });
    });
    map.on('zoom', updateZoomLabel);
    updateZoomLabel();
    if (data.tiles[0]) fillJump(data.tiles[0]);
  </script>
</body>
</html>
"""
    return html.replace("__GEODOT_DEMO_DATA__", demo_data)
