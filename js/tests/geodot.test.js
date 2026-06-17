import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  download,
  latlonToTile,
  metersPerPixel,
  tileBounds,
  tileGrid,
  tileGridBetween,
  tileGridForPolygon,
  tilePath,
} from "@geodot/lib";

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
    assert.deepEqual(Object.keys(manifest.tiles[0].bounds).sort(), [
      "lat_max",
      "lat_min",
      "lon_max",
      "lon_min",
    ]);
  } finally {
    globalThis.fetch = originalFetch;
    await rm(out, { recursive: true, force: true });
  }
});
