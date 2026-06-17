from dataclasses import asdict
from pathlib import Path

import geodot.core as core
import pytest
from geodot import (
    Coordinate,
    DownloadedTile,
    DownloadOptions,
    Tile,
    download,
    latlon_to_tile,
    meters_per_pixel,
    polygon_from_geojson,
    tile_bounds,
    tile_grid,
    tile_grid_between,
    tile_grid_for_polygon,
    tile_path,
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
    assert "./tiles/{z}/{x}/{y}.jpg" in demo
    assert "%7Bz%7D" not in demo
    assert "minZoom: data.zoom" in demo
    assert "fitBounds" not in demo


def test_download_can_skip_demo(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(core, "_download_tile", lambda tile: b"x" * 128)

    download(DownloadOptions(cols=1, rows=1, jobs=1, out=tmp_path, no_demo=True))

    assert (tmp_path / "manifest.json").exists()
    assert not (tmp_path / "index.html").exists()
