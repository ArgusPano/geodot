import { mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import path from "node:path";

export const TILE_SIZE = 256;

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
          .map((root) => `${root}/{z}/{x}/{y}.jpg`),
        mode: "virtual",
        tile_size: TILE_SIZE,
        image_roots_detected: [
          ...new Set(tiles.map((tile) => tile.root)),
        ].sort(),
        zoom_levels_detected: [...new Set(tiles.map((tile) => tile.z))].sort(
          (a, b) => a - b,
        ),
        patch_sizes: patchSizes,
        stride: config.stride,
        rotations: config.rotations,
        auto400m: config.auto400m,
        circular_crops_virtual: true,
        images_modified: false,
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
    const data = ${data};
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
      if (!file.endsWith(".jpg")) continue;
      const parsed = parseImagePath(root, file);
      if (!parsed) continue;
      const { captureId, z, x, y } = parsed;
      const maxTile = 2 ** z;
      if (x < 0 || x >= maxTile || y < 0 || y >= maxTile) continue;
      const bounds = tileBounds({ x, y, z });
      const info = await stat(file);
      tiles.push({
        tile_id: `${rootName}_${captureId}_z${z}_x${x}_y${y}`,
        root: rootName,
        capture_id: captureId,
        role: rootName === "tiles" ? "reference" : "query",
        z,
        x,
        y,
        path: path.relative(out, file),
        bbox: [bounds.lonMin, bounds.latMin, bounds.lonMax, bounds.latMax],
        center_lon: (bounds.lonMin + bounds.lonMax) / 2,
        center_lat: (bounds.latMin + bounds.latMax) / 2,
        image_width: TILE_SIZE,
        image_height: TILE_SIZE,
        pixel_width: TILE_SIZE,
        pixel_height: TILE_SIZE,
        bytes: info.size,
        valid: true,
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
  const y = Number(path.basename(yFile, ".jpg"));
  if (![z, x, y].every(Number.isInteger) || z < 0 || z > MAX_ZOOM)
    return undefined;
  return { captureId, z, x, y };
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
        patch_ids: [],
        quality_summary: { recommendation: "keep" },
      });
    }
    const place = places.get(key);
    place.available_roots.push(tile.root);
    place.available_captures.push(tile.capture_id);
    place.tile_ids.push(tile.tile_id);
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
