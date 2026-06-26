import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { repoRoot } from "./paths.mjs";

let cachedInvokeBinary = null;

export function coreInvokeBinaryPath() {
  if (cachedInvokeBinary && fs.existsSync(cachedInvokeBinary)) {
    return cachedInvokeBinary;
  }

  const debugBinary = path.join(repoRoot, "forge", "target", "debug", "core-invoke");
  if (fs.existsSync(debugBinary)) {
    cachedInvokeBinary = debugBinary;
    return debugBinary;
  }

  execFileSync("cargo", ["build", "-p", "forge-ffi", "--bin", "core-invoke", "--locked"], {
    cwd: path.join(repoRoot, "forge"),
    stdio: "pipe",
  });

  if (!fs.existsSync(debugBinary)) {
    throw new Error("core-invoke binary was not built");
  }

  cachedInvokeBinary = debugBinary;
  return debugBinary;
}

export function buildEnvelope({
  name,
  payload = {},
  requestId = "agent-adapter",
  workspaceId = "agent-adapter",
  actor = "agent-adapter",
  role = "owner",
}) {
  return {
    request_id: requestId,
    workspace_id: workspaceId,
    actor: {
      actor,
      role: String(role).trim().toLowerCase(),
    },
    name,
    payload,
  };
}

export async function invokeCoreCommand(envelope, { server = null, token = null } = {}) {
  if (server) {
    return invokeViaServer(envelope, { server, token });
  }
  return invokeViaCoreBinary(envelope);
}

function invokeViaCoreBinary(envelope) {
  const output = execFileSync(coreInvokeBinaryPath(), [], {
    input: JSON.stringify(envelope),
    encoding: "utf8",
    cwd: repoRoot,
  });
  return JSON.parse(output);
}

async function invokeViaServer(envelope, { server, token }) {
  const headers = {
    "content-type": "application/json",
  };
  if (token) {
    headers.authorization = `Bearer ${token}`;
  }

  const response = await fetch(new URL("/bridge", server), {
    method: "POST",
    headers,
    body: JSON.stringify(envelope),
  });

  const bodyText = await response.text();
  let body;
  try {
    body = JSON.parse(bodyText);
  } catch {
    throw new Error(`server returned non-JSON response (${response.status}): ${bodyText}`);
  }

  if (!response.ok && body?.ok !== false) {
    throw new Error(`server transport error (${response.status}): ${bodyText}`);
  }

  return body;
}

export async function fetchViaDescribe({
  server = null,
  token = null,
  tier = "public",
  role = "owner",
  workspaceId = "agent-adapter",
  actor = "agent-adapter",
}) {
  const envelope = buildEnvelope({
    name: "system.describe",
    payload: {
      tier,
      for_role: String(role).trim().toLowerCase(),
      include_inner: false,
    },
    workspaceId,
    actor,
    role,
    requestId: "agent-adapter-describe",
  });

  const response = await invokeCoreCommand(envelope, { server, token });
  if (!response.ok) {
    const detail = response.error?.detail ?? response.error ?? "system.describe failed";
    throw new Error(typeof detail === "string" ? detail : JSON.stringify(detail));
  }
  return response;
}