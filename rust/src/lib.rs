use anyhow::{Result, bail};
use futures::stream::{self, StreamExt};
use rand::prelude::IndexedRandom;
use serde::Serialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const TILE_SIZE: u32 = 256;
pub const MAX_ZOOM: u32 = 30;
pub const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &[".jpg", ".jpeg", ".png", ".webp"];

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

#[derive(Debug, Clone)]
pub struct PrepareOptions {
    pub out: PathBuf,
    pub patch_sizes: Vec<u32>,
    pub stride: u32,
    pub rotations: Vec<u32>,
    pub auto400m: bool,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        Self {
            out: PathBuf::from("data"),
            patch_sizes: vec![1, 2, 4],
            stride: 1,
            rotations: vec![0, 45, 90, 135, 180, 225, 270, 315],
            auto400m: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrepareReport {
    pub tiles: usize,
    pub patches: usize,
    pub variants: usize,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderReport {
    pub source_path: PathBuf,
    pub output_path: PathBuf,
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub counts: HashMap<String, usize>,
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

fn discover_tiles(out: &Path) -> Result<Vec<serde_json::Value>> {
    let roots = [
        ("tiles", out.join("tiles")),
        ("drone-view", out.join("drone-view")),
    ];
    if !roots.iter().any(|(_, root)| root.exists()) {
        bail!("tile directory not found: {}", out.join("tiles").display());
    }
    let mut tiles = Vec::new();
    for (root_name, root) in roots {
        if !root.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_files(&root, &mut files)?;
        files.sort();
        for file in files {
            let extension = file
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| format!(".{value}").to_ascii_lowercase())
                .unwrap_or_default();
            if !SUPPORTED_IMAGE_EXTENSIONS.contains(&extension.as_str()) {
                continue;
            }
            let Some((capture_id, z, x, y)) = parse_image_path(&root, &file) else {
                continue;
            };
            let max_tile = 2u64.pow(z);
            if x as u64 >= max_tile || y as u64 >= max_tile {
                continue;
            }
            let tile = Tile { x, y, z };
            let bounds = tile_bounds(tile);
            let bytes = fs::metadata(&file)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            let (image_width, image_height, detected_format) = read_image_header(&file);
            tiles.push(serde_json::json!({
                "tile_id": format!("{root_name}_{capture_id}_z{z}_x{x}_y{y}"),
                "root": root_name,
                "capture_id": capture_id,
                "role": if root_name == "tiles" { "reference" } else { "query" },
                "z": z,
                "x": x,
                "y": y,
                "extension": extension.trim_start_matches('.'),
                "detected_format": detected_format,
                "path": file.strip_prefix(out).unwrap_or(&file).to_string_lossy(),
                "bbox": [bounds.lon_min, bounds.lat_min, bounds.lon_max, bounds.lat_max],
                "center_lon": (bounds.lon_min + bounds.lon_max) / 2.0,
                "center_lat": (bounds.lat_min + bounds.lat_max) / 2.0,
                "image_width": image_width.unwrap_or(TILE_SIZE),
                "image_height": image_height.unwrap_or(TILE_SIZE),
                "pixel_width": image_width.unwrap_or(TILE_SIZE),
                "pixel_height": image_height.unwrap_or(TILE_SIZE),
                "bytes": bytes,
                "valid": image_width.is_some() && image_height.is_some(),
                "lon_min": bounds.lon_min,
                "lat_min": bounds.lat_min,
                "lon_max": bounds.lon_max,
                "lat_max": bounds.lat_max,
            }));
        }
    }
    if tiles.is_empty() {
        bail!("no valid tiles found under {}", out.join("tiles").display());
    }
    Ok(tiles)
}

fn parse_image_path(root: &Path, file: &Path) -> Option<(String, u32, u32, u32)> {
    let relative = file.strip_prefix(root).ok()?;
    let parts: Vec<_> = relative.components().collect();
    let (capture_id, z_index, x_index) = match parts.len() {
        3 => ("default".to_string(), 0, 1),
        4 => (parts[0].as_os_str().to_string_lossy().into_owned(), 1, 2),
        _ => return None,
    };
    let z = parts[z_index]
        .as_os_str()
        .to_string_lossy()
        .parse::<u32>()
        .ok()?;
    let x = parts[x_index]
        .as_os_str()
        .to_string_lossy()
        .parse::<u32>()
        .ok()?;
    let y = file.file_stem()?.to_string_lossy().parse::<u32>().ok()?;
    if z > MAX_ZOOM {
        return None;
    }
    Some((capture_id, z, x, y))
}

fn read_image_header(file: &Path) -> (Option<u32>, Option<u32>, Option<&'static str>) {
    let Ok(data) = fs::read(file) else {
        return (None, None, None);
    };
    if data.starts_with(b"\x89PNG\r\n\x1a\n") && data.len() >= 24 {
        return (
            Some(u32::from_be_bytes([data[16], data[17], data[18], data[19]])),
            Some(u32::from_be_bytes([data[20], data[21], data[22], data[23]])),
            Some("png"),
        );
    }
    if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        return read_webp_header(&data);
    }
    if data.starts_with(&[0xff, 0xd8]) {
        return read_jpeg_header(&data);
    }
    (None, None, None)
}

fn read_jpeg_header(data: &[u8]) -> (Option<u32>, Option<u32>, Option<&'static str>) {
    let mut index = 2;
    while index + 9 < data.len() {
        if data[index] != 0xff {
            index += 1;
            continue;
        }
        let marker = data[index + 1];
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            let height = u16::from_be_bytes([data[index + 5], data[index + 6]]) as u32;
            let width = u16::from_be_bytes([data[index + 7], data[index + 8]]) as u32;
            return (Some(width), Some(height), Some("jpeg"));
        }
        if index + 4 > data.len() {
            break;
        }
        let length = u16::from_be_bytes([data[index + 2], data[index + 3]]) as usize;
        index += 2 + length.max(1);
    }
    (None, None, Some("jpeg"))
}

fn read_webp_header(data: &[u8]) -> (Option<u32>, Option<u32>, Option<&'static str>) {
    match data.get(12..16) {
        Some(b"VP8X") if data.len() >= 30 => {
            let width = u32::from_le_bytes([data[24], data[25], data[26], 0]) + 1;
            let height = u32::from_le_bytes([data[27], data[28], data[29], 0]) + 1;
            (Some(width), Some(height), Some("webp"))
        }
        Some(b"VP8L") if data.len() >= 25 => {
            let bits = u32::from_le_bytes([data[21], data[22], data[23], data[24]]);
            (
                Some((bits & 0x3fff) + 1),
                Some(((bits >> 14) & 0x3fff) + 1),
                Some("webp"),
            )
        }
        _ => (None, None, Some("webp")),
    }
}

fn resolve_patch_sizes(tiles: &[serde_json::Value], options: &PrepareOptions) -> Vec<u32> {
    let mut sizes = options.patch_sizes.clone();
    if options.auto400m {
        for tile in tiles {
            let Some(lat) = tile["center_lat"].as_f64() else {
                continue;
            };
            let Some(z) = tile["z"].as_u64().map(|value| value as u32) else {
                continue;
            };
            let tile_width_m = meters_per_pixel(lat, z) * TILE_SIZE as f64;
            if tile_width_m > 0.0 {
                sizes.push(((400.0 / tile_width_m).round() as u32).clamp(1, 8));
            }
        }
    }
    sizes.sort_unstable();
    sizes.dedup();
    sizes
}

fn collect_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn build_patches(
    tiles: &[serde_json::Value],
    tile_ids: &HashMap<(String, String, u32, u32, u32), String>,
    options: &PrepareOptions,
    sizes: &[u32],
) -> Vec<serde_json::Value> {
    let mut by_group: HashMap<(String, String, u32), Vec<(u32, u32)>> = HashMap::new();
    for tile in tiles {
        let Some(root) = tile["root"].as_str().map(str::to_string) else {
            continue;
        };
        let Some(capture_id) = tile["capture_id"].as_str().map(str::to_string) else {
            continue;
        };
        let Some(z) = tile["z"].as_u64().map(|value| value as u32) else {
            continue;
        };
        let Some(x) = tile["x"].as_u64().map(|value| value as u32) else {
            continue;
        };
        let Some(y) = tile["y"].as_u64().map(|value| value as u32) else {
            continue;
        };
        by_group
            .entry((root, capture_id, z))
            .or_default()
            .push((x, y));
    }

    let mut patches = Vec::new();
    let mut groups: Vec<_> = by_group.into_iter().collect();
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    for ((root, capture_id, z), zoom_tiles) in groups {
        let Some(min_x) = zoom_tiles.iter().map(|(x, _)| *x).min() else {
            continue;
        };
        let Some(max_x) = zoom_tiles.iter().map(|(x, _)| *x).max() else {
            continue;
        };
        let Some(min_y) = zoom_tiles.iter().map(|(_, y)| *y).min() else {
            continue;
        };
        let Some(max_y) = zoom_tiles.iter().map(|(_, y)| *y).max() else {
            continue;
        };
        for size in sizes {
            if *size == 0 || max_x < min_x + size - 1 || max_y < min_y + size - 1 {
                continue;
            }
            let mut y = min_y;
            while y <= max_y - size + 1 {
                let mut x = min_x;
                while x <= max_x - size + 1 {
                    let mut source_tiles = Vec::new();
                    let mut complete = true;
                    for source_y in y..y + size {
                        for source_x in x..x + size {
                            if let Some(tile_id) = tile_ids.get(&(
                                root.clone(),
                                capture_id.clone(),
                                z,
                                source_x,
                                source_y,
                            )) {
                                source_tiles.push(tile_id.clone());
                            } else {
                                complete = false;
                            }
                        }
                    }
                    if complete {
                        let top_left = tile_bounds(Tile { x, y, z });
                        let bottom_right = tile_bounds(Tile {
                            x: x + size - 1,
                            y: y + size - 1,
                            z,
                        });
                        let lon_min = top_left.lon_min;
                        let lat_min = bottom_right.lat_min;
                        let lon_max = bottom_right.lon_max;
                        let lat_max = top_left.lat_max;
                        let patch_id = format!(
                            "{root}_{capture_id}_z{z}_x{}-{}_y{}-{}_s{}",
                            x,
                            x + size - 1,
                            y,
                            y + size - 1,
                            size
                        );
                        patches.push(serde_json::json!({
                            "patch_id": patch_id,
                            "place_id": format!("z{z}_x{x}_y{y}"),
                            "root": root,
                            "capture_id": capture_id,
                            "role": if root == "tiles" { "reference" } else { "query" },
                            "z": z,
                            "x": x,
                            "y": y,
                            "source_x_min": x,
                            "source_x_max": x + size - 1,
                            "source_y_min": y,
                            "source_y_max": y + size - 1,
                            "source_tiles": source_tiles,
                            "source_tile_ids": source_tiles,
                            "pixel_width": TILE_SIZE * size,
                            "pixel_height": TILE_SIZE * size,
                            "bbox": [lon_min, lat_min, lon_max, lat_max],
                            "lon_min": lon_min,
                            "lat_min": lat_min,
                            "lon_max": lon_max,
                            "lat_max": lat_max,
                            "center_lon": (lon_min + lon_max) / 2.0,
                            "center_lat": (lat_min + lat_max) / 2.0,
                            "mosaic_size_tiles": size,
                            "stride_tiles": options.stride,
                            "scale_profile": format!("z{z}_{size}x{size}"),
                            "ground_width_m_estimate": meters_per_pixel((lat_min + lat_max) / 2.0, z) * TILE_SIZE as f64 * *size as f64,
                            "ground_height_m_estimate": meters_per_pixel((lat_min + lat_max) / 2.0, z) * TILE_SIZE as f64 * *size as f64,
                            "complete": true,
                            "rotation_safe_circle_diameter_px": TILE_SIZE * size,
                            "circular_crop_available": true,
                            "image_written": false,
                            "virtual_compose_spec": {
                                "type": "virtual_mosaic",
                                "tile_ids": source_tiles,
                                "layout": [size, size],
                            },
                            "image_path_or_virtual_spec": {
                                "type": "virtual_mosaic",
                                "tile_ids": source_tiles,
                                "layout": [size, size],
                            },
                        }));
                    }
                    x = match x.checked_add(options.stride) {
                        Some(next) => next,
                        None => break,
                    };
                }
                y = match y.checked_add(options.stride) {
                    Some(next) => next,
                    None => break,
                };
            }
        }
    }
    patches
}

fn build_places(
    tiles: &[serde_json::Value],
    patches: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let mut grouped: HashMap<(u32, u32, u32), serde_json::Value> = HashMap::new();
    for tile in tiles {
        let Some(z) = tile["z"].as_u64().map(|value| value as u32) else {
            continue;
        };
        let Some(x) = tile["x"].as_u64().map(|value| value as u32) else {
            continue;
        };
        let Some(y) = tile["y"].as_u64().map(|value| value as u32) else {
            continue;
        };
        let place = grouped.entry((z, x, y)).or_insert_with(|| {
            serde_json::json!({
                "place_id": format!("z{z}_x{x}_y{y}"),
                "z": z,
                "x": x,
                "y": y,
                "center_lon": tile["center_lon"].clone(),
                "center_lat": tile["center_lat"].clone(),
                "bbox": tile["bbox"].clone(),
                "available_roots": [],
                "available_captures": [],
                "tile_ids": [],
                "reference_available": false,
                "query_available": false,
                "reference_tile_ids": [],
                "query_tile_ids": [],
                "patch_ids": [],
                "quality_summary": { "recommendation": "keep" },
            })
        });
        place["available_roots"]
            .as_array_mut()
            .unwrap()
            .push(tile["root"].clone());
        place["available_captures"]
            .as_array_mut()
            .unwrap()
            .push(tile["capture_id"].clone());
        place["tile_ids"]
            .as_array_mut()
            .unwrap()
            .push(tile["tile_id"].clone());
        if tile["role"].as_str() == Some("reference") {
            place["reference_available"] = serde_json::Value::Bool(true);
            place["reference_tile_ids"]
                .as_array_mut()
                .unwrap()
                .push(tile["tile_id"].clone());
        }
        if tile["role"].as_str() == Some("query") {
            place["query_available"] = serde_json::Value::Bool(true);
            place["query_tile_ids"]
                .as_array_mut()
                .unwrap()
                .push(tile["tile_id"].clone());
        }
    }
    for patch in patches {
        let Some(z) = patch["z"].as_u64().map(|value| value as u32) else {
            continue;
        };
        let Some(x) = patch["x"].as_u64().map(|value| value as u32) else {
            continue;
        };
        let Some(y) = patch["y"].as_u64().map(|value| value as u32) else {
            continue;
        };
        if let Some(place) = grouped.get_mut(&(z, x, y)) {
            place["patch_ids"]
                .as_array_mut()
                .unwrap()
                .push(patch["patch_id"].clone());
        }
    }
    grouped.into_values().collect()
}

fn build_quality(tiles: &[serde_json::Value], patches: &[serde_json::Value]) -> serde_json::Value {
    let tile_quality: Vec<_> = tiles
        .iter()
        .map(|tile| {
            let bytes = tile["bytes"].as_u64().unwrap_or(0);
            let reject = bytes < 128;
            serde_json::json!({
                "id": tile["tile_id"],
                "type": "tile",
                "blank_near_blank_score": if reject { 1.0 } else { 0.0 },
                "mean_brightness": null,
                "contrast": null,
                "blur_estimate": null,
                "edge_density": null,
                "entropy": null,
                "likely_low_information": reject,
                "recommendation": if reject { "reject" } else { "keep" },
                "reject_reasons": if reject { vec!["very_small_file"] } else { Vec::<&str>::new() },
            })
        })
        .collect();
    let patch_quality: Vec<_> = patches
        .iter()
        .map(|patch| {
            serde_json::json!({
                "id": patch["patch_id"],
                "type": "patch",
                "likely_low_information": false,
                "recommendation": "keep",
                "reject_reasons": [],
            })
        })
        .collect();
    serde_json::json!({ "tiles": tile_quality, "patches": patch_quality })
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

pub fn validate_prepare_options(options: &PrepareOptions) -> Result<()> {
    if options.patch_sizes.is_empty() {
        bail!("patch_sizes must not be empty");
    }
    if options.rotations.is_empty() {
        bail!("rotations must not be empty");
    }
    if options.stride == 0 {
        bail!("stride must be an integer at least 1");
    }
    if options.patch_sizes.contains(&0) {
        bail!("patch_sizes must be an integer at least 1");
    }
    if options.rotations.iter().any(|rotation| *rotation > 359) {
        bail!("rotations must be an integer from 0 to 359");
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

pub fn prepare_dataset(options: PrepareOptions) -> Result<PrepareReport> {
    validate_prepare_options(&options)?;
    let tiles = discover_tiles(&options.out)?;
    let patch_sizes = resolve_patch_sizes(&tiles, &options);
    let tile_ids: HashMap<(String, String, u32, u32, u32), String> = tiles
        .iter()
        .filter_map(|tile| {
            Some((
                (
                    tile.get("root")?.as_str()?.to_string(),
                    tile.get("capture_id")?.as_str()?.to_string(),
                    tile.get("z")?.as_u64()? as u32,
                    tile.get("x")?.as_u64()? as u32,
                    tile.get("y")?.as_u64()? as u32,
                ),
                tile.get("tile_id")?.as_str()?.to_string(),
            ))
        })
        .collect();
    let patches = build_patches(&tiles, &tile_ids, &options, &patch_sizes);
    let places = build_places(&tiles, &patches);
    let quality = build_quality(&tiles, &patches);
    let variants: Vec<_> = patches
        .iter()
        .flat_map(|patch| {
            let patch_id = patch["patch_id"].as_str().unwrap_or_default().to_string();
            options.rotations.iter().map(move |rotation| {
                let patch_id = patch_id.clone();
                serde_json::json!({
                    "variant_id": format!("{patch_id}_r{rotation}"),
                    "patch_id": patch_id,
                    "rotation_degrees": rotation,
                    "crop_shape": "square",
                    "virtual_only": true,
                    "image_written": false,
                    "descriptor_id": null,
                    "index_id": null,
                })
            })
        })
        .collect();

    let root = options.out.join("vpr");
    let manifest = root.join("manifest");
    let config = root.join("config");
    fs::create_dir_all(&manifest)?;
    fs::create_dir_all(&config)?;
    fs::write(
        manifest.join("tiles.json"),
        serde_json::to_vec_pretty(&tiles)?,
    )?;
    fs::write(
        manifest.join("patches.json"),
        serde_json::to_vec_pretty(&patches)?,
    )?;
    fs::write(
        manifest.join("variants.json"),
        serde_json::to_vec_pretty(&variants)?,
    )?;
    fs::write(
        manifest.join("places.json"),
        serde_json::to_vec_pretty(&places)?,
    )?;
    fs::write(
        manifest.join("quality.json"),
        serde_json::to_vec_pretty(&quality)?,
    )?;
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut roots: Vec<_> = tiles
        .iter()
        .filter_map(|tile| tile["root"].as_str())
        .collect();
    roots.sort_unstable();
    roots.dedup();
    let mut zooms: Vec<_> = tiles.iter().filter_map(|tile| tile["z"].as_u64()).collect();
    zooms.sort_unstable();
    zooms.dedup();
    let dataset = serde_json::json!({
        "schema_version": "1.0",
        "geodot_version": env!("CARGO_PKG_VERSION"),
        "created_at": created_at,
        "command": env::args().collect::<Vec<_>>().join(" "),
        "output_directory": options.out,
        "profile": "aerial-vpr-default",
        "tile_roots": roots.iter().map(|root| format!("{root}/{{z}}/{{x}}/{{y}}.{{jpg,jpeg,png,webp}}")).collect::<Vec<_>>(),
        "mode": "virtual",
        "tile_size": TILE_SIZE,
        "image_roots_detected": roots,
        "supported_image_extensions": SUPPORTED_IMAGE_EXTENSIONS,
        "zoom_levels_detected": zooms,
        "patch_sizes": patch_sizes,
        "stride": options.stride,
        "rotations": options.rotations,
        "auto400m": options.auto400m,
        "circular_crops_virtual": true,
        "images_modified": false,
        "generated_images_default": false,
        "descriptors_computed": false,
        "indexes_built": false,
        "appearance": [],
        "counts": {
            "tiles": tiles.len(),
            "patches": patches.len(),
            "variants": variants.len(),
            "places": places.len(),
        },
    });
    fs::write(
        config.join("dataset.json"),
        serde_json::to_vec_pretty(&dataset)?,
    )?;
    Ok(PrepareReport {
        tiles: tiles.len(),
        patches: patches.len(),
        variants: variants.len(),
        path: root,
    })
}

pub fn render_dataset(
    out: impl AsRef<Path>,
    patch_id: Option<&str>,
    variant_id: Option<&str>,
    output: impl AsRef<Path>,
) -> Result<RenderReport> {
    if patch_id.is_some() == variant_id.is_some() {
        bail!("provide exactly one of patch_id or variant_id");
    }
    let out = out.as_ref();
    let manifest = out.join("vpr").join("manifest");
    let patches: serde_json::Value =
        serde_json::from_slice(&fs::read(manifest.join("patches.json"))?)?;
    let tiles: serde_json::Value = serde_json::from_slice(&fs::read(manifest.join("tiles.json"))?)?;
    let selected_patch_id = if let Some(variant_id) = variant_id {
        let variants: serde_json::Value =
            serde_json::from_slice(&fs::read(manifest.join("variants.json"))?)?;
        variants
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item["variant_id"].as_str() == Some(variant_id))
            })
            .and_then(|item| item["patch_id"].as_str())
            .ok_or_else(|| anyhow::anyhow!("variant not found: {variant_id}"))?
            .to_string()
    } else {
        patch_id.unwrap_or_default().to_string()
    };
    let patch = patches
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["patch_id"].as_str() == Some(selected_patch_id.as_str()))
        })
        .ok_or_else(|| anyhow::anyhow!("patch not found: {selected_patch_id}"))?;
    let source_tile_ids = patch["source_tile_ids"]
        .as_array()
        .or_else(|| patch["source_tiles"].as_array())
        .ok_or_else(|| anyhow::anyhow!("patch has no source tiles"))?;
    if source_tile_ids.len() != 1 {
        bail!("render currently supports one-source-tile virtual patches only");
    }
    let source_tile_id = source_tile_ids[0]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("source tile id is not a string"))?;
    let tile = tiles
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["tile_id"].as_str() == Some(source_tile_id))
        })
        .ok_or_else(|| anyhow::anyhow!("source tile not found: {source_tile_id}"))?;
    let source_path = out.join(
        tile["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("source tile has no path"))?,
    );
    let output_path = output.as_ref().to_path_buf();
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = fs::read(&source_path)?;
    fs::write(&output_path, &data)?;
    Ok(RenderReport {
        source_path,
        output_path,
        bytes: data.len(),
    })
}

pub fn load_dataset(out: impl AsRef<Path>) -> Result<HashMap<String, serde_json::Value>> {
    let out = out.as_ref();
    let manifest = out.join("vpr").join("manifest");
    let config = out.join("vpr").join("config");
    let files = [
        ("tiles", manifest.join("tiles.json")),
        ("patches", manifest.join("patches.json")),
        ("variants", manifest.join("variants.json")),
        ("places", manifest.join("places.json")),
        ("quality", manifest.join("quality.json")),
        ("dataset", config.join("dataset.json")),
    ];
    let mut dataset = HashMap::new();
    for (name, path) in files {
        if !path.exists() {
            bail!("missing dataset manifest: {}", path.display());
        }
        dataset.insert(name.to_string(), serde_json::from_slice(&fs::read(path)?)?);
    }
    Ok(dataset)
}

pub fn validate_dataset(out: impl AsRef<Path>, strict: bool) -> Result<ValidationReport> {
    let out = out.as_ref();
    let dataset = load_dataset(out)?;
    let tiles = dataset["tiles"].as_array().cloned().unwrap_or_default();
    let patches = dataset["patches"].as_array().cloned().unwrap_or_default();
    let variants = dataset["variants"].as_array().cloned().unwrap_or_default();
    let places = dataset["places"].as_array().cloned().unwrap_or_default();
    let config = &dataset["dataset"];
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let tile_ids = check_unique_ids("tile", &tiles, "tile_id", &mut errors);
    let patch_ids = check_unique_ids("patch", &patches, "patch_id", &mut errors);
    check_unique_ids("variant", &variants, "variant_id", &mut errors);
    check_unique_ids("place", &places, "place_id", &mut errors);
    for tile in &tiles {
        let tile_id = tile["tile_id"].as_str().unwrap_or_default();
        let path = tile["path"].as_str().unwrap_or_default();
        if path.is_empty() || !out.join(path).exists() {
            errors.push(format!("missing source image for tile {tile_id}: {path}"));
        }
        if !valid_bbox(&tile["bbox"]) {
            errors.push(format!("invalid bbox for tile {tile_id}"));
        }
        if !positive_int(&tile["image_width"]) || !positive_int(&tile["image_height"]) {
            errors.push(format!("invalid image dimensions for tile {tile_id}"));
        }
    }
    for patch in &patches {
        let patch_id = patch["patch_id"].as_str().unwrap_or_default();
        if !valid_bbox(&patch["bbox"]) {
            errors.push(format!("invalid bbox for patch {patch_id}"));
        }
        for tile_id in patch["source_tile_ids"]
            .as_array()
            .or_else(|| patch["source_tiles"].as_array())
            .into_iter()
            .flatten()
        {
            let tile_id = tile_id.as_str().unwrap_or_default();
            if !tile_ids.contains(tile_id) {
                errors.push(format!(
                    "patch {patch_id} references missing tile {tile_id}"
                ));
            }
        }
    }
    for variant in &variants {
        let variant_id = variant["variant_id"].as_str().unwrap_or_default();
        let patch_id = variant["patch_id"].as_str().unwrap_or_default();
        if !patch_ids.contains(patch_id) {
            errors.push(format!(
                "variant {variant_id} references missing patch {patch_id}"
            ));
        }
    }
    for place in &places {
        let place_id = place["place_id"].as_str().unwrap_or_default();
        for field in ["tile_ids", "reference_tile_ids", "query_tile_ids"] {
            for tile_id in place[field].as_array().into_iter().flatten() {
                let tile_id = tile_id.as_str().unwrap_or_default();
                if !tile_ids.contains(tile_id) {
                    errors.push(format!(
                        "place {place_id} {field} references missing tile {tile_id}"
                    ));
                }
            }
        }
        for patch_id in place["patch_ids"].as_array().into_iter().flatten() {
            let patch_id = patch_id.as_str().unwrap_or_default();
            if !patch_ids.contains(patch_id) {
                errors.push(format!(
                    "place {place_id} references missing patch {patch_id}"
                ));
            }
        }
    }
    for field in [
        "images_modified",
        "descriptors_computed",
        "indexes_built",
        "generated_images_default",
    ] {
        if config[field].as_bool() != Some(false) {
            errors.push(format!("dataset config {field} must be false"));
        }
    }
    let mut generated = Vec::new();
    collect_files(&out.join("vpr"), &mut generated)?;
    let generated_count = generated
        .iter()
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .map(|value| {
                    SUPPORTED_IMAGE_EXTENSIONS
                        .contains(&format!(".{value}").to_ascii_lowercase().as_str())
                })
                .unwrap_or(false)
        })
        .count();
    if generated_count > 0 {
        warnings.push(format!(
            "found generated image(s) under vpr: {generated_count}"
        ));
    }
    if strict && !warnings.is_empty() {
        errors.append(&mut warnings);
    }
    let mut counts = HashMap::new();
    counts.insert("tiles".into(), tiles.len());
    counts.insert("patches".into(), patches.len());
    counts.insert("variants".into(), variants.len());
    counts.insert("places".into(), places.len());
    counts.insert(
        "query_tiles".into(),
        tiles
            .iter()
            .filter(|tile| tile["role"].as_str() == Some("query"))
            .count(),
    );
    counts.insert(
        "reference_tiles".into(),
        tiles
            .iter()
            .filter(|tile| tile["role"].as_str() == Some("reference"))
            .count(),
    );
    counts.insert("warnings".into(), warnings.len());
    counts.insert("errors".into(), errors.len());
    Ok(ValidationReport {
        valid: errors.is_empty(),
        errors,
        warnings,
        counts,
    })
}

fn check_unique_ids(
    kind: &str,
    items: &[serde_json::Value],
    field: &str,
    errors: &mut Vec<String>,
) -> std::collections::HashSet<String> {
    let mut seen = std::collections::HashSet::new();
    for item in items {
        let Some(value) = item[field].as_str() else {
            errors.push(format!("{kind} missing {field}"));
            continue;
        };
        if !seen.insert(value.to_string()) {
            errors.push(format!("duplicate {kind} id: {value}"));
        }
    }
    seen
}

fn valid_bbox(value: &serde_json::Value) -> bool {
    let Some(items) = value.as_array() else {
        return false;
    };
    if items.len() != 4 {
        return false;
    }
    let values: Option<Vec<_>> = items.iter().map(|item| item.as_f64()).collect();
    let Some(values) = values else {
        return false;
    };
    values[0] >= -180.0
        && values[0] < values[2]
        && values[2] <= 180.0
        && values[1] >= -90.0
        && values[1] < values[3]
        && values[3] <= 90.0
}

fn positive_int(value: &serde_json::Value) -> bool {
    value.as_u64().map(|value| value > 0).unwrap_or(false)
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
    let mut selected = 0;
    for (index, tile) in tiles_in_range(min_x, max_x, min_y, max_y, zoom).enumerate() {
        let scanned = index + 1;
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
  <link href="https://unpkg.com/maplibre-gl@5.24.0/dist/maplibre-gl.css" rel="stylesheet">
  <style>
    :root { color-scheme: light; --panel-bg: rgba(255,255,255,.94); --panel-border: rgba(17,24,39,.12); --text: #111827; --muted: #6b7280; --button: #111827; --button-text: #fff; --secondary: #e5e7eb; --secondary-text: #111827; --input: #fff; --label: #111827; }
    body.dark { color-scheme: dark; --panel-bg: rgba(17,24,39,.92); --panel-border: rgba(255,255,255,.16); --text: #f9fafb; --muted: #9ca3af; --button: #f9fafb; --button-text: #111827; --secondary: rgba(255,255,255,.14); --secondary-text: #f9fafb; --input: rgba(17,24,39,.85); --label: #f9fafb; }
    html, body, #map { height: 100%; margin: 0; }
    body { font: 13px/1.35 system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: var(--text); }
    .panel { position: absolute; top: 12px; right: 12px; z-index: 1; display: grid; gap: 12px; width: min(360px, calc(100vw - 24px)); padding: 14px; border: 1px solid var(--panel-border); border-radius: 18px; background: var(--panel-bg); box-shadow: 0 14px 40px rgba(0,0,0,.24); backdrop-filter: blur(12px); }
    .panel.collapsed { width: auto; }
    .panel.collapsed .panel-body { display: none; }
    .panel-header { display: flex; align-items: start; justify-content: space-between; gap: 12px; }
    .panel-title { display: grid; gap: 3px; }
    .panel h1 { margin: 0; font-size: 15px; }
    .panel-body { display: grid; gap: 12px; }
    .muted { color: var(--muted); }
    .control { display: grid; gap: 6px; }
    .row { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
    .buttons, .jump { display: grid; grid-template-columns: repeat(3, 1fr); gap: 6px; }
    .jump { grid-template-columns: .8fr 1fr 1fr auto; }
    .toggles { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
    .check { display: flex; align-items: center; gap: 8px; padding: 8px; border: 1px solid var(--panel-border); border-radius: 12px; background: color-mix(in srgb, var(--input) 85%, transparent); }
    button { border: 0; border-radius: 10px; padding: 8px 10px; background: var(--button); color: var(--button-text); font-weight: 650; cursor: pointer; }
    button.secondary { background: var(--secondary); color: var(--secondary-text); }
    input { min-width: 0; border: 1px solid #d1d5db; border-radius: 10px; padding: 8px; background: var(--input); color: var(--text); font: inherit; }
    input[type="checkbox"] { min-width: auto; accent-color: var(--button); }
    input[type="range"] { padding: 0; accent-color: var(--button); }
    .warning { max-width: 100%; color: #92400e; }
    .hidden { display: none; }
    @media (max-width: 640px) { .panel { left: 12px; right: 12px; bottom: 12px; top: auto; width: auto; } }
  </style>
</head>
<body>
  <div id="map"></div>
  <div id="panel" class="panel">
    <div class="panel-header">
      <div class="panel-title">
        <h1>geodot tile demo</h1>
        <div class="muted">Labels show <code>z/x/y</code>. Use <code>#12/2367/1306.jpg</code> to center a tile.</div>
      </div>
      <button id="togglePanel" type="button" class="secondary">Hide</button>
    </div>
    <div class="panel-body">
      <div class="toggles">
        <label class="check"><input id="labelsToggle" type="checkbox" checked> Labels</label>
        <button id="themeToggle" type="button" class="secondary">Dark theme</button>
      </div>
      <div class="control">
        <div class="row"><label for="opacity">Overlay transparency</label><strong id="opacityValue">65%</strong></div>
        <input id="opacity" type="range" min="0" max="1" step="0.05" value="0.65">
      </div>
      <div class="control">
        <div class="row"><span>View zoom</span><span id="viewZoom" class="muted"></span></div>
        <div class="buttons">
          <button id="zoomOut" type="button" class="secondary">−</button>
          <button id="fitTiles" type="button" class="secondary">Fit</button>
          <button id="zoomIn" type="button" class="secondary">+</button>
        </div>
      </div>
      <form id="jumpForm" class="control">
        <label>Jump to tile</label>
        <div class="jump">
          <input id="jumpZ" type="number" min="0" step="1" aria-label="z" placeholder="z">
          <input id="jumpX" type="number" min="0" step="1" aria-label="x" placeholder="x">
          <input id="jumpY" type="number" min="0" step="1" aria-label="y" placeholder="y">
          <button type="submit">Go</button>
        </div>
      </form>
      <div id="fileWarning" class="warning hidden">
        Local file mode cannot load tile files. Run geodot demo and open http://127.0.0.1:8000/.
      </div>
    </div>
  </div>
  <script src="https://unpkg.com/maplibre-gl@5.24.0/dist/maplibre-gl.js"></script>
  <script>
    const data = __GEODOT_DEMO_DATA__;
    const opacityInput = document.getElementById('opacity');
    const opacityValue = document.getElementById('opacityValue');
    const viewZoom = document.getElementById('viewZoom');
    const labelsToggle = document.getElementById('labelsToggle');
    const panel = document.getElementById('panel');
    if (location.protocol === 'file:') {
      document.getElementById('fileWarning').classList.remove('hidden');
    }
    const map = new maplibregl.Map({
      container: 'map',
      style: {
        version: 8,
        glyphs: 'https://demotiles.maplibre.org/font/{fontstack}/{range}.pbf',
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
      minZoom: Math.max(0, data.zoom - 8),
      maxZoom: data.zoom + 8,
      scrollZoom: false,
      boxZoom: false,
      doubleClickZoom: true,
      touchZoomRotate: true,
      keyboard: true,
      dragRotate: false,
      pitchWithRotate: false
    });

    function tileBounds(tile) {
      const n = 2 ** tile.z;
      const lonMin = tile.x / n * 360 - 180;
      const lonMax = (tile.x + 1) / n * 360 - 180;
      const latMax = Math.atan(Math.sinh(Math.PI * (1 - 2 * tile.y / n))) * 180 / Math.PI;
      const latMin = Math.atan(Math.sinh(Math.PI * (1 - 2 * (tile.y + 1) / n))) * 180 / Math.PI;
      return { lonMin, latMin, lonMax, latMax };
    }

    function tileCenter(tile) {
      const bounds = tileBounds(tile);
      return [(bounds.lonMin + bounds.lonMax) / 2, (bounds.latMin + bounds.latMax) / 2];
    }

    function tileFromLocation() {
      const value = location.hash.slice(1) || location.pathname.slice(1);
      const parts = value.split('/');
      if (parts.length !== 3) return undefined;
      const y = parts[2].split('.')[0];
      if (![parts[0], parts[1], y].every((part) => part && [...part].every((char) => char >= '0' && char <= '9'))) return undefined;
      return { z: Number(parts[0]), x: Number(parts[1]), y: Number(y) };
    }

    function setOpacity(value) {
      opacityValue.textContent = `${Math.round(value * 100)}%`;
      for (const tile of data.tiles) {
        const layer = `geodot-tile-${tile.z}-${tile.x}-${tile.y}`;
        if (map.getLayer(layer)) map.setPaintProperty(layer, 'raster-opacity', value);
      }
    }

    function updateZoomLabel() {
      viewZoom.textContent = map.getZoom().toFixed(2);
      if (map.getLayer('geodot-labels')) {
        map.setLayoutProperty('geodot-labels', 'text-size', Math.max(10, Math.min(24, 13 + (map.getZoom() - data.zoom) * 2)));
      }
    }

    function updateLabelStyle() {
      if (!map.getLayer('geodot-labels')) return;
      map.setLayoutProperty('geodot-labels', 'visibility', labelsToggle.checked ? 'visible' : 'none');
      map.setPaintProperty('geodot-labels', 'text-color', getComputedStyle(document.body).getPropertyValue('--label').trim());
    }

    function fillJump(tile) {
      document.getElementById('jumpZ').value = tile.z;
      document.getElementById('jumpX').value = tile.x;
      document.getElementById('jumpY').value = tile.y;
    }

    function jumpToTile(tile, updateHash = true) {
      fillJump(tile);
      map.easeTo({ center: tileCenter(tile), zoom: Math.max(map.getZoom(), tile.z), duration: 450 });
      if (updateHash) history.replaceState(null, '', `#${tile.z}/${tile.x}/${tile.y}.jpg`);
    }

    map.on('load', () => {
      for (const tile of data.tiles) {
        const bounds = tileBounds(tile);
        const id = `geodot-tile-${tile.z}-${tile.x}-${tile.y}`;
        map.addSource(id, {
          type: 'image',
          url: `./tiles/${tile.z}/${tile.x}/${tile.y}.jpg`,
          coordinates: [[bounds.lonMin, bounds.latMax], [bounds.lonMax, bounds.latMax], [bounds.lonMax, bounds.latMin], [bounds.lonMin, bounds.latMin]]
        });
        map.addLayer({ id, type: 'raster', source: id, paint: { 'raster-opacity': Number(opacityInput.value) } });
      }
      map.addSource('geodot-labels', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: data.tiles.map((tile) => ({ type: 'Feature', properties: { label: `${tile.z}/${tile.x}/${tile.y}` }, geometry: { type: 'Point', coordinates: tileCenter(tile) } })) }
      });
      map.addLayer({ id: 'geodot-labels', type: 'symbol', source: 'geodot-labels', layout: { 'text-field': ['get', 'label'], 'text-size': 13, 'text-font': ['Open Sans Bold'], 'text-allow-overlap': true }, paint: { 'text-color': '#111827', 'text-halo-width': 0 } });
      updateLabelStyle();
      updateZoomLabel();
      const requestedTile = tileFromLocation();
      if (requestedTile) jumpToTile(requestedTile, false);
    });

    opacityInput.addEventListener('input', (event) => setOpacity(Number(event.target.value)));
    document.getElementById('zoomOut').addEventListener('click', () => map.zoomOut());
    document.getElementById('zoomIn').addEventListener('click', () => map.zoomIn());
    document.getElementById('fitTiles').addEventListener('click', () => map.fitBounds(data.bounds, { padding: 48, duration: 450 }));
    document.getElementById('togglePanel').addEventListener('click', (event) => {
      panel.classList.toggle('collapsed');
      event.target.textContent = panel.classList.contains('collapsed') ? 'Show' : 'Hide';
    });
    document.getElementById('themeToggle').addEventListener('click', (event) => {
      document.body.classList.toggle('dark');
      event.target.textContent = document.body.classList.contains('dark') ? 'Light theme' : 'Dark theme';
      updateLabelStyle();
    });
    labelsToggle.addEventListener('change', updateLabelStyle);
    document.getElementById('jumpForm').addEventListener('submit', (event) => {
      event.preventDefault();
      jumpToTile({ z: Number(document.getElementById('jumpZ').value), x: Number(document.getElementById('jumpX').value), y: Number(document.getElementById('jumpY').value) });
    });
    map.on('zoom', updateZoomLabel);
    updateZoomLabel();
    if (data.tiles[0]) fillJump(data.tiles[0]);
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
    let mut rng = rand::rng();
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
