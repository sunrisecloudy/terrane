import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const serverDir = path.join(repoRoot, "server");

function hasZig() {
  try {
    execFileSync("zig", ["version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasCc() {
  try {
    execFileSync("cc", ["--version"], { stdio: "ignore" });
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

function zigServerModuleArgs() {
  return ["--dep", "zig_core", "--dep", "zig_crdt", "-Mroot=src/main.zig", "-Mzig_core=../zig-core/src/lib.zig", "-Mzig_crdt=../zig-crdt/src/lib.zig"];
}

test(
  "Zig server source compiles into a native executable",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 60_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-zig-server-"));
    const targetArgs = targetArgsForHost();
    const executablePath = path.join(scratch, process.platform === "win32" ? "terrane-server.exe" : "terrane-server");
    const zigEnv = {
      ...process.env,
      ZIG_GLOBAL_CACHE_DIR: path.join(scratch, "zig-global-cache"),
      ZIG_LOCAL_CACHE_DIR: path.join(scratch, "zig-local-cache"),
    };
    try {
      execFileSync("zig", ["build-exe", ...zigServerModuleArgs(), ...targetArgs, "-lc", "-lsqlite3", "-fno-emit-bin"], {
        cwd: serverDir,
        env: zigEnv,
        stdio: "ignore",
      });

      if (process.platform === "darwin") {
        assert.equal(hasCc(), true);
        const objectPath = path.join(scratch, "terrane-server.o");
        execFileSync(
          "zig",
          ["build-obj", ...zigServerModuleArgs(), ...targetArgs, "-lc", `-femit-bin=${objectPath}`],
          { cwd: serverDir, env: zigEnv, stdio: "ignore" },
        );
        execFileSync("cc", [objectPath, "-lsqlite3", "-o", executablePath], { stdio: "ignore" });
      } else {
        execFileSync(
          "zig",
          ["build-exe", ...zigServerModuleArgs(), ...targetArgs, "-lc", "-lsqlite3", `-femit-bin=${executablePath}`],
          { cwd: serverDir, env: zigEnv, stdio: "ignore" },
        );
      }

      assert.equal(fs.existsSync(executablePath), true);
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);
