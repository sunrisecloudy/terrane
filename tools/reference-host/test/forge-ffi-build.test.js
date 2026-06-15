import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
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

function commandWorks(command, args = ["--version"]) {
  try {
    execFileSync(command, args, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function cSmokeSource() {
  return String.raw`#include "forge_ffi.h"
#include <string.h>

int main(void) {
    ForgeCoreHandle *core = forge_core_open_in_memory("c-smoke");
    if (core == 0) return 1;

    const char *cmd = "{\"request_id\":\"c1\",\"actor\":{\"actor\":\"c-smoke\",\"role\":\"owner\"},\"workspace_id\":\"c-smoke\",\"name\":\"workspace.open\",\"payload\":{}}";
    char *out = forge_core_handle_command(core, cmd);
    if (out == 0) {
        forge_core_close(core);
        return 2;
    }

    int ok = strstr(out, "\"ok\":true") != 0;
    forge_string_free(out);

    char *events = forge_core_drain_events(core);
    if (events != 0) forge_string_free(events);

    forge_core_close(core);
    return ok ? 0 : 3;
}
`;
}

function runStaticlibCSmoke(releaseDir) {
  if (process.platform === "win32") return;
  const compiler = process.env.CC || "cc";
  assert.equal(commandWorks(compiler), true, `${compiler} must be available for forge_ffi C smoke`);

  const staticLibrary = staticLibraryCandidates()
    .map((candidate) => path.join(releaseDir, candidate))
    .find((candidate) => fs.existsSync(candidate));
  assert.ok(staticLibrary, "forge_ffi static library should exist");

  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "forge-ffi-c-smoke-"));
  try {
    const sourcePath = path.join(scratch, "smoke.c");
    const binaryPath = path.join(scratch, "smoke");
    fs.writeFileSync(sourcePath, cSmokeSource());
    execFileSync(
      compiler,
      [
        "-I",
        path.join(forgeDir, "crates", "ffi", "include"),
        sourcePath,
        staticLibrary,
        "-o",
        binaryPath,
      ],
      { stdio: "pipe" },
    );
    execFileSync(binaryPath, { stdio: "pipe" });
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
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

    runStaticlibCSmoke(releaseDir);
  },
);
