use anyhow::{Result, bail};
use futures::stream::{self, StreamExt};
use rand::seq::SliceRandom;
use serde::Serialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub const TILE_SIZE: u32 = 256;
pub const MAX_ZOOM: u32 = 30;

const USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
];

const SUBDOMAINS: &[&str] = &["mt0", "mt1", "mt2", "mt3"];
const TILE_URL_TEMPLATE_ENV: &str = "GEODOT_TILE_URL_TEMPLATE";

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub lat: f64,
    pub lon: f64,
    pub bottom_right_lat: Option<f64>,
    pub bottom_right_lon: Option<f64>,
    pub polygon: Vec<Coordinate>,
    pub geojson: Option<String>,
    pub zoom: u32,
    pub cols: u32,
    pub rows: u32,
    pub out: PathBuf,
    pub jobs: usize,
    pub tile_url_template: Option<String>,
    pub no_manifest: bool,
    pub no_demo: bool,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            lat: 55.7303,
            lon: 37.6504907,
            bottom_right_lat: None,
            bottom_right_lon: None,
            polygon: Vec::new(),
            geojson: None,
            zoom: 18,
            cols: 3,
            rows: 3,
            out: PathBuf::from("data"),
            jobs: 16,
            tile_url_template: None,
            no_manifest: false,
            no_demo: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Coordinate {
    pub lon: f64,
    pub lat: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TileBounds {
    pub lat_min: f64,
    pub lon_min: f64,
    pub lat_max: f64,
    pub lon_max: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadedTile {
    pub tile: Tile,
    pub bounds: TileBounds,
    pub path: PathBuf,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadReport {
    pub center: Tile,
    pub tiles: Vec<DownloadedTile>,
    pub failed: Vec<Tile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionProgress {
    pub scanned: usize,
    pub selected: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    pub completed: usize,
    pub downloaded: usize,
    pub failed: usize,
    pub tile: Tile,
}

pub fn latlon_to_tile(lat: f64, lon: f64, z: u32) -> Tile {
    let n = 2u64.pow(z) as f64;
    let x = ((lon + 180.0) / 360.0 * n).floor() as u32;
    let lat_rad = lat.to_radians();
    let y = ((1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n)
        .floor() as u32;
    Tile { x, y, z }
}

pub fn tile_bounds(tile: Tile) -> TileBounds {
    let n = 2u64.pow(tile.z) as f64;
    let lon_min = tile.x as f64 / n * 360.0 - 180.0;
    let lon_max = (tile.x + 1) as f64 / n * 360.0 - 180.0;
    let lat_max = ((std::f64::consts::PI * (1.0 - 2.0 * tile.y as f64 / n)).sinh())
        .atan()
        .to_degrees();
    let lat_min = ((std::f64::consts::PI * (1.0 - 2.0 * (tile.y + 1) as f64 / n)).sinh())
        .atan()
        .to_degrees();
    TileBounds {
        lat_min,
        lon_min,
        lat_max,
        lon_max,
    }
}

pub fn meters_per_pixel(lat: f64, z: u32) -> f64 {
    let world_pixels = TILE_SIZE as f64 * (2u64.pow(z) as f64);
    40_075_016.686 / world_pixels * lat.to_radians().cos()
}

pub fn tile_grid(lat: f64, lon: f64, zoom: u32, cols: u32, rows: u32) -> Vec<Tile> {
    tile_grid_iter(lat, lon, zoom, cols, rows).collect()
}

pub fn tile_grid_between(
    top_left_lat: f64,
    top_left_lon: f64,
    bottom_right_lat: f64,
    bottom_right_lon: f64,
    zoom: u32,
) -> Vec<Tile> {
    let a = latlon_to_tile(top_left_lat, top_left_lon, zoom);
    let b = latlon_to_tile(bottom_right_lat, bottom_right_lon, zoom);
    tiles_in_range(a.x.min(b.x), a.x.max(b.x), a.y.min(b.y), a.y.max(b.y), zoom).collect()
}

pub fn tile_grid_for_polygon(points: &[Coordinate], zoom: u32) -> Vec<Tile> {
    tile_grid_for_polygon_iter(points, zoom).collect()
}

pub fn tiles_for_options(options: &DownloadOptions) -> Vec<Tile> {
    tile_iter_for_options(options).collect()
}

pub fn count_tiles_for_options(options: &DownloadOptions) -> usize {
    tile_iter_for_options(options).count()
}

pub fn count_tiles_for_options_with_progress(
    options: &DownloadOptions,
    mut on_progress: impl FnMut(SelectionProgress),
) -> usize {
    if options.polygon.len() >= 3 {
        return count_polygon_tiles_with_progress(&options.polygon, options.zoom, on_progress);
    }
    if let (Some(lat2), Some(lon2)) = (options.bottom_right_lat, options.bottom_right_lon) {
        let a = latlon_to_tile(options.lat, options.lon, options.zoom);
        let b = latlon_to_tile(lat2, lon2, options.zoom);
        return count_range_tiles_with_progress(
            a.x.min(b.x),
            a.x.max(b.x),
            a.y.min(b.y),
            a.y.max(b.y),
            on_progress,
        );
    }
    let total = options.cols as usize * options.rows as usize;
    for scanned in 1..=total {
        on_progress(SelectionProgress {
            scanned,
            selected: scanned,
            total,
        });
    }
    total
}

fn tile_iter_for_options(options: &DownloadOptions) -> Box<dyn Iterator<Item = Tile> + '_> {
    if options.polygon.len() >= 3 {
        return tile_grid_for_polygon_iter(&options.polygon, options.zoom);
    }
    if let (Some(lat2), Some(lon2)) = (options.bottom_right_lat, options.bottom_right_lon) {
        let a = latlon_to_tile(options.lat, options.lon, options.zoom);
        let b = latlon_to_tile(lat2, lon2, options.zoom);
        return Box::new(tiles_in_range(
            a.x.min(b.x),
            a.x.max(b.x),
            a.y.min(b.y),
            a.y.max(b.y),
            options.zoom,
        ));
    }
    Box::new(tile_grid_iter(
        options.lat,
        options.lon,
        options.zoom,
        options.cols,
        options.rows,
    ))
}

fn tile_grid_for_polygon_iter(
    points: &[Coordinate],
    zoom: u32,
) -> Box<dyn Iterator<Item = Tile> + '_> {
    if points.len() < 3 {
        return Box::new(std::iter::empty());
    }
    let min_lat = points.iter().map(|p| p.lat).fold(f64::INFINITY, f64::min);
    let max_lat = points
        .iter()
        .map(|p| p.lat)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_lon = points.iter().map(|p| p.lon).fold(f64::INFINITY, f64::min);
    let max_lon = points
        .iter()
        .map(|p| p.lon)
        .fold(f64::NEG_INFINITY, f64::max);
    let a = latlon_to_tile(max_lat, min_lon, zoom);
    let b = latlon_to_tile(min_lat, max_lon, zoom);
    Box::new(
        tiles_in_range(a.x.min(b.x), a.x.max(b.x), a.y.min(b.y), a.y.max(b.y), zoom)
            .filter(move |tile| tile_intersects_polygon(*tile, points)),
    )
}

pub fn validate_options(options: &DownloadOptions) -> Result<()> {
    validate_finite_number("lat", options.lat)?;
    validate_finite_number("lon", options.lon)?;
    if let Some(lat) = options.bottom_right_lat {
        validate_finite_number("bottom_right_lat", lat)?;
    }
    if let Some(lon) = options.bottom_right_lon {
        validate_finite_number("bottom_right_lon", lon)?;
    }
    for (index, point) in options.polygon.iter().enumerate() {
        validate_finite_number(&format!("polygon[{index}].lon"), point.lon)?;
        validate_finite_number(&format!("polygon[{index}].lat"), point.lat)?;
    }
    if options.zoom > MAX_ZOOM {
        bail!("zoom must be an integer from 0 to {MAX_ZOOM}");
    }
    if options.cols == 0 {
        bail!("cols must be an integer at least 1");
    }
    if options.rows == 0 {
        bail!("rows must be an integer at least 1");
    }
    if options.jobs == 0 {
        bail!("jobs must be an integer at least 1");
    }
    Ok(())
}

fn validate_finite_number(name: &str, value: f64) -> Result<()> {
    if !value.is_finite() {
        bail!("{name} must be a finite number");
    }
    Ok(())
}

pub async fn load_geojson_polygon(source: &str) -> Result<Vec<Coordinate>> {
    let text = if is_url(source) {
        reqwest::get(source)
            .await?
            .error_for_status()?
            .text()
            .await?
    } else {
        fs::read_to_string(source)?
    };
    polygon_from_geojson_str(&text)
}

pub fn polygon_from_geojson_str(text: &str) -> Result<Vec<Coordinate>> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    polygon_from_geojson(&value)
}

pub fn polygon_from_geojson(value: &serde_json::Value) -> Result<Vec<Coordinate>> {
    let geometry = find_polygon_geometry(value)
        .ok_or_else(|| anyhow::anyhow!("GeoJSON does not contain a Polygon geometry"))?;
    let coordinates = geometry
        .get("coordinates")
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("GeoJSON geometry is missing coordinates"))?;
    let ring = if geometry.get("type").and_then(|value| value.as_str()) == Some("Polygon") {
        coordinates.first()
    } else {
        coordinates
            .first()
            .and_then(|polygon| polygon.as_array())
            .and_then(|polygon| polygon.first())
    }
    .and_then(|ring| ring.as_array())
    .ok_or_else(|| anyhow::anyhow!("GeoJSON polygon is missing an exterior ring"))?;
    let points: Result<Vec<_>> = ring
        .iter()
        .map(|point| {
            let pair = point
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("GeoJSON coordinates must be lon,lat arrays"))?;
            Ok(Coordinate {
                lon: pair
                    .first()
                    .and_then(|value| value.as_f64())
                    .ok_or_else(|| anyhow::anyhow!("GeoJSON longitude must be a number"))?,
                lat: pair
                    .get(1)
                    .and_then(|value| value.as_f64())
                    .ok_or_else(|| anyhow::anyhow!("GeoJSON latitude must be a number"))?,
            })
        })
        .collect();
    let points = points?;
    if points.len() < 3 {
        bail!("GeoJSON polygon requires at least three lon,lat coordinates");
    }
    Ok(points)
}

pub fn tile_path(out: impl AsRef<Path>, tile: Tile) -> PathBuf {
    out.as_ref()
        .join("tiles")
        .join(tile.z.to_string())
        .join(tile.x.to_string())
        .join(format!("{}.jpg", tile.y))
}

pub async fn download(options: DownloadOptions) -> Result<DownloadReport> {
    download_with_progress(options, |_| {}).await
}

pub async fn download_with_progress(
    mut options: DownloadOptions,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<DownloadReport> {
    if options.polygon.len() < 3
        && let Some(source) = &options.geojson
    {
        options.polygon = load_geojson_polygon(source).await?;
    }
    validate_options(&options)?;
    let center = latlon_to_tile(options.lat, options.lon, options.zoom);
    let client = Arc::new(
        reqwest::Client::builder()
            .user_agent(random_ua())
            .gzip(true)
            .brotli(true)
            .build()?,
    );

    let tile_url_template = Arc::new(
        options
            .tile_url_template
            .clone()
            .or_else(|| env::var(TILE_URL_TEMPLATE_ENV).ok()),
    );

    let mut downloads = stream::iter(tile_iter_for_options(&options))
        .map(|tile| {
            let client = client.clone();
            let tile_url_template = tile_url_template.clone();
            async move {
                let data = download_tile(&client, tile, tile_url_template.as_deref()).await;
                (tile, data)
            }
        })
        .buffer_unordered(options.jobs.max(1));

    let mut downloaded = Vec::new();
    let mut failed = Vec::new();
    let mut completed = 0;
    while let Some((tile, data)) = downloads.next().await {
        match data {
            Some(bytes) => {
                let path = tile_path(&options.out, tile);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&path, &bytes)?;
                downloaded.push(DownloadedTile {
                    tile,
                    bounds: tile_bounds(tile),
                    path,
                    bytes: bytes.len(),
                });
            }
            None => failed.push(tile),
        }
        completed += 1;
        on_progress(DownloadProgress {
            completed,
            downloaded: downloaded.len(),
            failed: failed.len(),
            tile,
        });
    }

    let report = DownloadReport {
        center,
        tiles: downloaded,
        failed,
    };
    if !options.no_manifest {
        write_manifest(&options.out, &report)?;
    }
    if !options.no_demo {
        write_demo(&options.out, &report)?;
    }
    Ok(report)
}

fn count_polygon_tiles_with_progress(
    points: &[Coordinate],
    zoom: u32,
    mut on_progress: impl FnMut(SelectionProgress),
) -> usize {
    if points.len() < 3 {
        return 0;
    }
    let min_lat = points.iter().map(|p| p.lat).fold(f64::INFINITY, f64::min);
    let max_lat = points
        .iter()
        .map(|p| p.lat)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_lon = points.iter().map(|p| p.lon).fold(f64::INFINITY, f64::min);
    let max_lon = points
        .iter()
        .map(|p| p.lon)
        .fold(f64::NEG_INFINITY, f64::max);
    let a = latlon_to_tile(max_lat, min_lon, zoom);
    let b = latlon_to_tile(min_lat, max_lon, zoom);
    let min_x = a.x.min(b.x);
    let max_x = a.x.max(b.x);
    let min_y = a.y.min(b.y);
    let max_y = a.y.max(b.y);
    let total = range_tile_count(min_x, max_x, min_y, max_y);
    let mut scanned = 0;
    let mut selected = 0;
    for tile in tiles_in_range(min_x, max_x, min_y, max_y, zoom) {
        scanned += 1;
        if tile_intersects_polygon(tile, points) {
            selected += 1;
        }
        on_progress(SelectionProgress {
            scanned,
            selected,
            total,
        });
    }
    selected
}

fn count_range_tiles_with_progress(
    min_x: u32,
    max_x: u32,
    min_y: u32,
    max_y: u32,
    mut on_progress: impl FnMut(SelectionProgress),
) -> usize {
    let total = range_tile_count(min_x, max_x, min_y, max_y);
    for scanned in 1..=total {
        on_progress(SelectionProgress {
            scanned,
            selected: scanned,
            total,
        });
    }
    total
}

fn range_tile_count(min_x: u32, max_x: u32, min_y: u32, max_y: u32) -> usize {
    (max_x - min_x + 1) as usize * (max_y - min_y + 1) as usize
}

fn tile_grid_iter(
    lat: f64,
    lon: f64,
    zoom: u32,
    cols: u32,
    rows: u32,
) -> impl Iterator<Item = Tile> {
    let center = latlon_to_tile(lat, lon, zoom);
    (0..rows).flat_map(move |row| {
        (0..cols).map(move |col| Tile {
            x: center.x + col,
            y: center.y + row,
            z: zoom,
        })
    })
}

fn tiles_in_range(
    min_x: u32,
    max_x: u32,
    min_y: u32,
    max_y: u32,
    z: u32,
) -> impl Iterator<Item = Tile> {
    (min_y..=max_y).flat_map(move |y| (min_x..=max_x).map(move |x| Tile { x, y, z }))
}

fn find_polygon_geometry(value: &serde_json::Value) -> Option<&serde_json::Value> {
    match value.get("type")?.as_str()? {
        "Polygon" | "MultiPolygon" => Some(value),
        "Feature" => find_polygon_geometry(value.get("geometry")?),
        "FeatureCollection" => value
            .get("features")?
            .as_array()?
            .iter()
            .find_map(find_polygon_geometry),
        _ => None,
    }
}

fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn tile_intersects_polygon(tile: Tile, points: &[Coordinate]) -> bool {
    let bounds = tile_bounds(tile);
    let center = tile_center(tile);
    if point_in_polygon(center, points) {
        return true;
    }
    let corners = [
        Coordinate {
            lon: bounds.lon_min,
            lat: bounds.lat_min,
        },
        Coordinate {
            lon: bounds.lon_min,
            lat: bounds.lat_max,
        },
        Coordinate {
            lon: bounds.lon_max,
            lat: bounds.lat_min,
        },
        Coordinate {
            lon: bounds.lon_max,
            lat: bounds.lat_max,
        },
    ];
    corners
        .iter()
        .any(|corner| point_in_polygon(*corner, points))
        || points.iter().any(|point| {
            point.lon >= bounds.lon_min
                && point.lon <= bounds.lon_max
                && point.lat >= bounds.lat_min
                && point.lat <= bounds.lat_max
        })
}

fn tile_center(tile: Tile) -> Coordinate {
    let bounds = tile_bounds(tile);
    Coordinate {
        lon: (bounds.lon_min + bounds.lon_max) / 2.0,
        lat: (bounds.lat_min + bounds.lat_max) / 2.0,
    }
}

fn point_in_polygon(point: Coordinate, polygon: &[Coordinate]) -> bool {
    let mut inside = false;
    let mut previous = polygon[polygon.len() - 1];
    for &current in polygon {
        if (current.lat > point.lat) != (previous.lat > point.lat) {
            let lon = (previous.lon - current.lon) * (point.lat - current.lat)
                / (previous.lat - current.lat)
                + current.lon;
            if point.lon < lon {
                inside = !inside;
            }
        }
        previous = current;
    }
    inside
}

fn write_manifest(out: impl AsRef<Path>, report: &DownloadReport) -> Result<()> {
    fs::create_dir_all(&out)?;
    let manifest = out.as_ref().join("manifest.json");
    fs::write(manifest, serde_json::to_vec_pretty(report)?)?;
    Ok(())
}

fn write_demo(out: impl AsRef<Path>, report: &DownloadReport) -> Result<()> {
    fs::create_dir_all(&out)?;
    let (bounds, zoom) = if report.tiles.is_empty() {
        let bounds = tile_bounds(report.center);
        (
            [
                [bounds.lon_min, bounds.lat_min],
                [bounds.lon_max, bounds.lat_max],
            ],
            report.center.z,
        )
    } else {
        let min_lon = report
            .tiles
            .iter()
            .map(|item| item.bounds.lon_min)
            .fold(f64::INFINITY, f64::min);
        let min_lat = report
            .tiles
            .iter()
            .map(|item| item.bounds.lat_min)
            .fold(f64::INFINITY, f64::min);
        let max_lon = report
            .tiles
            .iter()
            .map(|item| item.bounds.lon_max)
            .fold(f64::NEG_INFINITY, f64::max);
        let max_lat = report
            .tiles
            .iter()
            .map(|item| item.bounds.lat_max)
            .fold(f64::NEG_INFINITY, f64::max);
        (
            [[min_lon, min_lat], [max_lon, max_lat]],
            report.tiles[0].tile.z,
        )
    };
    let tiles: Vec<_> = report.tiles.iter().map(|item| item.tile).collect();
    let data = serde_json::json!({
        "tiles": tiles,
        "bounds": bounds,
        "mapCenter": [
            (bounds[0][0] + bounds[1][0]) / 2.0,
            (bounds[0][1] + bounds[1][1]) / 2.0,
        ],
        "zoom": zoom,
        "center": report.center,
    })
    .to_string();
    fs::write(out.as_ref().join("index.html"), demo_html(&data))?;
    Ok(())
}

fn demo_html(data: &str) -> String {
    DEMO_HTML.replace("__GEODOT_DEMO_DATA__", data)
}

const DEMO_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>geodot tile overlay demo</title>
  <link href="https://unpkg.com/maplibre-gl@5.14.0/dist/maplibre-gl.css" rel="stylesheet">
  <style>
    html, body, #map { height: 100%; margin: 0; }
    .panel { position: absolute; top: 12px; right: 12px; z-index: 1; display: grid; gap: 8px; padding: 10px; border-radius: 10px; background: rgba(255,255,255,.92); font: 13px system-ui, sans-serif; box-shadow: 0 6px 24px rgba(0,0,0,.18); }
    .panel button { border: 0; border-radius: 8px; padding: 8px 10px; background: #1f2937; color: white; cursor: pointer; }
    .opacity { display: grid; gap: 4px; }
    .warning { max-width: 260px; color: #92400e; }
    .hidden { display: none; }
  </style>
</head>
<body>
  <div id="map"></div>
  <div class="panel">
    <button id="toggle" type="button">Overlay opacity</button>
    <label id="opacityPanel" class="opacity hidden">Transparency
      <input id="opacity" type="range" min="0" max="1" step="0.05" value="0.65">
    </label>
    <div id="fileWarning" class="warning hidden">
      Local file mode cannot load tile files. Run geodot demo and open http://127.0.0.1:8000/.
    </div>
  </div>
  <script src="https://unpkg.com/maplibre-gl@5.14.0/dist/maplibre-gl.js"></script>
  <script>
    const data = __GEODOT_DEMO_DATA__;
    if (location.protocol === 'file:') {
      document.getElementById('fileWarning').classList.remove('hidden');
    }
    const map = new maplibregl.Map({
      container: 'map',
      style: {
        version: 8,
        sources: {
          satellite: {
            type: 'raster',
            tiles: [
              'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}'
            ],
            tileSize: 256,
            attribution: 'Sources: Esri, Maxar, Earthstar Geographics, and the GIS User Community'
          }
        },
        layers: [{ id: 'satellite', type: 'raster', source: 'satellite' }]
      },
      center: data.mapCenter,
      zoom: data.zoom,
      minZoom: data.zoom,
      maxZoom: data.zoom,
      scrollZoom: false,
      boxZoom: false,
      doubleClickZoom: false,
      touchZoomRotate: false,
      keyboard: false,
      dragRotate: false,
      pitchWithRotate: false
    });

    map.on('load', () => {
      map.addSource('geodot-tiles', {
        type: 'raster',
        tiles: ['./tiles/{z}/{x}/{y}.jpg'],
        tileSize: 256,
        minzoom: data.zoom,
        maxzoom: data.zoom,
        bounds: [data.bounds[0][0], data.bounds[0][1], data.bounds[1][0], data.bounds[1][1]]
      });
      map.addLayer({ id: 'geodot-tiles', type: 'raster', source: 'geodot-tiles', paint: { 'raster-opacity': 0.65 } });
    });

    document.getElementById('toggle').addEventListener('click', () => {
      document.getElementById('opacityPanel').classList.toggle('hidden');
    });
    document.getElementById('opacity').addEventListener('input', (event) => {
      if (map.getLayer('geodot-tiles')) map.setPaintProperty('geodot-tiles', 'raster-opacity', Number(event.target.value));
    });
  </script>
</body>
</html>
"#;

async fn download_tile(
    client: &reqwest::Client,
    tile: Tile,
    tile_url_template: Option<&str>,
) -> Option<Vec<u8>> {
    for &sub in SUBDOMAINS {
        let url = tile_url(sub, tile, tile_url_template);
        let result = client
            .get(&url)
            .header("User-Agent", random_ua())
            .header(
                "Accept",
                "image/avif,image/webp,image/apng,image/*,*/*;q=0.8",
            )
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Referer", "https://www.google.com/maps")
            .timeout(Duration::from_secs(15))
            .send()
            .await;
        match result {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(data) = resp.bytes().await
                    && data.len() > 100
                {
                    return Some(data.to_vec());
                }
            }
            _ => continue,
        }
    }
    None
}

fn tile_url(subdomain: &str, tile: Tile, tile_url_template: Option<&str>) -> String {
    if let Some(template) = tile_url_template {
        return template
            .replace("{sub}", subdomain)
            .replace("{x}", &tile.x.to_string())
            .replace("{y}", &tile.y.to_string())
            .replace("{z}", &tile.z.to_string());
    }
    format!(
        "https://{subdomain}.google.com/vt/lyrs=s&x={}&y={}&z={}",
        tile.x, tile.y, tile.z
    )
}

fn random_ua() -> &'static str {
    let mut rng = rand::thread_rng();
    USER_AGENTS.choose(&mut rng).unwrap_or(&USER_AGENTS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_lat_lon_to_tile() {
        assert_eq!(
            latlon_to_tile(55.7303, 37.6504907, 18),
            Tile {
                x: 158488,
                y: 81979,
                z: 18
            }
        );
    }

    #[test]
    fn builds_tile_grid_to_right_and_down() {
        let tiles = tile_grid(55.7303, 37.6504907, 18, 2, 2);
        assert_eq!(tiles.len(), 4);
        assert_eq!(
            tiles[0],
            Tile {
                x: 158488,
                y: 81979,
                z: 18
            }
        );
        assert_eq!(
            tiles[3],
            Tile {
                x: 158489,
                y: 81980,
                z: 18
            }
        );
    }

    #[test]
    fn builds_tile_grid_between_corners() {
        let tiles = tile_grid_between(55.7303, 37.6504907, 55.7297, 37.652, 18);
        assert_eq!(
            tiles,
            vec![
                Tile {
                    x: 158488,
                    y: 81979,
                    z: 18
                },
                Tile {
                    x: 158489,
                    y: 81979,
                    z: 18
                },
                Tile {
                    x: 158488,
                    y: 81980,
                    z: 18
                },
                Tile {
                    x: 158489,
                    y: 81980,
                    z: 18
                }
            ]
        );
    }

    #[test]
    fn builds_tile_grid_for_polygon() {
        let polygon = vec![
            Coordinate {
                lon: 37.6504,
                lat: 55.7304,
            },
            Coordinate {
                lon: 37.6520,
                lat: 55.7304,
            },
            Coordinate {
                lon: 37.6520,
                lat: 55.7297,
            },
            Coordinate {
                lon: 37.6504,
                lat: 55.7297,
            },
        ];
        assert_eq!(tile_grid_for_polygon(&polygon, 18).len(), 4);
    }

    #[test]
    fn reads_polygon_from_geojson_feature_collection() {
        let polygon = polygon_from_geojson_str(
            r#"{
                "type":"FeatureCollection",
                "features":[{
                    "type":"Feature",
                    "geometry":{
                        "type":"Polygon",
                        "coordinates":[[[37.6504,55.7304],[37.652,55.7304],[37.652,55.7297],[37.6504,55.7297],[37.6504,55.7304]]]
                    }
                }]
            }"#,
        )
        .unwrap();
        assert_eq!(
            &polygon[..4],
            &[
                Coordinate {
                    lon: 37.6504,
                    lat: 55.7304
                },
                Coordinate {
                    lon: 37.652,
                    lat: 55.7304
                },
                Coordinate {
                    lon: 37.652,
                    lat: 55.7297
                },
                Coordinate {
                    lon: 37.6504,
                    lat: 55.7297
                }
            ]
        );
    }

    #[test]
    fn builds_nested_tile_path() {
        let path = tile_path("data", Tile { x: 1, y: 2, z: 3 });
        assert_eq!(path, PathBuf::from("data/tiles/3/1/2.jpg"));
    }

    #[test]
    fn serializes_downloaded_tile_bounds() {
        let tile = Tile { x: 1, y: 2, z: 3 };
        let value = serde_json::to_value(DownloadedTile {
            tile,
            bounds: tile_bounds(tile),
            path: PathBuf::from("data/tiles/3/1/2.jpg"),
            bytes: 123,
        })
        .unwrap();
        assert!(value["bounds"]["lat_min"].is_number());
        assert!(value["bounds"]["lon_min"].is_number());
        assert!(value["bounds"]["lat_max"].is_number());
        assert!(value["bounds"]["lon_max"].is_number());
    }

    #[test]
    fn rejects_invalid_numeric_options() {
        let err = validate_options(&DownloadOptions {
            lat: f64::NAN,
            ..DownloadOptions::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("lat must be a finite number"));

        let err = validate_options(&DownloadOptions {
            cols: 0,
            ..DownloadOptions::default()
        })
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("cols must be an integer at least 1")
        );

        let err = validate_options(&DownloadOptions {
            zoom: MAX_ZOOM + 1,
            ..DownloadOptions::default()
        })
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("zoom must be an integer from 0 to 30")
        );
    }
}
