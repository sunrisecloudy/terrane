import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { repoRoot } from "./paths.js";

let cachedInvokeBinary = null;

function invokeBinaryPath() {
  if (cachedInvokeBinary && fs.existsSync(cachedInvokeBinary)) {
    return cachedInvokeBinary;
  }
  const debugBinary = path.join(
    repoRoot,
    "forge",
    "target",
    "debug",
    "control-invoke",
  );
  if (fs.existsSync(debugBinary)) {
    cachedInvokeBinary = debugBinary;
    return debugBinary;
  }
  execFileSync(
    "cargo",
    ["build", "-p", "forge-controlcore", "--bin", "control-invoke", "--locked"],
    { cwd: path.join(repoRoot, "forge"), stdio: "pipe" },
  );
  if (!fs.existsSync(debugBinary)) {
    throw new Error("control-invoke binary was not built");
  }
  cachedInvokeBinary = debugBinary;
  return debugBinary;
}

export function invokeControlCore(name, payload, { requestId = "reference-host-control", workspaceId = "reference-host" } = {}) {
  const envelope = {
    request_id: requestId,
    workspace_id: workspaceId,
    actor: { actor: "reference-host", role: "owner" },
    name,
    payload,
  };
  const output = execFileSync(invokeBinaryPath(), [], {
    input: JSON.stringify(envelope),
    encoding: "utf8",
    cwd: repoRoot,
  });
  const response = JSON.parse(output);
  if (!response.ok) {
    const detail = response.error?.detail ?? response.error ?? "control command failed";
    throw new Error(`${name} failed: ${typeof detail === "string" ? detail : JSON.stringify(detail)}`);
  }
  return response.payload;
}

export function controlCoreAvailable() {
  try {
    invokeBinaryPath();
    return true;
  } catch {
    return false;
  }
}