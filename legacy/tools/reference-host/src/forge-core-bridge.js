import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { repoRoot } from "./paths.js";

let cachedInvokeBinary = null;

function invokeBinaryPath() {
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

export const REFERENCE_HOST_PLATFORM_IDS = {
  platform: "reference-host",
  target: "reference-host",
};

export function forgeCoreAvailable() {
  try {
    invokeBinaryPath();
    return true;
  } catch {
    return false;
  }
}

export function invokeForgeCore(
  name,
  payload,
  { requestId = "reference-host-core", workspaceId = "reference-host", actor = "reference-host" } = {},
) {
  const envelope = {
    request_id: requestId,
    workspace_id: workspaceId,
    actor: { actor, role: "owner" },
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
    const detail = response.error?.detail ?? response.error ?? "forge core command failed";
    throw new Error(`${name} failed: ${typeof detail === "string" ? detail : JSON.stringify(detail)}`);
  }
  return response.payload;
}

export function packagePermissions(appId, manifest) {
  return invokeForgeCore("package.get_permissions", {
    app_id: appId,
    manifest_json: manifest,
  });
}

export function validateBridgeEnvelope(input) {
  return invokeForgeCore("bridge.validate_envelope", { input });
}

export function validateNetworkRequest({ networkPolicy, request, resourceBudget = null }) {
  return invokeForgeCore("bridge.validate_network_request", {
    network_policy: networkPolicy,
    request,
    ...(resourceBudget ? { resource_budget: resourceBudget } : {}),
  });
}

export function prepareBridgeSession({ appId, mountToken, metadata = null }) {
  return invokeForgeCore("bridge.prepare_session", {
    platform_ids: REFERENCE_HOST_PLATFORM_IDS,
    app_id: appId,
    mount_token: mountToken,
    ...(metadata ? { metadata } : {}),
  });
}

export function recordBridgeCall(record) {
  return invokeForgeCore("bridge.record_call", {
    record: {
      platform_ids: REFERENCE_HOST_PLATFORM_IDS,
      ...record,
    },
  });
}

export function recordCoreEvent(record) {
  return invokeForgeCore("bridge.record_core_event", {
    record: {
      platform_ids: REFERENCE_HOST_PLATFORM_IDS,
      ...record,
    },
  });
}

export function recordCrashRecovery({ source, canAutoRemount }) {
  return invokeForgeCore("bridge.record_crash_recovery", {
    source,
    can_auto_remount: canAutoRemount,
  });
}

export function provisionPlatformRegistry(snapshot) {
  return invokeForgeCore("package.provision_registry", { snapshot });
}

export function packageListVersions(appId) {
  return invokeForgeCore("package.list_versions", { app_id: appId });
}

export function packageActivateVersion({ appId, installId, createdAt, installationEventId = null }) {
  return invokeForgeCore("package.activate_version", {
    app_id: appId,
    install_id: installId,
    created_at: createdAt,
    ...(installationEventId ? { installation_event_id: installationEventId } : {}),
  });
}

export function packageRollbackVersion({ appId, targetInstallId = null, createdAt, installationEventId = null }) {
  return invokeForgeCore("package.rollback_version", {
    app_id: appId,
    created_at: createdAt,
    ...(targetInstallId ? { target_install_id: targetInstallId } : {}),
    ...(installationEventId ? { installation_event_id: installationEventId } : {}),
  });
}

export function packageSetStatus({
  appId,
  installId,
  status,
  createdAt,
  reason = null,
  restorePrevious = false,
  installationEventId = null,
}) {
  return invokeForgeCore("package.set_status", {
    app_id: appId,
    install_id: installId,
    status,
    created_at: createdAt,
    restore_previous: restorePrevious,
    ...(reason ? { reason } : {}),
    ...(installationEventId ? { installation_event_id: installationEventId } : {}),
  });
}

export function quotaAutoQuarantine({
  appId,
  installId,
  budgetErrorCount60s,
  isActiveInstall,
  error = null,
  createdAt,
}) {
  return invokeForgeCore("quota.auto_quarantine", {
    app_id: appId,
    install_id: installId,
    budget_error_count_60s: budgetErrorCount60s,
    is_active_install: isActiveInstall,
    created_at: createdAt,
    ...(error ? { error } : {}),
  });
}