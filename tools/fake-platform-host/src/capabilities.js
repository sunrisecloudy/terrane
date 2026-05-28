export const RUNTIME_VERSION = "0.1.0";

export function fakeHostCapabilities(appId = null) {
  return {
    platform: "fake-host",
    runtimeVersion: RUNTIME_VERSION,
    features: {
      "dialog.openFile": "mocked",
      "dialog.saveFile": "mocked",
      "network.request": "mocked",
      "notification.toast": "captured",
      snapshot: true,
      replay: true,
      unsafe_eval: false,
      unsafe_sql: false,
    },
    limits: {
      maxPackageBytes: 4194304,
      maxFileBytes: 2097152,
    },
    ...(appId ? { appId } : {}),
  };
}
