import { execFileSync } from "node:child_process";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const forgeDir = path.join(repoRoot, "forge");

test(
  "Forge sync and C ABI sync command tests pass",
  { timeout: 120_000 },
  () => {
    execFileSync("cargo", ["test", "-p", "forge-sync", "--locked"], {
      cwd: forgeDir,
      stdio: "ignore",
    });
    execFileSync(
      "cargo",
      [
        "test",
        "-p",
        "forge-ffi",
        "sync_export_import_crosses_the_c_abi_without_crdt_symbols",
        "--locked",
      ],
      {
        cwd: forgeDir,
        stdio: "ignore",
      },
    );
  },
);
