export const RUNTIME_VERSION = "0.1.0";

export function fakeHostCapabilities(appId = null) {
  return {
    runtimeVersion: RUNTIME_VERSION,
    platform: "fake",
    target: "fake-host",
    devMode: true,
    features: {
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
    },
    limits: {
      maxBodyBytes: 1048576,
      maxStorageBytes: 5242880,
      maxBridgeCallsPerMinute: 600,
      maxPackageBytes: 4194304,
      maxFileBytes: 2097152,
    },
    ...(appId ? { appId } : {}),
  };
}
