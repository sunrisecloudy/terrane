import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { catalogToTools } from "../catalog-to-tools.mjs";
import { executeTool } from "../execute-tool.mjs";
import { runReferenceAgentLoop } from "../reference-agent.mjs";
import { loadCatalogFromFile } from "../lib/catalog.mjs";

const moduleDir = path.dirname(fileURLToPath(import.meta.url));
const fixturePath = path.join(moduleDir, "..", "fixtures", "sample-catalog.json");

test("catalogToTools projects public tools and refuses inner surface", () => {
  const catalog = loadCatalogFromFile(fixturePath);
  const result = catalogToTools(catalog, { tier: "public", role: "editor" });

  const toolNames = result.tools.map((tool) => tool.name).sort();
  assert.deepEqual(toolNames, ["query_execute", "runtime_run", "ui_dispatch_event"]);
  assert.equal(result.reverseMap.query_execute, "query.execute");
  assert.equal(result.reverseMap.ui_dispatch_event, "ui.dispatch_event");
  assert.ok(result.tools.find((tool) => tool.name === "ui_dispatch_event"));
  assert.ok(!result.tools.find((tool) => tool.name === "ctx_db_insert"));
});

test("catalogToTools omits admin tools for public tier", () => {
  const catalog = loadCatalogFromFile(fixturePath);
  const result = catalogToTools(catalog, { tier: "public", role: "owner" });
  assert.ok(!result.tools.find((tool) => tool.name === "quota_set"));
});

test("catalogToTools includes operator tier when requested", () => {
  const catalog = loadCatalogFromFile(fixturePath);
  const result = catalogToTools(catalog, { tier: "operator", role: "owner" });
  assert.ok(result.tools.find((tool) => tool.name === "applet_install"));
});

test("tool descriptions include mutates/effectful hints", () => {
  const catalog = loadCatalogFromFile(fixturePath);
  const result = catalogToTools(catalog, { tier: "public", role: "owner" });
  const runtime = result.tools.find((tool) => tool.name === "runtime_run");
  assert.match(runtime.description, /mutates durable state/);
});

test("executeTool rejects tools above tier ceiling", async () => {
  const catalog = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
  const result = await executeTool("applet_install", {}, {
    catalogDocument: catalog,
    tier: "public",
    role: "owner",
    dryRun: true,
  });
  assert.equal(result.ok, false);
  assert.equal(result.error.code, "tool_not_offered");
});

test("executeTool refuses inner surface even if includeInner sneaks in", async () => {
  const catalog = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
  const result = await executeTool("ctx_db_insert", {}, {
    catalogDocument: catalog,
    tier: "public",
    role: "owner",
    dryRun: true,
  });
  assert.equal(result.ok, false);
  assert.equal(result.error.code, "tool_not_offered");
});

test("executeTool validates payload schema", async () => {
  const catalog = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
  const result = await executeTool("query_execute", {}, {
    catalogDocument: catalog,
    tier: "public",
    role: "owner",
    dryRun: true,
  });
  assert.equal(result.ok, false);
  assert.equal(result.error.code, "payload_validation");
});

test("executeTool dry-run builds envelope for valid read tool", async () => {
  const catalog = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
  const result = await executeTool(
    "query_execute",
    { collection: "notes" },
    {
      catalogDocument: catalog,
      tier: "public",
      role: "owner",
      dryRun: true,
    },
  );
  assert.equal(result.ok, true);
  assert.equal(result.envelope.name, "query.execute");
  assert.deepEqual(result.envelope.payload, { collection: "notes" });
});

test("executeTool requires confirm for mutating tools", async () => {
  const catalog = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
  const result = await executeTool(
    "runtime_run",
    { app_id: "notes-lite" },
    {
      catalogDocument: catalog,
      tier: "public",
      role: "owner",
    },
  );
  assert.equal(result.ok, false);
  assert.equal(result.error.code, "confirm_required");
});

test("reference agent notes-lite loop smoke projects tools and dry-runs query_execute", async () => {
  const result = await runReferenceAgentLoop({
    catalog: fixturePath,
    tier: "public",
    role: "owner",
    dryRun: true,
  });

  assert.equal(result.intent, "list my notes");
  assert.deepEqual(result.tools, ["query_execute", "runtime_run", "ui_dispatch_event"]);
  assert.equal(result.reverseMap.query_execute, "query.execute");
  assert.equal(result.queryResult.ok, true);
  assert.equal(result.queryResult.dry_run, true);
  assert.equal(result.queryResult.envelope.name, "query.execute");
  assert.deepEqual(result.queryResult.envelope.payload, { collection: "notes" });
});