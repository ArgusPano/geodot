import { mkdir, readFile, writeFile } from "node:fs/promises";
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
  const center = latlonToTile(lat, lon, zoom);
  return Array.from({ length: rows }, (_, row) =>
    Array.from({ length: cols }, (_, col) => ({
      x: center.x + col,
      y: center.y + row,
      z: zoom,
    })),
  ).flat();
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
  return tilesInRange(
    Math.min(first.x, second.x),
    Math.max(first.x, second.x),
    Math.min(first.y, second.y),
    Math.max(first.y, second.y),
    zoom,
  );
}

export function tileGridForPolygon(points, zoom) {
  if (points.length < 3) return [];
  const lats = points.map((point) => point.lat);
  const lons = points.map((point) => point.lon);
  return tileGridBetween(
    Math.max(...lats),
    Math.min(...lons),
    Math.min(...lats),
    Math.max(...lons),
    zoom,
  ).filter((tile) => tileIntersectsPolygon(tile, points));
}

export function tilesForOptions(options) {
  if (options.polygon?.length >= 3)
    return tileGridForPolygon(options.polygon, options.zoom);
  if (
    options.bottomRightLat !== undefined &&
    options.bottomRightLon !== undefined
  ) {
    return tileGridBetween(
      options.lat,
      options.lon,
      options.bottomRightLat,
      options.bottomRightLon,
      options.zoom,
    );
  }
  return tileGrid(
    options.lat,
    options.lon,
    options.zoom,
    options.cols,
    options.rows,
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
  const queue = tilesForOptions(config);
  const tiles = [];
  const failed = [];

  async function worker() {
    while (queue.length > 0) {
      const tile = queue.shift();
      const data = await downloadTile(tile);
      if (!data) {
        failed.push(tile);
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

function tilesInRange(minX, maxX, minY, maxY, z) {
  return Array.from({ length: maxY - minY + 1 }, (_, row) =>
    Array.from({ length: maxX - minX + 1 }, (_, col) => ({
      x: minX + col,
      y: minY + row,
      z,
    })),
  ).flat();
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
