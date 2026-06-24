from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from geodot import DownloadOptions, download

TILE_BYTES = b"x" * 128
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


class TileHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == "/area.geojson":
            body = json.dumps(GEOJSON).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/geo+json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(200)
        self.send_header("Content-Type", "image/jpeg")
        self.send_header("Content-Length", str(len(TILE_BYTES)))
        self.end_headers()
        self.wfile.write(TILE_BYTES)

    def log_message(self, format: str, *args: object) -> None:
        return


@contextmanager
def tile_server():
    server = ThreadingHTTPServer(("127.0.0.1", 0), TileHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/{{z}}/{{x}}/{{y}}.jpg"
    finally:
        server.shutdown()
        thread.join()


def assert_download_output(out: Path) -> None:
    manifest = json.loads((out / "manifest.json").read_text(encoding="utf-8"))
    tile = manifest["tiles"][0]
    assert tile["bytes"] == len(TILE_BYTES)
    assert set(tile["bounds"]) == {"lat_min", "lon_min", "lat_max", "lon_max"}
    assert (out / "tiles" / "18" / str(tile["tile"]["x"]) / f"{tile['tile']['y']}.jpg").read_bytes() == TILE_BYTES


def test_library_download_e2e(tmp_path: Path, monkeypatch) -> None:
    with tile_server() as template:
        monkeypatch.setenv("GEODOT_TILE_URL_TEMPLATE", template)
        report = download(DownloadOptions(lat=55.7303, lon=37.6504907, zoom=18, cols=1, rows=1, out=tmp_path, jobs=1))

    assert len(report.tiles) == 1
    assert report.failed == []
    assert_download_output(tmp_path)


def test_library_download_accepts_local_geojson(tmp_path: Path, monkeypatch) -> None:
    geojson_file = tmp_path / "area.geojson"
    geojson_file.write_text(json.dumps(GEOJSON), encoding="utf-8")
    with tile_server() as template:
        monkeypatch.setenv("GEODOT_TILE_URL_TEMPLATE", template)
        report = download(DownloadOptions(geojson=geojson_file, zoom=18, out=tmp_path, jobs=1))

    assert len(report.tiles) == 4
    assert report.failed == []
    assert_download_output(tmp_path)


def test_cli_download_e2e(tmp_path: Path) -> None:
    with tile_server() as template:
        env = {**os.environ, "GEODOT_TILE_URL_TEMPLATE": template}
        subprocess.run(
            [
                sys.executable,
                "-m",
                "geodot.cli",
                "-x",
                "37.6504907",
                "-y",
                "55.7303",
                "-z",
                "18",
                "-c",
                "1",
                "-r",
                "1",
                "-j",
                "1",
                "-o",
                str(tmp_path),
            ],
            check=True,
            capture_output=True,
            env=env,
            text=True,
        )

    assert_download_output(tmp_path)


def test_cli_prepare_existing_tiles(tmp_path: Path) -> None:
    for x in (1, 2):
        for y in (3, 4):
            path = tmp_path / "tiles" / "3" / str(x) / f"{y}.jpg"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(TILE_BYTES)

    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "geodot.cli",
            "--prepare",
            "-o",
            str(tmp_path),
            "--patch-sizes",
            "1,2",
            "--rotations",
            "0,90",
        ],
        check=True,
        capture_output=True,
        text=True,
    )

    assert "dataset preparation" in result.stdout
    patches = json.loads((tmp_path / "vpr" / "manifest" / "patches.json").read_text(encoding="utf-8"))
    variants = json.loads((tmp_path / "vpr" / "manifest" / "variants.json").read_text(encoding="utf-8"))
    assert len(patches) == 5
    assert len(variants) == 10


def test_cli_download_accepts_geojson_url(tmp_path: Path) -> None:
    with tile_server() as template:
        geojson_url = template.split("/{z}", 1)[0] + "/area.geojson"
        env = {**os.environ, "GEODOT_TILE_URL_TEMPLATE": template}
        subprocess.run(
            [
                sys.executable,
                "-m",
                "geodot.cli",
                "--geojson",
                geojson_url,
                "-z",
                "18",
                "-j",
                "1",
                "-o",
                str(tmp_path),
            ],
            check=True,
            capture_output=True,
            env=env,
            text=True,
        )

    assert_download_output(tmp_path)


def test_cli_prepare_with_geojson_downloads_then_prepares(tmp_path: Path) -> None:
    with tile_server() as template:
        geojson_url = template.split("/{z}", 1)[0] + "/area.geojson"
        env = {**os.environ, "GEODOT_TILE_URL_TEMPLATE": template}
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "geodot.cli",
                "--prepare",
                "--geojson",
                geojson_url,
                "-z",
                "18",
                "-j",
                "1",
                "-o",
                str(tmp_path),
            ],
            check=True,
            capture_output=True,
            env=env,
            text=True,
        )

    assert "satellite tiles" in result.stdout
    assert "dataset preparation" in result.stdout
    assert_download_output(tmp_path)
    assert (tmp_path / "vpr" / "manifest" / "tiles.json").exists()
    assert (tmp_path / "vpr" / "manifest" / "places.json").exists()
    assert (tmp_path / "vpr" / "manifest" / "quality.json").exists()


def test_cli_demo_help() -> None:
    result = subprocess.run(
        [sys.executable, "-m", "geodot.cli", "demo", "--help"],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0
    assert "--port" in result.stdout
    assert "--no-open" in result.stdout


def test_cli_rejects_invalid_numeric_options() -> None:
    result = subprocess.run(
        [sys.executable, "-m", "geodot.cli", "-j", "https://example.com/area.geojson"],
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0
    assert "invalid integer" in result.stderr

    result = subprocess.run(
        [sys.executable, "-m", "geodot.cli", "--lat", "nan"],
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0
    assert "must be a finite number" in result.stderr
