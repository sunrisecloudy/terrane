import assert from "node:assert/strict";
import test from "node:test";
import { TOOL_NAMES, toolDefinitions, inputSchemaFor, validateToolArguments } from "../src/tool-contract.js";

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

test("tool definitions expose focused input schemas", () => {
  assert.deepEqual(inputSchemaFor("platform.open_webapp").required, ["appId"]);
  assert.deepEqual(inputSchemaFor("platform.validate_package").anyOf, [{ required: ["packagePath"] }, { required: ["path"] }]);
  assert.deepEqual(inputSchemaFor("runtime.storage_set").required, ["appId", "key", "value"]);
  assert.deepEqual(inputSchemaFor("runtime.type").required, ["appId", "text"]);
  assert.equal(inputSchemaFor("platform.uninstall_webapp").properties.confirm.const, true);
  assert.equal(TOOL_NAMES.includes("runtime.unsafe_eval"), false);
});

test("tool argument validation rejects missing, malformed, and unconfirmed calls", () => {
  assert.equal(validateToolArguments("platform.open_webapp", { appId: "notes-lite" }).ok, true);
  assert.match(validateToolArguments("platform.open_webapp", {}).error, /missing required argument: appId/);
  assert.match(validateToolArguments("platform.open_webapp", { appId: "" }).error, /must not be empty/);
  assert.match(validateToolArguments("platform.validate_package", {}).error, /missing one required argument group/);
  assert.equal(validateToolArguments("platform.validate_package", { path: "webapps/examples/notes-lite" }).ok, true);
  assert.match(validateToolArguments("platform.uninstall_webapp", { appId: "notes-lite" }).error, /confirm/);
  assert.equal(validateToolArguments("platform.uninstall_webapp", { appId: "notes-lite", confirm: true }).ok, true);
  assert.match(validateToolArguments("runtime.storage_set", { appId: "notes-lite", key: "notes-lite:x" }).error, /value/);
  assert.match(validateToolArguments("runtime.storage_set", null).error, /arguments must be an object/);
});
