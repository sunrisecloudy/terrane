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

function createSimulatorAppBundle(scratchRoot, binaryPath) {
  const appBundle = path.join(scratchRoot, "NativeAIHostIOS.app");
  fs.mkdirSync(appBundle, { recursive: true });
  fs.copyFileSync(binaryPath, path.join(appBundle, "NativeAIHostIOS"));
  fs.chmodSync(path.join(appBundle, "NativeAIHostIOS"), 0o755);

  fs.cpSync(path.join(repoRoot, "runtime-web"), path.join(appBundle, "runtime"), { recursive: true });
  fs.mkdirSync(path.join(appBundle, "webapps"), { recursive: true });
  fs.cpSync(path.join(repoRoot, "webapps", "examples"), path.join(appBundle, "webapps", "examples"), { recursive: true });

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

function waitForSmokeMarker({ markerPath, stdoutPath, stderrPath, timeoutMs }) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const markerFile = fs.existsSync(markerPath) ? fs.readFileSync(markerPath, "utf8") : "";
    const stdout = fs.existsSync(stdoutPath) ? fs.readFileSync(stdoutPath, "utf8") : "";
    const stderr = fs.existsSync(stderrPath) ? fs.readFileSync(stderrPath, "utf8") : "";
    if (`${markerFile}\n${stdout}\n${stderr}`.includes(smokeLoadedMarker)) {
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
    fs.rmSync(markerPath, { force: true });
    const stdoutPath = path.join(scratchRoot, "ios-smoke.stdout.log");
    const stderrPath = path.join(scratchRoot, "ios-smoke.stderr.log");
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
        "--native-ai-smoke-runtime-load",
        "--native-ai-smoke-exit-on-runtime-load",
      ],
      { encoding: "utf8" },
    );

    const logs = waitForSmokeMarker({ markerPath, stdoutPath, stderrPath, timeoutMs: 30_000 });
    if (!`${logs.markerFile}\n${logs.stdout}\n${logs.stderr}`.includes(smokeLoadedMarker)) {
      const screenshotPath = path.join(scratchRoot, "ios-smoke.png");
      execFileSync("xcrun", ["simctl", "io", device.udid, "screenshot", screenshotPath], { stdio: "ignore" });
      assert.fail(`iOS runtime load smoke marker was not emitted; marker: ${markerPath}; screenshot: ${screenshotPath}\nmarker file:\n${logs.markerFile}\nstdout:\n${logs.stdout}\nstderr:\n${logs.stderr}`);
    }
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

      const appBundle = createSimulatorAppBundle(scratchRoot, build.binaryPath);
      assert.equal(fs.existsSync(path.join(appBundle, "runtime", "index.html")), true);
      assert.equal(fs.existsSync(path.join(appBundle, "webapps", "examples", "notes-lite", "manifest.json")), true);
      assert.equal(fs.existsSync(path.join(appBundle, "webapps", "examples", "task-workbench", "manifest.json")), true);

      if (process.env.NATIVE_AI_IOS_SMOKE_LAUNCH === "1") {
        launchInSimulator({ scratchRoot, appBundle });
      }
    } finally {
      fs.rmSync(scratchRoot, { recursive: true, force: true });
    }
  },
);
