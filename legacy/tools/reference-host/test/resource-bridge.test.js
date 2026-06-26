import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir } from "../src/paths.js";
import { MOCK_JPEG_BYTES } from "../src/resource-mock.js";

test("resource.invoke stores a visible JPEG and round-trips read/materialize", async () => {
  const host = new ReferenceHost();
  try {
    const packagePath = path.join(examplesDir, "test-camera");
    const install = await host.runControlCommand("platform.install_webapp_package", { packagePath });
    assert.equal(install.status, "enabled");

    const opened = await host.runControlCommand("platform.open_webapp", { appId: "test-camera" });
    const invoke = await host.dispatchBridge(
      {
        id: "req-1",
        method: "resource.invoke",
        params: { kind: "camera", options: { max_bytes: 524288, content_type: "image/jpeg" } },
      },
      { appId: "test-camera", sessionId: opened.sessionId },
    );
    assert.equal(invoke.ok, true, JSON.stringify(invoke.error));
    assert.match(invoke.result.asset_id, /^res_camera_/);
    assert.ok(invoke.result.size_bytes > 1000, `expected visible JPEG, got ${invoke.result.size_bytes} bytes`);

    const read = await host.dispatchBridge(
      {
        id: "req-2",
        method: "resource.read",
        params: { asset_id: invoke.result.asset_id },
      },
      { appId: "test-camera", sessionId: opened.sessionId },
    );
    assert.equal(read.ok, true);
    assert.ok(read.result.bytes_base64.length > 1000);
    assert.equal(Buffer.from(read.result.bytes_base64, "base64").compare(MOCK_JPEG_BYTES), 0);

    const materialized = await host.dispatchBridge(
      {
        id: "req-3",
        method: "resource.materialize",
        params: {
          asset_id: invoke.result.asset_id,
          request: { path: "attachments/test.jpg", handle: "workspace_data" },
        },
      },
      { appId: "test-camera", sessionId: opened.sessionId },
    );
    assert.equal(materialized.ok, true);
    assert.equal(materialized.result.path, "attachments/test.jpg");
  } finally {
    host.close();
  }
});

test("resource.invoke accepts submit_base64 camera frames", async () => {
  const host = new ReferenceHost();
  try {
    const packagePath = path.join(examplesDir, "test-camera");
    const install = await host.runControlCommand("platform.install_webapp_package", { packagePath });
    assert.equal(install.status, "enabled");

    const opened = await host.runControlCommand("platform.open_webapp", { appId: "test-camera" });
    const custom = Buffer.from("custom-camera-payload");
    const invoke = await host.dispatchBridge(
      {
        id: "req-4",
        method: "resource.invoke",
        params: {
          kind: "camera",
          options: {
            submit_base64: custom.toString("base64"),
            width: 640,
            height: 480,
            max_bytes: 524288,
          },
        },
      },
      { appId: "test-camera", sessionId: opened.sessionId },
    );
    assert.equal(invoke.ok, true);
    assert.equal(invoke.result.size_bytes, custom.length);
    assert.equal(invoke.result.width, 640);
  } finally {
    host.close();
  }
});