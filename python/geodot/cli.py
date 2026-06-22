from __future__ import annotations

import argparse
import base64
import io
import math
import os
import sys
import time
import webbrowser
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

from .core import (
    Coordinate,
    DownloadOptions,
    count_tiles_for_options,
    download,
    latlon_to_tile,
    meters_per_pixel,
    resolve_options,
)


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "demo":
        _demo(sys.argv[2:])
        return

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
    parser.add_argument("--no-manifest", action="store_true", help="do not write manifest.json")
    parser.add_argument("--no-demo", action="store_true", help="do not write index.html")
    args = parser.parse_args()

    start = time.perf_counter()
    options = resolve_options(DownloadOptions(**vars(args)))
    center = latlon_to_tile(args.lat, args.lon, args.zoom)
    selected_tile_count = count_tiles_for_options(options)
    print("\n  geodot - satellite tiles")
    print("  -------------------------------------")
    print(f"  Top-left: {args.lat} {args.lon}")
    print(f"  Tile:    ({center.x}, {center.y})  at zoom {args.zoom}")
    print(f"  Tiles:   {selected_tile_count}")
    print(f"  m/px:    {meters_per_pixel(args.lat, args.zoom):.2f}")
    print(f"  Output:  {args.out}\n")

    report = download(options)
    for item in report.tiles:
        print(f"  ({item.tile.x},{item.tile.y})  {item.bytes:>6} B  {item.path}")
    for tile in report.failed:
        print(f"  ({tile.x},{tile.y})  FAILED")

    print("\n  -------------------------------------")
    print(f"  {len(report.tiles)} tiles  |  {time.perf_counter() - start:.1f}s  |  failed: {len(report.failed)}")


_EMPTY_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
)


def _demo(argv: list[str]) -> None:
    parser = argparse.ArgumentParser(description="Serve a geodot output directory for the HTML demo.")
    parser.add_argument("-o", "--out", default="data", help="output directory to serve")
    parser.add_argument("--host", default="127.0.0.1", help="host to bind")
    parser.add_argument("--port", type=_positive_int, default=8000, help="port to bind")
    parser.add_argument("--no-open", action="store_true", help="do not open the browser")
    args = parser.parse_args(argv)

    class _Handler(SimpleHTTPRequestHandler):
        def __init__(self, *handler_args, **kwargs):
            super().__init__(*handler_args, directory=args.out, **kwargs)

        def send_head(self):
            path = self.translate_path(self.path)
            if not os.path.exists(path) and "/tiles/" in self.path and self.path.endswith(".jpg"):
                self.send_response(200)
                self.send_header("Content-Type", "image/png")
                self.send_header("Content-Length", str(len(_EMPTY_PNG)))
                self.end_headers()
                return io.BytesIO(_EMPTY_PNG)
            return super().send_head()

    server = ThreadingHTTPServer((args.host, args.port), _Handler)
    url = f"http://{args.host}:{args.port}/"
    print(f"Serving {args.out} at {url}")
    if not args.no_open:
        webbrowser.open(url)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


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
