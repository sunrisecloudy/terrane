import assert from "node:assert/strict";
import test from "node:test";
import { TOOL_NAMES, toolDefinitions } from "../src/tool-contract.js";

test("tool names are unique", () => {
  assert.equal(new Set(TOOL_NAMES).size, TOOL_NAMES.length);
});

test("core micro-test and database tools are exposed", () => {
  for (const name of [
    "runtime.run_microtest",
    "runtime.bridge_calls",
    "runtime.replay_events",
    "db.snapshot",
    "db.query_app_storage",
    "db.query_app_versions",
    "db.query_bridge_calls",
    "db.query_core_events",
    "db.query_test_runs",
    "db.export_debug_bundle",
  ]) {
    assert.equal(TOOL_NAMES.includes(name), true, name);
  }
});

test("tool definitions are MCP list compatible", () => {
  const definitions = toolDefinitions();
  assert.equal(definitions.length, TOOL_NAMES.length);
  assert.deepEqual(
    definitions.map((tool) => tool.name),
    TOOL_NAMES,
  );
  assert.equal(definitions.every((tool) => tool.inputSchema.type === "object"), true);
});
