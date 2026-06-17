#!/usr/bin/env node
import {
  download,
  latlonToTile,
  loadGeoJSONPolygon,
  metersPerPixel,
  tilesForOptions,
} from "@geodot/lib";

const defaults = {
  lat: 55.7303,
  lon: 37.6504907,
  bottomRightLat: undefined,
  bottomRightLon: undefined,
  polygon: undefined,
  geojson: undefined,
  zoom: 18,
  cols: 3,
  rows: 3,
  out: "data",
  jobs: 16,
};

const flags = {
  "-y": "lat",
  "--lat": "lat",
  "-x": "lon",
  "--lon": "lon",
  "--x2": "bottomRightLon",
  "--bottom-right-lon": "bottomRightLon",
  "--y2": "bottomRightLat",
  "--bottom-right-lat": "bottomRightLat",
  "-p": "polygon",
  "--polygon": "polygon",
  "-g": "geojson",
  "--geojson": "geojson",
  "-z": "zoom",
  "--zoom": "zoom",
  "-c": "cols",
  "--cols": "cols",
  "-r": "rows",
  "--rows": "rows",
  "-o": "out",
  "--out": "out",
  "-j": "jobs",
  "--jobs": "jobs",
};

const integerOptions = new Set(["zoom", "cols", "rows", "jobs"]);
const ranges = {
  zoom: [0, 30],
  cols: [1, Number.MAX_SAFE_INTEGER],
  rows: [1, Number.MAX_SAFE_INTEGER],
  jobs: [1, Number.MAX_SAFE_INTEGER],
};

function parseArgs(argv) {
  const options = { ...defaults };
  for (let i = 0; i < argv.length; i += 2) {
    if (argv[i] === "-h" || argv[i] === "--help") {
      usage();
      process.exit(0);
    }
    const key = flags[argv[i]];
    if (!key || argv[i + 1] === undefined) {
      usage();
      process.exit(1);
    }
    if (key === "out" || key === "geojson") {
      options[key] = argv[i + 1];
    } else if (key === "polygon") {
      options[key] = parsePolygon(argv[i + 1]);
    } else {
      const value = Number(argv[i + 1]);
      if (!Number.isFinite(value)) {
        console.error(`${argv[i]} requires a number, got ${argv[i + 1]}`);
        usage();
        process.exit(1);
      }
      if (integerOptions.has(key) && !Number.isInteger(value)) {
        console.error(`${argv[i]} requires an integer, got ${argv[i + 1]}`);
        usage();
        process.exit(1);
      }
      const [min, max] = ranges[key] ?? [-Infinity, Infinity];
      if (value < min || value > max) {
        console.error(`${argv[i]} requires a value from ${min} to ${max}`);
        usage();
        process.exit(1);
      }
      options[key] = value;
    }
  }
  return options;
}

function parsePolygon(value) {
  const points = value.split(";").map((item) => {
    const [lon, lat] = item.split(",");
    return { lon: Number(lon), lat: Number(lat) };
  });
  if (
    points.length < 3 ||
    points.some(
      (point) => !Number.isFinite(point.lon) || !Number.isFinite(point.lat),
    )
  ) {
    throw new Error("polygon requires at least three lon,lat pairs");
  }
  return points;
}

function usage() {
  console.log(
    'Usage: geodot [-x lon] [-y lat] [--x2 lon --y2 lat] [-p|--polygon "lon,lat;lon,lat;lon,lat"] [-g|--geojson file-or-url] [-z zoom] [-c cols] [-r rows] [-o out] [-j jobs]',
  );
}

const options = parseArgs(process.argv.slice(2));
if (options.geojson && !options.polygon) {
  options.polygon = await loadGeoJSONPolygon(options.geojson);
}
const start = performance.now();
const center = latlonToTile(options.lat, options.lon, options.zoom);
const selectedTiles = tilesForOptions(options);

console.log("\n  geodot - satellite tiles");
console.log("  -------------------------------------");
console.log(`  Top-left: ${options.lat} ${options.lon}`);
console.log(`  Tile:     (${center.x}, ${center.y})  at zoom ${options.zoom}`);
console.log(`  Tiles:    ${selectedTiles.length}`);
console.log(
  `  m/px:     ${metersPerPixel(options.lat, options.zoom).toFixed(2)}`,
);
console.log(`  Output:   ${options.out}\n`);

const report = await download(options);
for (const item of report.tiles) {
  console.log(
    `  (${item.tile.x},${item.tile.y})  ${String(item.bytes).padStart(6)} B  ${item.path}`,
  );
}
for (const tile of report.failed) {
  console.log(`  (${tile.x},${tile.y})  FAILED`);
}
console.log("\n  -------------------------------------");
console.log(
  `  ${report.tiles.length} tiles  |  ${((performance.now() - start) / 1000).toFixed(1)}s  |  failed: ${report.failed.length}`,
);
