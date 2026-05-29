export const TOOL_NAMES = [
  "platform.health",
  "platform.list_targets",
  "platform.launch",
  "platform.stop",
  "platform.reload_runtime",
  "platform.validate_package",
  "platform.install_webapp_package",
  "platform.list_webapps",
  "platform.open_webapp",
  "platform.uninstall_webapp",
  "platform.reset_webapp",
  "runtime.snapshot",
  "runtime.query",
  "runtime.click",
  "runtime.type",
  "runtime.set_value",
  "runtime.press_key",
  "runtime.drag",
  "runtime.wait_for",
  "runtime.screenshot",
  "runtime.console_logs",
  "runtime.bridge_calls",
  "runtime.event_log",
  "runtime.clear_logs",
  "runtime.call_bridge",
  "runtime.storage_get",
  "runtime.storage_set",
  "runtime.storage_reset",
  "runtime.network_mock_set",
  "runtime.network_mock_reset",
  "runtime.dialog_mock_set",
  "runtime.notification_capture",
  "runtime.timer_advance",
  "runtime.fault_inject",
  "runtime.core_step",
  "runtime.core_snapshot",
  "runtime.replay_events",
  "platform.sign_webapp_package",
  "platform.install_report",
  "platform.list_webapp_versions",
  "platform.approve_webapp_update",
  "platform.rollback_webapp",
  "platform.quarantine_webapp",
  "platform.create_snapshot",
  "platform.restore_snapshot",
  "platform.migration_dry_run",
  "platform.migration_apply",
  "platform.run_policy_audit",
  "platform.run_platform_smoke",
  "platform.run_repair_loop",
  "runtime.capabilities",
  "runtime.compare_snapshot",
  "runtime.resource_usage",
  "runtime.run_accessibility_audit",
  "runtime.accessibility_snapshot",
  "runtime.assert_visible",
  "runtime.assert_text",
  "runtime.assert_bridge_call",
  "runtime.assert_no_console_errors",
  "runtime.assert_accessibility",
  "runtime.assert_storage",
  "runtime.assert_core_action",
  "runtime.run_microtest",
  "runtime.run_smoke_tests",
  "db.snapshot",
  "db.query_app_storage",
  "db.query_app_versions",
  "db.query_bridge_calls",
  "db.query_core_events",
  "db.query_test_runs",
  "db.export_backup",
  "db.import_backup",
  "db.export_debug_bundle",
];

const STRING = { type: "string", minLength: 1 };
const BOOLEAN = { type: "boolean" };
const NUMBER = { type: "number" };
const OBJECT = { type: "object" };
const ARRAY = { type: "array" };
const ANY_JSON = {
  anyOf: [
    { type: "null" },
    { type: "boolean" },
    { type: "number" },
    { type: "string" },
    { type: "array" },
    { type: "object" },
  ],
};
const CONFIRM_TRUE = { const: true };

const PACKAGE_PATH_SCHEMA = schema({
  packagePath: STRING,
  path: STRING,
  trustLevel: STRING,
  maxAttempts: NUMBER,
  runSmokeTests: BOOLEAN,
  smokeRunner: STRING,
  runner: STRING,
  microtestPath: STRING,
  microtestPaths: ARRAY,
  patchPlan: ARRAY,
}, {
  anyOf: [{ required: ["packagePath"] }, { required: ["path"] }],
});

const APP_ID_SCHEMA = schema({ appId: STRING }, { required: ["appId"] });
const OPTIONAL_APP_ID_SCHEMA = schema({ appId: STRING });
const APP_ID_CONFIRM_SCHEMA = schema({ appId: STRING, confirm: CONFIRM_TRUE }, { required: ["appId", "confirm"] });
const APP_ID_SELECTOR_SCHEMA = schema({
  appId: STRING,
  testId: STRING,
  selector: STRING,
  text: STRING,
  role: STRING,
  name: STRING,
}, {
  required: ["appId"],
  anyOf: [{ required: ["testId"] }, { required: ["selector"] }, { required: ["text"] }, { required: ["role", "name"] }],
});

const TOOL_INPUT_SCHEMAS = new Map([
  ["platform.health", schema({ target: STRING })],
  ["platform.list_targets", schema({})],
  ["platform.launch", schema({ target: STRING, port: NUMBER })],
  ["platform.stop", schema({ target: STRING })],
  ["platform.reload_runtime", schema({ target: STRING })],
  ["platform.validate_package", PACKAGE_PATH_SCHEMA],
  ["platform.install_webapp_package", PACKAGE_PATH_SCHEMA],
  ["platform.sign_webapp_package", PACKAGE_PATH_SCHEMA],
  ["platform.run_policy_audit", PACKAGE_PATH_SCHEMA],
  ["platform.run_repair_loop", PACKAGE_PATH_SCHEMA],
  ["platform.list_webapps", schema({ includeUninstalled: BOOLEAN })],
  ["platform.open_webapp", APP_ID_SCHEMA],
  ["platform.uninstall_webapp", APP_ID_CONFIRM_SCHEMA],
  ["platform.reset_webapp", APP_ID_CONFIRM_SCHEMA],
  ["platform.install_report", schema({ appId: STRING, installId: STRING })],
  ["platform.list_webapp_versions", APP_ID_SCHEMA],
  ["platform.approve_webapp_update", schema({ appId: STRING, installId: STRING })],
  ["platform.rollback_webapp", schema({ appId: STRING, installId: STRING, confirm: CONFIRM_TRUE }, { required: ["appId", "confirm"] })],
  ["platform.quarantine_webapp", schema({ appId: STRING, installId: STRING, reason: STRING, confirm: CONFIRM_TRUE }, { required: ["appId", "confirm"] })],
  ["platform.create_snapshot", schema({ appId: STRING, type: STRING, sessionId: STRING }, { required: ["appId"] })],
  ["platform.restore_snapshot", schema({ snapshotId: STRING, confirm: CONFIRM_TRUE }, { required: ["snapshotId", "confirm"] })],
  ["platform.migration_dry_run", schema({ migration: OBJECT }, { required: ["migration"] })],
  ["platform.migration_apply", schema({ migration: OBJECT, confirm: CONFIRM_TRUE }, { required: ["migration", "confirm"] })],
  ["platform.run_platform_smoke", schema({ spec: OBJECT, smokePath: STRING, platform: STRING })],

  ["runtime.snapshot", APP_ID_SCHEMA],
  ["runtime.query", APP_ID_SELECTOR_SCHEMA],
  ["runtime.click", APP_ID_SELECTOR_SCHEMA],
  ["runtime.type", schema({ appId: STRING, testId: STRING, selector: STRING, text: STRING, value: STRING }, { required: ["appId", "text"], anyOf: [{ required: ["testId"] }, { required: ["selector"] }] })],
  ["runtime.set_value", schema({ appId: STRING, testId: STRING, selector: STRING, value: ANY_JSON }, { required: ["appId", "value"], anyOf: [{ required: ["testId"] }, { required: ["selector"] }] })],
  ["runtime.press_key", schema({ appId: STRING, key: STRING }, { required: ["key"] })],
  ["runtime.drag", schema({ appId: STRING, testId: STRING, selector: STRING, toTestId: STRING, toSelector: STRING }, { required: ["appId"], anyOf: [{ required: ["testId"] }, { required: ["selector"] }] })],
  ["runtime.wait_for", schema({ appId: STRING, kind: STRING, testId: STRING, selector: STRING, text: STRING, timeoutMs: NUMBER })],
  ["runtime.screenshot", APP_ID_SCHEMA],
  ["runtime.console_logs", OPTIONAL_APP_ID_SCHEMA],
  ["runtime.bridge_calls", OPTIONAL_APP_ID_SCHEMA],
  ["runtime.event_log", OPTIONAL_APP_ID_SCHEMA],
  ["runtime.clear_logs", OPTIONAL_APP_ID_SCHEMA],
  ["runtime.call_bridge", schema({ appId: STRING, sessionId: STRING, id: STRING, method: STRING, params: OBJECT }, { required: ["appId", "method"] })],
  ["runtime.storage_get", schema({ appId: STRING, sessionId: STRING, key: STRING, defaultValue: ANY_JSON }, { required: ["appId", "key"] })],
  ["runtime.storage_set", schema({ appId: STRING, sessionId: STRING, key: STRING, value: ANY_JSON }, { required: ["appId", "key", "value"] })],
  ["runtime.storage_reset", APP_ID_CONFIRM_SCHEMA],
  ["runtime.network_mock_set", schema({ appId: STRING, sessionId: STRING, method: STRING, urlPattern: STRING, match: OBJECT, response: OBJECT, error: OBJECT }, { anyOf: [{ required: ["urlPattern"] }, { required: ["match"] }] })],
  ["runtime.network_mock_reset", OPTIONAL_APP_ID_SCHEMA],
  ["runtime.dialog_mock_set", schema({ appId: STRING, sessionId: STRING, method: STRING, dialogType: STRING, files: ARRAY, selectedPath: STRING, cancelled: BOOLEAN, response: OBJECT, error: OBJECT }, { anyOf: [{ required: ["method"] }, { required: ["dialogType"] }] })],
  ["runtime.notification_capture", OPTIONAL_APP_ID_SCHEMA],
  ["runtime.timer_advance", schema({ appId: STRING, ms: NUMBER, milliseconds: NUMBER })],
  ["runtime.fault_inject", schema({ appId: STRING, method: STRING, code: STRING, message: STRING, once: BOOLEAN })],
  ["runtime.core_step", schema({ appId: STRING, sessionId: STRING, id: STRING, event: OBJECT }, { required: ["appId", "event"] })],
  ["runtime.core_snapshot", OPTIONAL_APP_ID_SCHEMA],
  ["runtime.replay_events", schema({ appId: STRING, events: ARRAY }, { required: ["appId", "events"] })],
  ["runtime.capabilities", OPTIONAL_APP_ID_SCHEMA],
  ["runtime.compare_snapshot", schema({ left: OBJECT, right: OBJECT, leftSnapshotId: STRING, rightSnapshotId: STRING }, { anyOf: [{ required: ["left", "right"] }, { required: ["leftSnapshotId", "rightSnapshotId"] }] })],
  ["runtime.resource_usage", APP_ID_SCHEMA],
  ["runtime.run_accessibility_audit", APP_ID_SCHEMA],
  ["runtime.accessibility_snapshot", APP_ID_SCHEMA],
  ["runtime.assert_visible", APP_ID_SELECTOR_SCHEMA],
  ["runtime.assert_text", schema({ appId: STRING, text: STRING }, { required: ["appId", "text"] })],
  ["runtime.assert_bridge_call", schema({ appId: STRING, method: STRING }, { required: ["appId", "method"] })],
  ["runtime.assert_no_console_errors", OPTIONAL_APP_ID_SCHEMA],
  ["runtime.assert_accessibility", schema({ appId: STRING, rule: STRING }, { required: ["appId"] })],
  ["runtime.assert_storage", schema({ appId: STRING, key: STRING, value: ANY_JSON }, { required: ["appId", "key", "value"] })],
  ["runtime.assert_core_action", schema({ appId: STRING, type: STRING, match: OBJECT }, { required: ["appId"] })],
  ["runtime.run_microtest", schema({ spec: OBJECT, microtestPath: STRING }, { anyOf: [{ required: ["spec"] }, { required: ["microtestPath"] }] })],
  ["runtime.run_smoke_tests", schema({ appId: STRING, runner: STRING, mode: STRING }, { required: ["appId"] })],

  ["db.snapshot", schema({})],
  ["db.query_app_storage", APP_ID_SCHEMA],
  ["db.query_app_versions", APP_ID_SCHEMA],
  ["db.query_bridge_calls", OPTIONAL_APP_ID_SCHEMA],
  ["db.query_core_events", OPTIONAL_APP_ID_SCHEMA],
  ["db.query_test_runs", OPTIONAL_APP_ID_SCHEMA],
  ["db.export_backup", schema({ appId: STRING })],
  ["db.import_backup", schema({ backup: OBJECT }, { required: ["backup"] })],
  ["db.export_debug_bundle", schema({ appId: STRING })],
]);

export function toolDefinitions() {
  return TOOL_NAMES.map((name) => ({
    name,
    description: descriptionFor(name),
    inputSchema: inputSchemaFor(name),
  }));
}

export function inputSchemaFor(name) {
  return cloneSchema(TOOL_INPUT_SCHEMAS.get(name) ?? schema({}));
}

export function validateToolArguments(name, args) {
  if (!TOOL_NAMES.includes(name)) {
    return { ok: false, error: `Unknown tool: ${name}`, details: { name } };
  }
  if (!args || typeof args !== "object" || Array.isArray(args)) {
    return { ok: false, error: `${name} arguments must be an object`, details: { expected: "object" } };
  }
  const schema = TOOL_INPUT_SCHEMAS.get(name) ?? schema({});
  return validateAgainstSchema(name, args, schema);
}

function descriptionFor(name) {
  if (name.startsWith("db.")) return `Safe database inspection command: ${name}`;
  if (name.startsWith("runtime.assert_")) return `Runtime assertion command: ${name}`;
  if (name.startsWith("runtime.")) return `Runtime control command: ${name}`;
  if (name.startsWith("platform.")) return `Platform control command: ${name}`;
  return `Platform tool: ${name}`;
}

function schema(properties, options = {}) {
  return {
    type: "object",
    additionalProperties: true,
    properties,
    required: options.required ?? Object.keys(properties).filter((key) => properties[key]?.required === true),
    ...(options.anyOf ? { anyOf: options.anyOf } : {}),
  };
}

function validateAgainstSchema(name, args, inputSchema) {
  const missing = [];
  for (const property of inputSchema.required ?? []) {
    if (!(property in args)) missing.push(property);
  }
  if (missing.length > 0) {
    return { ok: false, error: `${name} missing required argument: ${missing.join(", ")}`, details: { missing } };
  }

  if (inputSchema.anyOf && !inputSchema.anyOf.some((option) => hasRequiredArguments(args, option.required ?? []))) {
    return {
      ok: false,
      error: `${name} missing one required argument group`,
      details: { anyOf: inputSchema.anyOf.map((option) => option.required ?? []) },
    };
  }

  for (const [property, value] of Object.entries(args)) {
    const propertySchema = inputSchema.properties?.[property];
    if (!propertySchema) continue;
    const error = validateValue(property, value, propertySchema);
    if (error) {
      return { ok: false, error: `${name} invalid argument: ${error}`, details: { property } };
    }
  }

  return { ok: true };
}

function hasRequiredArguments(args, required) {
  return required.every((property) => property in args);
}

function validateValue(property, value, valueSchema) {
  if ("const" in valueSchema && value !== valueSchema.const) {
    return `${property} must equal ${JSON.stringify(valueSchema.const)}`;
  }
  if (valueSchema.anyOf) {
    return valueSchema.anyOf.some((option) => !validateValue(property, value, option)) ? null : `${property} has unsupported JSON type`;
  }
  if (!valueMatchesType(value, valueSchema.type)) {
    return `${property} must be ${valueSchema.type}`;
  }
  if (valueSchema.minLength && typeof value === "string" && value.length < valueSchema.minLength) {
    return `${property} must not be empty`;
  }
  return null;
}

function valueMatchesType(value, type) {
  if (!type) return true;
  if (type === "array") return Array.isArray(value);
  if (type === "null") return value === null;
  if (type === "object") return Boolean(value) && typeof value === "object" && !Array.isArray(value);
  return typeof value === type;
}

function cloneSchema(value) {
  return JSON.parse(JSON.stringify(value));
}
