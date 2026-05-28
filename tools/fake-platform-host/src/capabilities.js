export const RUNTIME_VERSION = "0.1.0";

export const DEFAULT_FAKE_HOST_FEATURES = Object.freeze({
  "core.step": true,
  "storage.read": true,
  "storage.write": true,
  "dialog.openFile": true,
  "dialog.saveFile": true,
  "network.request": true,
  "notification.toast": true,
  "app.log": true,
  "runtime.capabilities": true,
  "runtime.snapshot": true,
  "runtime.replay": true,
});

export function fakeHostCapabilities(options = null) {
  const normalized = normalizeCapabilityOptions(options);
  return {
    runtimeVersion: normalized.runtimeVersion,
    platform: "fake",
    target: "fake-host",
    devMode: true,
    features: { ...DEFAULT_FAKE_HOST_FEATURES, ...normalized.featureOverrides },
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
