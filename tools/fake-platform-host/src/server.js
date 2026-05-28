import http from "node:http";
import fs from "node:fs";
import { FakePlatformHost } from "./fake-host.js";

export async function startFakePlatformHost(options = {}) {
  const host = new FakePlatformHost({
    dbFile: options.dbFile ?? ":memory:",
    controlToken: options.controlToken ?? "dev-token-change-me",
  });

  if (options.seedBundled) {
    await seedBundledApps(host);
  }

  const server = http.createServer((req, res) => host.handleHttp(req, res));
  await new Promise((resolve) => server.listen(options.port ?? 7878, options.bind ?? "127.0.0.1", resolve));

  return {
    host,
    server,
    url: `http://${options.bind ?? "127.0.0.1"}:${server.address().port}`,
    close: async () => {
      await new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
      host.close();
    },
  };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const options = parseArgs(process.argv.slice(2));
  if (options.tokenFile) {
    fs.writeFileSync(options.tokenFile, options.controlToken, { mode: 0o600 });
  }
  const started = await startFakePlatformHost(options);
  console.error(`fake-platform-host listening on ${started.url}`);
}

function parseArgs(args) {
  const options = {
    port: 7878,
    bind: "127.0.0.1",
    dbFile: ":memory:",
    controlToken: process.env.PLATFORM_CONTROL_TOKEN ?? "dev-token-change-me",
    seedBundled: false,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--port") options.port = Number(args[++index]);
    else if (arg === "--bind") options.bind = args[++index];
    else if (arg === "--db-file") options.dbFile = args[++index];
    else if (arg === "--token-file") options.tokenFile = args[++index];
    else if (arg === "--control-token") options.controlToken = args[++index];
    else if (arg === "--seed-bundled") options.seedBundled = true;
    else throw new Error(`Unknown option: ${arg}`);
  }

  return options;
}

async function seedBundledApps(host) {
  const { examplesDir } = await import("./paths.js");
  const path = await import("node:path");
  for (const entry of fs.readdirSync(examplesDir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      host.installPackage(path.join(examplesDir, entry.name), { trustLevel: "bundled" });
    }
  }
}
