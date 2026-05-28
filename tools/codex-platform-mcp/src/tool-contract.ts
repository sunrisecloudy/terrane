export type PlatformTarget =
  | "fake-host"
  | "macos"
  | "ios-simulator"
  | "android-emulator"
  | "windows"
  | "linux"
  | "server";

export type ControlResponse<T = unknown> = {
  ok: boolean;
  result?: T;
  error?: {
    code: string;
    message: string;
    details?: unknown;
  };
  diagnostics?: {
    target?: PlatformTarget;
    sessionId?: string;
    appId?: string;
    timestamp?: string;
  };
};

export type ToolName =
  | "platform.health"
  | "platform.list_targets"
  | "platform.launch"
  | "platform.stop"
  | "platform.validate_package"
  | "platform.install_webapp_package"
  | "platform.open_webapp"
  | "platform.reset_webapp"
  | "runtime.snapshot"
  | "runtime.query"
  | "runtime.click"
  | "runtime.type"
  | "runtime.set_value"
  | "runtime.press_key"
  | "runtime.wait_for"
  | "runtime.screenshot"
  | "runtime.console_logs"
  | "runtime.bridge_calls"
  | "runtime.event_log"
  | "runtime.clear_logs"
  | "runtime.storage_get"
  | "runtime.storage_set"
  | "runtime.storage_reset"
  | "runtime.network_mock_set"
  | "runtime.dialog_mock_set"
  | "runtime.timer_advance"
  | "runtime.core_step"
  | "runtime.core_snapshot"
  | "runtime.replay_events"
  | "platform.sign_webapp_package"
  | "platform.install_report"
  | "platform.list_webapp_versions"
  | "platform.rollback_webapp"
  | "platform.quarantine_webapp"
  | "platform.create_snapshot"
  | "platform.restore_snapshot"
  | "platform.run_policy_audit"
  | "platform.run_repair_loop"
  | "runtime.capabilities"
  | "runtime.compare_snapshot"
  | "runtime.resource_usage"
  | "runtime.run_accessibility_audit"
  | "runtime.accessibility_snapshot"
  | "runtime.assert_visible"
  | "runtime.assert_text"
  | "runtime.assert_bridge_call"
  | "runtime.assert_no_console_errors"
  | "runtime.assert_accessibility"
  | "runtime.run_microtest"
  | "runtime.run_smoke_tests"
  | "db.snapshot"
  | "db.query_app_storage"
  | "db.query_app_versions"
  | "db.query_bridge_calls"
  | "db.query_core_events"
  | "db.query_test_runs"
  | "db.export_debug_bundle";

export const TOOL_NAMES: ToolName[] = [
  "platform.health",
  "platform.list_targets",
  "platform.launch",
  "platform.stop",
  "platform.validate_package",
  "platform.install_webapp_package",
  "platform.open_webapp",
  "platform.reset_webapp",
  "runtime.snapshot",
  "runtime.query",
  "runtime.click",
  "runtime.type",
  "runtime.set_value",
  "runtime.press_key",
  "runtime.wait_for",
  "runtime.screenshot",
  "runtime.console_logs",
  "runtime.bridge_calls",
  "runtime.event_log",
  "runtime.clear_logs",
  "runtime.storage_get",
  "runtime.storage_set",
  "runtime.storage_reset",
  "runtime.network_mock_set",
  "runtime.dialog_mock_set",
  "runtime.timer_advance",
  "runtime.core_step",
  "runtime.core_snapshot",
  "runtime.replay_events",
  "platform.sign_webapp_package",
  "platform.install_report",
  "platform.list_webapp_versions",
  "platform.rollback_webapp",
  "platform.quarantine_webapp",
  "platform.create_snapshot",
  "platform.restore_snapshot",
  "platform.run_policy_audit",
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
  "runtime.run_microtest",
  "runtime.run_smoke_tests",
  "db.snapshot",
  "db.query_app_storage",
  "db.query_app_versions",
  "db.query_bridge_calls",
  "db.query_core_events",
  "db.query_test_runs",
  "db.export_debug_bundle",
];
