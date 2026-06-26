import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { defaultPlatform, linuxDockerCommands } from "../../../tools/run-linux-native-docker.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("Linux Docker helper builds an image and runs the native launch smoke with a read-only repo mount", () => {
  const commands = linuxDockerCommands({ rootDir: repoRoot, image: "terrane-linux-smoke:test", platform: "linux/amd64" });
  assert.deepEqual(commands.buildArgs.slice(0, 2), ["build", "-f"]);
  assert.equal(commands.buildArgs.includes(path.join(repoRoot, "native", "linux", "Dockerfile")), true);
  assert.equal(commands.buildArgs.includes("terrane-linux-smoke:test"), true);
  assert.equal(commands.buildArgs.includes("linux/amd64"), true);

  const mount = commands.runArgs.find((arg) => arg.startsWith("type=bind,"));
  assert.equal(mount, `type=bind,source=${repoRoot},target=/workspace,readonly`);
  assert.equal(commands.runArgs.includes("TERRANE_LINUX_SMOKE_LAUNCH=1"), true);
  assert.equal(commands.runArgs.includes("GTK_A11Y=none"), true);
  assert.equal(commands.runArgs.includes("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1"), true);
  assert.equal(commands.runArgs.includes("WEBKIT_DISABLE_COMPOSITING_MODE=1"), true);
  assert.equal(commands.runArgs.includes("tools/reference-host/test/linux-native-build.test.js"), true);
});

test("Linux Docker helper defaults to the supported linux-x86_64 release target on non-x64 hosts", () => {
  const commands = linuxDockerCommands({ rootDir: repoRoot, image: "terrane-linux-smoke:test-default" });
  assert.equal(defaultPlatform, process.arch === "x64" ? "" : "linux/amd64");
  assert.equal(commands.buildArgs.includes(defaultPlatform), defaultPlatform !== "");
  assert.equal(commands.runArgs.includes(defaultPlatform), defaultPlatform !== "");
});

test("Linux native Dockerfile pins the smoke dependencies and Rust toolchain", () => {
  const dockerfile = fs.readFileSync(path.join(repoRoot, "native", "linux", "Dockerfile"), "utf8");
  for (const snippet of [
    "ubuntu:24.04",
    "RUST_VERSION=1.96.0",
    "https://sh.rustup.rs",
    "cargo --version",
    "libgtk-4-dev",
    "libwebkitgtk-6.0-dev",
    "libjson-glib-dev",
    "libsoup-3.0-dev",
    "libsqlite3-dev",
    "meson",
    "ninja-build",
    "sqlite3",
    "xvfb",
    "dbus-x11",
    "GTK_A11Y=none",
    "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1",
    "TERRANE_LINUX_SMOKE_LAUNCH=1",
  ]) {
    assert.equal(dockerfile.includes(snippet), true, `Dockerfile should contain ${snippet}`);
  }
});
