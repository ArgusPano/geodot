import base64
import json
from dataclasses import asdict
from pathlib import Path

import geodot.core as core
import pytest
from geodot import (
    Coordinate,
    DownloadedTile,
    DownloadOptions,
    PrepareOptions,
    Tile,
    download,
    latlon_to_tile,
    meters_per_pixel,
    polygon_from_geojson,
    prepare_dataset,
    tile_bounds,
    tile_grid,
    tile_grid_between,
    tile_grid_for_polygon,
    tile_path,
    validate_dataset,
)

GEOJSON = {
    "type": "FeatureCollection",
    "features": [
        {
            "type": "Feature",
            "geometry": {
                "type": "Polygon",
                "coordinates": [
                    [
                        [37.6504, 55.7304],
                        [37.6520, 55.7304],
                        [37.6520, 55.7297],
                        [37.6504, 55.7297],
                        [37.6504, 55.7304],
                    ]
                ],
            },
        }
    ],
}

TINY_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
)


def test_latlon_to_tile() -> None:
    assert latlon_to_tile(55.7303, 37.6504907, 18) == Tile(x=158488, y=81979, z=18)


def test_tile_grid() -> None:
    tiles = tile_grid(55.7303, 37.6504907, 18, cols=2, rows=2)
    assert tiles == [
        Tile(x=158488, y=81979, z=18),
        Tile(x=158489, y=81979, z=18),
        Tile(x=158488, y=81980, z=18),
        Tile(x=158489, y=81980, z=18),
    ]


def test_tile_path() -> None:
    assert tile_path("data", Tile(x=1, y=2, z=3)) == Path("data/tiles/3/1/2.jpg")


def test_tile_grid_between_corners() -> None:
    assert tile_grid_between(55.7303, 37.6504907, 55.7297, 37.652, 18) == [
        Tile(x=158488, y=81979, z=18),
        Tile(x=158489, y=81979, z=18),
        Tile(x=158488, y=81980, z=18),
        Tile(x=158489, y=81980, z=18),
    ]


def test_tile_grid_for_polygon() -> None:
    polygon = [
        Coordinate(lon=37.6504, lat=55.7304),
        Coordinate(lon=37.6520, lat=55.7304),
        Coordinate(lon=37.6520, lat=55.7297),
        Coordinate(lon=37.6504, lat=55.7297),
    ]
    assert len(tile_grid_for_polygon(polygon, 18)) == 4


def test_polygon_from_geojson_feature_collection() -> None:
    assert polygon_from_geojson(GEOJSON)[:4] == [
        Coordinate(lon=37.6504, lat=55.7304),
        Coordinate(lon=37.6520, lat=55.7304),
        Coordinate(lon=37.6520, lat=55.7297),
        Coordinate(lon=37.6504, lat=55.7297),
    ]


def test_tile_bounds_and_resolution() -> None:
    bounds = tile_bounds(Tile(x=158488, y=81979, z=18))
    assert bounds.lon_min < 37.6504907 < bounds.lon_max
    assert bounds.lat_min < 55.7303 < bounds.lat_max
    assert 0.2 < meters_per_pixel(55.7303, 18) < 0.4


def test_downloaded_tile_serializes_bounds() -> None:
    tile = Tile(x=1, y=2, z=3)
    data = asdict(DownloadedTile(tile=tile, bounds=tile_bounds(tile), path="data/tiles/3/1/2.jpg", bytes=123))
    assert set(data["bounds"]) == {"lat_min", "lon_min", "lat_max", "lon_max"}


def test_download_rejects_invalid_numeric_options() -> None:
    with pytest.raises(ValueError, match="lat must be a finite number"):
        download(DownloadOptions(lat=float("nan")))
    with pytest.raises(ValueError, match="cols must be an integer at least 1"):
        download(DownloadOptions(cols=0))
    with pytest.raises(ValueError, match="zoom must be an integer 0 to 30"):
        download(DownloadOptions(zoom=31))


def test_download_can_skip_manifest_and_writes_demo(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(core, "_download_tile", lambda tile: b"x" * 128)

    report = download(DownloadOptions(cols=1, rows=1, jobs=1, out=tmp_path, no_manifest=True))

    assert len(report.tiles) == 1
    assert not (tmp_path / "manifest.json").exists()
    demo = (tmp_path / "index.html").read_text(encoding="utf-8")
    assert "maplibregl.Map" in demo
    assert "World_Imagery" in demo
    assert "./tiles/${tile.z}/${tile.x}/${tile.y}.jpg" in demo
    assert "%7Bz%7D" not in demo
    assert "geodot-labels" in demo
    assert "geodot-borders" in demo
    assert "Jump to tile" in demo
    assert "labelsToggle" in demo
    assert "themeToggle" in demo
    assert "togglePanel" in demo
    assert "maxActiveTiles" in demo
    assert "syncVisibleTiles" in demo
    assert "minZoom: Math.max(0, data.zoom - 8)" in demo
    assert "fitBounds" in demo


def test_download_can_skip_demo(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(core, "_download_tile", lambda tile: b"x" * 128)

    download(DownloadOptions(cols=1, rows=1, jobs=1, out=tmp_path, no_demo=True))

    assert (tmp_path / "manifest.json").exists()
    assert not (tmp_path / "index.html").exists()


def test_prepare_dataset_writes_virtual_vpr_manifests(tmp_path: Path) -> None:
    for x in (1, 2):
        for y in (3, 4):
            path = tmp_path / "tiles" / "3" / str(x) / f"{y}.jpg"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"x" * 128)

    report = prepare_dataset(PrepareOptions(out=tmp_path, patch_sizes=(1, 2), rotations=(0, 90)))

    assert report.tiles == 4
    assert report.patches == 5
    assert report.variants == 10
    patches = json.loads((tmp_path / "vpr" / "manifest" / "patches.json").read_text(encoding="utf-8"))
    mosaic = next(patch for patch in patches if patch["mosaic_size_tiles"] == 2)
    assert mosaic["source_x_min"] == 1
    assert mosaic["source_x_max"] == 2
    assert mosaic["source_y_min"] == 3
    assert mosaic["source_y_max"] == 4
    assert mosaic["image_path_or_virtual_spec"]["type"] == "virtual_mosaic"
    dataset = json.loads((tmp_path / "vpr" / "config" / "dataset.json").read_text(encoding="utf-8"))
    assert dataset["mode"] == "virtual"


def test_prepare_dataset_detects_drone_view_captures_and_metadata_only(tmp_path: Path) -> None:
    paths = {
        tmp_path / "tiles" / "18" / "140140" / "97408.jpg": TINY_PNG,
        tmp_path / "drone-view" / "18" / "140140" / "97408.png": TINY_PNG,
        tmp_path / "drone-view" / "18" / "140141" / "97408.jpeg": TINY_PNG,
        tmp_path / "drone-view" / "18" / "140142" / "97408.webp": TINY_PNG,
        tmp_path / "tiles" / "2021_summer" / "18" / "140143" / "97408.jpg": TINY_PNG,
    }
    for path, data in paths.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    report = prepare_dataset(PrepareOptions(out=tmp_path))

    assert report.tiles == 5
    assert all(path.read_bytes() == data for path, data in paths.items())
    assert not (tmp_path / "vpr" / "descriptors").exists()
    assert not (tmp_path / "vpr" / "index").exists()
    assert not list((tmp_path / "vpr").rglob("*.jpg"))
    assert not list((tmp_path / "vpr").rglob("*.png"))
    assert not list((tmp_path / "vpr").rglob("*.webp"))

    tiles = json.loads((tmp_path / "vpr" / "manifest" / "tiles.json").read_text(encoding="utf-8"))
    assert {tile["root"] for tile in tiles} == {"tiles", "drone-view"}
    assert {tile["role"] for tile in tiles} == {"reference", "query"}
    assert {tile["capture_id"] for tile in tiles} == {"default", "2021_summer"}
    assert {tile["extension"] for tile in tiles} == {"jpg", "png", "jpeg", "webp"}
    assert {tile["detected_format"] for tile in tiles} == {"png"}
    assert all(tile["image_width"] == 1 and tile["image_height"] == 1 for tile in tiles)

    patches = json.loads((tmp_path / "vpr" / "manifest" / "patches.json").read_text(encoding="utf-8"))
    assert all(patch["virtual_compose_spec"]["type"] == "virtual_mosaic" for patch in patches)
    assert all(patch["image_written"] is False for patch in patches)
    assert {patch["mosaic_size_tiles"] for patch in patches} >= {1}

    variants = json.loads((tmp_path / "vpr" / "manifest" / "variants.json").read_text(encoding="utf-8"))
    assert any(variant["rotation_degrees"] == 45 for variant in variants)
    assert all(variant["virtual_only"] is True for variant in variants)
    assert all(variant["descriptor_id"] is None and variant["index_id"] is None for variant in variants)

    places = json.loads((tmp_path / "vpr" / "manifest" / "places.json").read_text(encoding="utf-8"))
    matched = next(place for place in places if place["x"] == 140140 and place["y"] == 97408)
    assert matched["place_id"] == "z18_x140140_y97408"
    assert matched["available_roots"] == ["drone-view", "tiles"]
    assert matched["reference_available"] is True
    assert matched["query_available"] is True
    assert matched["reference_tile_ids"] == ["tiles_default_z18_x140140_y97408"]
    assert matched["query_tile_ids"] == ["drone-view_default_z18_x140140_y97408"]
    query_only = next(place for place in places if place["x"] == 140141 and place["y"] == 97408)
    assert query_only["reference_available"] is False
    assert query_only["query_available"] is True

    quality = json.loads((tmp_path / "vpr" / "manifest" / "quality.json").read_text(encoding="utf-8"))
    assert len(quality["tiles"]) == 5
    assert all("recommendation" in item for item in quality["tiles"])

    dataset = json.loads((tmp_path / "vpr" / "config" / "dataset.json").read_text(encoding="utf-8"))
    assert dataset["images_modified"] is False
    assert dataset["generated_images_default"] is False
    assert dataset["descriptors_computed"] is False
    assert dataset["indexes_built"] is False
    assert dataset["auto400m"] is True
    assert dataset["supported_image_extensions"] == [".jpg", ".jpeg", ".png", ".webp"]


def test_prepare_dataset_user_overrides_disable_auto400m(tmp_path: Path) -> None:
    for x in (158488, 158489, 158490):
        path = tmp_path / "tiles" / "18" / str(x) / "81979.jpg"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x" * 128)

    prepare_dataset(PrepareOptions(out=tmp_path, patch_sizes=(1, 2, 3), rotations=(0, 90), auto400m=False))

    dataset = json.loads((tmp_path / "vpr" / "config" / "dataset.json").read_text(encoding="utf-8"))
    variants = json.loads((tmp_path / "vpr" / "manifest" / "variants.json").read_text(encoding="utf-8"))
    assert dataset["patch_sizes"] == [1, 2, 3]
    assert dataset["rotations"] == [0, 90]
    assert dataset["auto400m"] is False
    assert {variant["rotation_degrees"] for variant in variants} == {0, 90}


def test_validate_dataset_reports_consistency_errors_and_warnings(tmp_path: Path) -> None:
    path = tmp_path / "tiles" / "18" / "140140" / "97408.png"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(TINY_PNG)
    prepare_dataset(PrepareOptions(out=tmp_path, patch_sizes=(1,), rotations=(0,), auto400m=False))

    report = validate_dataset(tmp_path)
    assert report.valid is True
    assert report.counts["tiles"] == 1
    assert report.counts["reference_tiles"] == 1

    generated = tmp_path / "vpr" / "preview.png"
    generated.write_bytes(TINY_PNG)
    report = validate_dataset(tmp_path)
    assert report.valid is True
    assert report.counts["warnings"] == 1
    strict = validate_dataset(tmp_path, strict=True)
    assert strict.valid is False
    assert strict.counts["errors"] == 1
    generated.unlink()

    patches_file = tmp_path / "vpr" / "manifest" / "patches.json"
    patches = json.loads(patches_file.read_text(encoding="utf-8"))
    patches[0]["source_tile_ids"] = ["missing_tile"]
    patches_file.write_text(json.dumps(patches), encoding="utf-8")
    assert "references missing tile" in validate_dataset(tmp_path).errors[0]

    patches[0]["source_tile_ids"] = ["tiles_default_z18_x140140_y97408"]
    patches[0]["bbox"] = [10, 10, 9, 11]
    patches_file.write_text(json.dumps(patches), encoding="utf-8")
    assert "invalid bbox" in validate_dataset(tmp_path).errors[0]

    patches[0]["bbox"] = [12.45, 41.9, 12.46, 41.91]
    patches.append({**patches[0]})
    patches_file.write_text(json.dumps(patches), encoding="utf-8")
    assert "duplicate patch id" in validate_dataset(tmp_path).errors[0]


def test_validate_dataset_fails_missing_patch_and_source(tmp_path: Path) -> None:
    path = tmp_path / "tiles" / "18" / "140140" / "97408.png"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(TINY_PNG)
    prepare_dataset(PrepareOptions(out=tmp_path, patch_sizes=(1,), rotations=(0,), auto400m=False))

    variants_file = tmp_path / "vpr" / "manifest" / "variants.json"
    variants = json.loads(variants_file.read_text(encoding="utf-8"))
    variants[0]["patch_id"] = "missing_patch"
    variants_file.write_text(json.dumps(variants), encoding="utf-8")
    assert "references missing patch" in validate_dataset(tmp_path).errors[0]

    variants[0]["patch_id"] = "tiles_default_z18_x140140-140140_y97408-97408_s1"
    variants_file.write_text(json.dumps(variants), encoding="utf-8")
    path.unlink()
    assert "missing source image" in validate_dataset(tmp_path).errors[0]
