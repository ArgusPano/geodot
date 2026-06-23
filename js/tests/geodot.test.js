import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  download,
  latlonToTile,
  polygonFromGeoJSON,
  metersPerPixel,
  tileBounds,
  tileGrid,
  tileGridBetween,
  tileGridForPolygon,
  tilePath,
  prepareDataset,
} from "@geodot/lib";

const geojson = {
  type: "FeatureCollection",
  features: [
    {
      type: "Feature",
      geometry: {
        type: "Polygon",
        coordinates: [
          [
            [37.6504, 55.7304],
            [37.652, 55.7304],
            [37.652, 55.7297],
            [37.6504, 55.7297],
            [37.6504, 55.7304],
          ],
        ],
      },
    },
  ],
};

test("latlonToTile converts coordinates", () => {
  assert.deepEqual(latlonToTile(55.7303, 37.6504907, 18), {
    x: 158488,
    y: 81979,
    z: 18,
  });
});

test("tileGrid expands right and down", () => {
  assert.deepEqual(tileGrid(55.7303, 37.6504907, 18, 2, 2), [
    { x: 158488, y: 81979, z: 18 },
    { x: 158489, y: 81979, z: 18 },
    { x: 158488, y: 81980, z: 18 },
    { x: 158489, y: 81980, z: 18 },
  ]);
});

test("tilePath builds nested path", () => {
  assert.equal(tilePath("data", { x: 1, y: 2, z: 3 }), "data/tiles/3/1/2.jpg");
});

test("tileGridBetween expands a top-left to bottom-right rectangle", () => {
  assert.deepEqual(tileGridBetween(55.7303, 37.6504907, 55.7297, 37.652, 18), [
    { x: 158488, y: 81979, z: 18 },
    { x: 158489, y: 81979, z: 18 },
    { x: 158488, y: 81980, z: 18 },
    { x: 158489, y: 81980, z: 18 },
  ]);
});

test("tileGridForPolygon selects tiles inside a polygon", () => {
  const polygon = [
    { lon: 37.6504, lat: 55.7304 },
    { lon: 37.652, lat: 55.7304 },
    { lon: 37.652, lat: 55.7297 },
    { lon: 37.6504, lat: 55.7297 },
  ];
  assert.equal(tileGridForPolygon(polygon, 18).length, 4);
});

test("polygonFromGeoJSON reads a FeatureCollection polygon", () => {
  assert.deepEqual(polygonFromGeoJSON(geojson).slice(0, 4), [
    { lon: 37.6504, lat: 55.7304 },
    { lon: 37.652, lat: 55.7304 },
    { lon: 37.652, lat: 55.7297 },
    { lon: 37.6504, lat: 55.7297 },
  ]);
});

test("tileBounds and resolution include the source point", () => {
  const bounds = tileBounds({ x: 158488, y: 81979, z: 18 });
  assert.equal(bounds.lonMin < 37.6504907 && 37.6504907 < bounds.lonMax, true);
  assert.equal(bounds.latMin < 55.7303 && 55.7303 < bounds.latMax, true);
  assert.equal(metersPerPixel(55.7303, 18) > 0.2, true);
  assert.equal(metersPerPixel(55.7303, 18) < 0.4, true);
});

test("download manifest includes per-tile bounds", async () => {
  const originalFetch = globalThis.fetch;
  const out = await mkdtemp(path.join(tmpdir(), "geodot-"));
  globalThis.fetch = async () => new Response(Buffer.alloc(128));
  try {
    await download({
      lat: 55.7303,
      lon: 37.6504907,
      zoom: 18,
      cols: 1,
      rows: 1,
      out,
      jobs: 1,
    });
    const manifest = JSON.parse(
      await readFile(path.join(out, "manifest.json"), "utf8"),
    );
    const demo = await readFile(path.join(out, "index.html"), "utf8");
    assert.deepEqual(Object.keys(manifest.tiles[0].bounds).sort(), [
      "lat_max",
      "lat_min",
      "lon_max",
      "lon_min",
    ]);
    assert.match(demo, /maplibregl\.Map/);
    assert.match(demo, /World_Imagery/);
    assert.match(demo, /\.\/tiles\/\{z\}\/\{x\}\/\{y\}\.jpg/);
    assert.doesNotMatch(demo, /%7Bz%7D/);
    assert.match(demo, /minZoom: data\.zoom/);
    assert.doesNotMatch(demo, /fitBounds/);
  } finally {
    globalThis.fetch = originalFetch;
    await rm(out, { recursive: true, force: true });
  }
});

test("download can skip manifest and still write demo", async () => {
  const originalFetch = globalThis.fetch;
  const out = await mkdtemp(path.join(tmpdir(), "geodot-"));
  globalThis.fetch = async () => new Response(Buffer.alloc(128));
  try {
    await download({
      lat: 55.7303,
      lon: 37.6504907,
      zoom: 18,
      cols: 1,
      rows: 1,
      out,
      jobs: 1,
      noManifest: true,
    });
    await assert.rejects(() =>
      readFile(path.join(out, "manifest.json"), "utf8"),
    );
    const demo = await readFile(path.join(out, "index.html"), "utf8");
    assert.match(demo, /maplibregl\.Map/);
  } finally {
    globalThis.fetch = originalFetch;
    await rm(out, { recursive: true, force: true });
  }
});

test("download can skip demo", async () => {
  const originalFetch = globalThis.fetch;
  const out = await mkdtemp(path.join(tmpdir(), "geodot-"));
  globalThis.fetch = async () => new Response(Buffer.alloc(128));
  try {
    await download({
      lat: 55.7303,
      lon: 37.6504907,
      zoom: 18,
      cols: 1,
      rows: 1,
      out,
      jobs: 1,
      noDemo: true,
    });
    await readFile(path.join(out, "manifest.json"), "utf8");
    await assert.rejects(() => readFile(path.join(out, "index.html"), "utf8"));
  } finally {
    globalThis.fetch = originalFetch;
    await rm(out, { recursive: true, force: true });
  }
});

test("prepareDataset writes virtual VPR manifests", async () => {
  const out = await mkdtemp(path.join(tmpdir(), "geodot-prepare-"));
  try {
    for (const x of [1, 2]) {
      for (const y of [3, 4]) {
        const file = path.join(out, "tiles", "3", String(x), `${y}.jpg`);
        await mkdir(path.dirname(file), { recursive: true });
        await writeFile(file, Buffer.alloc(128));
      }
    }
    const report = await prepareDataset({
      out,
      patchSizes: [1, 2],
      rotations: [0, 90],
    });
    assert.equal(report.tiles, 4);
    assert.equal(report.patches, 5);
    assert.equal(report.variants, 10);
    const patches = JSON.parse(
      await readFile(path.join(out, "vpr", "manifest", "patches.json"), "utf8"),
    );
    const mosaic = patches.find((patch) => patch.mosaic_size_tiles === 2);
    assert.equal(mosaic.source_x_min, 1);
    assert.equal(mosaic.source_x_max, 2);
    assert.equal(mosaic.source_y_min, 3);
    assert.equal(mosaic.source_y_max, 4);
    assert.equal(mosaic.image_path_or_virtual_spec.type, "virtual_mosaic");
  } finally {
    await rm(out, { recursive: true, force: true });
  }
});

test("download rejects invalid jobs", async () => {
  await assert.rejects(() => download({ jobs: Number.NaN }), {
    name: "TypeError",
    message: "jobs must be an integer from 1 to 9007199254740991",
  });
});

test("download rejects invalid numeric options", async () => {
  await assert.rejects(() => download({ lat: Number.NaN }), {
    name: "TypeError",
    message: "lat must be a finite number",
  });
  await assert.rejects(() => download({ cols: 0 }), {
    name: "TypeError",
    message: "cols must be an integer from 1 to 9007199254740991",
  });
  await assert.rejects(() => download({ zoom: 31 }), {
    name: "TypeError",
    message: "zoom must be an integer from 0 to 30",
  });
});
