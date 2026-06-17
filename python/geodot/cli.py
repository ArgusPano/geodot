from __future__ import annotations

import argparse
import math
import time

from .core import (
    Coordinate,
    DownloadOptions,
    download,
    latlon_to_tile,
    meters_per_pixel,
    resolve_options,
    tiles_for_options,
)


def main() -> None:
    parser = argparse.ArgumentParser(description="Download satellite map tiles (256x256 px).")
    parser.add_argument("-y", "--lat", type=_finite_float, default=55.7303, help="top-left latitude")
    parser.add_argument("-x", "--lon", type=_finite_float, default=37.6504907, help="top-left longitude")
    parser.add_argument(
        "--y2", "--bottom-right-lat", dest="bottom_right_lat", type=_finite_float, help="bottom-right latitude"
    )
    parser.add_argument(
        "--x2", "--bottom-right-lon", dest="bottom_right_lon", type=_finite_float, help="bottom-right longitude"
    )
    parser.add_argument("-p", "--polygon", type=_parse_polygon, help="closed area as 'lon,lat;lon,lat;lon,lat'")
    parser.add_argument("-g", "--geojson", help="GeoJSON Polygon, Feature, or FeatureCollection file path or URL")
    parser.add_argument("-z", "--zoom", type=_zoom, default=18, help="zoom level (0-30)")
    parser.add_argument("-c", "--cols", type=_positive_int, default=3, help="tile columns to the right of center")
    parser.add_argument("-r", "--rows", type=_positive_int, default=3, help="tile rows downward from center")
    parser.add_argument("-o", "--out", default="data", help="output directory")
    parser.add_argument("-j", "--jobs", type=_positive_int, default=16, help="max concurrent downloads")
    args = parser.parse_args()

    start = time.perf_counter()
    options = resolve_options(DownloadOptions(**vars(args)))
    center = latlon_to_tile(args.lat, args.lon, args.zoom)
    selected_tiles = tiles_for_options(options)
    print("\n  geodot - satellite tiles")
    print("  -------------------------------------")
    print(f"  Top-left: {args.lat} {args.lon}")
    print(f"  Tile:    ({center.x}, {center.y})  at zoom {args.zoom}")
    print(f"  Tiles:   {len(selected_tiles)}")
    print(f"  m/px:    {meters_per_pixel(args.lat, args.zoom):.2f}")
    print(f"  Output:  {args.out}\n")

    report = download(options)
    for item in report.tiles:
        print(f"  ({item.tile.x},{item.tile.y})  {item.bytes:>6} B  {item.path}")
    for tile in report.failed:
        print(f"  ({tile.x},{tile.y})  FAILED")

    print("\n  -------------------------------------")
    print(f"  {len(report.tiles)} tiles  |  {time.perf_counter() - start:.1f}s  |  failed: {len(report.failed)}")


def _parse_polygon(value: str) -> list[Coordinate]:
    points = []
    for item in value.split(";"):
        lon, lat = item.split(",", 1)
        points.append(Coordinate(lon=float(lon), lat=float(lat)))
    if len(points) < 3:
        raise argparse.ArgumentTypeError("polygon requires at least three lon,lat pairs")
    if any(not math.isfinite(point.lon) or not math.isfinite(point.lat) for point in points):
        raise argparse.ArgumentTypeError("polygon coordinates must be finite numbers")
    return points


def _finite_float(value: str) -> float:
    try:
        number = float(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"invalid number: {value}") from error
    if not math.isfinite(number):
        raise argparse.ArgumentTypeError("must be a finite number")
    return number


def _positive_int(value: str) -> int:
    try:
        number = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"invalid integer: {value}") from error
    if number < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return number


def _zoom(value: str) -> int:
    try:
        number = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"invalid integer: {value}") from error
    if number < 0 or number > 30:
        raise argparse.ArgumentTypeError("must be from 0 to 30")
    return number


if __name__ == "__main__":
    main()
