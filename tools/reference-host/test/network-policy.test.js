import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir } from "../src/paths.js";

test("network policy rejects unallowlisted headers and oversized request bodies", async () => {
  const host = new ReferenceHost();
  try {
    host.installPackage(path.join(examplesDir, "api-dashboard"));

    const badHeader = await networkRequest(host, {
      url: "https://api.example.com/status",
      headers: { "x-debug": "1" },
    });
    assert.equal(badHeader.ok, false);
    assert.equal(badHeader.error.code, "network_policy_denied");
    assert.equal(badHeader.error.details.header, "x-debug");

    const cookieHeader = await networkRequest(host, {
      url: "https://api.example.com/status",
      headers: { cookie: "sid=secret" },
    });
    assert.equal(cookieHeader.ok, false);
    assert.equal(cookieHeader.error.code, "network_policy_denied");
    assert.equal(cookieHeader.error.message, "network.request credential headers are not allowed");
    assert.equal(cookieHeader.error.details.header, "cookie");

    const credentialed = await networkRequest(host, {
      url: "https://api.example.com/status",
      credentials: "include",
    });
    assert.equal(credentialed.ok, false);
    assert.equal(credentialed.error.code, "network_policy_denied");
    assert.equal(credentialed.error.message, "network.request credentials are not allowed");
    assert.equal(credentialed.error.details.credentials, "include");

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

test("network policy denies private network targets by default unless explicitly disabled", async () => {
  const deniedHost = new ReferenceHost();
  try {
    deniedHost.installPackage(privateNetworkPackage());
    deniedHost.database.addNetworkMock({
      appId: "api-dashboard",
      method: "GET",
      urlPattern: "https://192.168.0.1/status",
      response: { status: 200, headers: {}, bodyText: "ok" },
    });

    const denied = await networkRequest(deniedHost, { url: "https://192.168.0.1/status" });
    assert.equal(denied.ok, false);
    assert.equal(denied.error.code, "network_policy_denied");
    assert.equal(denied.error.message, "network.request private network targets are denied");
    assert.deepEqual(denied.error.details, {
      origin: "https://192.168.0.1",
      host: "192.168.0.1",
    });

    const mappedLoopback = await networkRequest(deniedHost, { url: "https://[::ffff:7f00:1]/status" });
    assert.equal(mappedLoopback.ok, false);
    assert.equal(mappedLoopback.error.code, "network_policy_denied");
    assert.equal(mappedLoopback.error.details.host, "::ffff:7f00:1");
  } finally {
    deniedHost.close();
  }

  const allowedHost = new ReferenceHost();
  try {
    allowedHost.installPackage(privateNetworkPackage({ denyPrivateNetwork: false }));
    allowedHost.database.addNetworkMock({
      appId: "api-dashboard",
      method: "GET",
      urlPattern: "https://192.168.0.1/status",
      response: { status: 200, headers: {}, bodyText: "ok" },
    });

    const allowed = await networkRequest(allowedHost, { url: "https://192.168.0.1/status" });
    assert.equal(allowed.ok, true);
    assert.equal(allowed.result.status, 200);
    assert.equal(allowed.result.bodyText, "ok");
  } finally {
    allowedHost.close();
  }
});

test("network policy rejects oversized responses, disallowed redirects, and timeouts", async () => {
  const host = new ReferenceHost();
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
        ...(patch.credentials == null ? {} : { credentials: patch.credentials }),
      },
    },
    { appId: "api-dashboard" },
  );
}

function privateNetworkPackage({ denyPrivateNetwork } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "private-network-package-"));
  fs.cpSync(path.join(examplesDir, "api-dashboard"), dir, { recursive: true });
  const manifestPath = path.join(dir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.networkPolicy = {
    allow: [
      {
        origin: "https://192.168.0.1",
        methods: ["GET"],
        allowedHeaders: [],
        maxRequestBytes: 65536,
        maxResponseBytes: 1048576,
        timeoutMs: 10000,
      },
    ],
    ...(denyPrivateNetwork === undefined ? {} : { denyPrivateNetwork }),
  };
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
  return dir;
}
