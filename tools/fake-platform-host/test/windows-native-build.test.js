import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const windowsDir = path.join(repoRoot, "native", "windows");

function commandWorks(command, args = ["--version"]) {
  try {
    execFileSync(command, args, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

test(
  "Windows WebView2 host builds and optionally runs native smoke",
  {
    skip: process.platform !== "win32"
      ? "Windows native smoke only runs on Windows hosts"
      : !commandWorks("cmake")
        ? "cmake is not available"
        : !commandWorks("zig", ["version"])
          ? "zig is not available"
          : false,
    timeout: 180_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-windows-smoke-"));
    try {
      const zigCoreDll = path.join(scratch, "zig_core.dll");
      execFileSync(
        "zig",
        [
          "build-lib",
          "src/lib.zig",
          "--name",
          "zig_core",
          "-dynamic",
          "-lc",
          `-femit-bin=${zigCoreDll}`,
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
      assert.equal(fs.existsSync(zigCoreDll), true);

      const buildDir = path.join(scratch, "build");
      execFileSync("cmake", ["-S", windowsDir, "-B", buildDir, `-DNATIVE_AI_ZIG_CORE_DLL=${zigCoreDll}`], { stdio: "ignore" });
      execFileSync("cmake", ["--build", buildDir, "--config", "Debug"], { stdio: "ignore" });
      const binaryPath = resolveWindowsHostBinary(buildDir);
      assert.notEqual(binaryPath, null, "NativeAIWebappHost.exe should exist after CMake build");
      const binaryDir = path.dirname(binaryPath);
      assert.equal(
        fs.existsSync(path.join(binaryDir, "zig_core.dll")),
        true,
        "zig_core.dll should be staged next to NativeAIWebappHost.exe for package-style loading",
      );
      assert.equal(
        fs.existsSync(path.join(binaryDir, "resources", "runtime", "index.html")),
        true,
        "runtime-web should be staged under the WebView2 /runtime resource path",
      );
      assert.equal(
        fs.existsSync(path.join(binaryDir, "resources", "webapps", "examples", "notes-lite", "manifest.json")),
        true,
        "example apps should be staged under the WebView2 /webapps/examples resource path",
      );
      assert.equal(
        fs.existsSync(path.join(binaryDir, "resources", "db", "sqlite", "001_initial.sql")),
        true,
        "SQLite migrations should be staged under packaged resources",
      );

      runOptionalSmoke({ binaryPath, scratch });
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

function resolveWindowsHostBinary(buildDir) {
  for (const candidate of [
    path.join(buildDir, "Debug", "NativeAIWebappHost.exe"),
    path.join(buildDir, "NativeAIWebappHost.exe"),
    path.join(buildDir, "Release", "NativeAIWebappHost.exe"),
  ]) {
    if (fs.existsSync(candidate)) return candidate;
  }
  return null;
}

function runOptionalSmoke({ binaryPath, scratch }) {
  if (process.env.NATIVE_AI_WINDOWS_SMOKE_LAUNCH !== "1") return;
  const storageKey = `notes-lite:windows-smoke-${process.pid}-${Date.now()}`;
  const storageValue = `windows-smoke-${process.pid}-${Date.now()}`;
  const dataHome = path.join(scratch, "data-home");
  const resultFile = path.join(scratch, "smoke-result.txt");
  const { NATIVE_AI_ZIG_CORE_DLL: _ignoredZigCoreDll, ...smokeEnv } = process.env;
  const baseEnv = {
    ...smokeEnv,
    NATIVE_AI_WINDOWS_SMOKE_DATA_HOME: dataHome,
    NATIVE_AI_WINDOWS_SMOKE_EXIT_AFTER: "1",
    NATIVE_AI_WINDOWS_SMOKE_RESULT_FILE: resultFile,
  };

  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_RUNTIME_LOADED", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "runtime-load",
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_STORAGE_SET_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "storage-set",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_STORAGE_GET_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "storage-get",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_CORE_STEP_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "core-step",
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_FIXED_BRIDGE_SURFACE_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "fixed-bridge-surface",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_BRIDGE_STORAGE_SET_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "bridge-storage-set",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_BRIDGE_STORAGE_GET_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "bridge-storage-get",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_BRIDGE_CORE_STEP_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "bridge-core-step",
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_RUNTIME_APP_STORAGE_GET_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "runtime-app-storage-get",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
}

function runSmoke(binaryPath, resultFile, marker, env) {
  fs.rmSync(resultFile, { force: true });
  const result = spawnSync(binaryPath, [], { env, cwd: path.dirname(binaryPath), encoding: "utf8", timeout: 30_000 });
  const markerOutput = fs.existsSync(resultFile) ? fs.readFileSync(resultFile, "utf8") : "";
  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}\n${markerOutput}`;
  assert.equal(output.includes("NATIVE_AI_WINDOWS_SMOKE_FAILED"), false, output);
  assert.equal(result.error, undefined, output);
  assert.equal(result.status, 0, output);
  assert.equal(output.includes(marker), true, `Timed out waiting for ${marker}\n${output}`);
}
