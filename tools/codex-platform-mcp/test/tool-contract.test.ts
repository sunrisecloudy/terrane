import { describe, expect, it } from "vitest";
import { TOOL_NAMES } from "../src/tool-contract";

describe("tool contract", () => {
  it("has unique tool names", () => {
    expect(new Set(TOOL_NAMES).size).toBe(TOOL_NAMES.length);
  });

  it("includes core micro-test tools", () => {
    expect(TOOL_NAMES).toContain("runtime.run_microtest");
    expect(TOOL_NAMES).toContain("runtime.bridge_calls");
    expect(TOOL_NAMES).toContain("runtime.replay_events");
  });

  it("includes database inspection tools", () => {
    expect(TOOL_NAMES).toContain("db.snapshot");
    expect(TOOL_NAMES).toContain("db.query_app_storage");
    expect(TOOL_NAMES).toContain("db.query_app_versions");
    expect(TOOL_NAMES).toContain("db.query_bridge_calls");
    expect(TOOL_NAMES).toContain("db.query_core_events");
    expect(TOOL_NAMES).toContain("db.query_test_runs");
    expect(TOOL_NAMES).toContain("db.export_debug_bundle");
  });
});
