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


class TileHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
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
