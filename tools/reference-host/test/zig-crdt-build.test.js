import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const zigCrdtDir = path.join(repoRoot, "zig-crdt");

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
  if (process.platform === "darwin") return "libzig_crdt.dylib";
  if (process.platform === "win32") return "zig_crdt.dll";
  return "libzig_crdt.so";
}

function exportedSymbols(binaryPath) {
  if (process.platform === "win32") return "";
  const args = process.platform === "darwin" ? ["-gU", binaryPath] : ["-g", binaryPath];
  return execFileSync("nm", args, { encoding: "utf8" });
}

test(
  "Zig CRDT unit tests pass and native static/shared libraries build",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 60_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-zig-crdt-"));
    const targetArgs = targetArgsForHost();
    const zigEnv = {
      ...process.env,
      ZIG_GLOBAL_CACHE_DIR: path.join(scratch, "zig-global-cache"),
      ZIG_LOCAL_CACHE_DIR: path.join(scratch, "zig-local-cache"),
    };
    try {
      execFileSync("zig", ["test", "src/lib.zig", ...targetArgs, "-lc", "-fno-emit-bin"], {
        cwd: zigCrdtDir,
        env: zigEnv,
        stdio: "ignore",
      });

      const staticPath = path.join(scratch, "libzig_crdt.a");
      execFileSync(
        "zig",
        ["build-lib", "src/lib.zig", "-static", ...targetArgs, "-lc", `-femit-bin=${staticPath}`],
        { cwd: zigCrdtDir, env: zigEnv, stdio: "ignore" },
      );
      assert.equal(fs.existsSync(staticPath), true);

      const dynamicPath = path.join(scratch, dynamicLibraryName());
      execFileSync(
        "zig",
        ["build-lib", "src/lib.zig", "-dynamic", ...targetArgs, "-lc", `-femit-bin=${dynamicPath}`],
        { cwd: zigCrdtDir, env: zigEnv, stdio: "ignore" },
      );
      assert.equal(fs.existsSync(dynamicPath), true);

      const symbols = exportedSymbols(dynamicPath);
      if (symbols) {
        assert.match(symbols, /\b_?crdt_create\b/);
        assert.match(symbols, /\b_?crdt_apply_json\b/);
        assert.match(symbols, /\b_?crdt_merge_json\b/);
        assert.match(symbols, /\b_?crdt_materialize_json\b/);
        assert.match(symbols, /\b_?crdt_free\b/);
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);
