import http from "node:http";
import fs from "node:fs";
import { controlTokenPath, generateControlToken, writeControlTokenFile } from "../../control-token.js";
import { ReferenceHost } from "./reference-host.js";
import { defaultPlatformKeyFile } from "./signing.js";

export async function startReferenceHost(options = {}) {
  const controlToken = options.controlToken ?? generateControlToken();
  const host = new ReferenceHost({
    dbFile: options.dbFile ?? ":memory:",
    controlToken,
    keyFile: options.keyFile ?? false,
  });

  if (options.seedBundled) {
    await seedBundledApps(host);
  }

  const server = http.createServer((req, res) => host.handleHttp(req, res));
  await new Promise((resolve) => server.listen(options.port ?? 7878, options.bind ?? "127.0.0.1", resolve));
  if (options.tokenFile) {
    writeControlTokenFile(controlToken, options.tokenFile);
  }

  return {
    host,
    server,
    controlToken,
    tokenFile: options.tokenFile ?? null,
    url: `http://${options.bind ?? "127.0.0.1"}:${server.address().port}`,
    close: async () => {
      await new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
      host.close();
    },
  };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const options = parseArgs(process.argv.slice(2));
  const started = await startReferenceHost(options);
  console.error(`reference-host listening on ${started.url}`);
  console.error(`control token file: ${started.tokenFile}`);
}

function parseArgs(args) {
  const options = {
    port: 7878,
    bind: "127.0.0.1",
    dbFile: ":memory:",
    controlToken: process.env.PLATFORM_CONTROL_TOKEN ?? generateControlToken(),
    keyFile: defaultPlatformKeyFile(),
    tokenFile: controlTokenPath({ env: process.env }),
    seedBundled: true,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--port") options.port = Number(args[++index]);
    else if (arg === "--bind") options.bind = args[++index];
    else if (arg === "--db-file") options.dbFile = args[++index];
    else if (arg === "--key-file") options.keyFile = args[++index];
    else if (arg === "--token-file") options.tokenFile = args[++index];
    else if (arg === "--control-token") options.controlToken = args[++index];
    else if (arg === "--seed-bundled") options.seedBundled = true;
    else if (arg === "--no-seed-bundled") options.seedBundled = false;
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
