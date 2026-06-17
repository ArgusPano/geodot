use anyhow::Result;
use clap::Parser;
use geodot::{Coordinate, DownloadOptions, download, meters_per_pixel, tiles_for_options};
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
    #[arg(short = 'y', long, default_value = "55.7303")]
    lat: f64,

    /// Longitude of the top-left point
    #[arg(short = 'x', long, default_value = "37.6504907")]
    lon: f64,

    /// Latitude of the bottom-right point
    #[arg(long = "y2", alias = "bottom-right-lat")]
    bottom_right_lat: Option<f64>,

    /// Longitude of the bottom-right point
    #[arg(long = "x2", alias = "bottom-right-lon")]
    bottom_right_lon: Option<f64>,

    /// Closed polygon as 'lon,lat;lon,lat;lon,lat'
    #[arg(short = 'p', long, value_parser = parse_polygon)]
    polygon: Option<Vec<Coordinate>>,

    /// Zoom level (1-22)
    #[arg(short, long, default_value = "18")]
    zoom: u32,

    /// Number of tile columns to the right of center
    #[arg(short, long, default_value = "3")]
    cols: u32,

    /// Number of tile rows downward from center
    #[arg(short, long, default_value = "3")]
    rows: u32,

    /// Output directory
    #[arg(short, long, default_value = "data")]
    out: PathBuf,

    /// Max concurrent downloads
    #[arg(short = 'j', long, default_value = "16")]
    jobs: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let start = Instant::now();
    let options = DownloadOptions {
        lat: args.lat,
        lon: args.lon,
        bottom_right_lat: args.bottom_right_lat,
        bottom_right_lon: args.bottom_right_lon,
        polygon: args.polygon.unwrap_or_default(),
        zoom: args.zoom,
        cols: args.cols,
        rows: args.rows,
        out: args.out,
        jobs: args.jobs,
        tile_url_template: None,
    };
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
    Ok(points)
}
