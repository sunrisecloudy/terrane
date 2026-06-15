import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const forgeDir = path.join(repoRoot, "forge");

function dynamicLibraryName() {
  if (process.platform === "darwin") return "libforge_ffi.dylib";
  if (process.platform === "win32") return "forge_ffi.dll";
  return "libforge_ffi.so";
}

function staticLibraryCandidates() {
  if (process.platform === "win32") return ["forge_ffi.lib", "forge_ffi.dll.lib"];
  return ["libforge_ffi.a"];
}

function exportedSymbols(binaryPath) {
  if (process.platform === "win32") return "";
  const args = process.platform === "darwin" ? ["-gU", binaryPath] : ["-g", binaryPath];
  return execFileSync("nm", args, { encoding: "utf8" });
}

test(
  "Forge FFI tests pass and native libraries build",
  { timeout: 120_000 },
  () => {
    execFileSync("cargo", ["test", "-p", "forge-ffi", "--locked"], {
      cwd: forgeDir,
      stdio: "ignore",
    });
    execFileSync("cargo", ["build", "-p", "forge-ffi", "--release", "--locked"], {
      cwd: forgeDir,
      stdio: "ignore",
    });

    const releaseDir = path.join(forgeDir, "target", "release");
    const dynamicPath = path.join(releaseDir, dynamicLibraryName());
    assert.equal(fs.existsSync(dynamicPath), true);
    assert.equal(
      staticLibraryCandidates().some((candidate) => fs.existsSync(path.join(releaseDir, candidate))),
      true,
    );

    const header = fs.readFileSync(path.join(forgeDir, "crates", "ffi", "include", "forge_ffi.h"), "utf8");
    for (const symbol of [
      "forge_core_open",
      "forge_core_handle_command",
      "forge_core_drain_events",
      "forge_core_close",
      "forge_string_free",
    ]) {
      assert.match(header, new RegExp(`\\b${symbol}\\b`));
    }

    const symbols = exportedSymbols(dynamicPath);
    if (symbols) {
      assert.match(symbols, /\b_?forge_core_open\b/);
      assert.match(symbols, /\b_?forge_core_handle_command\b/);
      assert.match(symbols, /\b_?forge_core_drain_events\b/);
      assert.match(symbols, /\b_?forge_string_free\b/);
    }
  },
);
