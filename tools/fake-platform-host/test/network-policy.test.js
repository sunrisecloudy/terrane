import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";

test("network policy rejects unallowlisted headers and oversized request bodies", async () => {
  const host = new FakePlatformHost();
  try {
    host.installPackage(path.join(examplesDir, "api-dashboard"));

    const badHeader = await networkRequest(host, {
      url: "https://api.example.com/status",
      headers: { "x-debug": "1" },
    });
    assert.equal(badHeader.ok, false);
    assert.equal(badHeader.error.code, "network_policy_denied");
    assert.equal(badHeader.error.details.header, "x-debug");

    const largeBody = await networkRequest(host, {
      url: "https://api.example.com/status",
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "x".repeat(65537),
    });
    assert.equal(largeBody.ok, false);
    assert.equal(largeBody.error.code, "network_policy_denied");
    assert.equal(largeBody.error.details.maxRequestBytes, 65536);
  } finally {
    host.close();
  }
});

test("network policy rejects oversized responses, disallowed redirects, and timeouts", async () => {
  const host = new FakePlatformHost();
  try {
    host.installPackage(path.join(examplesDir, "api-dashboard"));
    host.database.addNetworkMock({
      appId: "api-dashboard",
      method: "GET",
      urlPattern: "https://api.example.com/huge",
      response: { status: 200, headers: {}, bodyText: "x".repeat(1048577) },
    });
    host.database.addNetworkMock({
      appId: "api-dashboard",
      method: "GET",
      urlPattern: "https://api.example.com/redirect",
      response: { status: 302, headers: { location: "https://other.example.com/status" }, bodyText: "" },
    });
    host.database.addNetworkMock({
      appId: "api-dashboard",
      method: "GET",
      urlPattern: "https://api.example.com/slow",
      response: { status: 200, headers: {}, bodyText: "ok", delayMs: 50 },
    });

    const huge = await networkRequest(host, { url: "https://api.example.com/huge" });
    assert.equal(huge.ok, false);
    assert.equal(huge.error.code, "network_policy_denied");
    assert.equal(huge.error.details.maxResponseBytes, 1048576);

    const redirect = await networkRequest(host, { url: "https://api.example.com/redirect" });
    assert.equal(redirect.ok, false);
    assert.equal(redirect.error.code, "network_policy_denied");
    assert.equal(redirect.error.details.origin, "https://other.example.com");

    const timeout = await networkRequest(host, { url: "https://api.example.com/slow", timeoutMs: 10 });
    assert.equal(timeout.ok, false);
    assert.equal(timeout.error.code, "timeout");
    assert.equal(timeout.error.details.timeoutMs, 10);
  } finally {
    host.close();
  }
});

async function networkRequest(host, patch) {
  return host.dispatchBridge(
    {
      id: "req_network",
      method: "network.request",
      params: {
        url: patch.url,
        method: patch.method ?? "GET",
        headers: patch.headers ?? {},
        body: patch.body ?? null,
        timeoutMs: patch.timeoutMs ?? 10000,
      },
    },
    { appId: "api-dashboard" },
  );
}
