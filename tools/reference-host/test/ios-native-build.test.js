import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const iosDir = path.join(repoRoot, "native", "ios");
const bundleId = "dev.nativeai.host.ios";
const smokeLoadedMarker = "NATIVE_AI_IOS_SMOKE_RUNTIME_LOADED";
const smokeStorageSetMarker = "NATIVE_AI_IOS_SMOKE_STORAGE_SET_OK";
const smokeStorageGetMarker = "NATIVE_AI_IOS_SMOKE_STORAGE_GET_OK";
const smokeCoreStepMarker = "NATIVE_AI_IOS_SMOKE_CORE_STEP_OK";
const smokeMarkerFile = "native-ai-ios-smoke-runtime-loaded.txt";

function commandWorks(command, args) {
  try {
    execFileSync(command, args, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function simulatorSdkPath() {
  return execFileSync("xcrun", ["--sdk", "iphonesimulator", "--show-sdk-path"], {
    encoding: "utf8",
  }).trim();
}

function hasIPhoneSimulatorSdk() {
  try {
    return simulatorSdkPath().length > 0;
  } catch {
    return false;
  }
}

function buildIOSHost(scratchRoot) {
  const buildScratch = path.join(scratchRoot, "spm-build");
  const moduleCache = path.join(scratchRoot, "module-cache");
  const output = execFileSync(
    "swift",
    [
      "build",
      "--disable-sandbox",
      "--cache-path",
      path.join(scratchRoot, "swift-cache"),
      "--config-path",
      path.join(scratchRoot, "swift-config"),
      "--security-path",
      path.join(scratchRoot, "swift-security"),
      "--scratch-path",
      buildScratch,
      "--triple",
      "arm64-apple-ios17.0-simulator",
      "--sdk",
      simulatorSdkPath(),
      "-Xcc",
      `-fmodules-cache-path=${moduleCache}`,
      "-Xswiftc",
      "-module-cache-path",
      "-Xswiftc",
      moduleCache,
      "-Xswiftc",
      "-D",
      "-Xswiftc",
      "DEBUG",
    ],
    {
      cwd: iosDir,
      encoding: "utf8",
      env: {
        ...process.env,
        CLANG_MODULE_CACHE_PATH: moduleCache,
        SWIFT_MODULE_CACHE_PATH: moduleCache,
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  const binaryPath = path.join(buildScratch, "arm64-apple-ios-simulator", "debug", "NativeAIHostIOS");
  return { buildScratch, binaryPath, output };
}

function buildIOSZigCore(scratchRoot) {
  const dylibPath = path.join(scratchRoot, "libzig_core.dylib");
  execFileSync(
    "zig",
    [
      "build-lib",
      "src/lib.zig",
      "-dynamic",
      "-target",
      "aarch64-ios-simulator",
      "-lc",
      "-L",
      path.join(simulatorSdkPath(), "usr", "lib"),
      `-femit-bin=${dylibPath}`,
    ],
    {
      cwd: path.join(repoRoot, "zig-core"),
      env: {
        ...process.env,
        ZIG_GLOBAL_CACHE_DIR: path.join(scratchRoot, "zig-global-cache"),
        ZIG_LOCAL_CACHE_DIR: path.join(scratchRoot, "zig-local-cache"),
      },
      stdio: "ignore",
    },
  );
  assert.equal(fs.existsSync(dylibPath), true);
  const loadCommands = execFileSync("otool", ["-l", dylibPath], { encoding: "utf8" });
  assert.match(loadCommands, /platform 7/);
  const symbols = execFileSync("nm", ["-gU", dylibPath], { encoding: "utf8" });
  assert.match(symbols, /_core_create/);
  assert.match(symbols, /_core_step_json/);
  assert.match(symbols, /_core_free/);
  return dylibPath;
}

function createSimulatorAppBundle(scratchRoot, binaryPath, zigCoreDylibPath = null) {
  const appBundle = path.join(scratchRoot, "NativeAIHostIOS.app");
  fs.mkdirSync(appBundle, { recursive: true });
  fs.copyFileSync(binaryPath, path.join(appBundle, "NativeAIHostIOS"));
  fs.chmodSync(path.join(appBundle, "NativeAIHostIOS"), 0o755);
  if (zigCoreDylibPath) {
    const bundledCorePath = path.join(appBundle, "libzig_core.dylib");
    fs.copyFileSync(zigCoreDylibPath, bundledCorePath);
    fs.chmodSync(bundledCorePath, 0o755);
    execFileSync("codesign", ["--force", "--sign", "-", bundledCorePath], { stdio: "ignore" });
  }

  fs.cpSync(path.join(repoRoot, "runtime-web"), path.join(appBundle, "runtime"), { recursive: true });
  fs.mkdirSync(path.join(appBundle, "webapps"), { recursive: true });
  fs.cpSync(path.join(repoRoot, "webapps", "examples"), path.join(appBundle, "webapps", "examples"), { recursive: true });
  fs.mkdirSync(path.join(appBundle, "db"), { recursive: true });
  fs.cpSync(path.join(repoRoot, "db", "sqlite"), path.join(appBundle, "db", "sqlite"), { recursive: true });

  fs.writeFileSync(
    path.join(appBundle, "Info.plist"),
    `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>NativeAIHostIOS</string>
  <key>CFBundleIdentifier</key><string>${bundleId}</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>NativeAIHostIOS</string>
  <key>CFBundleDisplayName</key><string>NativeAIHostIOS</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>MinimumOSVersion</key><string>17.0</string>
  <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
  <key>UIApplicationSupportsIndirectInputEvents</key><true/>
</dict>
</plist>
`,
  );

  execFileSync("codesign", ["--force", "--sign", "-", appBundle], { stdio: "ignore" });
  execFileSync("codesign", ["--verify", appBundle], { stdio: "ignore" });
  return appBundle;
}

function availableIOSDevices() {
  const listing = JSON.parse(execFileSync("xcrun", ["simctl", "list", "devices", "available", "--json"], { encoding: "utf8" }));
  return Object.entries(listing.devices ?? {})
    .filter(([runtime]) => runtime.includes("iOS"))
    .flatMap(([, devices]) => devices)
    .filter((device) => device.isAvailable && device.name.includes("iPhone"));
}

function selectIOSDevice() {
  if (process.env.NATIVE_AI_IOS_SMOKE_DEVICE) {
    return { udid: process.env.NATIVE_AI_IOS_SMOKE_DEVICE, state: "Unknown" };
  }
  const devices = availableIOSDevices();
  return devices.find((device) => device.state === "Booted") ??
    devices.find((device) => device.name === "iPhone 17") ??
    devices[0];
}

function waitForSmokeMarker({ markerPath, stdoutPath, stderrPath, expectedMarker, timeoutMs }) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const markerFile = fs.existsSync(markerPath) ? fs.readFileSync(markerPath, "utf8") : "";
    const stdout = fs.existsSync(stdoutPath) ? fs.readFileSync(stdoutPath, "utf8") : "";
    const stderr = fs.existsSync(stderrPath) ? fs.readFileSync(stderrPath, "utf8") : "";
    if (`${markerFile}\n${stdout}\n${stderr}`.includes(expectedMarker)) {
      return { markerFile, stdout, stderr };
    }
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 250);
  }
  return {
    markerFile: fs.existsSync(markerPath) ? fs.readFileSync(markerPath, "utf8") : "",
    stdout: fs.existsSync(stdoutPath) ? fs.readFileSync(stdoutPath, "utf8") : "",
    stderr: fs.existsSync(stderrPath) ? fs.readFileSync(stderrPath, "utf8") : "",
  };
}

function launchAndWaitForMarker({ device, scratchRoot, markerPath, expectedMarker, launchArgs }) {
  fs.rmSync(markerPath, { force: true });
  const logStem = expectedMarker.toLowerCase().replaceAll("_", "-");
  const stdoutPath = path.join(scratchRoot, `${logStem}.stdout.log`);
  const stderrPath = path.join(scratchRoot, `${logStem}.stderr.log`);
  fs.rmSync(stdoutPath, { force: true });
  fs.rmSync(stderrPath, { force: true });

  execFileSync(
    "xcrun",
    [
      "simctl",
      "launch",
      "--terminate-running-process",
      `--stdout=${stdoutPath}`,
      `--stderr=${stderrPath}`,
      device.udid,
      bundleId,
      ...launchArgs,
    ],
    { encoding: "utf8" },
  );

  const logs = waitForSmokeMarker({ markerPath, stdoutPath, stderrPath, expectedMarker, timeoutMs: 30_000 });
  if (!`${logs.markerFile}\n${logs.stdout}\n${logs.stderr}`.includes(expectedMarker)) {
    const screenshotPath = path.join(scratchRoot, `${logStem}.png`);
    execFileSync("xcrun", ["simctl", "io", device.udid, "screenshot", screenshotPath], { stdio: "ignore" });
    assert.fail(`iOS smoke marker ${expectedMarker} was not emitted; marker: ${markerPath}; screenshot: ${screenshotPath}\nmarker file:\n${logs.markerFile}\nstdout:\n${logs.stdout}\nstderr:\n${logs.stderr}`);
  }
}

function launchInSimulator({ scratchRoot, appBundle }) {
  const device = selectIOSDevice();
  assert.ok(device?.udid, "an available iOS simulator device should exist");

  const wasBooted = device.state === "Booted";
  if (!wasBooted) {
    execFileSync("xcrun", ["simctl", "boot", device.udid], { stdio: "ignore" });
    execFileSync("xcrun", ["simctl", "bootstatus", device.udid, "-b"], { stdio: "ignore" });
  }

  try {
    execFileSync("xcrun", ["simctl", "install", device.udid, appBundle], { stdio: "ignore" });
    const dataContainer = execFileSync("xcrun", ["simctl", "get_app_container", device.udid, bundleId, "data"], { encoding: "utf8" }).trim();
    const markerPath = path.join(dataContainer, "tmp", smokeMarkerFile);

    launchAndWaitForMarker({
      device,
      scratchRoot,
      markerPath,
      expectedMarker: smokeLoadedMarker,
      launchArgs: ["--native-ai-smoke-runtime-load", "--native-ai-smoke-exit-on-runtime-load"],
    });

    const storageKey = `notes-lite:ios-smoke-${process.pid}-${Date.now()}`;
    const storageValue = `ios-smoke-${process.pid}-${Date.now()}`;
    launchAndWaitForMarker({
      device,
      scratchRoot,
      markerPath,
      expectedMarker: smokeStorageSetMarker,
      launchArgs: [
        "--native-ai-smoke-storage-set",
        "--native-ai-smoke-storage-key",
        storageKey,
        "--native-ai-smoke-storage-value",
        storageValue,
        "--native-ai-smoke-exit-on-runtime-load",
      ],
    });
    launchAndWaitForMarker({
      device,
      scratchRoot,
      markerPath,
      expectedMarker: smokeStorageGetMarker,
      launchArgs: [
        "--native-ai-smoke-storage-get",
        "--native-ai-smoke-storage-key",
        storageKey,
        "--native-ai-smoke-storage-value",
        storageValue,
        "--native-ai-smoke-exit-on-runtime-load",
      ],
    });
    launchAndWaitForMarker({
      device,
      scratchRoot,
      markerPath,
      expectedMarker: smokeCoreStepMarker,
      launchArgs: ["--native-ai-smoke-core-step", "--native-ai-smoke-exit-on-runtime-load"],
    });
  } finally {
    if (!wasBooted) {
      execFileSync("xcrun", ["simctl", "shutdown", device.udid], { stdio: "ignore" });
    }
  }
}

test(
  "iOS native scaffold builds a simulator app bundle with runtime resources",
  {
    skip: process.platform !== "darwin"
      ? "iOS simulator build smoke only runs on Darwin hosts"
      : !commandWorks("swift", ["--version"])
        ? "swift is not available"
        : process.env.NATIVE_AI_IOS_SMOKE_LAUNCH === "1" && !commandWorks("xcrun", ["simctl", "help"])
          ? "simctl is not available"
          : process.env.NATIVE_AI_IOS_SMOKE_LAUNCH === "1" && !commandWorks("zig", ["version"])
            ? "zig is not available"
            : !hasIPhoneSimulatorSdk()
              ? "iPhone simulator SDK is not available"
              : false,
    timeout: process.env.NATIVE_AI_IOS_SMOKE_LAUNCH === "1" ? 180_000 : 120_000,
  },
  () => {
    const scratchRoot = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-ios-smoke-"));
    try {
      const build = buildIOSHost(scratchRoot);
      assert.match(build.output, /Build complete!/);
      assert.equal(fs.existsSync(build.binaryPath), true);

      const fileOutput = execFileSync("file", [build.binaryPath], { encoding: "utf8" });
      assert.match(fileOutput, /Mach-O 64-bit executable arm64/);
      const loadCommands = execFileSync("otool", ["-l", build.binaryPath], { encoding: "utf8" });
      assert.match(loadCommands, /platform 7/);
      assert.match(loadCommands, /minos 17\.0/);
      const linkedLibraries = execFileSync("otool", ["-L", build.binaryPath], { encoding: "utf8" });
      assert.match(linkedLibraries, /UIKit\.framework\/UIKit/);
      assert.match(linkedLibraries, /WebKit\.framework\/WebKit/);
      assert.match(linkedLibraries, /libsqlite3\.dylib/);

      const zigCoreDylibPath = process.env.NATIVE_AI_IOS_SMOKE_LAUNCH === "1" ? buildIOSZigCore(scratchRoot) : null;
      const appBundle = createSimulatorAppBundle(scratchRoot, build.binaryPath, zigCoreDylibPath);
      assert.equal(fs.existsSync(path.join(appBundle, "runtime", "index.html")), true);
      assert.equal(fs.existsSync(path.join(appBundle, "webapps", "examples", "notes-lite", "manifest.json")), true);
      assert.equal(fs.existsSync(path.join(appBundle, "webapps", "examples", "task-workbench", "manifest.json")), true);
      assert.equal(fs.existsSync(path.join(appBundle, "db", "sqlite", "001_initial.sql")), true);
      if (zigCoreDylibPath) {
        assert.equal(fs.existsSync(path.join(appBundle, "libzig_core.dylib")), true);
      }

      if (process.env.NATIVE_AI_IOS_SMOKE_LAUNCH === "1") {
        launchInSimulator({ scratchRoot, appBundle });
      }
    } finally {
      fs.rmSync(scratchRoot, { recursive: true, force: true });
    }
  },
);
