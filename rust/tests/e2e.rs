use geodot::{DownloadOptions, PrepareOptions, download, prepare_dataset};
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

const TILE_BYTES: &[u8] = &[b'x'; 128];
const GEOJSON: &str = r#"{
  "type": "FeatureCollection",
  "features": [{
    "type": "Feature",
    "geometry": {
      "type": "Polygon",
      "coordinates": [[
        [37.6504, 55.7304],
        [37.6520, 55.7304],
        [37.6520, 55.7297],
        [37.6504, 55.7297],
        [37.6504, 55.7304]
      ]]
    }
  }]
}"#;

#[test]
fn library_and_cli_write_tiles_and_manifest() {
    let (template, server) = tile_server(2);

    let lib_out = temp_dir("geodot-lib");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let report = runtime
        .block_on(download(DownloadOptions {
            lat: 55.7303,
            lon: 37.6504907,
            zoom: 18,
            cols: 1,
            rows: 1,
            out: lib_out.clone(),
            jobs: 1,
            tile_url_template: Some(template.clone()),
            ..DownloadOptions::default()
        }))
        .unwrap();
    assert_eq!(report.tiles.len(), 1);
    assert!(report.failed.is_empty());
    assert_download_output(&lib_out);

    let cli_out = temp_dir("geodot-cli");
    let output = Command::new(env!("CARGO_BIN_EXE_geodot"))
        .env("GEODOT_TILE_URL_TEMPLATE", &template)
        .args([
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
            cli_out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_download_output(&cli_out);

    server.join().unwrap();
    fs::remove_dir_all(lib_out).unwrap();
    fs::remove_dir_all(cli_out).unwrap();
}

#[test]
fn library_accepts_local_geojson() {
    let (template, server) = tile_server(4);
    let out = temp_dir("geodot-lib-geojson");
    let geojson_file = out.join("area.geojson");
    fs::write(&geojson_file, GEOJSON).unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let report = runtime
        .block_on(download(DownloadOptions {
            geojson: Some(geojson_file.to_string_lossy().into_owned()),
            zoom: 18,
            out: out.clone(),
            jobs: 1,
            tile_url_template: Some(template),
            ..DownloadOptions::default()
        }))
        .unwrap();
    assert_eq!(report.tiles.len(), 4);
    assert!(report.failed.is_empty());
    assert_download_output(&out);

    server.join().unwrap();
    fs::remove_dir_all(out).unwrap();
}

#[test]
fn cli_accepts_geojson_url() {
    let (template, server) = tile_server(5);
    let out = temp_dir("geodot-cli-geojson");
    let base_url = template.split("/{z}").next().unwrap();
    let geojson_url = format!("{base_url}/area.geojson");

    let output = Command::new(env!("CARGO_BIN_EXE_geodot"))
        .env("GEODOT_TILE_URL_TEMPLATE", &template)
        .args([
            "--geojson",
            &geojson_url,
            "-z",
            "18",
            "-j",
            "1",
            "-o",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_download_output(&out);

    server.join().unwrap();
    fs::remove_dir_all(out).unwrap();
}

#[test]
fn library_and_cli_prepare_existing_tiles() {
    let lib_out = temp_dir("geodot-lib-prepare");
    write_prepare_tiles(&lib_out);
    let report = prepare_dataset(PrepareOptions {
        out: lib_out.clone(),
        patch_sizes: vec![1, 2],
        stride: 1,
        rotations: vec![0, 90],
        auto400m: false,
    })
    .unwrap();
    assert_eq!(report.tiles, 4);
    assert_eq!(report.patches, 5);
    assert_eq!(report.variants, 10);
    assert_prepare_output(&lib_out);

    let cli_out = temp_dir("geodot-cli-prepare");
    write_prepare_tiles(&cli_out);
    let output = Command::new(env!("CARGO_BIN_EXE_geodot"))
        .args([
            "--prepare",
            "-o",
            cli_out.to_str().unwrap(),
            "--patch-sizes",
            "1,2",
            "--rotations",
            "0,90",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("dataset preparation"));
    assert_prepare_output(&cli_out);

    fs::remove_dir_all(lib_out).unwrap();
    fs::remove_dir_all(cli_out).unwrap();
}

#[test]
fn cli_can_skip_manifest_and_still_write_demo() {
    let (template, server) = tile_server(1);
    let out = temp_dir("geodot-cli-no-manifest");

    let output = Command::new(env!("CARGO_BIN_EXE_geodot"))
        .env("GEODOT_TILE_URL_TEMPLATE", &template)
        .args([
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
            out.to_str().unwrap(),
            "--no-manifest",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!out.join("manifest.json").exists());
    let demo = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(demo.contains("maplibregl.Map"));
    assert!(demo.contains("World_Imagery"));
    assert!(demo.contains("./tiles/{z}/{x}/{y}.jpg"));
    assert!(!demo.contains("%7Bz%7D"));
    assert!(demo.contains("minZoom: data.zoom"));
    assert!(!demo.contains("fitBounds"));

    server.join().unwrap();
    fs::remove_dir_all(out).unwrap();
}

#[test]
fn cli_can_skip_demo() {
    let (template, server) = tile_server(1);
    let out = temp_dir("geodot-cli-no-demo");

    let output = Command::new(env!("CARGO_BIN_EXE_geodot"))
        .env("GEODOT_TILE_URL_TEMPLATE", &template)
        .args([
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
            out.to_str().unwrap(),
            "--no-demo",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(out.join("manifest.json").exists());
    assert!(!out.join("index.html").exists());

    server.join().unwrap();
    fs::remove_dir_all(out).unwrap();
}

#[test]
fn cli_exposes_demo_command_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_geodot"))
        .args(["demo", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--port"));
    assert!(stdout.contains("--no-open"));
}

#[test]
fn cli_rejects_invalid_numeric_options() {
    let output = Command::new(env!("CARGO_BIN_EXE_geodot"))
        .args(["-j", "https://example.com/area.geojson"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid integer"));

    let output = Command::new(env!("CARGO_BIN_EXE_geodot"))
        .args(["--lat", "NaN"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must be a finite number"));
}

fn tile_server(requests: usize) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        for stream in listener.incoming().take(requests) {
            let mut stream = stream.unwrap();
            let mut buffer = [0; 1024];
            let size = stream.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..size]);
            if request.starts_with("GET /area.geojson ") {
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/geo+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    GEOJSON.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.write_all(GEOJSON.as_bytes()).unwrap();
                continue;
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                TILE_BYTES.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(TILE_BYTES).unwrap();
        }
    });
    (
        format!("http://127.0.0.1:{port}/{{z}}/{{x}}/{{y}}.jpg"),
        server,
    )
}

fn assert_download_output(out: &Path) {
    let demo = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(demo.contains("maplibregl.Map"));
    assert!(demo.contains("World_Imagery"));
    assert!(demo.contains("./tiles/{z}/{x}/{y}.jpg"));
    assert!(!demo.contains("%7Bz%7D"));
    assert!(demo.contains("minZoom: data.zoom"));
    assert!(!demo.contains("fitBounds"));

    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    let tile = &manifest["tiles"][0];
    assert_eq!(tile["bytes"], TILE_BYTES.len());
    assert!(tile["bounds"]["lat_min"].is_number());
    assert!(tile["bounds"]["lon_min"].is_number());
    assert!(tile["bounds"]["lat_max"].is_number());
    assert!(tile["bounds"]["lon_max"].is_number());

    let z = tile["tile"]["z"].as_u64().unwrap();
    let x = tile["tile"]["x"].as_u64().unwrap();
    let y = tile["tile"]["y"].as_u64().unwrap();
    let bytes = fs::read(
        out.join("tiles")
            .join(z.to_string())
            .join(x.to_string())
            .join(format!("{y}.jpg")),
    )
    .unwrap();
    assert_eq!(bytes, TILE_BYTES);
}

fn write_prepare_tiles(out: &Path) {
    for x in [1, 2] {
        for y in [3, 4] {
            let path = out
                .join("tiles")
                .join("3")
                .join(x.to_string())
                .join(format!("{y}.jpg"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, TILE_BYTES).unwrap();
        }
    }
}

fn assert_prepare_output(out: &Path) {
    let patches: Value = serde_json::from_str(
        &fs::read_to_string(out.join("vpr").join("manifest").join("patches.json")).unwrap(),
    )
    .unwrap();
    let variants: Value = serde_json::from_str(
        &fs::read_to_string(out.join("vpr").join("manifest").join("variants.json")).unwrap(),
    )
    .unwrap();
    let dataset: Value = serde_json::from_str(
        &fs::read_to_string(out.join("vpr").join("config").join("dataset.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(patches.as_array().unwrap().len(), 5);
    assert_eq!(variants.as_array().unwrap().len(), 10);
    assert_eq!(dataset["mode"], "virtual");
    let mosaic = patches
        .as_array()
        .unwrap()
        .iter()
        .find(|patch| patch["mosaic_size_tiles"] == 2)
        .unwrap();
    assert_eq!(mosaic["source_x_min"], 1);
    assert_eq!(mosaic["source_x_max"], 2);
    assert_eq!(mosaic["source_y_min"], 3);
    assert_eq!(mosaic["source_y_max"], 4);
    assert_eq!(
        mosaic["image_path_or_virtual_spec"]["type"],
        "virtual_mosaic"
    );
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
    fs::create_dir_all(&path).unwrap();
    path
}
