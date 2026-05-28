import assert from "node:assert/strict";
import test from "node:test";
import { ControlClient } from "../src/control-client.js";

test("control client sends the spec control token header", async (t) => {
  const originalFetch = globalThis.fetch;
  let request = null;
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  globalThis.fetch = async (url, options) => {
    request = { url, options };
    return {
      ok: true,
      async json() {
        return { ok: true, result: { accepted: true } };
      },
    };
  };

  const client = new ControlClient("http://127.0.0.1:7878", "test-token");
  const result = await client.command("platform.health", { target: "fake-host" });

  assert.equal(result.ok, true);
  assert.equal(request.url, "http://127.0.0.1:7878/control/command");
  assert.equal(request.options.headers["x-platform-control-token"], "test-token");
  assert.equal("authorization" in request.options.headers, false);
  assert.equal(request.options.body, JSON.stringify({ tool: "platform.health", args: { target: "fake-host" } }));
});
