import {
  access,
  mkdir,
  readFile,
  readdir,
  stat,
  writeFile,
} from "node:fs/promises";
import path from "node:path";

export const TILE_SIZE = 256;
export const SUPPORTED_IMAGE_EXTENSIONS = [".jpg", ".jpeg", ".png", ".webp"];

const USER_AGENTS = [
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15",
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0",
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
];

const SUBDOMAINS = ["mt0", "mt1", "mt2", "mt3"];
const TILE_URL_TEMPLATE_ENV = "GEODOT_TILE_URL_TEMPLATE";
const MAX_ZOOM = 30;

export function latlonToTile(lat, lon, z) {
  const n = 2 ** z;
  const x = Math.floor(((lon + 180) / 360) * n);
  const latRad = (lat * Math.PI) / 180;
  const y = Math.floor(
    ((1 - Math.log(Math.tan(latRad) + 1 / Math.cos(latRad)) / Math.PI) / 2) * n,
  );
  return { x, y, z };
}

export function tileBounds(tile) {
  const n = 2 ** tile.z;
  const lonMin = (tile.x / n) * 360 - 180;
  const lonMax = ((tile.x + 1) / n) * 360 - 180;
  const latMax =
    (Math.atan(Math.sinh(Math.PI * (1 - (2 * tile.y) / n))) * 180) / Math.PI;
  const latMin =
    (Math.atan(Math.sinh(Math.PI * (1 - (2 * (tile.y + 1)) / n))) * 180) /
    Math.PI;
  return { latMin, lonMin, latMax, lonMax };
}

export function metersPerPixel(lat, z) {
  return (
    (40_075_016.686 / (TILE_SIZE * 2 ** z)) * Math.cos((lat * Math.PI) / 180)
  );
}

export function tileGrid(lat, lon, zoom, cols, rows) {
  return Array.from(tileGridIterator(lat, lon, zoom, cols, rows));
}

export function tileGridBetween(
  topLeftLat,
  topLeftLon,
  bottomRightLat,
  bottomRightLon,
  zoom,
) {
  const first = latlonToTile(topLeftLat, topLeftLon, zoom);
  const second = latlonToTile(bottomRightLat, bottomRightLon, zoom);
  return Array.from(
    tilesInRange(
      Math.min(first.x, second.x),
      Math.max(first.x, second.x),
      Math.min(first.y, second.y),
      Math.max(first.y, second.y),
      zoom,
    ),
  );
}

export function tileGridForPolygon(points, zoom) {
  return Array.from(tileGridForPolygonIterator(points, zoom));
}

export function tilesForOptions(options) {
  return Array.from(tileIteratorForOptions(options));
}

export function countTilesForOptions(options, onProgress) {
  let count = 0;
  const tiles = tileIteratorForOptions(options, onProgress);
  while (!tiles.next().done) count += 1;
  return count;
}

function tileIteratorForOptions(options, onProgress) {
  if (options.polygon?.length >= 3)
    return tileGridForPolygonIterator(
      options.polygon,
      options.zoom,
      onProgress,
    );
  if (
    options.bottomRightLat !== undefined &&
    options.bottomRightLon !== undefined
  ) {
    const first = latlonToTile(options.lat, options.lon, options.zoom);
    const second = latlonToTile(
      options.bottomRightLat,
      options.bottomRightLon,
      options.zoom,
    );
    return tilesInRange(
      Math.min(first.x, second.x),
      Math.max(first.x, second.x),
      Math.min(first.y, second.y),
      Math.max(first.y, second.y),
      options.zoom,
      onProgress,
    );
  }
  return tileGridIterator(
    options.lat,
    options.lon,
    options.zoom,
    options.cols,
    options.rows,
    onProgress,
  );
}

export function tilePath(out, tile) {
  return path.join(
    out,
    "tiles",
    String(tile.z),
    String(tile.x),
    `${tile.y}.jpg`,
  );
}

export async function prepareDataset(options = {}) {
  const patchSizesProvided = Object.hasOwn(options, "patchSizes");
  const config = {
    out: "data",
    patchSizes: [1, 2, 4],
    stride: 1,
    rotations: [0, 45, 90, 135, 180, 225, 270, 315],
    auto400m: !patchSizesProvided,
    ...options,
  };
  validatePrepareOptions(config);
  const tiles = await discoverTiles(config.out);
  const patchSizes = resolvePatchSizes(tiles, config);
  const tileIds = new Map(
    tiles.map((tile) => [
      `${tile.root}/${tile.capture_id}/${tile.z}/${tile.x}/${tile.y}`,
      tile.tile_id,
    ]),
  );
  const patches = buildPatches(tiles, tileIds, config, patchSizes);
  const places = buildPlaces(tiles, patches);
  const quality = buildQuality(tiles, patches);
  const variants = patches.flatMap((patch) =>
    config.rotations.map((rotation) => ({
      variant_id: `${patch.patch_id}_r${rotation}`,
      patch_id: patch.patch_id,
      rotation_degrees: rotation,
      crop_shape: "square",
      virtual_only: true,
      image_written: false,
      descriptor_id: null,
      index_id: null,
    })),
  );
  const root = path.join(config.out, "vpr");
  const manifest = path.join(root, "manifest");
  const configDir = path.join(root, "config");
  await mkdir(manifest, { recursive: true });
  await mkdir(configDir, { recursive: true });
  await writeFile(
    path.join(manifest, "tiles.json"),
    JSON.stringify(tiles, null, 2),
  );
  await writeFile(
    path.join(manifest, "patches.json"),
    JSON.stringify(patches, null, 2),
  );
  await writeFile(
    path.join(manifest, "variants.json"),
    JSON.stringify(variants, null, 2),
  );
  await writeFile(
    path.join(manifest, "places.json"),
    JSON.stringify(places, null, 2),
  );
  await writeFile(
    path.join(manifest, "quality.json"),
    JSON.stringify(quality, null, 2),
  );
  await writeFile(
    path.join(configDir, "dataset.json"),
    JSON.stringify(
      {
        schema_version: "1.0",
        geodot_version: "unknown",
        created_at: new Date().toISOString(),
        command: process.argv.join(" "),
        output_directory: config.out,
        profile: "aerial-vpr-default",
        tile_roots: [...new Set(tiles.map((tile) => tile.root))]
          .sort()
          .map((root) => `${root}/{z}/{x}/{y}.{jpg,jpeg,png,webp}`),
        mode: "virtual",
        tile_size: TILE_SIZE,
        image_roots_detected: [
          ...new Set(tiles.map((tile) => tile.root)),
        ].sort(),
        supported_image_extensions: SUPPORTED_IMAGE_EXTENSIONS,
        zoom_levels_detected: [...new Set(tiles.map((tile) => tile.z))].sort(
          (a, b) => a - b,
        ),
        patch_sizes: patchSizes,
        stride: config.stride,
        rotations: config.rotations,
        auto400m: config.auto400m,
        circular_crops_virtual: true,
        images_modified: false,
        generated_images_default: false,
        descriptors_computed: false,
        indexes_built: false,
        appearance: [],
        counts: {
          tiles: tiles.length,
          patches: patches.length,
          variants: variants.length,
          places: places.length,
        },
      },
      null,
      2,
    ),
  );
  return {
    tiles: tiles.length,
    patches: patches.length,
    variants: variants.length,
    path: root,
  };
}

export async function renderDataset(options = {}) {
  const { out = "data", patchId, variantId, output } = options;
  if (!output) throw new Error("output is required");
  if (Boolean(patchId) === Boolean(variantId)) {
    throw new Error("provide exactly one of patchId or variantId");
  }
  const manifest = path.join(out, "vpr", "manifest");
  const patches = JSON.parse(
    await readFile(path.join(manifest, "patches.json"), "utf8"),
  );
  const tiles = JSON.parse(
    await readFile(path.join(manifest, "tiles.json"), "utf8"),
  );
  let selectedPatchId = patchId;
  if (variantId) {
    const variants = JSON.parse(
      await readFile(path.join(manifest, "variants.json"), "utf8"),
    );
    const variant = variants.find((item) => item.variant_id === variantId);
    if (!variant) throw new Error(`variant not found: ${variantId}`);
    selectedPatchId = variant.patch_id;
  }
  const patch = patches.find((item) => item.patch_id === selectedPatchId);
  if (!patch) throw new Error(`patch not found: ${selectedPatchId}`);
  const sourceTileIds = patch.source_tile_ids ?? patch.source_tiles ?? [];
  if (sourceTileIds.length !== 1) {
    throw new Error(
      "render currently supports one-source-tile virtual patches only",
    );
  }
  const tile = tiles.find((item) => item.tile_id === sourceTileIds[0]);
  if (!tile) throw new Error(`source tile not found: ${sourceTileIds[0]}`);
  const source = path.join(out, tile.path);
  const data = await readFile(source);
  await mkdir(path.dirname(output), { recursive: true });
  await writeFile(output, data);
  return { sourcePath: source, outputPath: output, bytes: data.byteLength };
}

export async function loadDataset(out = "data") {
  const manifest = path.join(out, "vpr", "manifest");
  const config = path.join(out, "vpr", "config");
  const files = {
    tiles: path.join(manifest, "tiles.json"),
    patches: path.join(manifest, "patches.json"),
    variants: path.join(manifest, "variants.json"),
    places: path.join(manifest, "places.json"),
    quality: path.join(manifest, "quality.json"),
    dataset: path.join(config, "dataset.json"),
  };
  for (const file of Object.values(files)) await access(file);
  const dataset = Object.fromEntries(
    await Promise.all(
      Object.entries(files).map(async ([name, file]) => [
        name,
        JSON.parse(await readFile(file, "utf8")),
      ]),
    ),
  );
  dataset._root = out;
  return dataset;
}

export async function validateDataset(out = "data", options = {}) {
  const errors = [];
  let warnings = [];
  let dataset;
  try {
    dataset = await loadDataset(out);
  } catch (error) {
    error.code = error.code ?? "GEODOT_MISSING_DATASET";
    throw error;
  }
  const tileIds = checkUniqueIds("tile", dataset.tiles, "tile_id", errors);
  const patchIds = checkUniqueIds("patch", dataset.patches, "patch_id", errors);
  checkUniqueIds("variant", dataset.variants, "variant_id", errors);
  checkUniqueIds("place", dataset.places, "place_id", errors);
  for (const tile of dataset.tiles) {
    if (!tile.path || !(await exists(path.join(out, tile.path))))
      errors.push(
        `missing source image for tile ${tile.tile_id}: ${tile.path}`,
      );
    if (!validBBox(tile.bbox))
      errors.push(`invalid bbox for tile ${tile.tile_id}`);
    if (
      !positiveInteger(tile.image_width) ||
      !positiveInteger(tile.image_height)
    )
      errors.push(`invalid image dimensions for tile ${tile.tile_id}`);
  }
  for (const patch of dataset.patches) {
    if (!validBBox(patch.bbox))
      errors.push(`invalid bbox for patch ${patch.patch_id}`);
    for (const tileId of patch.source_tile_ids ?? patch.source_tiles ?? []) {
      if (!tileIds.has(tileId))
        errors.push(
          `patch ${patch.patch_id} references missing tile ${tileId}`,
        );
    }
  }
  for (const variant of dataset.variants) {
    if (!patchIds.has(variant.patch_id))
      errors.push(
        `variant ${variant.variant_id} references missing patch ${variant.patch_id}`,
      );
  }
  for (const place of dataset.places) {
    for (const field of ["tile_ids", "reference_tile_ids", "query_tile_ids"]) {
      for (const tileId of place[field] ?? []) {
        if (!tileIds.has(tileId))
          errors.push(
            `place ${place.place_id} ${field} references missing tile ${tileId}`,
          );
      }
    }
    for (const patchId of place.patch_ids ?? []) {
      if (!patchIds.has(patchId))
        errors.push(
          `place ${place.place_id} references missing patch ${patchId}`,
        );
    }
  }
  for (const field of [
    "images_modified",
    "descriptors_computed",
    "indexes_built",
    "generated_images_default",
  ]) {
    if (dataset.dataset[field] !== false)
      errors.push(`dataset config ${field} must be false`);
  }
  const generated = (await listFiles(path.join(out, "vpr"))).filter((file) =>
    SUPPORTED_IMAGE_EXTENSIONS.includes(path.extname(file).toLowerCase()),
  );
  if (generated.length)
    warnings.push(`found generated image(s) under vpr: ${generated.length}`);
  if (options.strict && warnings.length) {
    errors.push(...warnings);
    warnings = [];
  }
  const counts = {
    tiles: dataset.tiles.length,
    patches: dataset.patches.length,
    variants: dataset.variants.length,
    places: dataset.places.length,
    query_tiles: dataset.tiles.filter((tile) => tile.role === "query").length,
    reference_tiles: dataset.tiles.filter((tile) => tile.role === "reference")
      .length,
    warnings: warnings.length,
    errors: errors.length,
  };
  return { valid: errors.length === 0, errors, warnings, counts };
}

async function exists(file) {
  return access(file).then(
    () => true,
    () => false,
  );
}

function checkUniqueIds(kind, items, field, errors) {
  const seen = new Set();
  for (const item of items) {
    const value = item[field];
    if (!value || typeof value !== "string") {
      errors.push(`${kind} missing ${field}`);
    } else if (seen.has(value)) {
      errors.push(`duplicate ${kind} id: ${value}`);
    }
    if (value) seen.add(value);
  }
  return seen;
}

function validBBox(value) {
  return (
    Array.isArray(value) &&
    value.length === 4 &&
    value.every(Number.isFinite) &&
    value[0] < value[2] &&
    value[1] < value[3] &&
    value[0] >= -180 &&
    value[2] <= 180 &&
    value[1] >= -90 &&
    value[3] <= 90
  );
}

function positiveInteger(value) {
  return Number.isInteger(value) && value > 0;
}

export async function download(options = {}) {
  const config = {
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
    ...options,
  };
  if (config.geojson && !config.polygon) {
    config.polygon = await loadGeoJSONPolygon(config.geojson);
  }
  validateOptions(config);
  const center = latlonToTile(config.lat, config.lon, config.zoom);
  const queue = tileIteratorForOptions(config);
  const tiles = [];
  const failed = [];
  let completed = 0;

  async function worker() {
    while (true) {
      const next = queue.next();
      if (next.done) break;
      const tile = next.value;
      const data = await downloadTile(tile);
      if (!data) {
        failed.push(tile);
        completed += 1;
        config.onProgress?.({
          phase: "download",
          completed,
          downloaded: tiles.length,
          failed: failed.length,
          tile,
        });
        continue;
      }
      const file = tilePath(config.out, tile);
      await mkdir(path.dirname(file), { recursive: true });
      await writeFile(file, data);
      tiles.push({
        tile,
        bounds: manifestBounds(tile),
        path: file,
        bytes: data.byteLength,
      });
      completed += 1;
      config.onProgress?.({
        phase: "download",
        completed,
        downloaded: tiles.length,
        failed: failed.length,
        tile,
      });
    }
  }

  await Promise.all(Array.from({ length: Math.max(1, config.jobs) }, worker));
  const report = { center, tiles, failed };
  await mkdir(config.out, { recursive: true });
  if (!config.noManifest) {
    await writeFile(
      path.join(config.out, "manifest.json"),
      JSON.stringify(report, null, 2),
    );
  }
  if (!config.noDemo) {
    await writeDemo(config.out, report);
  }
  return report;
}

async function writeDemo(out, report) {
  const downloaded = report.tiles;
  const bounds = downloaded.length
    ? [
        [
          Math.min(...downloaded.map((item) => item.bounds.lon_min)),
          Math.min(...downloaded.map((item) => item.bounds.lat_min)),
        ],
        [
          Math.max(...downloaded.map((item) => item.bounds.lon_max)),
          Math.max(...downloaded.map((item) => item.bounds.lat_max)),
        ],
      ]
    : (() => {
        const tile = tileBounds(report.center);
        return [
          [tile.lonMin, tile.latMin],
          [tile.lonMax, tile.latMax],
        ];
      })();
  const data = JSON.stringify({
    tiles: downloaded.map((item) => item.tile),
    bounds,
    mapCenter: [
      (bounds[0][0] + bounds[1][0]) / 2,
      (bounds[0][1] + bounds[1][1]) / 2,
    ],
    zoom: downloaded[0]?.tile.z ?? report.center.z,
    center: report.center,
  });
  await writeFile(path.join(out, "index.html"), demoHTML(data));
}

function demoHTML(data) {
  return `<!doctype html>
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
    const data = ${data};
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
      opacityValue.textContent = \`\${Math.round(value * 100)}%\`;
      for (const tile of data.tiles) {
        const layer = \`geodot-tile-\${tile.z}-\${tile.x}-\${tile.y}\`;
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
      if (updateHash) history.replaceState(null, '', \`#\${tile.z}/\${tile.x}/\${tile.y}.jpg\`);
    }

    map.on('load', () => {
      for (const tile of data.tiles) {
        const bounds = tileBounds(tile);
        const id = \`geodot-tile-\${tile.z}-\${tile.x}-\${tile.y}\`;
        map.addSource(id, {
          type: 'image',
          url: \`./tiles/\${tile.z}/\${tile.x}/\${tile.y}.jpg\`,
          coordinates: [[bounds.lonMin, bounds.latMax], [bounds.lonMax, bounds.latMax], [bounds.lonMax, bounds.latMin], [bounds.lonMin, bounds.latMin]]
        });
        map.addLayer({ id, type: 'raster', source: id, paint: { 'raster-opacity': Number(opacityInput.value) } });
      }
      map.addSource('geodot-labels', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: data.tiles.map((tile) => ({ type: 'Feature', properties: { label: \`\${tile.z}/\${tile.x}/\${tile.y}\` }, geometry: { type: 'Point', coordinates: tileCenter(tile) } })) }
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
`;
}

export async function loadGeoJSONPolygon(source) {
  const text = isUrl(source)
    ? await fetch(source).then((response) => {
        if (!response.ok) throw new Error(`failed to fetch GeoJSON: ${source}`);
        return response.text();
      })
    : await readFile(source, "utf8");
  return polygonFromGeoJSON(JSON.parse(text));
}

export function polygonFromGeoJSON(geojson) {
  const geometry = findPolygonGeometry(geojson);
  if (!geometry) throw new Error("GeoJSON does not contain a Polygon geometry");
  const ring =
    geometry.type === "Polygon"
      ? geometry.coordinates[0]
      : geometry.coordinates[0]?.[0];
  const points =
    ring?.map(([lon, lat]) => ({ lon: Number(lon), lat: Number(lat) })) ?? [];
  if (
    points.length < 3 ||
    points.some((point) => Number.isNaN(point.lon) || Number.isNaN(point.lat))
  ) {
    throw new Error(
      "GeoJSON polygon requires at least three lon,lat coordinates",
    );
  }
  return points;
}

function findPolygonGeometry(value) {
  if (!value || typeof value !== "object") return undefined;
  if (value.type === "Polygon" || value.type === "MultiPolygon") return value;
  if (value.type === "Feature") return findPolygonGeometry(value.geometry);
  if (value.type === "FeatureCollection") {
    for (const feature of value.features ?? []) {
      const geometry = findPolygonGeometry(feature);
      if (geometry) return geometry;
    }
  }
  return undefined;
}

function isUrl(source) {
  return /^https?:\/\//i.test(source);
}

function manifestBounds(tile) {
  const bounds = tileBounds(tile);
  return {
    lat_min: bounds.latMin,
    lon_min: bounds.lonMin,
    lat_max: bounds.latMax,
    lon_max: bounds.lonMax,
  };
}

function validatePrepareOptions(options) {
  validateIntegerRange("stride", options.stride, 1, Number.MAX_SAFE_INTEGER);
  if (!Array.isArray(options.patchSizes) || options.patchSizes.length === 0) {
    throw new TypeError("patchSizes must not be empty");
  }
  if (!Array.isArray(options.rotations) || options.rotations.length === 0) {
    throw new TypeError("rotations must not be empty");
  }
  for (const size of options.patchSizes) {
    validateIntegerRange("patchSizes", size, 1, Number.MAX_SAFE_INTEGER);
  }
  for (const rotation of options.rotations) {
    validateIntegerRange("rotations", rotation, 0, 359);
  }
}

async function discoverTiles(out) {
  const tiles = [];
  for (const rootName of ["tiles", "drone-view"]) {
    const root = path.join(out, rootName);
    const files = await listFiles(root).catch((error) => {
      if (error.code === "ENOENT") return [];
      throw error;
    });
    for (const file of files.sort()) {
      const extension = path.extname(file).toLowerCase();
      if (!SUPPORTED_IMAGE_EXTENSIONS.includes(extension)) continue;
      const parsed = parseImagePath(root, file);
      if (!parsed) continue;
      const { captureId, z, x, y } = parsed;
      const maxTile = 2 ** z;
      if (x < 0 || x >= maxTile || y < 0 || y >= maxTile) continue;
      const bounds = tileBounds({ x, y, z });
      const info = await stat(file);
      const image = await readImageHeader(file);
      tiles.push({
        tile_id: `${rootName}_${captureId}_z${z}_x${x}_y${y}`,
        root: rootName,
        capture_id: captureId,
        role: rootName === "tiles" ? "reference" : "query",
        z,
        x,
        y,
        extension: extension.slice(1),
        detected_format: image.format,
        path: path.relative(out, file),
        bbox: [bounds.lonMin, bounds.latMin, bounds.lonMax, bounds.latMax],
        center_lon: (bounds.lonMin + bounds.lonMax) / 2,
        center_lat: (bounds.latMin + bounds.latMax) / 2,
        image_width: image.width ?? TILE_SIZE,
        image_height: image.height ?? TILE_SIZE,
        pixel_width: image.width ?? TILE_SIZE,
        pixel_height: image.height ?? TILE_SIZE,
        bytes: info.size,
        valid: image.width !== undefined && image.height !== undefined,
        lon_min: bounds.lonMin,
        lat_min: bounds.latMin,
        lon_max: bounds.lonMax,
        lat_max: bounds.latMax,
      });
    }
  }
  if (tiles.length === 0)
    throw new Error(`no valid tiles found under ${path.join(out, "tiles")}`);
  return tiles;
}

function parseImagePath(root, file) {
  const parts = path.relative(root, file).split(path.sep);
  const values = parts.length === 3 ? ["default", ...parts] : parts;
  if (values.length !== 4) return undefined;
  const [captureId, zText, xText, yFile] = values;
  const z = Number(zText);
  const x = Number(xText);
  const y = Number(path.basename(yFile, path.extname(yFile)));
  if (![z, x, y].every(Number.isInteger) || z < 0 || z > MAX_ZOOM)
    return undefined;
  return { captureId, z, x, y };
}

async function readImageHeader(file) {
  const data = await readFile(file);
  if (
    data
      .subarray(0, 8)
      .equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) &&
    data.length >= 24
  ) {
    return {
      width: data.readUInt32BE(16),
      height: data.readUInt32BE(20),
      format: "png",
    };
  }
  if (
    data.subarray(0, 4).toString() === "RIFF" &&
    data.subarray(8, 12).toString() === "WEBP"
  ) {
    return readWebPHeader(data);
  }
  if (data[0] === 0xff && data[1] === 0xd8) return readJPEGHeader(data);
  return { width: undefined, height: undefined, format: undefined };
}

function readJPEGHeader(data) {
  let index = 2;
  while (index + 9 < data.length) {
    if (data[index] !== 0xff) {
      index += 1;
      continue;
    }
    const marker = data[index + 1];
    if (
      [
        0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7, 0xc9, 0xca, 0xcb, 0xcd, 0xce,
        0xcf,
      ].includes(marker)
    ) {
      return {
        width: data.readUInt16BE(index + 7),
        height: data.readUInt16BE(index + 5),
        format: "jpeg",
      };
    }
    if (index + 4 > data.length) break;
    index += 2 + Math.max(data.readUInt16BE(index + 2), 1);
  }
  return { width: undefined, height: undefined, format: "jpeg" };
}

function readWebPHeader(data) {
  const chunk = data.subarray(12, 16).toString();
  if (chunk === "VP8X" && data.length >= 30) {
    return {
      width: data.readUIntLE(24, 3) + 1,
      height: data.readUIntLE(27, 3) + 1,
      format: "webp",
    };
  }
  if (chunk === "VP8L" && data.length >= 25) {
    const bits = data.readUInt32LE(21);
    return {
      width: (bits & 0x3fff) + 1,
      height: ((bits >> 14) & 0x3fff) + 1,
      format: "webp",
    };
  }
  return { width: undefined, height: undefined, format: "webp" };
}

function resolvePatchSizes(tiles, options) {
  const sizes = new Set(options.patchSizes);
  if (options.auto400m) {
    for (const tile of tiles) {
      const tileWidthM = metersPerPixel(tile.center_lat, tile.z) * TILE_SIZE;
      if (tileWidthM > 0)
        sizes.add(Math.max(1, Math.min(8, Math.round(400 / tileWidthM))));
    }
  }
  return [...sizes].sort((a, b) => a - b);
}

async function listFiles(root) {
  const entries = await readdir(root, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const file = path.join(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await listFiles(file)));
    } else if (entry.isFile()) {
      files.push(file);
    }
  }
  return files;
}

function buildPatches(tiles, tileIds, options, patchSizes) {
  const byGroup = new Map();
  for (const tile of tiles) {
    const key = `${tile.root}/${tile.capture_id}/${tile.z}`;
    if (!byGroup.has(key)) byGroup.set(key, []);
    byGroup.get(key).push(tile);
  }
  const patches = [];
  for (const [key, zoomTiles] of [...byGroup.entries()].sort(([a], [b]) =>
    a.localeCompare(b),
  )) {
    const [root, captureId, zText] = key.split("/");
    const z = Number(zText);
    const xs = zoomTiles.map((tile) => tile.x);
    const ys = zoomTiles.map((tile) => tile.y);
    for (const size of patchSizes) {
      for (
        let y = Math.min(...ys);
        y <= Math.max(...ys) - size + 1;
        y += options.stride
      ) {
        for (
          let x = Math.min(...xs);
          x <= Math.max(...xs) - size + 1;
          x += options.stride
        ) {
          const sourceTiles = [];
          for (let sourceY = y; sourceY < y + size; sourceY += 1) {
            for (let sourceX = x; sourceX < x + size; sourceX += 1) {
              sourceTiles.push(
                tileIds.get(`${root}/${captureId}/${z}/${sourceX}/${sourceY}`),
              );
            }
          }
          if (sourceTiles.some((tileId) => tileId === undefined)) continue;
          const topLeft = tileBounds({ x, y, z });
          const bottomRight = tileBounds({
            x: x + size - 1,
            y: y + size - 1,
            z,
          });
          const lonMin = topLeft.lonMin;
          const latMin = bottomRight.latMin;
          const lonMax = bottomRight.lonMax;
          const latMax = topLeft.latMax;
          const patchId = `${root}_${captureId}_z${z}_x${x}-${x + size - 1}_y${y}-${y + size - 1}_s${size}`;
          const groundWidth =
            metersPerPixel((latMin + latMax) / 2, z) * TILE_SIZE * size;
          patches.push({
            patch_id: patchId,
            place_id: `z${z}_x${x}_y${y}`,
            root,
            capture_id: captureId,
            role: root === "tiles" ? "reference" : "query",
            z,
            x,
            y,
            source_x_min: x,
            source_x_max: x + size - 1,
            source_y_min: y,
            source_y_max: y + size - 1,
            source_tiles: sourceTiles,
            source_tile_ids: sourceTiles,
            pixel_width: TILE_SIZE * size,
            pixel_height: TILE_SIZE * size,
            bbox: [lonMin, latMin, lonMax, latMax],
            lon_min: lonMin,
            lat_min: latMin,
            lon_max: lonMax,
            lat_max: latMax,
            center_lon: (lonMin + lonMax) / 2,
            center_lat: (latMin + latMax) / 2,
            mosaic_size_tiles: size,
            stride_tiles: options.stride,
            scale_profile: `z${z}_${size}x${size}`,
            ground_width_m_estimate: groundWidth,
            ground_height_m_estimate: groundWidth,
            complete: true,
            rotation_safe_circle_diameter_px: TILE_SIZE * size,
            circular_crop_available: true,
            image_written: false,
            virtual_compose_spec: {
              type: "virtual_mosaic",
              tile_ids: sourceTiles,
              layout: [size, size],
            },
            image_path_or_virtual_spec: {
              type: "virtual_mosaic",
              tile_ids: sourceTiles,
              layout: [size, size],
            },
          });
        }
      }
    }
  }
  return patches;
}

function buildPlaces(tiles, patches) {
  const places = new Map();
  for (const tile of tiles) {
    const key = `${tile.z}/${tile.x}/${tile.y}`;
    if (!places.has(key)) {
      places.set(key, {
        place_id: `z${tile.z}_x${tile.x}_y${tile.y}`,
        z: tile.z,
        x: tile.x,
        y: tile.y,
        center_lon: tile.center_lon,
        center_lat: tile.center_lat,
        bbox: tile.bbox,
        available_roots: [],
        available_captures: [],
        tile_ids: [],
        reference_available: false,
        query_available: false,
        reference_tile_ids: [],
        query_tile_ids: [],
        patch_ids: [],
        quality_summary: { recommendation: "keep" },
      });
    }
    const place = places.get(key);
    place.available_roots.push(tile.root);
    place.available_captures.push(tile.capture_id);
    place.tile_ids.push(tile.tile_id);
    if (tile.role === "reference") {
      place.reference_available = true;
      place.reference_tile_ids.push(tile.tile_id);
    }
    if (tile.role === "query") {
      place.query_available = true;
      place.query_tile_ids.push(tile.tile_id);
    }
  }
  for (const patch of patches) {
    places
      .get(`${patch.z}/${patch.x}/${patch.y}`)
      ?.patch_ids.push(patch.patch_id);
  }
  return [...places.values()].map((place) => ({
    ...place,
    available_roots: [...new Set(place.available_roots)].sort(),
    available_captures: [...new Set(place.available_captures)].sort(),
  }));
}

function buildQuality(tiles, patches) {
  return {
    tiles: tiles.map((tile) => {
      const reasons = tile.bytes < 128 ? ["very_small_file"] : [];
      return {
        id: tile.tile_id,
        type: "tile",
        blank_near_blank_score: reasons.length ? 1 : 0,
        mean_brightness: null,
        contrast: null,
        blur_estimate: null,
        edge_density: null,
        entropy: null,
        likely_low_information: reasons.length > 0,
        recommendation: reasons.length ? "reject" : "keep",
        reject_reasons: reasons,
      };
    }),
    patches: patches.map((patch) => ({
      id: patch.patch_id,
      type: "patch",
      likely_low_information: false,
      recommendation: "keep",
      reject_reasons: [],
    })),
  };
}

function* tileGridIterator(lat, lon, zoom, cols, rows, onProgress) {
  const center = latlonToTile(lat, lon, zoom);
  let scanned = 0;
  for (let row = 0; row < rows; row += 1) {
    for (let col = 0; col < cols; col += 1) {
      scanned += 1;
      onProgress?.({
        phase: "select",
        scanned,
        selected: scanned,
        total: cols * rows,
      });
      yield { x: center.x + col, y: center.y + row, z: zoom };
    }
  }
}

function* tileGridForPolygonIterator(points, zoom, onProgress) {
  if (points.length < 3) return;
  const lats = points.map((point) => point.lat);
  const lons = points.map((point) => point.lon);
  const first = latlonToTile(Math.max(...lats), Math.min(...lons), zoom);
  const second = latlonToTile(Math.min(...lats), Math.max(...lons), zoom);
  const minX = Math.min(first.x, second.x);
  const maxX = Math.max(first.x, second.x);
  const minY = Math.min(first.y, second.y);
  const maxY = Math.max(first.y, second.y);
  const total = (maxX - minX + 1) * (maxY - minY + 1);
  let scanned = 0;
  let selected = 0;
  for (const tile of tilesInRange(minX, maxX, minY, maxY, zoom)) {
    scanned += 1;
    if (tileIntersectsPolygon(tile, points)) {
      selected += 1;
      onProgress?.({ phase: "select", scanned, selected, total });
      yield tile;
    } else {
      onProgress?.({ phase: "select", scanned, selected, total });
    }
  }
}

function* tilesInRange(minX, maxX, minY, maxY, z, onProgress) {
  const total = (maxX - minX + 1) * (maxY - minY + 1);
  let scanned = 0;
  for (let y = minY; y <= maxY; y += 1) {
    for (let x = minX; x <= maxX; x += 1) {
      scanned += 1;
      onProgress?.({ phase: "select", scanned, selected: scanned, total });
      yield { x, y, z };
    }
  }
}

function validateOptions(options) {
  validateFiniteNumber("lat", options.lat);
  validateFiniteNumber("lon", options.lon);
  if (options.bottomRightLat !== undefined) {
    validateFiniteNumber("bottomRightLat", options.bottomRightLat);
  }
  if (options.bottomRightLon !== undefined) {
    validateFiniteNumber("bottomRightLon", options.bottomRightLon);
  }
  for (const [index, point] of (options.polygon ?? []).entries()) {
    validateFiniteNumber(`polygon[${index}].lon`, point.lon);
    validateFiniteNumber(`polygon[${index}].lat`, point.lat);
  }
  validateIntegerRange("zoom", options.zoom, 0, MAX_ZOOM);
  validateIntegerRange("cols", options.cols, 1, Number.MAX_SAFE_INTEGER);
  validateIntegerRange("rows", options.rows, 1, Number.MAX_SAFE_INTEGER);
  validateIntegerRange("jobs", options.jobs, 1, Number.MAX_SAFE_INTEGER);
}

function validateFiniteNumber(name, value) {
  if (!Number.isFinite(value)) {
    throw new TypeError(`${name} must be a finite number`);
  }
}

function validateIntegerRange(name, value, min, max) {
  if (!Number.isInteger(value) || value < min || value > max) {
    throw new TypeError(`${name} must be an integer from ${min} to ${max}`);
  }
}

function tileIntersectsPolygon(tile, points) {
  const bounds = tileBounds(tile);
  const center = {
    lon: (bounds.lonMin + bounds.lonMax) / 2,
    lat: (bounds.latMin + bounds.latMax) / 2,
  };
  if (pointInPolygon(center, points)) return true;
  const corners = [
    { lon: bounds.lonMin, lat: bounds.latMin },
    { lon: bounds.lonMin, lat: bounds.latMax },
    { lon: bounds.lonMax, lat: bounds.latMin },
    { lon: bounds.lonMax, lat: bounds.latMax },
  ];
  return (
    corners.some((corner) => pointInPolygon(corner, points)) ||
    points.some(
      (point) =>
        point.lon >= bounds.lonMin &&
        point.lon <= bounds.lonMax &&
        point.lat >= bounds.latMin &&
        point.lat <= bounds.latMax,
    )
  );
}

function pointInPolygon(point, polygon) {
  let inside = false;
  let previous = polygon[polygon.length - 1];
  for (const current of polygon) {
    if (current.lat > point.lat !== previous.lat > point.lat) {
      const lon =
        ((previous.lon - current.lon) * (point.lat - current.lat)) /
          (previous.lat - current.lat) +
        current.lon;
      if (point.lon < lon) inside = !inside;
    }
    previous = current;
  }
  return inside;
}

async function downloadTile(tile) {
  for (const subdomain of SUBDOMAINS) {
    const response = await fetch(tileUrl(subdomain, tile), {
      headers: {
        "User-Agent":
          USER_AGENTS[Math.floor(Math.random() * USER_AGENTS.length)],
        Accept: "image/avif,image/webp,image/apng,image/*,*/*;q=0.8",
        "Accept-Language": "en-US,en;q=0.9",
        Referer: "https://www.google.com/maps",
      },
    }).catch(() => null);
    if (response?.ok) {
      const data = Buffer.from(await response.arrayBuffer());
      if (data.byteLength > 100) return data;
    }
  }
  return null;
}

function tileUrl(subdomain, tile) {
  const template = process.env[TILE_URL_TEMPLATE_ENV];
  if (template) {
    return template
      .replaceAll("{sub}", subdomain)
      .replaceAll("{x}", String(tile.x))
      .replaceAll("{y}", String(tile.y))
      .replaceAll("{z}", String(tile.z));
  }
  return `https://${subdomain}.google.com/vt/lyrs=s&x=${tile.x}&y=${tile.y}&z=${tile.z}`;
}
