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
    ...options,
  };
  if (config.geojson && !config.polygon) {
    config.polygon = await loadGeoJSONPolygon(config.geojson);
  }
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
  await writeFile(
    path.join(config.out, "manifest.json"),
    JSON.stringify(report, null, 2),
  );
  return report;
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
