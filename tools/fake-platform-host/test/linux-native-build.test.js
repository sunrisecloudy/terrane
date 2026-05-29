import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

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

test(
  "Linux GTK/WebKitGTK host builds and optionally runs native smoke",
  {
    skip: process.platform !== "linux"
      ? "Linux native smoke only runs on Linux hosts"
      : !commandWorks("meson")
        ? "meson is not available"
        : !commandWorks("ninja")
          ? "ninja is not available"
          : !commandWorks("zig", ["version"])
            ? "zig is not available"
            : !hasLinuxNativeDependencies()
              ? "GTK/WebKitGTK development dependencies are not available"
              : false,
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

function runSmoke(binaryPath, marker, env) {
  let args = [];
  let command = binaryPath;
  if (!process.env.DISPLAY && !process.env.WAYLAND_DISPLAY) {
    assert.equal(commandWorks("xvfb-run"), true, "xvfb-run is required for headless Linux smoke");
    command = "xvfb-run";
    args.push("-a", binaryPath);
  }
  if (commandWorks("dbus-run-session", ["--version"])) {
    args = ["--", command, ...args];
    command = "dbus-run-session";
  }

  const result = spawnSync(command, args, { env, cwd: repoRoot, encoding: "utf8", timeout: 30_000 });
  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
  assert.equal(output.includes("NATIVE_AI_LINUX_SMOKE_FAILED"), false, output);
  if (output.includes(marker)) return;
  assert.fail(`Timed out waiting for ${marker}\n${output}`);
}
