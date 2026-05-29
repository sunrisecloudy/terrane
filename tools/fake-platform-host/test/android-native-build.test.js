import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const androidDir = path.join(repoRoot, "native", "android");

function commandExists(command) {
  try {
    execFileSync(command, ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasAndroidSdk() {
  return Boolean(process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT || fs.existsSync(path.join(process.env.HOME ?? "", "Library", "Android", "sdk")));
}

function findFiles(directory, predicate) {
  if (!fs.existsSync(directory)) return [];
  const found = [];
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const absolutePath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      found.push(...findFiles(absolutePath, predicate));
      continue;
    }
    if (predicate(absolutePath)) {
      found.push(absolutePath);
    }
  }
  return found;
}

test(
  "Android native scaffold assembles debug APK with synced runtime assets and JNI libraries",
  {
    skip: !commandExists("gradle") ? "gradle is not available" : !hasAndroidSdk() ? "Android SDK is not available" : false,
    timeout: 180_000,
  },
  () => {
    const output = execFileSync("gradle", [":app:assembleDebug"], {
      cwd: androidDir,
      encoding: "utf8",
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"],
    });

    assert.match(output, /BUILD SUCCESSFUL/);
    assert.equal(fs.existsSync(path.join(androidDir, "app", "build", "outputs", "apk", "debug", "app-debug.apk")), true);
    assert.equal(
      fs.existsSync(path.join(androidDir, "app", "build", "generated", "native-ai-assets", "runtime", "index.html")),
      true,
      "runtime-web assets should be synced under the Android /runtime asset path",
    );
    assert.equal(
      fs.existsSync(path.join(androidDir, "app", "build", "generated", "native-ai-assets", "webapps", "examples", "notes-lite", "manifest.json")),
      true,
      "generated example apps should be synced into Android assets",
    );

    for (const abi of ["arm64-v8a", "armeabi-v7a", "x86", "x86_64"]) {
      assert.equal(
        findFiles(path.join(androidDir, "app", "build", "intermediates", "cxx", "Debug"), (filePath) =>
          filePath.endsWith(path.join("obj", abi, "libzig_core_jni.so")),
        ).length > 0,
        true,
        `JNI bridge library should build for ${abi}`,
      );
    }
  },
);
