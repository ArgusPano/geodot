from __future__ import annotations

import argparse
import time

from .core import Coordinate, DownloadOptions, download, latlon_to_tile, meters_per_pixel, tiles_for_options


def main() -> None:
    parser = argparse.ArgumentParser(description="Download satellite map tiles (256x256 px).")
    parser.add_argument("-y", "--lat", type=float, default=55.7303, help="top-left latitude")
    parser.add_argument("-x", "--lon", type=float, default=37.6504907, help="top-left longitude")
    parser.add_argument("--y2", "--bottom-right-lat", dest="bottom_right_lat", type=float, help="bottom-right latitude")
    parser.add_argument(
        "--x2", "--bottom-right-lon", dest="bottom_right_lon", type=float, help="bottom-right longitude"
    )
    parser.add_argument("-p", "--polygon", type=_parse_polygon, help="closed area as 'lon,lat;lon,lat;lon,lat'")
    parser.add_argument("-z", "--zoom", type=int, default=18, help="zoom level (1-22)")
    parser.add_argument("-c", "--cols", type=int, default=3, help="tile columns to the right of center")
    parser.add_argument("-r", "--rows", type=int, default=3, help="tile rows downward from center")
    parser.add_argument("-o", "--out", default="data", help="output directory")
    parser.add_argument("-j", "--jobs", type=int, default=16, help="max concurrent downloads")
    args = parser.parse_args()

    start = time.perf_counter()
    options = DownloadOptions(**vars(args))
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
    return points


if __name__ == "__main__":
    main()
