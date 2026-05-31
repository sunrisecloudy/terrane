import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { packageReleaseArtifacts } from "../../../tools/package-release.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const linuxDir = path.join(repoRoot, "native", "linux");

function commandWorks(command, args = ["--version"]) {
  try {
    execFileSync(command, args, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function commandExists(command) {
  try {
    execFileSync("sh", ["-c", "command -v \"$1\" >/dev/null", "sh", command], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasLinuxNativeDependencies() {
  return commandWorks("pkg-config", [
    "--exists",
    "gtk4",
    "webkitgtk-6.0",
    "json-glib-1.0",
    "sqlite3",
    "libsoup-3.0",
  ]);
}

function linuxNativeSkipReason({ requireZig = false, requireSqliteCli = false } = {}) {
  if (process.platform !== "linux") return "Linux native smoke only runs on Linux hosts";
  if (!commandWorks("meson")) return "meson is not available";
  if (!commandWorks("ninja")) return "ninja is not available";
  if (requireZig && !commandWorks("zig", ["version"])) return "zig is not available";
  if (requireSqliteCli && !commandWorks("sqlite3", ["-version"])) return "sqlite3 CLI is not available";
  if (!hasLinuxNativeDependencies()) return "GTK/WebKitGTK development dependencies are not available";
  return false;
}

function linuxPackagedNativeSmokeSkipReason() {
  const baseReason = linuxNativeSkipReason({ requireZig: true });
  if (baseReason) return baseReason;
  if (process.env.NATIVE_AI_LINUX_SMOKE_LAUNCH !== "1") {
    return "set NATIVE_AI_LINUX_SMOKE_LAUNCH=1 to run packaged Linux native launch smoke";
  }
  return false;
}

test(
  "Linux GTK/WebKitGTK host builds and optionally runs native smoke",
  {
    skip: linuxNativeSkipReason({ requireZig: true }),
    timeout: 180_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-linux-smoke-"));
    try {
      const zigCoreSo = path.join(scratch, "libzig_core.so");
      execFileSync(
        "zig",
        [
          "build-lib",
          "src/lib.zig",
          "--name",
          "zig_core",
          "-dynamic",
          "-lc",
          "-fsoname=libzig_core.so",
          `-femit-bin=${zigCoreSo}`,
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
      assert.equal(fs.existsSync(zigCoreSo), true);

      const buildDir = path.join(scratch, "build");
      execFileSync("meson", ["setup", buildDir, linuxDir], { stdio: "ignore" });
      execFileSync("meson", ["compile", "-C", buildDir], { stdio: "ignore" });
      const binaryPath = path.join(buildDir, "native-ai-webapp-host");
      assert.equal(fs.existsSync(binaryPath), true);

      runOptionalSmoke({ binaryPath, scratch, zigCoreSo });
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Linux release host rejects dev-only startup flags and audits the rejection",
  {
    skip: linuxNativeSkipReason({ requireSqliteCli: true }),
    timeout: 120_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-linux-production-guard-"));
    try {
      const buildDir = path.join(scratch, "release-build");
      execFileSync("meson", ["setup", "--buildtype=release", buildDir, linuxDir], { stdio: "ignore" });
      execFileSync("meson", ["compile", "-C", buildDir], { stdio: "ignore" });

      const binaryPath = path.join(buildDir, "native-ai-webapp-host");
      const xdgDataHome = path.join(scratch, "xdg-data");
      const result = spawnSync(binaryPath, ["--allow-unsigned-dev"], {
        cwd: repoRoot,
        env: { ...process.env, XDG_DATA_HOME: xdgDataHome },
        encoding: "utf8",
        timeout: 30_000,
      });
      const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
      assert.equal(result.status, 1, output);
      assert.match(output, /production build rejects dev-only startup flag --allow-unsigned-dev/);

      const dbPath = path.join(xdgDataHome, "NativeAIWebappPlatform", "platform.sqlite");
      assert.equal(fs.existsSync(dbPath), true, "production guard should create the platform audit database");
      const count = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'native.production_guard' AND decision = 'rejected' AND error_code = 'dev_only_flag' AND args_json LIKE '%--allow-unsigned-dev%';",
        ],
        { encoding: "utf8" },
      ).trim();
      assert.equal(count, "1");
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Linux packaged native artifact launches from executable-relative resources",
  {
    skip: linuxPackagedNativeSmokeSkipReason(),
    timeout: 240_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-linux-packaged-smoke-"));
    try {
      const outDir = path.join(scratch, "artifacts");
      const result = packageReleaseArtifacts({ outDir, buildNativeLinux: true });
      const nativeArtifact = result.artifacts.find((artifact) => artifact.id === "native-linux-linux-x86_64");
      assert.notEqual(nativeArtifact, undefined, "release manifest should include the Linux native host artifact");

      const appDir = path.join(outDir, "native-apps", "linux", "linux-x86_64", "NativeAIWebappHost");
      const binaryPath = path.join(appDir, "native-ai-webapp-host");
      const packagedCorePath = path.join(appDir, "libzig_core.so");
      for (const relativePath of [
        "native-ai-webapp-host",
        "libzig_core.so",
        "resources/runtime/index.html",
        "resources/runtime/runtime.js",
        "resources/webapps/examples/notes-lite/manifest.json",
        "resources/webapps/examples/task-workbench/app.js",
        "resources/db/sqlite/001_initial.sql",
      ]) {
        assert.equal(fs.existsSync(path.join(appDir, relativePath)), true, `${relativePath} should be packaged`);
      }
      assert.notEqual(fs.statSync(binaryPath).mode & 0o111, 0);
      assert.notEqual(fs.statSync(packagedCorePath).mode & 0o111, 0);

      runPackagedArtifactSmoke({ binaryPath, scratch, appDir });
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

function runOptionalSmoke({ binaryPath, scratch, zigCoreSo }) {
  if (process.env.NATIVE_AI_LINUX_SMOKE_LAUNCH !== "1") return;
  const storageKey = `notes-lite:linux-smoke-${process.pid}-${Date.now()}`;
  const storageValue = `linux-smoke-${process.pid}-${Date.now()}`;
  const baseEnv = {
    ...process.env,
    NATIVE_AI_ZIG_CORE_SO: zigCoreSo,
    NATIVE_AI_LINUX_SMOKE_EXIT_AFTER: "1",
    XDG_DATA_HOME: path.join(scratch, "xdg-data"),
  };

  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_RUNTIME_LOADED", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "runtime-load",
  });
  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_STORAGE_SET_OK", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "storage-set",
    NATIVE_AI_LINUX_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_STORAGE_GET_OK", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "storage-get",
    NATIVE_AI_LINUX_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_CORE_STEP_OK", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "core-step",
  });
  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_FIXED_BRIDGE_SURFACE_OK", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "fixed-bridge-surface",
    NATIVE_AI_LINUX_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_BRIDGE_STORAGE_SET_OK", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "bridge-storage-set",
    NATIVE_AI_LINUX_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_BRIDGE_STORAGE_GET_OK", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "bridge-storage-get",
    NATIVE_AI_LINUX_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_BRIDGE_CORE_STEP_OK", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "bridge-core-step",
  });
  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_RUNTIME_APP_STORAGE_GET_OK", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "runtime-app-storage-get",
  });
}

function runPackagedArtifactSmoke({ binaryPath, scratch, appDir }) {
  const storageKey = `notes-lite:linux-packaged-smoke-${process.pid}-${Date.now()}`;
  const storageValue = `linux-packaged-smoke-${process.pid}-${Date.now()}`;
  const outsideRepoCwd = path.join(scratch, "outside-repo-cwd");
  fs.mkdirSync(outsideRepoCwd, { recursive: true });
  const { NATIVE_AI_ZIG_CORE_SO: _ignoredZigCoreSo, ...smokeEnv } = process.env;
  const baseEnv = {
    ...smokeEnv,
    NATIVE_AI_LINUX_SMOKE_EXIT_AFTER: "1",
    XDG_DATA_HOME: path.join(scratch, "packaged-xdg-data"),
  };

  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_RUNTIME_LOADED", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "runtime-load",
  }, { cwd: outsideRepoCwd });
  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_BRIDGE_STORAGE_SET_OK", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "bridge-storage-set",
    NATIVE_AI_LINUX_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  }, { cwd: outsideRepoCwd });
  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_BRIDGE_STORAGE_GET_OK", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "bridge-storage-get",
    NATIVE_AI_LINUX_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  }, { cwd: outsideRepoCwd });
  runSmoke(binaryPath, "NATIVE_AI_LINUX_SMOKE_BRIDGE_CORE_STEP_OK", {
    ...baseEnv,
    NATIVE_AI_LINUX_SMOKE: "bridge-core-step",
  }, { cwd: outsideRepoCwd });

  const dbPath = path.join(baseEnv.XDG_DATA_HOME, "NativeAIWebappPlatform", "platform.sqlite");
  assert.equal(fs.existsSync(dbPath), true, "packaged smoke should persist the platform database");
  assert.equal(appDir.includes(repoRoot), false, "packaged artifact should live outside the repo root");
}

function runSmoke(binaryPath, marker, env, { cwd = repoRoot } = {}) {
  let args = [];
  let command = binaryPath;
  if (!process.env.DISPLAY && !process.env.WAYLAND_DISPLAY) {
    assert.equal(commandExists("xvfb-run"), true, "xvfb-run is required for headless Linux smoke");
    command = "xvfb-run";
    args.push("-a", binaryPath);
  }
  if (commandWorks("dbus-run-session", ["--version"])) {
    args = ["--", command, ...args];
    command = "dbus-run-session";
  }

  const result = spawnSync(command, args, { env, cwd, encoding: "utf8", timeout: 30_000 });
  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
  assert.equal(output.includes("NATIVE_AI_LINUX_SMOKE_FAILED"), false, output);
  if (output.includes(marker)) return;
  assert.fail(`Timed out waiting for ${marker}\n${output}`);
}
