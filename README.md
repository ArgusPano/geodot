# geodot

`geodot` downloads satellite map tiles and prepares metadata-only visual place recognition (VPR) datasets. It can be used as a command-line tool or as a library from Python, JavaScript, and Rust.

## Contents

- [What It Does](#what-it-does)
- [Install](#install)
- [Quick Start](#quick-start)
- [CLI Reference](#cli-reference)
- [Output Layout](#output-layout)
- [Dataset Preparation](#dataset-preparation)
- [Dataset Contract](#dataset-contract)
- [APIs](#apis)
- [Tile Math](#tile-math)
- [Development](#development)

## What It Does

| Capability | Description |
| --- | --- |
| Tile download | Downloads 256x256 satellite tiles for a grid, rectangle, polygon, or GeoJSON area. |
| Demo page | Writes `index.html` for inspecting downloaded tiles over a MapLibre satellite basemap. |
| VPR preparation | Scans local imagery and writes virtual patch, variant, place, and quality manifests. |
| Validation | Checks prepared manifests, IDs, references, image paths, bboxes, and metadata-only flags. |
| Debug rendering | Renders one prepared single-tile patch or variant preview on demand. |

`geodot` writes downloaded tiles directly from the tile server without re-compression.

## Install

Choose the ecosystem that matches how you want to run or embed `geodot`.

| Toolchain | Command | Provides |
| --- | --- | --- |
| Rust | `cargo install geodot` | `geodot` CLI and Rust crate |
| npm CLI | `npm install -g @geodot/cli` | `geodot` CLI |
| npm library | `npm install @geodot/lib` | JavaScript library |
| Python | `pip install geodot` | `geodot` CLI and Python package |

Supported runtimes are Node.js 18 or newer, Python 3.10 or newer, and Rust 1.85 or newer.

Run without a global npm or Python install:

```bash
npx -y @geodot/cli -x 37.6504907 -y 55.7303 -z 18 -c 1 -r 1
uvx geodot -x 37.6504907 -y 55.7303 -z 18 -c 1 -r 1
```

Rust uses `cargo install geodot` to install the CLI binary before running `geodot`.

## Quick Start

Show CLI help or the installed package version:

```bash
geodot
geodot --help
geodot --version
```

Running `geodot` without arguments prints help and does not download tiles.

Download commands are explicit. For example, download a 3 by 3 grid of tiles at zoom 18:

```bash
geodot -x 37.6504907 -y 55.7303 -z 18 -c 3 -r 3 -o data -j 16
```

Serve the demo page:

```bash
geodot demo -o data
```

Then open `http://127.0.0.1:8000/`.

Prepare a metadata-only VPR dataset from downloaded tiles:

```bash
geodot --prepare -o data
geodot validate -o data
```

For a normal tile download, `-o data` writes:

```text
data/
├── manifest.json
├── index.html
└── tiles/
    └── {z}/
        └── {x}/
            └── {y}.jpg
```

For dataset preparation, `-o data` writes:

```text
data/
└── vpr/
    ├── manifest/
    │   ├── tiles.json
    │   ├── patches.json
    │   ├── variants.json
    │   ├── places.json
    │   └── quality.json
    └── config/
        └── dataset.json
```

## CLI Reference

### Selection Modes

| Mode | Required arguments | Behavior |
| --- | --- | --- |
| Grid | `-x`, `-y`, `-z`, `-c`, `-r` | Selects the tile containing the longitude/latitude and expands right/down by columns and rows. |
| Rectangle | `-x`, `-y`, `--x2`, `--y2`, `-z` | Selects all tiles between the two geographic corners. Argument order is top-left lon/lat and bottom-right lon/lat. |
| Polygon | `-p`, `-z` | Selects tiles whose center or corners are inside the polygon, plus tiles containing polygon vertices. |
| GeoJSON | `--geojson`, `-z` | Reads the first Polygon geometry from a Polygon, MultiPolygon, Feature, or FeatureCollection. |

Polygon coordinates use `lon,lat` pairs. The closing edge is implicit.

### Examples

| Task | Command |
| --- | --- |
| Grid download | `geodot -x 37.6504907 -y 55.7303 -z 18 -c 3 -r 3 -o data -j 16` |
| Rectangle download | `geodot -x 37.6504907 -y 55.7303 --x2 37.652 --y2 55.7297 -z 18 -o data` |
| Polygon download | `geodot -p "37.6504,55.7304;37.6520,55.7304;37.6520,55.7297;37.6504,55.7297" -z 18 -o data` |
| Local GeoJSON download | `geodot --geojson area.geojson -z 18 -o data` |
| Remote GeoJSON download | `geodot --geojson https://example.com/area.geojson -z 18 -o data` |
| Remote country GeoJSON download | `geodot -g "https://raw.githubusercontent.com/georgique/world-geojson/refs/heads/develop/countries/vatican.json" --out data` |
| Prepare existing local tiles | `geodot --prepare -o data` |
| Download GeoJSON area and prepare | `geodot --prepare --geojson https://example.com/area.geojson -z 18 -o data` |
| Validate prepared dataset | `geodot validate -o data` |
| Validate with warnings as errors | `geodot validate -o data --strict` |
| Render a patch preview | `geodot render -o data --patch-id <patch_id> --out preview.jpg` |
| Render a variant preview | `geodot render -o data --variant-id <variant_id> --out preview.jpg` |
| Serve the demo | `geodot demo -o data` |

### Options

| Flag | Default | Description |
| --- | --- | --- |
| `-h`, `--help` | n/a | Print help and exit. Running `geodot` without arguments also prints help. |
| `-v`, `--version` | n/a | Print the installed package version and exit. |
| `-x`, `--lon` | none | Longitude for the grid/rectangle starting point. Required unless `--polygon` or `--geojson` is used. |
| `-y`, `--lat` | none | Latitude for the grid/rectangle starting point. Required unless `--polygon` or `--geojson` is used. |
| `--x2`, `--bottom-right-lon` | none | Rectangle bottom-right longitude. |
| `--y2`, `--bottom-right-lat` | none | Rectangle bottom-right latitude. |
| `-p`, `--polygon` | none | Polygon as `lon,lat;lon,lat;lon,lat`. |
| `-g`, `--geojson` | none | GeoJSON Polygon, MultiPolygon, Feature, or FeatureCollection file path or URL. |
| `-z`, `--zoom` | `18` | Web Mercator zoom level, `0` through `30`. |
| `-c`, `--cols` | `3` | Tile columns to select in grid mode. |
| `-r`, `--rows` | `3` | Tile rows to select in grid mode. |
| `-o`, `--out` | `data` | Output directory. |
| `-j`, `--jobs` | `16` | Concurrent downloads. |
| `--prepare` | off | Prepare a VPR dataset. With selection arguments, downloads first and then prepares. Without selection arguments, scans an existing local dataset. |
| `--patch-sizes` | `1,2,4,auto400m` | Mosaic sizes in tiles for `--prepare`. Passing explicit values disables `auto400m`. |
| `--stride` | `1` | Tile stride for prepared mosaics. |
| `--rotations` | `0,45,90,135,180,225,270,315` | Rotation variants to record in metadata. |
| `--no-manifest` | off | Do not write `manifest.json`. |
| `--no-demo` | off | Do not write `index.html`. |

### Subcommands

| Subcommand | Purpose | Notes |
| --- | --- | --- |
| `geodot demo -o data` | Serves `{out}/index.html` over HTTP. | Do not open `index.html` with `file://`; browsers block local tile loading from file origins. |
| `geodot validate -o data` | Validates a prepared VPR dataset. | Exit code `0` means valid, `1` means validation errors, and `2` means missing dataset/manifests or invalid command usage. |
| `geodot render -o data --patch-id <id> --out preview.jpg` | Writes one debug preview for a patch. | Currently supports one-source-tile virtual patches. |
| `geodot render -o data --variant-id <id> --out preview.jpg` | Writes one debug preview for a variant. | Does not batch-render the dataset or compute descriptors. |

`validate` checks that manifests parse, IDs are unique, references resolve, source images exist, bboxes and image dimensions are valid, dataset flags remain metadata-only, and preparation did not create images under `vpr/`. Warnings do not fail validation unless `--strict` is passed.

### Tile URL Template

Set `GEODOT_TILE_URL_TEMPLATE` to use a different tile source:

```bash
GEODOT_TILE_URL_TEMPLATE='https://example.com/tiles/{z}/{x}/{y}.jpg' geodot -x 37.6504907 -y 55.7303 -z 18
```

The template can contain `{sub}`, `{x}`, `{y}`, and `{z}`. `{sub}` is one of `mt0`, `mt1`, `mt2`, or `mt3`.

## Output Layout

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

`manifest.json` contains the selected center tile, downloaded tile records, and failed tile records:

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

The demo page reads tiles from `{out}/tiles/{z}/{x}/{y}.jpg` and overlays them on a satellite basemap. Zooming is disabled because the output folder only contains the downloaded zoom level. Use the opacity control to compare the downloaded overlay with the basemap.

## Dataset Preparation

Preparation is metadata-only. It scans source images, validates coordinates, creates virtual patches/mosaics/variants, and writes manifests. It does not mutate source images, write cropped/rotated/resized/circular-masked/augmented images, compute descriptors, train models, or build ANN indexes.

Run preparation while downloading, after downloading tiles, or against any existing local folder that already follows the tile layout:

```bash
geodot --prepare --geojson https://example.com/area.geojson -z 18 -o data
geodot --prepare -o data
geodot --prepare -o data --patch-sizes 1,2,3 --stride 1 --rotations 0,90,180,270
```

`geodot --prepare --geojson ...` uses the normal GeoJSON tile download logic first, then immediately prepares the dataset. `geodot --prepare -o data` only scans an existing local dataset and does not download anything.

### Source Image Roots

Preparation auto-detects these layouts:

| Layout | Role | Capture ID |
| --- | --- | --- |
| `data/tiles/{z}/{x}/{y}.jpg` | reference | `default` |
| `data/tiles/{z}/{x}/{y}.jpeg` | reference | `default` |
| `data/tiles/{z}/{x}/{y}.png` | reference | `default` |
| `data/tiles/{z}/{x}/{y}.webp` | reference | `default` |
| `data/drone-view/{z}/{x}/{y}.jpg` | query | `default` |
| `data/drone-view/{z}/{x}/{y}.jpeg` | query | `default` |
| `data/drone-view/{z}/{x}/{y}.png` | query | `default` |
| `data/drone-view/{z}/{x}/{y}.webp` | query | `default` |
| `data/tiles/{capture_id}/{z}/{x}/{y}.jpg` | reference | folder name |
| `data/drone-view/{capture_id}/{z}/{x}/{y}.jpg` | query | folder name |

Supported source extensions are `.jpg`, `.jpeg`, `.png`, and `.webp`. Preparation reads image headers to record dimensions and detected format, but it does not require the extension to match the encoded bytes and never rewrites or converts source images.

`drone-view/{z}/{x}/{y}` assumes the drone image is already georeferenced to the corresponding Web Mercator tile location, for example by orthorectification or manual assignment to a known tile footprint. Zoom is a rough scale bucket, not UAV altitude. True altitude depends on camera FOV, sensor size, image resolution, pitch, terrain height, GSD, and whether the image is nadir or oblique. True UAV altitude and pose need optional metadata outside this filename convention.

Manual drone-view example using an externally supplied PNG:

```bash
mkdir -p data/drone-view/18/140140
curl -L "https://i.imgur.com/Aw7aFQb.png" -o data/drone-view/18/140140/97408.png
geodot --prepare -o data
```

Use `.png` for externally supplied PNG examples instead of saving PNG bytes under a `.jpg` filename.

### Default Preparation Profile

| Setting | Default |
| --- | --- |
| Native patches | 1x1 tiles |
| Virtual mosaics | Overlapping 2x2 and 4x4 mosaics |
| Automatic scale | `auto400m`, clamped to 1-8 tiles |
| Zoom handling | All discovered zoom levels |
| Rotation variants | Metadata only |
| Circular crops | Metadata only |
| Appearance augmentation | None |

`auto400m` estimates tile ground width at each patch latitude and zoom, chooses the nearest integer mosaic size for roughly 400 meters, clamps it to 1-8 tiles, and deduplicates it with explicit patch sizes. At zoom 18 near the default latitude this is usually around a 5x5 patch, matching the scale of LASED-like aerial VPR datasets.

### Prepared Files

| File | Contents |
| --- | --- |
| `vpr/manifest/tiles.json` | One record per discovered source tile with tile ID, root, capture ID, role, path, z/x/y, image size, byte size, bbox, center lon/lat, and validity. |
| `vpr/manifest/patches.json` | One record per native tile or complete mosaic window with place ID, source tile IDs, pixel size, bbox, center lon/lat, estimated ground size, circular-crop availability, and a virtual compose spec. |
| `vpr/manifest/variants.json` | Virtual rotations with empty descriptor/index IDs for downstream descriptor extraction. |
| `vpr/manifest/places.json` | Matching reference/query imagery grouped by z/x/y location. |
| `vpr/manifest/quality.json` | Conservative, non-destructive low-information labels from cheap image/file statistics. |
| `vpr/config/dataset.json` | Dataset profile, schema version, detected roots, zoom levels, preparation settings, and counts. |

Mosaics, rotations, and circular crops are virtual by default. A patch points to source tile IDs and layout instructions instead of writing new JPEGs, keeping storage small and source tiles immutable.

## Dataset Contract

`geodot` creates image datasets and virtual patch metadata only. It intentionally does not extract descriptors, train models, build ANN indexes, or choose model-specific preprocessing.

External descriptor tools should treat these IDs as stable dataset IDs:

| ID | Meaning |
| --- | --- |
| `tile_id` | Source image tile identity. |
| `place_id` | Geographic z/x/y place identity. |
| `patch_id` | Native tile or virtual mosaic patch identity. |
| `variant_id` | Patch plus virtual transform identity. |

The usual downstream Python flow is:

```python
from geodot import load_dataset, render_variant

dataset = load_dataset("data")
image = render_variant(dataset, variant_id)
# pass image bytes to DINOv3 / ResNet / VLAD / GeM outside geodot
```

Descriptor outputs should live outside `geodot`, or in a separate downstream folder, and reference `variant_id`. Re-running descriptors with DINOv3 SAT, ResNet, VLAD, GeM, NetVLAD, steerable CNNs, or another model should not require changing source images or regenerating the prepared dataset.

## APIs

### Python API

```python
from geodot import (
    Coordinate,
    DownloadOptions,
    PrepareOptions,
    download,
    latlon_to_tile,
    prepare_dataset,
    tile_bounds,
    tile_grid,
    tile_grid_between,
    tile_grid_for_polygon,
)

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

dataset = prepare_dataset(PrepareOptions(out="data"))
```

### JavaScript API

```js
import {
  download,
  latlonToTile,
  prepareDataset,
  tileBounds,
  tileGrid,
  tileGridBetween,
  tileGridForPolygon,
} from '@geodot/lib';

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

const dataset = await prepareDataset({ out: 'data' });
```

### Rust API

```rust
use geodot::{
    Coordinate, DownloadOptions, PrepareOptions, download, latlon_to_tile, prepare_dataset,
    tile_bounds, tile_grid, tile_grid_between, tile_grid_for_polygon,
};

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
        geojson: None,
        zoom: 18,
        cols: 3,
        rows: 3,
        out: "data".into(),
        jobs: 16,
        tile_url_template: None,
        no_manifest: false,
        no_demo: false,
    })
    .await?;

    let dataset = prepare_dataset(PrepareOptions {
        out: "data".into(),
        patch_sizes: vec![1, 2, 4],
        stride: 1,
        rotations: vec![0, 45, 90, 135, 180, 225, 270, 315],
        auto400m: true,
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

Tile bounds are represented as `lat_min`, `lon_min`, `lat_max`, and `lon_max` fields.

Approximate resolution near the default latitude (`55.7303`):

| Zoom | m/px | Tile covers |
| --- | --- | --- |
| 18 | 0.34 | 86 x 86 m |
| 16 | 1.36 | 347 x 347 m |
| 14 | 5.45 | 1.4 x 1.4 km |
| 10 | 86 | 22 x 22 km |
| 8 | 345 | 88 x 88 km |

## Development

Install local Python test dependencies and run the Python tests:

```bash
python -m pip install -e '.[test,dev]'
python -m pytest
```

Run JavaScript and Rust tests:

```bash
npm install
npm test
cargo test --manifest-path rust/Cargo.toml
```

Run lint and format checks:

```bash
python -m pip install -e '.[test,dev]'
ruff format --check python
ruff check python
npm run format:check
npm run lint
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
```

Install the local pre-commit hook to run format, lint, and tests before every commit:

```bash
cp scripts/pre-commit .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit
```
