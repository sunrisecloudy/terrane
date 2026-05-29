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
    "platform.migration_dry_run",
    "platform.migration_apply",
    "db.snapshot",
    "db.query_app_storage",
    "db.query_app_versions",
    "db.query_bridge_calls",
    "db.query_core_events",
    "db.query_test_runs",
    "db.export_backup",
    "db.import_backup",
    "db.export_debug_bundle",
  ]) {
    assert.equal(TOOL_NAMES.includes(name), true, name);
  }
});

test("database tool contract does not expose arbitrary SQL", () => {
  const dbTools = TOOL_NAMES.filter((name) => name.startsWith("db."));
  assert.deepEqual(dbTools, [
    "db.snapshot",
    "db.query_app_storage",
    "db.query_app_versions",
    "db.query_bridge_calls",
    "db.query_core_events",
    "db.query_test_runs",
    "db.export_backup",
    "db.import_backup",
    "db.export_debug_bundle",
  ]);
  assert.equal(TOOL_NAMES.some((name) => /\b(sql|query_sql|raw|unsafe)\b/i.test(name)), false);
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
