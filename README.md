# geodot

`geodot` downloads satellite map tiles and can be used as a CLI or as a library from Python, JavaScript, and Rust.

Tiles are saved as:

```text
{out}/tiles/{z}/{x}/{y}.jpg
{out}/manifest.json
{out}/index.html
```

Prepared retrieval datasets are saved as:

```text
{out}/vpr/manifest/tiles.json
{out}/vpr/manifest/patches.json
{out}/vpr/manifest/variants.json
{out}/vpr/config/dataset.json
```

## Install

```bash
cargo install geodot
npm install -g @geodot/cli
npm install @geodot/lib
pip install geodot
```

Run without installing globally:

```bash
npx -y @geodot/cli -x 37.6504907 -y 55.7303 -z 18 -c 1 -r 1
uvx geodot -x 37.6504907 -y 55.7303 -z 18 -c 1 -r 1
cargo install geodot && geodot -x 37.6504907 -y 55.7303 -z 18 -c 1 -r 1
```

During local development:

```bash
python -m pip install -e '.[test]'
npm test
cargo test --manifest-path rust/Cargo.toml
```

Lint and format checks:

```bash
python -m pip install -e '.[test,dev]'
ruff format --check python
ruff check python

npm install
npm run format:check
npm run lint

cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
```

## CLI

```bash
geodot -x 37.6504907 -y 55.7303 -z 18 -c 3 -r 3 -o data -j 16

geodot -x 37.6504907 -y 55.7303 --x2 37.652 --y2 55.7297 -z 18 -o data

geodot -p "37.6504,55.7304;37.6520,55.7304;37.6520,55.7297;37.6504,55.7297" -z 18 -o data

geodot --geojson area.geojson -z 18 -o data

geodot --geojson https://example.com/area.geojson -z 18 -o data

geodot --prepare -o data
```

| Flag | Default | Description |
|------|---------|-------------|
| `-x`, `--lon` | `37.6504907` | Top-left longitude |
| `-y`, `--lat` | `55.7303` | Top-left latitude |
| `--x2`, `--bottom-right-lon` | none | Bottom-right longitude |
| `--y2`, `--bottom-right-lat` | none | Bottom-right latitude |
| `-p`, `--polygon` | none | Closed area as `lon,lat;lon,lat;lon,lat` |
| `-g`, `--geojson` | none | GeoJSON Polygon, Feature, or FeatureCollection file path or URL |
| `-z`, `--zoom` | `18` | Zoom level |
| `-c`, `--cols` | `3` | Tile columns to the right of the top-left tile |
| `-r`, `--rows` | `3` | Tile rows downward from the top-left tile |
| `-o`, `--out` | `data` | Output directory |
| `-j`, `--jobs` | `16` | Concurrent downloads |
| `--prepare` | off | Prepare a virtual retrieval dataset from existing `{out}/tiles/{z}/{x}/{y}.jpg` files |
| `--patch-sizes` | `1,2,3,4` | Mosaic sizes in tiles for `--prepare` |
| `--stride` | `1` | Tile stride for `--prepare` mosaics |
| `--rotations` | `0,90,180,270` | Rotation variants to record for `--prepare` |
| `--no-manifest` | off | Do not write `manifest.json` |
| `--no-demo` | off | Do not write `index.html` |

## Output

For `-o data`, a 3 by 3 download at zoom 18 writes files like this:

```text
data/
├── manifest.json
├── index.html
└── tiles/
    └── 18/
        └── 158488/
            └── 81979.jpg
```

JPEG bytes are written directly from the tile server without re-compression.

## Dataset Preparation

Run preparation after downloading tiles, or against any existing local folder that already follows the tile layout:

```bash
geodot --prepare -o data
geodot --prepare -o data --patch-sizes 1,2,3 --stride 1 --rotations 0,90,180,270
```

Preparation does not download tiles, mutate source JPEGs, compute descriptors, or build an ANN index. It scans `data/tiles/{z}/{x}/{y}.jpg`, validates tile coordinates, and writes virtual dataset manifests for later descriptor/index generation.

The default profile is conservative and geometric:

```text
native 1x1 tiles
overlapping 2x2, 3x3, and 4x4 virtual mosaics
all available zoom levels
rotation variant metadata only
no synthetic weather, season, night, snow, cloud, or haze variants
```

For `-o data`, preparation writes:

```text
data/
└── vpr/
    ├── manifest/
    │   ├── tiles.json
    │   ├── patches.json
    │   └── variants.json
    └── config/
        └── dataset.json
```

`tiles.json` contains one record per discovered source tile with path, z/x/y, image size, bbox, and center lon/lat. `patches.json` contains one record per native tile or complete mosaic window with source tile IDs, pixel size, bbox, center lon/lat, mosaic size, stride, scale profile, and a virtual compose spec. `variants.json` records rotation variants with empty descriptor/index IDs so descriptor extraction can fill them later.

Mosaics are virtual by default. A patch points to source tile IDs and layout instructions instead of writing new JPEGs, keeping storage small and source tiles immutable.

Run `geodot demo` to inspect the downloaded tiles as a MapLibre raster overlay on a satellite base map at their tile coordinates and zoom. The demo serves `{out}/index.html` and reads tiles from `{out}/tiles/{z}/{x}/{y}.jpg`; it does not depend on `manifest.json`. Use the corner opacity control to compare the overlay against the base map. Zooming is disabled because the output folder only contains the downloaded zoom level.

Do not open `index.html` with `file://`; browsers block local tile loading from file origins. Serve the output folder instead:

```bash
geodot demo
```

Then open `http://127.0.0.1:8000/`. Use `geodot demo -o other-dir` for a different output folder. Pass `--no-manifest` when you only want tiles and the demo, or `--no-demo` when you only want tiles and `manifest.json`.

`manifest.json` contains:

```json
{
  "center": { "x": 158488, "y": 81979, "z": 18 },
  "tiles": [
    {
      "tile": { "x": 158488, "y": 81979, "z": 18 },
      "bounds": {
        "lat_min": 55.730012,
        "lon_min": 37.650146,
        "lat_max": 55.730793,
        "lon_max": 37.651520
      },
      "path": "data/tiles/18/158488/81979.jpg",
      "bytes": 12345
    }
  ],
  "failed": []
}
```

## Selection Modes

`geodot` supports three tile selection modes:

1. Grid mode: `-x/-y` selects the top-left tile, then `cols/rows` expands right and down.
2. Rectangle mode: `-x/-y` is the top-left geographic coordinate and `--x2/--y2` is the bottom-right geographic coordinate.
3. Polygon mode: `-p/--polygon` specifies a closed area as semicolon-separated `lon,lat` pairs. The closing edge is implicit.

Polygon downloads include tiles whose center or corners fall inside the polygon, plus tiles containing polygon vertices.

GeoJSON input uses the first Polygon geometry found in a Polygon, MultiPolygon, Feature, or FeatureCollection and uses its exterior ring as the download polygon.

## Python API

```python
from geodot import Coordinate, DownloadOptions, PrepareOptions, download, latlon_to_tile, prepare_dataset, tile_bounds, tile_grid, tile_grid_between, tile_grid_for_polygon

tile = latlon_to_tile(55.7303, 37.6504907, 18)
bounds = tile_bounds(tile)
tiles = tile_grid(55.7303, 37.6504907, zoom=18, cols=3, rows=3)
rectangle = tile_grid_between(55.7303, 37.6504907, 55.7297, 37.652, zoom=18)
polygon = tile_grid_for_polygon([
    Coordinate(lon=37.6504, lat=55.7304),
    Coordinate(lon=37.6520, lat=55.7304),
    Coordinate(lon=37.6520, lat=55.7297),
    Coordinate(lon=37.6504, lat=55.7297),
], zoom=18)

report = download(DownloadOptions(
    lat=55.7303,
    lon=37.6504907,
    bottom_right_lat=55.7297,
    bottom_right_lon=37.652,
    zoom=18,
    cols=3,
    rows=3,
    out="data",
    jobs=16,
    geojson=None,
    no_manifest=False,
    no_demo=False,
))

dataset = prepare_dataset(PrepareOptions(out="data", patch_sizes=(1, 2, 3, 4), stride=1, rotations=(0, 90, 180, 270)))
```

## JavaScript API

```js
import { download, latlonToTile, prepareDataset, tileBounds, tileGrid, tileGridBetween, tileGridForPolygon } from '@geodot/lib';

const tile = latlonToTile(55.7303, 37.6504907, 18);
const bounds = tileBounds(tile);
const tiles = tileGrid(55.7303, 37.6504907, 18, 3, 3);
const rectangle = tileGridBetween(55.7303, 37.6504907, 55.7297, 37.652, 18);
const polygon = tileGridForPolygon([
  { lon: 37.6504, lat: 55.7304 },
  { lon: 37.6520, lat: 55.7304 },
  { lon: 37.6520, lat: 55.7297 },
  { lon: 37.6504, lat: 55.7297 },
], 18);

const report = await download({
  lat: 55.7303,
  lon: 37.6504907,
  bottomRightLat: 55.7297,
  bottomRightLon: 37.652,
  zoom: 18,
  cols: 3,
  rows: 3,
  out: 'data',
  jobs: 16,
  geojson: undefined,
  noManifest: false,
  noDemo: false,
});

const dataset = await prepareDataset({ out: 'data', patchSizes: [1, 2, 3, 4], stride: 1, rotations: [0, 90, 180, 270] });
```

## Rust API

```rust
use geodot::{download, latlon_to_tile, prepare_dataset, tile_bounds, tile_grid, tile_grid_between, tile_grid_for_polygon, Coordinate, DownloadOptions, PrepareOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let tile = latlon_to_tile(55.7303, 37.6504907, 18);
    let bounds = tile_bounds(tile);
    let tiles = tile_grid(55.7303, 37.6504907, 18, 3, 3);
    let rectangle = tile_grid_between(55.7303, 37.6504907, 55.7297, 37.652, 18);
    let polygon = tile_grid_for_polygon(&[
        Coordinate { lon: 37.6504, lat: 55.7304 },
        Coordinate { lon: 37.6520, lat: 55.7304 },
        Coordinate { lon: 37.6520, lat: 55.7297 },
        Coordinate { lon: 37.6504, lat: 55.7297 },
    ], 18);

    let report = download(DownloadOptions {
        lat: 55.7303,
        lon: 37.6504907,
        bottom_right_lat: Some(55.7297),
        bottom_right_lon: Some(37.652),
        polygon: Vec::new(),
        zoom: 18,
        cols: 3,
        rows: 3,
        out: "data".into(),
        jobs: 16,
        geojson: None,
        tile_url_template: None,
        no_manifest: false,
        no_demo: false,
    })
    .await?;

    let dataset = prepare_dataset(PrepareOptions {
        out: "data".into(),
        patch_sizes: vec![1, 2, 3, 4],
        stride: 1,
        rotations: vec![0, 90, 180, 270],
    })?;

    Ok(())
}
```

## Tile Math

Given tile `{ z: 18, x: 158488, y: 81979 }`:

```text
n = 2^z
lon_min = x / n * 360 - 180
lon_max = (x + 1) / n * 360 - 180
lat_max = atan(sinh(pi * (1 - 2y/n))) * 180/pi
lat_min = atan(sinh(pi * (1 - 2(y+1)/n))) * 180/pi
```

Tile bounds are returned as `[lat_min, lon_min, lat_max, lon_max]` fields.

Approximate resolution:

| Zoom | m/px | Tile covers |
|------|------|-------------|
| 18 | 0.34 | 86 x 86 m |
| 16 | 1.36 | 347 x 347 m |
| 14 | 5.45 | 1.4 x 1.4 km |
| 10 | 86 | 22 x 22 km |
| 8 | 345 | 88 x 88 km |
