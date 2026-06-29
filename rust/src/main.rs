use anyhow::{Result, anyhow};
use clap::builder::ArgAction;
use clap::{CommandFactory, Parser};
use geodot::{
    Coordinate, DownloadOptions, DownloadProgress, MAX_ZOOM, PrepareOptions, SelectionProgress,
    count_tiles_for_options_with_progress, download_with_progress, load_geojson_polygon,
    meters_per_pixel, prepare_dataset, render_dataset, validate_dataset, validate_options,
};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::time::Instant;
use std::{fs, io::Write, net::TcpListener};

const EMPTY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0x64, 0xF8, 0xCF, 0x50,
    0x0F, 0x00, 0x03, 0x86, 0x01, 0x80, 0x5A, 0x34, 0x7D, 0x6B, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[derive(Parser, Debug)]
#[command(
    name = "geodot",
    about = "Download satellite map tiles (256x256 px).",
    version,
    disable_version_flag = true
)]
struct Args {
    /// Print version
    #[arg(short = 'v', long = "version", action = ArgAction::SetTrue)]
    version: bool,

    /// Latitude of the top-left point
    #[arg(short = 'y', long, value_parser = parse_finite_f64)]
    lat: Option<f64>,

    /// Longitude of the top-left point
    #[arg(short = 'x', long, value_parser = parse_finite_f64)]
    lon: Option<f64>,

    /// Latitude of the bottom-right point
    #[arg(long = "y2", alias = "bottom-right-lat", value_parser = parse_finite_f64)]
    bottom_right_lat: Option<f64>,

    /// Longitude of the bottom-right point
    #[arg(long = "x2", alias = "bottom-right-lon", value_parser = parse_finite_f64)]
    bottom_right_lon: Option<f64>,

    /// Closed polygon as 'lon,lat;lon,lat;lon,lat'
    #[arg(short = 'p', long, value_parser = parse_polygon)]
    polygon: Option<Vec<Coordinate>>,

    /// GeoJSON Polygon, Feature, or FeatureCollection file path or URL
    #[arg(short = 'g', long)]
    geojson: Option<String>,

    /// Zoom level (0-30)
    #[arg(short, long, default_value = "18", value_parser = parse_zoom)]
    zoom: u32,

    /// Number of tile columns to the right of center
    #[arg(short, long, default_value = "3", value_parser = parse_positive_u32)]
    cols: u32,

    /// Number of tile rows downward from center
    #[arg(short, long, default_value = "3", value_parser = parse_positive_u32)]
    rows: u32,

    /// Output directory
    #[arg(short, long, default_value = "data")]
    out: PathBuf,

    /// Max concurrent downloads
    #[arg(short = 'j', long, default_value = "16", value_parser = parse_positive_usize)]
    jobs: usize,

    /// Prepare a virtual VPR dataset from existing output tiles
    #[arg(long)]
    prepare: bool,

    /// Mosaic sizes in tiles for --prepare
    #[arg(long)]
    patch_sizes: Option<String>,

    /// Tile stride for --prepare mosaics
    #[arg(long, default_value = "1", value_parser = parse_positive_u32)]
    stride: u32,

    /// Rotation variants for --prepare
    #[arg(long)]
    rotations: Option<String>,

    /// Do not write manifest.json
    #[arg(long)]
    no_manifest: bool,

    /// Do not write index.html
    #[arg(long)]
    no_demo: bool,
}

#[derive(Parser, Debug)]
#[command(
    name = "geodot demo",
    about = "Serve a geodot output directory for the HTML demo."
)]
struct DemoArgs {
    /// Output directory to serve
    #[arg(short, long, default_value = "data")]
    out: PathBuf,

    /// Host to bind
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to bind
    #[arg(long, default_value = "8000", value_parser = parse_positive_u16)]
    port: u16,

    /// Do not open the browser
    #[arg(long)]
    no_open: bool,
}

#[derive(Parser, Debug)]
#[command(
    name = "geodot render",
    about = "Render one prepared patch or variant for debugging."
)]
struct RenderArgs {
    /// Prepared dataset directory
    #[arg(short, long = "output-dir", default_value = "data")]
    out: PathBuf,

    /// Patch ID to render
    #[arg(long, conflicts_with = "variant_id")]
    patch_id: Option<String>,

    /// Variant ID to render
    #[arg(long, conflicts_with = "patch_id")]
    variant_id: Option<String>,

    /// Preview image path to write
    #[arg(long = "out")]
    output: PathBuf,
}

#[derive(Parser, Debug)]
#[command(name = "geodot validate", about = "Validate a prepared VPR dataset.")]
struct ValidateArgs {
    /// Prepared dataset directory
    #[arg(short, long, default_value = "data")]
    out: PathBuf,

    /// Treat warnings as errors
    #[arg(long)]
    strict: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut raw_args = std::env::args();
    let program = raw_args.next().unwrap_or_else(|| "geodot".to_string());
    match raw_args.next().as_deref() {
        None => {
            Args::command().print_help()?;
            println!();
            return Ok(());
        }
        Some("-v" | "--version") => {
            println!(env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("demo") => {
            let args = std::iter::once(format!("{program} demo")).chain(raw_args);
            serve_demo(DemoArgs::parse_from(args))?;
            return Ok(());
        }
        Some("render") => {
            let args = std::iter::once(format!("{program} render")).chain(raw_args);
            render_preview(RenderArgs::parse_from(args))?;
            return Ok(());
        }
        Some("validate") => {
            let args = std::iter::once(format!("{program} validate")).chain(raw_args);
            validate_prepared_dataset(ValidateArgs::parse_from(args))?;
            return Ok(());
        }
        _ => {}
    }

    let args = Args::parse();
    if args.version {
        println!(env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.prepare {
        let should_download = args.geojson.is_some()
            || args.polygon.is_some()
            || args.bottom_right_lat.is_some()
            || args.bottom_right_lon.is_some();
        if !should_download {
            let defaults = PrepareOptions::default();
            let report = prepare_dataset(PrepareOptions {
                out: args.out,
                patch_sizes: match &args.patch_sizes {
                    Some(value) => parse_u32_list(value).map_err(|error| anyhow!(error))?,
                    None => defaults.patch_sizes,
                },
                stride: args.stride,
                rotations: match &args.rotations {
                    Some(value) => parse_u32_list(value).map_err(|error| anyhow!(error))?,
                    None => defaults.rotations,
                },
                auto400m: args.patch_sizes.is_none(),
            })?;
            print_prepare_report(&report);
            return Ok(());
        }
    }
    let start = Instant::now();
    let polygon = match (args.polygon.clone(), args.geojson.as_deref()) {
        (Some(polygon), _) => polygon,
        (None, Some(source)) => load_geojson_polygon(source).await?,
        (None, None) => Vec::new(),
    };
    let (lat, lon) = selection_origin(args.lat, args.lon, &polygon)?;
    let options = DownloadOptions {
        lat,
        lon,
        bottom_right_lat: args.bottom_right_lat,
        bottom_right_lon: args.bottom_right_lon,
        polygon,
        geojson: args.geojson,
        zoom: args.zoom,
        cols: args.cols,
        rows: args.rows,
        out: args.out.clone(),
        jobs: args.jobs,
        tile_url_template: None,
        no_manifest: args.no_manifest,
        no_demo: args.no_demo,
    };
    validate_options(&options)?;
    let center = geodot::latlon_to_tile(options.lat, options.lon, options.zoom);

    println!();
    println!("  geodot - satellite tiles");
    println!("  -------------------------------------");
    println!("  Top-left: {} {}", options.lat, options.lon);
    println!(
        "  Tile:     ({}, {})  at zoom {}",
        center.x, center.y, options.zoom
    );
    println!("  Selecting tiles...");
    let mut selecting = selection_progress_printer();
    let selected_tile_count = count_tiles_for_options_with_progress(&options, &mut selecting);
    println!("  Tiles:    {}", selected_tile_count);
    println!(
        "  m/px:     {:.2}",
        meters_per_pixel(options.lat, options.zoom)
    );
    println!("  Output:   {}", options.out.display());
    println!();

    let mut downloading = download_progress_printer(selected_tile_count);
    let report = download_with_progress(options, &mut downloading).await?;
    downloading(DownloadProgress {
        completed: report.tiles.len() + report.failed.len(),
        downloaded: report.tiles.len(),
        failed: report.failed.len(),
        tile: center,
    });

    for tile in &report.tiles {
        println!(
            "  ({},{})  {:>6} B  {}",
            tile.tile.x,
            tile.tile.y,
            tile.bytes,
            tile.path.display()
        );
    }
    for tile in &report.failed {
        eprintln!("  ({},{})  FAILED", tile.x, tile.y);
    }

    println!();
    println!("  -------------------------------------");
    println!(
        "  {} tiles  |  {:.1}s  |  failed: {}",
        report.tiles.len(),
        start.elapsed().as_secs_f64(),
        report.failed.len()
    );
    if args.prepare {
        let defaults = PrepareOptions::default();
        let report = prepare_dataset(PrepareOptions {
            out: args.out,
            patch_sizes: match &args.patch_sizes {
                Some(value) => parse_u32_list(value).map_err(|error| anyhow!(error))?,
                None => defaults.patch_sizes,
            },
            stride: args.stride,
            rotations: match &args.rotations {
                Some(value) => parse_u32_list(value).map_err(|error| anyhow!(error))?,
                None => defaults.rotations,
            },
            auto400m: args.patch_sizes.is_none(),
        })?;
        print_prepare_report(&report);
    }
    Ok(())
}

fn selection_origin(
    lat: Option<f64>,
    lon: Option<f64>,
    polygon: &[Coordinate],
) -> Result<(f64, f64)> {
    if let (Some(lat), Some(lon)) = (lat, lon) {
        return Ok((lat, lon));
    }
    if !polygon.is_empty() {
        let lat = polygon
            .iter()
            .map(|point| point.lat)
            .fold(f64::NEG_INFINITY, f64::max);
        let lon = polygon
            .iter()
            .map(|point| point.lon)
            .fold(f64::INFINITY, f64::min);
        return Ok((lat, lon));
    }
    Err(anyhow!(
        "geodot requires -x/--lon and -y/--lat for grid or rectangle downloads"
    ))
}

fn render_preview(args: RenderArgs) -> Result<()> {
    if args.patch_id.is_none() && args.variant_id.is_none() {
        return Err(anyhow!("provide --patch-id or --variant-id"));
    }
    let report = render_dataset(
        args.out,
        args.patch_id.as_deref(),
        args.variant_id.as_deref(),
        args.output,
    )?;
    println!();
    println!("  geodot - render preview");
    println!("  -------------------------------------");
    println!("  Source: {}", report.source_path.display());
    println!("  Output: {}", report.output_path.display());
    println!("  Bytes:  {}", report.bytes);
    Ok(())
}

fn validate_prepared_dataset(args: ValidateArgs) -> Result<()> {
    let report = match validate_dataset(args.out, args.strict) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("geodot validate: {error}");
            std::process::exit(2);
        }
    };
    println!();
    println!("  geodot - dataset validation");
    println!("  -------------------------------------");
    for (label, key) in [
        ("Tiles", "tiles"),
        ("Patches", "patches"),
        ("Variants", "variants"),
        ("Places", "places"),
        ("Query tiles", "query_tiles"),
        ("Reference tiles", "reference_tiles"),
        ("Warnings", "warnings"),
        ("Errors", "errors"),
    ] {
        println!(
            "  {label}: {}",
            report.counts.get(key).copied().unwrap_or(0)
        );
    }
    for warning in &report.warnings {
        eprintln!("  WARNING: {warning}");
    }
    for error in &report.errors {
        eprintln!("  ERROR: {error}");
    }
    if !report.valid {
        std::process::exit(1);
    }
    Ok(())
}

fn print_prepare_report(report: &geodot::PrepareReport) {
    println!();
    println!("  geodot - dataset preparation");
    println!("  -------------------------------------");
    println!("  Tiles:    {}", report.tiles);
    println!("  Patches:  {}", report.patches);
    println!("  Variants: {}", report.variants);
    println!("  Output:   {}", report.path.display());
}

fn selection_progress_printer() -> impl FnMut(SelectionProgress) {
    let mut last = Instant::now();
    move |progress| {
        let now = Instant::now();
        if progress.total != 0 && now.duration_since(last).as_secs_f64() < 1.0 {
            return;
        }
        last = now;
        let percent = if progress.total == 0 {
            String::new()
        } else {
            format!(
                " ({:.1}%)",
                progress.scanned as f64 / progress.total as f64 * 100.0
            )
        };
        eprintln!(
            "  Selecting: scanned {}{}, matched {}",
            progress.scanned, percent, progress.selected
        );
    }
}

fn download_progress_printer(total: usize) -> impl FnMut(DownloadProgress) {
    let mut last = Instant::now();
    move |progress| {
        let now = Instant::now();
        if progress.completed != total && now.duration_since(last).as_secs_f64() < 1.0 {
            return;
        }
        last = now;
        let percent = if total == 0 {
            String::new()
        } else {
            format!(
                " ({:.1}%)",
                progress.completed as f64 / total as f64 * 100.0
            )
        };
        eprintln!(
            "  Downloading: {}/{}{}, ok {}, failed {}",
            progress.completed, total, percent, progress.downloaded, progress.failed
        );
    }
}

fn serve_demo(args: DemoArgs) -> Result<()> {
    let root = args.out.canonicalize().unwrap_or(args.out.clone());
    let listener = TcpListener::bind((args.host.as_str(), args.port))?;
    let url = format!("http://{}:{}/", args.host, args.port);
    println!("Serving {} at {url}", root.display());
    if !args.no_open {
        open_browser(&url);
    }
    for stream in listener.incoming() {
        let mut stream = stream?;
        let mut buffer = [0; 2048];
        let size = std::io::Read::read(&mut stream, &mut buffer)?;
        let request = String::from_utf8_lossy(&buffer[..size]);
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        let path = percent_decode(path.split('?').next().unwrap_or("/"));
        let relative = if path == "/" || is_tile_route(&path) {
            "index.html"
        } else {
            path.trim_start_matches('/')
        };
        if relative.split('/').any(|part| part == "..") {
            write_response(&mut stream, "403 Forbidden", "text/plain", b"Forbidden")?;
            continue;
        }
        let file = root.join(relative);
        if !file.starts_with(&root) {
            write_response(&mut stream, "403 Forbidden", "text/plain", b"Forbidden")?;
            continue;
        }
        match fs::read(&file) {
            Ok(bytes) if file.is_file() => {
                write_response(&mut stream, "200 OK", content_type(&file), &bytes)?
            }
            _ => {
                if relative.starts_with("tiles/") && relative.ends_with(".jpg") {
                    write_response(&mut stream, "200 OK", "image/png", EMPTY_PNG)?
                } else {
                    write_response(&mut stream, "404 Not Found", "text/plain", b"Not found")?
                }
            }
        }
    }
    Ok(())
}

fn is_tile_route(path: &str) -> bool {
    let mut parts = path.trim_start_matches('/').split('/');
    let Some(z) = parts.next() else { return false };
    let Some(x) = parts.next() else { return false };
    let Some(y) = parts.next() else { return false };
    if parts.next().is_some() {
        return false;
    }
    z.parse::<u32>().is_ok()
        && x.parse::<u32>().is_ok()
        && y.split('.')
            .next()
            .is_some_and(|value| value.parse::<u32>().is_ok())
}

fn write_response(
    stream: &mut std::net::TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    Ok(())
}

fn content_type(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(hex) = u8::from_str_radix(&value[index + 1..index + 3], 16)
        {
            decoded.push(hex);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = ProcessCommand::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = ProcessCommand::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut command = ProcessCommand::new("xdg-open");
        command.arg(url);
        command
    };
    let _ = command.spawn();
}

fn parse_polygon(value: &str) -> std::result::Result<Vec<Coordinate>, String> {
    let mut points = Vec::new();
    for item in value.split(';') {
        let (lon, lat) = item
            .split_once(',')
            .ok_or_else(|| "polygon points must be lon,lat pairs".to_string())?;
        points.push(Coordinate {
            lon: lon
                .parse()
                .map_err(|_| format!("invalid longitude: {lon}"))?,
            lat: lat
                .parse()
                .map_err(|_| format!("invalid latitude: {lat}"))?,
        });
    }
    if points.len() < 3 {
        return Err("polygon requires at least three lon,lat pairs".to_string());
    }
    if points
        .iter()
        .any(|point| !point.lon.is_finite() || !point.lat.is_finite())
    {
        return Err("polygon coordinates must be finite numbers".to_string());
    }
    Ok(points)
}

fn parse_finite_f64(value: &str) -> std::result::Result<f64, String> {
    let number: f64 = value
        .parse()
        .map_err(|_| format!("invalid number: {value}"))?;
    if !number.is_finite() {
        return Err("must be a finite number".to_string());
    }
    Ok(number)
}

fn parse_positive_u32(value: &str) -> std::result::Result<u32, String> {
    let number: u32 = value
        .parse()
        .map_err(|_| format!("invalid integer: {value}"))?;
    if number == 0 {
        return Err("must be at least 1".to_string());
    }
    Ok(number)
}

fn parse_positive_usize(value: &str) -> std::result::Result<usize, String> {
    let number: usize = value
        .parse()
        .map_err(|_| format!("invalid integer: {value}"))?;
    if number == 0 {
        return Err("must be at least 1".to_string());
    }
    Ok(number)
}

fn parse_positive_u16(value: &str) -> std::result::Result<u16, String> {
    let number: u16 = value
        .parse()
        .map_err(|_| format!("invalid integer: {value}"))?;
    if number == 0 {
        return Err("must be at least 1".to_string());
    }
    Ok(number)
}

fn parse_u32_list(value: &str) -> std::result::Result<Vec<u32>, String> {
    let numbers: Result<Vec<_>, _> = value
        .split(',')
        .filter(|item| !item.is_empty())
        .map(|item| {
            item.parse::<u32>()
                .map_err(|_| format!("invalid integer list: {value}"))
        })
        .collect();
    let numbers = numbers?;
    if numbers.is_empty() {
        return Err("must contain at least one integer".to_string());
    }
    Ok(numbers)
}

fn parse_zoom(value: &str) -> std::result::Result<u32, String> {
    let number: u32 = value
        .parse()
        .map_err(|_| format!("invalid integer: {value}"))?;
    if number > MAX_ZOOM {
        return Err(format!("must be from 0 to {MAX_ZOOM}"));
    }
    Ok(number)
}
