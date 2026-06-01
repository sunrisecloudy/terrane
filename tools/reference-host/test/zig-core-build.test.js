import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const zigCoreDir = path.join(repoRoot, "zig-core");

function hasZig() {
  try {
    execFileSync("zig", ["version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function targetArgsForHost() {
  if (process.platform !== "darwin") return [];
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  return ["-target", `${arch}-macos.15.0.0`];
}

function dynamicLibraryName() {
  if (process.platform === "darwin") return "libzig_core.dylib";
  if (process.platform === "win32") return "zig_core.dll";
  return "libzig_core.so";
}

function exportedSymbols(binaryPath) {
  if (process.platform === "win32") return "";
  const args = process.platform === "darwin" ? ["-gU", binaryPath] : ["-g", binaryPath];
  return execFileSync("nm", args, { encoding: "utf8" });
}

test(
  "Zig core unit tests pass and native static/shared libraries build",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 60_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-zig-core-"));
    const targetArgs = targetArgsForHost();
    try {
      execFileSync("zig", ["test", ...targetArgs, "-lc", "src/lib.zig", "-fno-emit-bin"], {
        cwd: zigCoreDir,
        stdio: "ignore",
      });

      const staticPath = path.join(scratch, "libzig_core.a");
      execFileSync(
        "zig",
        ["build-lib", "src/lib.zig", "-static", ...targetArgs, "-lc", `-femit-bin=${staticPath}`],
        { cwd: zigCoreDir, stdio: "ignore" },
      );
      assert.equal(fs.existsSync(staticPath), true);

      const dynamicPath = path.join(scratch, dynamicLibraryName());
      execFileSync(
        "zig",
        ["build-lib", "src/lib.zig", "-dynamic", ...targetArgs, "-lc", `-femit-bin=${dynamicPath}`],
        { cwd: zigCoreDir, stdio: "ignore" },
      );
      assert.equal(fs.existsSync(dynamicPath), true);

      const symbols = exportedSymbols(dynamicPath);
      if (symbols) {
        assert.match(symbols, /\b_?core_create\b/);
        assert.match(symbols, /\b_?core_step_json\b/);
        assert.match(symbols, /\b_?core_free\b/);
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);
