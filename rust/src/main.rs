use anyhow::Result;
use clap::Parser;
use geodot::{
    Coordinate, DownloadOptions, MAX_ZOOM, download, load_geojson_polygon, meters_per_pixel,
    tiles_for_options, validate_options,
};
use std::path::PathBuf;
use std::time::Instant;

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
}

#[tokio::main]
async fn main() -> Result<()> {
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
    };
    validate_options(&options)?;
    let center = geodot::latlon_to_tile(options.lat, options.lon, options.zoom);
    let selected_tiles = tiles_for_options(&options);

    println!();
    println!("  geodot - satellite tiles");
    println!("  -------------------------------------");
    println!("  Top-left: {} {}", options.lat, options.lon);
    println!(
        "  Tile:     ({}, {})  at zoom {}",
        center.x, center.y, options.zoom
    );
    println!("  Tiles:    {}", selected_tiles.len());
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

fn parse_zoom(value: &str) -> std::result::Result<u32, String> {
    let number: u32 = value
        .parse()
        .map_err(|_| format!("invalid integer: {value}"))?;
    if number > MAX_ZOOM {
        return Err(format!("must be from 0 to {MAX_ZOOM}"));
    }
    Ok(number)
}
