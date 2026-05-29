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

function hasZig() {
  try {
    execFileSync("zig", ["version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function macosZigTarget() {
  return process.arch === "arm64" ? "aarch64-macos" : "x86_64-macos";
}

function buildMacOSZigCore(scratch) {
  if (!hasZig()) return null;
  const dylibPath = path.join(scratch, "libzig_core.dylib");
  execFileSync(
    "zig",
    [
      "build-lib",
      "src/lib.zig",
      "-dynamic",
      "-target",
      macosZigTarget(),
      "-lc",
      `-femit-bin=${dylibPath}`,
    ],
    {
      cwd: path.join(repoRoot, "zig-core"),
      env: {
        ...process.env,
        ZIG_GLOBAL_CACHE_DIR: path.join(scratch, "zig-global-cache"),
        ZIG_LOCAL_CACHE_DIR: path.join(scratch, "zig-local-cache"),
      },
      stdio: "ignore",
    },
  );
  assert.equal(fs.existsSync(dylibPath), true);
  const symbols = execFileSync("nm", ["-gU", dylibPath], { encoding: "utf8" });
  assert.match(symbols, /_core_create/);
  assert.match(symbols, /_core_step_json/);
  assert.match(symbols, /_core_free/);
  return dylibPath;
}

function runOptionalLaunchSmoke({ scratch, env }) {
  if (process.env.NATIVE_AI_MACOS_SMOKE_LAUNCH !== "1") return;
  const smokeTmp = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-macos-launch-"));
  const markerPath = path.join(smokeTmp, "native-ai-macos-smoke-launched.txt");
  try {
    execFileSync(
      "swift",
      [
        "run",
        "--scratch-path",
        scratch,
        "NativeAIHostMac",
        "--native-ai-smoke-launch",
        "--native-ai-smoke-exit-after-launch",
      ],
      {
        cwd: macosDir,
        encoding: "utf8",
        env: {
          ...env,
          NATIVE_AI_MACOS_SMOKE_MARKER_PATH: markerPath,
          TMPDIR: `${smokeTmp}${path.sep}`,
        },
        timeout: 60_000,
      },
    );
    assert.equal(fs.existsSync(markerPath), true);
    assert.equal(fs.readFileSync(markerPath, "utf8"), "NATIVE_AI_MACOS_SMOKE_APP_LAUNCHED");
  } finally {
    fs.rmSync(smokeTmp, { recursive: true, force: true });
  }
}

test(
  "macOS native scaffold builds, passes SwiftPM tests, and optionally launches",
  {
    skip: process.platform !== "darwin" ? "macOS SwiftPM build smoke only runs on Darwin hosts" : !hasSwift() ? "swift is not available" : false,
    timeout: 120_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-macos-swiftpm-"));
    try {
      const zigCoreDylib = buildMacOSZigCore(scratch);
      const env = {
        ...process.env,
        MACOSX_DEPLOYMENT_TARGET: "13.0",
        ...(zigCoreDylib ? { NATIVE_AI_ZIG_CORE_DYLIB_FOR_TEST: zigCoreDylib } : {}),
      };
      const output = execFileSync("swift", ["test", "--scratch-path", scratch], {
        cwd: macosDir,
        encoding: "utf8",
        env,
      });

      assert.match(output, /Test run with 5 tests in 1 suite passed/);
      runOptionalLaunchSmoke({ scratch, env });
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);
