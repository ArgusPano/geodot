use anyhow::Result;
use clap::Parser;
use geodot::{
    Coordinate, DownloadOptions, MAX_ZOOM, count_tiles_for_options, download, load_geojson_polygon,
    meters_per_pixel, validate_options,
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
    version
)]
struct Args {
    /// Latitude of the top-left point
    #[arg(short = 'y', long, default_value = "55.7303", value_parser = parse_finite_f64)]
    lat: f64,

    /// Longitude of the top-left point
    #[arg(short = 'x', long, default_value = "37.6504907", value_parser = parse_finite_f64)]
    lon: f64,

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

#[tokio::main]
async fn main() -> Result<()> {
    let mut raw_args = std::env::args();
    let program = raw_args.next().unwrap_or_else(|| "geodot".to_string());
    if raw_args.next().as_deref() == Some("demo") {
        let args = std::iter::once(format!("{program} demo")).chain(raw_args);
        serve_demo(DemoArgs::parse_from(args))?;
        return Ok(());
    }

    let args = Args::parse();
    let start = Instant::now();
    let polygon = match (args.polygon, args.geojson.as_deref()) {
        (Some(polygon), _) => polygon,
        (None, Some(source)) => load_geojson_polygon(source).await?,
        (None, None) => Vec::new(),
    };
    let options = DownloadOptions {
        lat: args.lat,
        lon: args.lon,
        bottom_right_lat: args.bottom_right_lat,
        bottom_right_lon: args.bottom_right_lon,
        polygon,
        geojson: args.geojson,
        zoom: args.zoom,
        cols: args.cols,
        rows: args.rows,
        out: args.out,
        jobs: args.jobs,
        tile_url_template: None,
        no_manifest: args.no_manifest,
        no_demo: args.no_demo,
    };
    validate_options(&options)?;
    let center = geodot::latlon_to_tile(options.lat, options.lon, options.zoom);
    let selected_tile_count = count_tiles_for_options(&options);

    println!();
    println!("  geodot - satellite tiles");
    println!("  -------------------------------------");
    println!("  Top-left: {} {}", options.lat, options.lon);
    println!(
        "  Tile:     ({}, {})  at zoom {}",
        center.x, center.y, options.zoom
    );
    println!("  Tiles:    {}", selected_tile_count);
    println!(
        "  m/px:     {:.2}",
        meters_per_pixel(options.lat, options.zoom)
    );
    println!("  Output:   {}", options.out.display());
    println!();

    let report = download(options).await?;

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
    Ok(())
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
        let relative = if path == "/" {
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

fn parse_zoom(value: &str) -> std::result::Result<u32, String> {
    let number: u32 = value
        .parse()
        .map_err(|_| format!("invalid integer: {value}"))?;
    if number > MAX_ZOOM {
        return Err(format!("must be from 0 to {MAX_ZOOM}"));
    }
    Ok(number)
}
