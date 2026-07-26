#!/usr/bin/env node

import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import http from "node:http";
import { basename, join, resolve } from "node:path";

function value(name, fallback) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : fallback;
}

const root = resolve(value("--root", ""));
const port = Number(value("--port", "8765"));
const bytesPerSecond = Number(value("--bytes-per-second", String(1024 * 1024)));
if (!root || !Number.isSafeInteger(port) || port < 1 || port > 65535) {
  process.stderr.write(
    "usage: bootstrap-test-server.mjs --root DIR [--port 8765] [--bytes-per-second 1048576]\n",
  );
  process.exit(2);
}

const server = http.createServer(async (request, response) => {
  try {
    const name = basename(new URL(request.url ?? "/", "http://127.0.0.1").pathname);
    if (!["terrane-bootstrap-manifest.json", "TerraneRuntime-arm64.zip"].includes(name)) {
      response.writeHead(404).end("not found\n");
      return;
    }
    const path = join(root, name);
    const info = await stat(path);
    const range = request.headers.range?.match(/^bytes=(\d+)-$/);
    const start = range ? Number(range[1]) : 0;
    if (!Number.isSafeInteger(start) || start < 0 || start >= info.size) {
      response.writeHead(416, { "Content-Range": `bytes */${info.size}` }).end();
      return;
    }
    const headers = {
      "Accept-Ranges": "bytes",
      "Cache-Control": "no-store",
      "Content-Length": String(info.size - start),
      "Content-Type": name.endsWith(".json") ? "application/json" : "application/zip",
    };
    if (start > 0) {
      headers["Content-Range"] = `bytes ${start}-${info.size - 1}/${info.size}`;
    }
    response.writeHead(start > 0 ? 206 : 200, headers);
    process.stdout.write(`${request.method} ${name} ${start}-${info.size - 1}\n`);

    const stream = createReadStream(path, {
      start,
      highWaterMark: Math.max(16 * 1024, Math.floor(bytesPerSecond / 10)),
    });
    let queued = Promise.resolve();
    stream.on("data", (chunk) => {
      stream.pause();
      queued = queued.then(
        () =>
          new Promise((finish) => {
            response.write(chunk);
            setTimeout(() => {
              stream.resume();
              finish();
            }, Math.max(1, Math.round((chunk.length / bytesPerSecond) * 1000)));
          }),
      );
    });
    stream.on("end", () => queued.then(() => response.end()));
    stream.on("error", (error) => response.destroy(error));
  } catch (error) {
    response.writeHead(500).end(`${error.message}\n`);
  }
});

server.listen(port, "127.0.0.1", () => {
  process.stdout.write(`bootstrap fixture server: http://127.0.0.1:${port}\n`);
});
