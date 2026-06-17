import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { promisify } from "node:util";

import { download } from "@geodot/lib";

const execFileAsync = promisify(execFile);
const tileBytes = Buffer.alloc(128, "x");
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

async function withTileServer(callback) {
  const server = createServer((request, response) => {
    if (request.url === "/area.geojson") {
      const body = JSON.stringify(geojson);
      response.writeHead(200, {
        "Content-Type": "application/geo+json",
        "Content-Length": Buffer.byteLength(body),
      });
      response.end(body);
      return;
    }
    response.writeHead(200, {
      "Content-Type": "image/jpeg",
      "Content-Length": tileBytes.byteLength,
    });
    response.end(tileBytes);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  try {
    await callback(
      `http://127.0.0.1:${port}/{z}/{x}/{y}.jpg`,
      `http://127.0.0.1:${port}/area.geojson`,
    );
  } finally {
    await new Promise((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
}

async function assertDownloadOutput(out) {
  const manifest = JSON.parse(
    await readFile(path.join(out, "manifest.json"), "utf8"),
  );
  const tile = manifest.tiles[0];
  assert.equal(tile.bytes, tileBytes.byteLength);
  assert.deepEqual(Object.keys(tile.bounds).sort(), [
    "lat_max",
    "lat_min",
    "lon_max",
    "lon_min",
  ]);
  assert.deepEqual(
    await readFile(
      path.join(out, "tiles", "18", String(tile.tile.x), `${tile.tile.y}.jpg`),
    ),
    tileBytes,
  );
}

test("library download writes tiles and manifest", async () => {
  const out = await mkdtemp(path.join(tmpdir(), "geodot-lib-"));
  await withTileServer(async (template) => {
    process.env.GEODOT_TILE_URL_TEMPLATE = template;
    try {
      const report = await download({
        lat: 55.7303,
        lon: 37.6504907,
        zoom: 18,
        cols: 1,
        rows: 1,
        out,
        jobs: 1,
      });
      assert.equal(report.tiles.length, 1);
      assert.deepEqual(report.failed, []);
      await assertDownloadOutput(out);
    } finally {
      delete process.env.GEODOT_TILE_URL_TEMPLATE;
    }
  });
  await rm(out, { recursive: true, force: true });
});

test("library download accepts a local GeoJSON file", async () => {
  const out = await mkdtemp(path.join(tmpdir(), "geodot-lib-geojson-"));
  const geojsonFile = path.join(out, "area.geojson");
  await writeFile(geojsonFile, JSON.stringify(geojson));
  await withTileServer(async (template) => {
    process.env.GEODOT_TILE_URL_TEMPLATE = template;
    try {
      const report = await download({
        geojson: geojsonFile,
        zoom: 18,
        out,
        jobs: 1,
      });
      assert.equal(report.tiles.length, 4);
      assert.deepEqual(report.failed, []);
      await assertDownloadOutput(out);
    } finally {
      delete process.env.GEODOT_TILE_URL_TEMPLATE;
    }
  });
  await rm(out, { recursive: true, force: true });
});

test("CLI download writes tiles and manifest", async () => {
  const out = await mkdtemp(path.join(tmpdir(), "geodot-cli-"));
  await withTileServer(async (template, geojsonUrl) => {
    await execFileAsync(
      process.execPath,
      [
        "js/packages/cli/bin/geodot.js",
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
        "--geojson",
        geojsonUrl,
        "-o",
        out,
      ],
      { env: { ...process.env, GEODOT_TILE_URL_TEMPLATE: template } },
    );
    await assertDownloadOutput(out);
  });
  await rm(out, { recursive: true, force: true });
});

test("CLI rejects non-numeric jobs", async () => {
  await assert.rejects(
    execFileAsync(process.execPath, [
      "js/packages/cli/bin/geodot.js",
      "-j",
      "https://example.com/area.geojson",
    ]),
    (error) => {
      assert.match(error.stderr, /-j requires a number/);
      assert.match(error.stdout, /Usage: geodot/);
      return true;
    },
  );
});

test("CLI exposes demo command help", async () => {
  const { stdout } = await execFileAsync(process.execPath, [
    "js/packages/cli/bin/geodot.js",
    "demo",
    "--help",
  ]);
  assert.match(stdout, /geodot demo/);
  assert.match(stdout, /--no-open/);
});

test("CLI rejects invalid numeric options", async () => {
  await assert.rejects(
    execFileAsync(process.execPath, [
      "js/packages/cli/bin/geodot.js",
      "-z",
      "1.5",
    ]),
    (error) => {
      assert.match(error.stderr, /-z requires an integer/);
      assert.match(error.stdout, /Usage: geodot/);
      return true;
    },
  );
  await assert.rejects(
    execFileAsync(process.execPath, [
      "js/packages/cli/bin/geodot.js",
      "-c",
      "0",
    ]),
    (error) => {
      assert.match(error.stderr, /-c requires a value from 1/);
      assert.match(error.stdout, /Usage: geodot/);
      return true;
    },
  );
});
