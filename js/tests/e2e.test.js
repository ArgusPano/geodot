import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { promisify } from "node:util";

import { download } from "../src/index.js";

const execFileAsync = promisify(execFile);
const tileBytes = Buffer.alloc(128, "x");

async function withTileServer(callback) {
  const server = createServer((request, response) => {
    response.writeHead(200, {
      "Content-Type": "image/jpeg",
      "Content-Length": tileBytes.byteLength,
    });
    response.end(tileBytes);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  try {
    await callback(`http://127.0.0.1:${port}/{z}/{x}/{y}.jpg`);
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

test("CLI download writes tiles and manifest", async () => {
  const out = await mkdtemp(path.join(tmpdir(), "geodot-cli-"));
  await withTileServer(async (template) => {
    await execFileAsync(
      process.execPath,
      [
        "js/bin/geodot.js",
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
        out,
      ],
      { env: { ...process.env, GEODOT_TILE_URL_TEMPLATE: template } },
    );
    await assertDownloadOutput(out);
  });
  await rm(out, { recursive: true, force: true });
});
