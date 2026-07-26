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
const stallAfterBytes = Number(value("--stall-after-bytes", "0"));
const stallSeconds = Number(value("--stall-seconds", "0"));
const stallCount = Number(value("--stall-count", "1"));
if (!root || !Number.isSafeInteger(port) || port < 1 || port > 65535) {
  process.stderr.write(
    "usage: bootstrap-test-server.mjs --root DIR [--port 8765] "
      + "[--bytes-per-second 1048576] [--stall-after-bytes 0] [--stall-seconds 0] "
      + "[--stall-count 1]\n",
  );
  process.exit(2);
}

let activeResponses = 0;
let peakResponses = 0;
let stalledResponses = 0;

const server = http.createServer(async (request, response) => {
  try {
    const name = basename(new URL(request.url ?? "/", "http://127.0.0.1").pathname);
    if (!["terrane-bootstrap-manifest.json", "TerraneRuntime-arm64.zip"].includes(name)) {
      response.writeHead(404).end("not found\n");
      return;
    }
    const path = join(root, name);
    const info = await stat(path);
    const range = request.headers.range?.match(/^bytes=(\d+)-(\d*)$/);
    const start = range ? Number(range[1]) : 0;
    const end = range && range[2] ? Number(range[2]) : info.size - 1;
    if (
      !Number.isSafeInteger(start)
      || !Number.isSafeInteger(end)
      || start < 0
      || start >= info.size
      || end < start
      || end >= info.size
    ) {
      response.writeHead(416, { "Content-Range": `bytes */${info.size}` }).end();
      return;
    }
    const headers = {
      "Accept-Ranges": "bytes",
      "Cache-Control": "no-store",
      "Content-Length": String(end - start + 1),
      "Content-Type": name.endsWith(".json") ? "application/json" : "application/zip",
    };
    if (range) {
      headers["Content-Range"] = `bytes ${start}-${end}/${info.size}`;
    }
    response.writeHead(range ? 206 : 200, headers);
    process.stdout.write(`${request.method} ${name} ${start}-${end}\n`);
    if (request.method === "HEAD") {
      response.end();
      return;
    }

    const stream = createReadStream(path, {
      start,
      end,
      highWaterMark: Math.max(16 * 1024, Math.floor(bytesPerSecond / 10)),
    });
    activeResponses += 1;
    peakResponses = Math.max(peakResponses, activeResponses);
    process.stdout.write(`active=${activeResponses} peak=${peakResponses}\n`);
    let responseClosed = false;
    const closeResponse = () => {
      if (responseClosed) return;
      responseClosed = true;
      activeResponses -= 1;
    };
    response.on("close", closeResponse);
    let sent = 0;
    let didStall = false;
    let queued = Promise.resolve();
    stream.on("data", (chunk) => {
      stream.pause();
      queued = queued.then(
        () =>
          new Promise((finish) => {
            response.write(chunk);
            sent += chunk.length;
            const shouldStall =
              !didStall
              && stalledResponses < stallCount
              && stallAfterBytes > 0
              && sent >= stallAfterBytes;
            const stallDelay =
              shouldStall
                ? stallSeconds * 1000
                : 0;
            if (stallDelay > 0) {
              didStall = true;
              stalledResponses += 1;
              process.stdout.write(`stall ${name} ${start}-${end} for ${stallSeconds}s\n`);
            }
            setTimeout(() => {
              stream.resume();
              finish();
            }, stallDelay + Math.max(1, Math.round((chunk.length / bytesPerSecond) * 1000)));
          }),
      );
    });
    stream.on("end", () =>
      queued.then(() => {
        closeResponse();
        response.end();
      }),
    );
    stream.on("error", (error) => response.destroy(error));
  } catch (error) {
    response.writeHead(500).end(`${error.message}\n`);
  }
});

server.listen(port, "127.0.0.1", () => {
  process.stdout.write(`bootstrap fixture server: http://127.0.0.1:${port}\n`);
});
