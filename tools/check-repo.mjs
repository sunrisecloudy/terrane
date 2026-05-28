#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { PlatformDatabase } from "./fake-platform-host/src/platform-database.js";
import { examplesDir, repoRoot } from "./fake-platform-host/src/paths.js";
import { validatePackage } from "./fake-platform-host/src/package-validator.js";

const checks = [];

await runCheck("json.parse", checkJsonParse);
await runCheck("sqlite.migrate", checkSqliteMigrations);
await runCheck("postgres.static", checkPostgresSql);
await runCheck("examples.validate", checkExamplePackages);
await runCheck("manifests.sync", checkManifestSync);
await runCheck("spec.security_lint", checkSecurityLint);

for (const check of checks) {
  console.log(`${check.ok ? "ok" : "fail"} ${check.name}${check.detail ? ` ${check.detail}` : ""}`);
}

const failed = checks.filter((check) => !check.ok);
if (failed.length > 0) {
  process.exitCode = 1;
}

async function runCheck(name, fn) {
  try {
    const detail = await fn();
    checks.push({ name, ok: true, detail });
  } catch (error) {
    checks.push({ name, ok: false, detail: error.message });
  }
}

function checkJsonParse() {
  const files = walk(repoRoot).filter((filePath) => filePath.endsWith(".json"));
  for (const filePath of files) {
    JSON.parse(fs.readFileSync(filePath, "utf8"));
  }
  return `files=${files.length}`;
}

function checkSqliteMigrations() {
  const db = new PlatformDatabase();
  try {
    const requiredTables = [
      "apps",
      "app_versions",
      "app_files",
      "app_permissions",
      "app_installations",
      "app_storage",
      "runtime_sessions",
      "bridge_calls",
      "core_events",
      "core_actions",
      "runtime_snapshots",
      "control_sessions",
      "control_commands",
      "micro_tests",
      "test_runs",
      "network_mocks",
      "dialog_mocks",
      "app_migrations",
      "migration_runs",
      "app_install_reports",
      "backup_exports",
    ];
    const existing = new Set(db.all("SELECT name FROM sqlite_master WHERE type = 'table'").map((row) => row.name));
    const missing = requiredTables.filter((table) => !existing.has(table));
    if (missing.length > 0) {
      throw new Error(`missing tables: ${missing.join(", ")}`);
    }
    return `tables=${requiredTables.length}`;
  } finally {
    db.close();
  }
}

function checkPostgresSql() {
  const files = walk(path.join(repoRoot, "db", "postgres")).filter((filePath) => filePath.endsWith(".sql"));
  for (const filePath of files) {
    const sql = fs.readFileSync(filePath, "utf8");
    if (!/CREATE TABLE/i.test(sql)) {
      throw new Error(`${relative(filePath)} does not declare tables`);
    }
    if (/\bJSON\b/.test(sql) && !/\bJSONB\b/.test(sql)) {
      throw new Error(`${relative(filePath)} should use JSONB for logical JSON columns`);
    }
  }
  return `files=${files.length}`;
}

function checkExamplePackages() {
  const apps = fs.readdirSync(examplesDir).filter((entry) => fs.statSync(path.join(examplesDir, entry)).isDirectory());
  for (const app of apps) {
    const result = validatePackage(path.join(examplesDir, app));
    if (!result.ok) {
      throw new Error(`${app}: ${JSON.stringify(result.errors)}`);
    }
  }
  return `apps=${apps.length}`;
}

function checkManifestSync() {
  const rootExamples = path.join(repoRoot, "examples");
  if (!fs.existsSync(rootExamples)) {
    return "deprecated examples absent";
  }
  const apps = fs.readdirSync(examplesDir).filter((entry) => fs.statSync(path.join(examplesDir, entry)).isDirectory());
  for (const app of apps) {
    const canonical = readJson(path.join(examplesDir, app, "manifest.json"));
    const duplicatePath = path.join(rootExamples, app, "manifest.json");
    if (!fs.existsSync(duplicatePath)) {
      throw new Error(`missing duplicate manifest for ${app}`);
    }
    const duplicate = readJson(duplicatePath);
    if (JSON.stringify(canonical) !== JSON.stringify(duplicate)) {
      throw new Error(`manifest drift: ${app}`);
    }
  }
  return `apps=${apps.length}`;
}

function checkSecurityLint() {
  const nativeFiles = walk(path.join(repoRoot, "native")).filter((filePath) => /\.(kt|java|swift|cs|cpp|cc|c|h|hpp|rs|js|ts)$/.test(filePath));
  for (const filePath of nativeFiles) {
    const source = fs.readFileSync(filePath, "utf8");
    if (source.includes("addJavascriptInterface")) {
      throw new Error(`forbidden addJavascriptInterface in ${relative(filePath)}`);
    }
    if (source.includes("SharedPreferences")) {
      throw new Error(`forbidden SharedPreferences persistence in ${relative(filePath)}`);
    }
  }
  const manifestFiles = walk(repoRoot).filter((filePath) => path.basename(filePath) === "manifest.json");
  for (const filePath of manifestFiles) {
    const manifest = readJson(filePath);
    if ("networkAllowlist" in manifest) {
      throw new Error(`removed networkAllowlist in ${relative(filePath)}`);
    }
  }
  return `nativeFiles=${nativeFiles.length} manifests=${manifestFiles.length}`;
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function walk(root) {
  const files = [];
  if (!fs.existsSync(root)) return files;
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    if (entry.name === ".git" || entry.name === "node_modules" || entry.name === ".zig-cache" || entry.name === "zig-out") {
      continue;
    }
    const abs = path.join(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...walk(abs));
    } else if (entry.isFile()) {
      files.push(abs);
    }
  }
  return files;
}

function relative(filePath) {
  return path.relative(repoRoot, filePath);
}

if (process.argv[1] !== fileURLToPath(import.meta.url)) {
  throw new Error("check-repo.mjs is meant to be executed directly");
}
