from dataclasses import asdict
from pathlib import Path

from geodot import (
    Coordinate,
    DownloadedTile,
    Tile,
    latlon_to_tile,
    meters_per_pixel,
    tile_bounds,
    tile_grid,
    tile_grid_between,
    tile_grid_for_polygon,
    tile_path,
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


def test_tile_bounds_and_resolution() -> None:
    bounds = tile_bounds(Tile(x=158488, y=81979, z=18))
    assert bounds.lon_min < 37.6504907 < bounds.lon_max
    assert bounds.lat_min < 55.7303 < bounds.lat_max
    assert 0.2 < meters_per_pixel(55.7303, 18) < 0.4


def test_downloaded_tile_serializes_bounds() -> None:
    tile = Tile(x=1, y=2, z=3)
    data = asdict(DownloadedTile(tile=tile, bounds=tile_bounds(tile), path="data/tiles/3/1/2.jpg", bytes=123))
    assert set(data["bounds"]) == {"lat_min", "lon_min", "lat_max", "lon_max"}
