#!/usr/bin/env node
/**
 * Emit forge/data/commands.json from a live core via system.describe.
 * Source of truth remains the Rust catalog; this is a generated projection.
 */
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const outPath = join(repoRoot, "forge/data/commands.json");

const command = {
  request_id: "export-catalog",
  actor: { actor: "export", role: "owner" },
  workspace_id: "ws-export",
  name: "system.describe",
  payload: { tier: "debug", include_inner: false },
};

function main() {
  const build = spawnSync("cargo", ["build", "-p", "forge-ffi", "--bin", "core-invoke"], {
    cwd: join(repoRoot, "forge"),
    stdio: "inherit",
  });
  if (build.status !== 0) {
    process.exit(build.status ?? 1);
  }

  const debugBin = join(repoRoot, "forge/target/debug/core-invoke");
  const bin = existsSync(debugBin)
    ? debugBin
    : join(repoRoot, "forge/target/release/core-invoke");

  const run = spawnSync(bin, [], {
    input: JSON.stringify(command),
    encoding: "utf8",
  });
  if (run.status !== 0) {
    console.error(run.stderr || run.stdout);
    process.exit(run.status ?? 1);
  }

  const response = JSON.parse(run.stdout);
  if (!response.ok) {
    console.error("system.describe failed:", response);
    process.exit(1);
  }

  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, `${JSON.stringify(response.payload, null, 2)}\n`);
  console.log(`Wrote ${outPath} (${response.payload.commands?.length ?? 0} commands)`);
}

main();