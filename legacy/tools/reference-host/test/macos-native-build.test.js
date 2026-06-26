import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const macosDir = path.join(repoRoot, "native", "macos");

function hasSwift() {
  try {
    execFileSync("swift", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasCargo() {
  try {
    execFileSync("cargo", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function buildMacOSForgeFfi() {
  execFileSync(
    "cargo",
    [
      "build",
      "-p",
      "forge-ffi",
      "--locked",
    ],
    {
      cwd: path.join(repoRoot, "forge"),
      stdio: "ignore",
    },
  );
  const dylibPath = path.join(repoRoot, "forge", "target", "debug", "libforge_ffi.dylib");
  assert.equal(fs.existsSync(dylibPath), true);
  const symbols = execFileSync("nm", ["-gU", dylibPath], { encoding: "utf8" });
  assert.match(symbols, /_forge_core_open_in_memory/);
  assert.match(symbols, /_forge_core_handle_command/);
  assert.match(symbols, /_forge_string_free/);
  return dylibPath;
}

function runOptionalLaunchSmoke({ scratch, env }) {
  if (process.env.TERRANE_MACOS_SMOKE_LAUNCH !== "1") return;
  const smokeTmp = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-macos-launch-"));
  const markerPath = path.join(smokeTmp, "terrane-macos-smoke-launched.txt");
  try {
    execFileSync(
      "swift",
      [
        "run",
        "--scratch-path",
        scratch,
        "TerraneHostMac",
        "--terrane-smoke-launch",
        "--terrane-smoke-exit-after-launch",
      ],
      {
        cwd: macosDir,
        encoding: "utf8",
        env: {
          ...env,
          TERRANE_MACOS_SMOKE_MARKER_PATH: markerPath,
          TMPDIR: `${smokeTmp}${path.sep}`,
        },
        timeout: 60_000,
      },
    );
    assert.equal(fs.existsSync(markerPath), true);
    assert.equal(fs.readFileSync(markerPath, "utf8"), "TERRANE_MACOS_SMOKE_APP_LAUNCHED");
  } finally {
    fs.rmSync(smokeTmp, { recursive: true, force: true });
  }
}

test(
  "macOS native scaffold builds, passes SwiftPM tests, and optionally launches",
  {
    skip: process.platform !== "darwin" ? "macOS SwiftPM build smoke only runs on Darwin hosts" : !hasSwift() ? "swift is not available" : !hasCargo() ? "cargo is not available" : false,
    timeout: 120_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-macos-swiftpm-"));
    try {
      const forgeFfiDylib = buildMacOSForgeFfi();
      const env = {
        ...process.env,
        MACOSX_DEPLOYMENT_TARGET: "13.0",
        TERRANE_FORGE_FFI_DYLIB_FOR_TEST: forgeFfiDylib,
      };
      const output = execFileSync("swift", ["test", "--scratch-path", scratch], {
        cwd: macosDir,
        encoding: "utf8",
        env,
      });

      assert.match(output, /Test run with \d+ tests in 1 suite passed/);
      runOptionalLaunchSmoke({ scratch, env });
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);
