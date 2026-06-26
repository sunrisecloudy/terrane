import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const docsDir = path.join(repoRoot, "forge/docs/public-api");
const commands = JSON.parse(
  fs.readFileSync(path.join(repoRoot, "forge/data/commands.json"), "utf8"),
);
const bundledApps = JSON.parse(
  fs.readFileSync(path.join(repoRoot, "forge/data/bundled-apps.json"), "utf8"),
);

test("build-forge-api-docs generates a complete public API page", () => {
  execFileSync("node", ["--no-warnings", "tools/build-forge-api-docs.mjs"], {
    cwd: repoRoot,
    stdio: "pipe",
  });

  const html = fs.readFileSync(path.join(docsDir, "index.html"), "utf8");
  assert.match(html, /<title>Forge Public API Reference<\/title>/);
  assert.ok(fs.existsSync(path.join(docsDir, "styles.css")));
  assert.ok(fs.existsSync(path.join(docsDir, "app.js")));

  for (const surface of ["ctx.db", "ctx.storage", "ctx.net", "ctx.files", "ctx.ui", "ctx.time", "ctx.random"]) {
    assert.match(html, new RegExp(surface.replace(".", "\\.")), `${surface} documented`);
  }

  const outerCommands = commands.commands.filter((entry) => entry.surface === "outer");
  for (const cmd of outerCommands) {
    assert.match(html, new RegExp(cmd.name.replace(".", "\\.")), `${cmd.name} documented`);
  }

  for (const app of bundledApps) {
    assert.match(html, new RegExp(`id="example-${app.id}"`), `${app.id} example section`);
  }

  for (const cli of ["forge commands", "forge run", "forge demo", "forge trace"]) {
    assert.match(html, new RegExp(cli.replace(" ", "\\s+")), `${cli} documented`);
  }

  assert.match(html, /POST.*\/bridge/s);
  assert.match(html, /GET.*\/docs/s);
});