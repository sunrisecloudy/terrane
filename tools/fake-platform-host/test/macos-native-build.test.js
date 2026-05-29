import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

function hasSwift() {
  try {
    execFileSync("swift", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

test(
  "macOS native scaffold builds with SwiftPM",
  {
    skip: process.platform !== "darwin" ? "macOS SwiftPM build smoke only runs on Darwin hosts" : !hasSwift() ? "swift is not available" : false,
    timeout: 120_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-macos-swiftpm-"));
    try {
      const output = execFileSync("swift", ["build", "--scratch-path", scratch], {
        cwd: path.join(repoRoot, "native", "macos"),
        encoding: "utf8",
        env: {
          ...process.env,
          MACOSX_DEPLOYMENT_TARGET: "13.0",
        },
      });

      assert.match(output, /Build complete!/);
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);
