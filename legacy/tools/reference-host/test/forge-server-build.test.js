import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const forgeDir = path.join(repoRoot, "forge");

test(
  "Forge server tests pass and native executable builds",
  { timeout: 120_000 },
  () => {
    execFileSync("cargo", ["test", "-p", "forge-server", "--locked"], {
      cwd: forgeDir,
      stdio: "ignore",
    });
    execFileSync("cargo", ["build", "-p", "forge-server", "--release", "--locked"], {
      cwd: forgeDir,
      stdio: "ignore",
    });

    const executable = process.platform === "win32" ? "forge-server.exe" : "forge-server";
    assert.equal(fs.existsSync(path.join(forgeDir, "target", "release", executable)), true);
  },
);
