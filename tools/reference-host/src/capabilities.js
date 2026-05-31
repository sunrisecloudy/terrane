export const RUNTIME_VERSION = "0.1.0";

export const DEFAULT_REFERENCE_HOST_FEATURES = Object.freeze({
  "core.step": true,
  "storage.read": true,
  "storage.write": true,
  "storage.get": true,
  "storage.set": true,
  "storage.remove": true,
  "storage.list": true,
  "dialog.openFile": true,
  "dialog.saveFile": true,
  "network.request": true,
  "notification.toast": true,
  "app.log": true,
  "runtime.capabilities": true,
  "runtime.snapshot": true,
  "runtime.replay": true,
  "notebook.read": true,
  "notebook.write": true,
  "notebook.propose": true,
  "notebook.approve": true,
  "notebook.sync": true,
  "notebook.open": true,
  "notebook.apply_local": true,
  "notebook.propose_ai_patch": true,
  "notebook.accept_proposal": true,
  "notebook.reject_proposal": true,
  "notebook.snapshot": true,
  "notebook.checkout": true,
  "notebook.sync_pull": true,
  "notebook.sync_push": true,
  "notebook.subscribe": true,
});

export function referenceHostCapabilities(options = null) {
  const normalized = normalizeCapabilityOptions(options);
  return {
    runtimeVersion: normalized.runtimeVersion,
    platform: "reference",
    target: "reference-host",
    devMode: true,
    features: { ...DEFAULT_REFERENCE_HOST_FEATURES, ...normalized.featureOverrides },
    limits: {
      maxBodyBytes: 1048576,
      maxStorageBytes: 5242880,
      maxBridgeCallsPerMinute: 600,
      maxPackageBytes: 4194304,
      maxFileBytes: 2097152,
    },
    ...(normalized.appId ? { appId: normalized.appId } : {}),
  };
}

function normalizeCapabilityOptions(options) {
  if (typeof options === "string") {
    return { appId: options, runtimeVersion: RUNTIME_VERSION, featureOverrides: {} };
  }
  return {
    appId: options?.appId ?? null,
    runtimeVersion: options?.runtimeVersion ?? RUNTIME_VERSION,
    featureOverrides: options?.featureOverrides ?? {},
  };
}
