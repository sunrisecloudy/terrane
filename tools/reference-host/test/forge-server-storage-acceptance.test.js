import { execFileSync } from "node:child_process";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const forgeDir = path.join(repoRoot, "forge");

test(
  "Forge server opens file-backed workspace storage through the bridge",
  { timeout: 120_000 },
  () => {
    execFileSync(
      "cargo",
      [
        "test",
        "-p",
        "forge-server",
        "file_backed_server_opens_workspace_and_handles_bridge",
        "--locked",
      ],
      {
        cwd: forgeDir,
        stdio: "ignore",
      },
    );
  },
);
