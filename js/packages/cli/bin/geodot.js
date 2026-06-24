#!/usr/bin/env node
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { spawn } from "node:child_process";
import { URL } from "node:url";

const EMPTY_PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
  "base64",
);
import {
  download,
  countTilesForOptions,
  latlonToTile,
  loadGeoJSONPolygon,
  metersPerPixel,
  prepareDataset,
  renderDataset,
  validateDataset,
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
  noManifest: false,
  noDemo: false,
  prepare: false,
  patchSizes: undefined,
  stride: 1,
  rotations: undefined,
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
  "--patch-sizes": "patchSizes",
  "--stride": "stride",
  "--rotations": "rotations",
};

const booleanFlags = {
  "--no-manifest": "noManifest",
  "--no-demo": "noDemo",
  "--prepare": "prepare",
};

const integerOptions = new Set(["zoom", "cols", "rows", "jobs", "stride"]);
const ranges = {
  zoom: [0, 30],
  cols: [1, Number.MAX_SAFE_INTEGER],
  rows: [1, Number.MAX_SAFE_INTEGER],
  jobs: [1, Number.MAX_SAFE_INTEGER],
  stride: [1, Number.MAX_SAFE_INTEGER],
};

function parseArgs(argv) {
  const options = { ...defaults };
  for (let i = 0; i < argv.length; ) {
    if (argv[i] === "-h" || argv[i] === "--help") {
      usage();
      process.exit(0);
    }
    const booleanKey = booleanFlags[argv[i]];
    if (booleanKey) {
      options[booleanKey] = true;
      i += 1;
      continue;
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
    } else if (key === "patchSizes" || key === "rotations") {
      options[key] = parseIntegerList(argv[i + 1], argv[i]);
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
    i += 2;
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

function parseIntegerList(value, flag) {
  const numbers = value.split(",").filter(Boolean).map(Number);
  if (
    numbers.length === 0 ||
    numbers.some((number) => !Number.isInteger(number))
  ) {
    console.error(
      `${flag} requires a comma-separated integer list, got ${value}`,
    );
    usage();
    process.exit(1);
  }
  return numbers;
}

function usage() {
  console.log(
    'Usage: geodot [--prepare] [-x lon] [-y lat] [--x2 lon --y2 lat] [-p|--polygon "lon,lat;lon,lat;lon,lat"] [-g|--geojson file-or-url] [-z zoom] [-c cols] [-r rows] [-o out] [-j jobs] [--patch-sizes list] [--stride n] [--rotations list] [--no-manifest] [--no-demo]\n       geodot validate -o data [--strict]\n       geodot render -o data (--patch-id id | --variant-id id) --out preview.jpg\n       geodot demo [-o out] [--host host] [--port port] [--no-open]',
  );
}

function parseValidateArgs(argv) {
  const options = { out: "data", strict: false };
  for (let i = 0; i < argv.length; ) {
    if (argv[i] === "--strict") {
      options.strict = true;
      i += 1;
      continue;
    }
    if (argv[i] === "-h" || argv[i] === "--help") {
      usage();
      process.exit(0);
    }
    if (
      (argv[i] === "-o" || argv[i] === "--out") &&
      argv[i + 1] !== undefined
    ) {
      options.out = argv[i + 1];
      i += 2;
      continue;
    }
    usage();
    process.exit(2);
  }
  return options;
}

async function runValidate(argv) {
  const options = parseValidateArgs(argv);
  let report;
  try {
    report = await validateDataset(options.out, { strict: options.strict });
  } catch (error) {
    console.error(`geodot validate: ${error.message}`);
    process.exit(2);
  }
  console.log("\n  geodot - dataset validation");
  console.log("  -------------------------------------");
  for (const [label, key] of [
    ["Tiles", "tiles"],
    ["Patches", "patches"],
    ["Variants", "variants"],
    ["Places", "places"],
    ["Query tiles", "query_tiles"],
    ["Reference tiles", "reference_tiles"],
    ["Warnings", "warnings"],
    ["Errors", "errors"],
  ]) {
    console.log(`  ${label}: ${report.counts[key]}`);
  }
  for (const warning of report.warnings) console.error(`  WARNING: ${warning}`);
  for (const error of report.errors) console.error(`  ERROR: ${error}`);
  if (!report.valid) process.exit(1);
}

function parseRenderArgs(argv) {
  const options = {
    out: "data",
    output: undefined,
    patchId: undefined,
    variantId: undefined,
  };
  for (let i = 0; i < argv.length; ) {
    const value = argv[i + 1];
    if (argv[i] === "-h" || argv[i] === "--help") {
      usage();
      process.exit(0);
    }
    if (value === undefined) {
      usage();
      process.exit(1);
    }
    if (argv[i] === "-o") options.out = value;
    else if (argv[i] === "--out" || argv[i] === "--output")
      options.output = value;
    else if (argv[i] === "--patch-id") options.patchId = value;
    else if (argv[i] === "--variant-id") options.variantId = value;
    else {
      usage();
      process.exit(1);
    }
    i += 2;
  }
  if (
    !options.output ||
    Boolean(options.patchId) === Boolean(options.variantId)
  ) {
    usage();
    process.exit(1);
  }
  return options;
}

async function runRender(argv) {
  const report = await renderDataset(parseRenderArgs(argv));
  console.log("\n  geodot - render preview");
  console.log("  -------------------------------------");
  console.log(`  Source: ${report.sourcePath}`);
  console.log(`  Output: ${report.outputPath}`);
  console.log(`  Bytes:  ${report.bytes}`);
}

function parseDemoArgs(argv) {
  const options = { out: "data", host: "127.0.0.1", port: 8000, open: true };
  for (let i = 0; i < argv.length; ) {
    if (argv[i] === "-h" || argv[i] === "--help") {
      usage();
      process.exit(0);
    }
    if (argv[i] === "--no-open") {
      options.open = false;
      i += 1;
      continue;
    }
    const value = argv[i + 1];
    if (value === undefined) {
      usage();
      process.exit(1);
    }
    if (argv[i] === "-o" || argv[i] === "--out") {
      options.out = value;
    } else if (argv[i] === "--host") {
      options.host = value;
    } else if (argv[i] === "--port") {
      options.port = Number(value);
      if (!Number.isInteger(options.port) || options.port < 1) {
        console.error(`${argv[i]} requires a positive integer, got ${value}`);
        process.exit(1);
      }
    } else {
      usage();
      process.exit(1);
    }
    i += 2;
  }
  return options;
}

function serveDemo(options) {
  const root = path.resolve(options.out);
  const server = createServer(async (request, response) => {
    const url = new URL(
      request.url ?? "/",
      `http://${options.host}:${options.port}`,
    );
    const decoded = decodeURIComponent(url.pathname);
    const requested = decoded === "/" ? "/index.html" : decoded;
    const file = path.resolve(root, `.${requested}`);
    if (!file.startsWith(`${root}${path.sep}`) && file !== root) {
      response.writeHead(403);
      response.end("Forbidden");
      return;
    }
    try {
      const info = await stat(file);
      if (!info.isFile()) throw new Error("not a file");
      response.writeHead(200, { "Content-Type": contentType(file) });
      createReadStream(file).pipe(response);
    } catch {
      if (decoded.startsWith("/tiles/") && decoded.endsWith(".jpg")) {
        response.writeHead(200, {
          "Content-Type": "image/png",
          "Content-Length": EMPTY_PNG.length,
        });
        response.end(EMPTY_PNG);
      } else {
        response.writeHead(404);
        response.end("Not found");
      }
    }
  });
  server.listen(options.port, options.host, () => {
    const url = `http://${options.host}:${options.port}/`;
    console.log(`Serving ${root} at ${url}`);
    if (options.open) openBrowser(url);
  });
}

function contentType(file) {
  if (file.endsWith(".html")) return "text/html; charset=utf-8";
  if (file.endsWith(".jpg") || file.endsWith(".jpeg")) return "image/jpeg";
  if (file.endsWith(".json")) return "application/json";
  return "application/octet-stream";
}

function openBrowser(url) {
  const command =
    process.platform === "darwin"
      ? "open"
      : process.platform === "win32"
        ? "cmd"
        : "xdg-open";
  const args = process.platform === "win32" ? ["/c", "start", "", url] : [url];
  const child = spawn(command, args, { detached: true, stdio: "ignore" });
  child.unref();
}

async function runDownload() {
  const options = parseArgs(process.argv.slice(2));
  if (options.prepare) {
    const shouldDownload = Boolean(
      options.geojson ||
      options.polygon ||
      options.bottomRightLat !== undefined ||
      options.bottomRightLon !== undefined,
    );
    if (!shouldDownload) {
      const prepareOptions = {
        out: options.out,
        stride: options.stride,
        ...(options.patchSizes ? { patchSizes: options.patchSizes } : {}),
        ...(options.rotations ? { rotations: options.rotations } : {}),
      };
      const report = await prepareDataset(prepareOptions);
      printPrepareReport(report);
      return;
    }
  }
  if (options.geojson && !options.polygon) {
    options.polygon = await loadGeoJSONPolygon(options.geojson);
  }
  const start = performance.now();
  const center = latlonToTile(options.lat, options.lon, options.zoom);

  console.log("\n  geodot - satellite tiles");
  console.log("  -------------------------------------");
  console.log(`  Top-left: ${options.lat} ${options.lon}`);
  console.log(
    `  Tile:     (${center.x}, ${center.y})  at zoom ${options.zoom}`,
  );
  console.log("  Selecting tiles...");
  const selecting = progressPrinter("select");
  const selectedTileCount = countTilesForOptions(options, selecting);
  console.log(`  Tiles:    ${selectedTileCount}`);
  console.log(
    `  m/px:     ${metersPerPixel(options.lat, options.zoom).toFixed(2)}`,
  );
  console.log(`  Output:   ${options.out}\n`);

  const downloading = progressPrinter("download", selectedTileCount);
  const report = await download({ ...options, onProgress: downloading });
  downloading({
    phase: "download",
    completed: report.tiles.length + report.failed.length,
    downloaded: report.tiles.length,
    failed: report.failed.length,
    done: true,
  });
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
  if (options.prepare) {
    const prepareOptions = {
      out: options.out,
      stride: options.stride,
      ...(options.patchSizes ? { patchSizes: options.patchSizes } : {}),
      ...(options.rotations ? { rotations: options.rotations } : {}),
    };
    printPrepareReport(await prepareDataset(prepareOptions));
  }
}

function printPrepareReport(report) {
  console.log("\n  geodot - dataset preparation");
  console.log("  -------------------------------------");
  console.log(`  Tiles:    ${report.tiles}`);
  console.log(`  Patches:  ${report.patches}`);
  console.log(`  Variants: ${report.variants}`);
  console.log(`  Output:   ${report.path}`);
}

function progressPrinter(phase, total) {
  let last = 0;
  return (event) => {
    const now = performance.now();
    if (!event.done && now - last < 1000) return;
    last = now;
    if (phase === "select") {
      const percent = event.total
        ? ` (${((event.scanned / event.total) * 100).toFixed(1)}%)`
        : "";
      console.error(
        `  Selecting: scanned ${event.scanned ?? 0}${percent}, matched ${event.selected ?? 0}`,
      );
      return;
    }
    const completed = event.completed ?? 0;
    const percent = total
      ? ` (${((completed / total) * 100).toFixed(1)}%)`
      : "";
    console.error(
      `  Downloading: ${completed}/${total ?? "?"}${percent}, ok ${event.downloaded ?? 0}, failed ${event.failed ?? 0}`,
    );
  };
}

if (process.argv[2] === "demo") {
  serveDemo(parseDemoArgs(process.argv.slice(3)));
} else if (process.argv[2] === "render") {
  await runRender(process.argv.slice(3));
} else if (process.argv[2] === "validate") {
  await runValidate(process.argv.slice(3));
} else {
  await runDownload();
}
