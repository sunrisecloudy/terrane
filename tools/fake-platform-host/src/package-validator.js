import fs from "node:fs";
import path from "node:path";
import { PlatformError } from "./errors.js";
import { canonicalPackageHashes } from "./signing.js";
import { readJsonFile } from "./util.js";

const REQUIRED_FILES = ["manifest.json", "index.html", "styles.css", "app.js"];
const OPTIONAL_FILES = new Set(["smoke-tests.json", "README.md"]);
const REQUIRED_BUDGET_KEYS = [
  "maxDomNodes",
  "maxStorageBytes",
  "maxBridgeCallsPerMinute",
  "maxNetworkRequestsPerMinute",
  "maxTimers",
  "maxLogLinesPerMinute",
  "maxPackageBytes",
  "maxFileBytes",
];
const PERMISSIONS = new Set([
  "core.step",
  "storage.read",
  "storage.write",
  "dialog.openFile",
  "dialog.saveFile",
  "notification.toast",
  "network.request",
  "app.log",
]);
const METHOD_PERMISSIONS = new Map([
  ["core.step", "core.step"],
  ["storage.get", "storage.read"],
  ["storage.list", "storage.read"],
  ["storage.set", "storage.write"],
  ["storage.remove", "storage.write"],
  ["dialog.openFile", "dialog.openFile"],
  ["dialog.saveFile", "dialog.saveFile"],
  ["notification.toast", "notification.toast"],
  ["network.request", "network.request"],
]);
const ALLOWED_METHODS = new Set([
  ...METHOD_PERMISSIONS.keys(),
  "app.log",
  "runtime.capabilities",
]);
const MIGRATION_OPS = new Set([
  "renameKey",
  "setDefault",
  "deleteKey",
  "copyKey",
  "transformEnum",
  "moveStorageKey",
  "deleteStorageKey",
]);

const JS_POLICY = [
  ["forbidden_eval", /\beval\s*\(/],
  ["forbidden_function_constructor", /\bnew\s+Function\s*\(/],
  ["forbidden_dynamic_import", /\bimport\s*\(/],
  ["forbidden_network_api", /\bfetch\s*\(/],
  ["forbidden_network_api", /\bXMLHttpRequest\b/],
  ["forbidden_network_api", /\bWebSocket\b|\bEventSource\b/],
  ["forbidden_storage_api", /\blocalStorage\b|\bsessionStorage\b|\bindexedDB\b|\bdocument\.cookie\b/],
  ["forbidden_native_bridge", /\bwebkit\.messageHandlers\b|\bchrome\.webview\b|\bAndroid\./],
  ["forbidden_parent_access", /\bwindow\.(parent|top|opener)\b/],
];

export function validatePackage(packageDir) {
  let files;
  const errors = [];
  const warnings = [];

  try {
    files = readPackageFiles(packageDir);
  } catch (error) {
    if (error instanceof PlatformError) {
      return validationResult([issue(error.code, error.message, error.details)], warnings);
    }
    throw error;
  }

  for (const required of REQUIRED_FILES) {
    if (!files.has(required)) {
      errors.push(issue("missing_required_file", `${required} is required`, { path: required }));
    }
  }

  if (errors.length > 0) {
    return validationResult(errors, warnings);
  }

  const manifest = readManifest(path.join(packageDir, "manifest.json"), errors);
  if (manifest) {
    validateManifest(manifest, errors);
  }

  validateHtml(files.get("index.html"), errors);
  validateCss(files.get("styles.css"), errors);
  const bridgeMethods = validateJs(files.get("app.js"), errors);

  if (manifest) {
    validateBridgePermissions(manifest, bridgeMethods, errors);
    validateBudgets(manifest, files, errors);
    validateMigrations(manifest, files, errors);
  }

  return validationResult(errors, warnings, manifest, files, bridgeMethods);
}

export function readPackage(packageDir) {
  const result = validatePackage(packageDir);
  if (!result.ok) {
    throw new PlatformError("package_validation_failed", "Generated webapp package failed validation", {
      errors: result.errors,
    });
  }

  return {
    manifest: result.manifest,
    files: result.files,
    bridgeMethods: result.bridgeMethods,
    validation: result,
  };
}

export function validateSourceSnippet(source) {
  const errors = [];
  validateJs(source, errors);
  return validationResult(errors, [], null, new Map(), []);
}

export function packageHashes(manifest, files) {
  return canonicalPackageHashes(manifest, files);
}

function readPackageFiles(packageDir) {
  const files = new Map();
  walk(packageDir, "");

  for (const filePath of files.keys()) {
    if (filePath.startsWith("assets/")) {
      throw new PlatformError("unexpected_package_path", "assets/ is not allowed before v0.5", { path: filePath });
    }

    if (
      !REQUIRED_FILES.includes(filePath) &&
      !OPTIONAL_FILES.has(filePath) &&
      !filePath.startsWith("migrations/")
    ) {
      throw new PlatformError("unexpected_package_path", "Package contains an unexpected path", { path: filePath });
    }
  }

  return files;

  function walk(root, prefix) {
    for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
      const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
      const abs = path.join(root, entry.name);
      if (entry.isDirectory()) {
        walk(abs, rel);
      } else if (entry.isFile()) {
        files.set(rel, fs.readFileSync(abs, "utf8"));
      }
    }
  }
}

function readManifest(manifestPath, errors) {
  try {
    return readJsonFile(manifestPath);
  } catch (error) {
    errors.push(issue("invalid_manifest_json", "manifest.json must parse as JSON", { message: error.message }));
    return null;
  }
}

function validateManifest(manifest, errors) {
  const required = [
    "id",
    "name",
    "version",
    "runtimeVersion",
    "dataVersion",
    "entry",
    "description",
    "permissions",
    "storagePrefix",
    "capabilities",
    "resourceBudget",
    "networkPolicy",
  ];

  for (const key of required) {
    if (!(key in manifest)) {
      errors.push(issue("missing_manifest_field", `manifest.${key} is required`, { field: key }));
    }
  }

  if ("networkAllowlist" in manifest) {
    errors.push(issue("removed_manifest_field", "manifest.networkAllowlist was removed; use networkPolicy", {
      field: "networkAllowlist",
    }));
  }

  if (typeof manifest.id !== "string" || !/^[a-z][a-z0-9-]{2,63}$/.test(manifest.id)) {
    errors.push(issue("invalid_manifest_id", "manifest.id must be lowercase kebab-case", { value: manifest.id }));
  }

  if (manifest.entry !== "index.html") {
    errors.push(issue("invalid_entry", "manifest.entry must be index.html", { value: manifest.entry }));
  }

  if (manifest.storagePrefix !== `${manifest.id}:`) {
    errors.push(issue("invalid_storage_prefix", "manifest.storagePrefix must equal <id>:", {
      expected: `${manifest.id}:`,
      actual: manifest.storagePrefix,
    }));
  }

  if (!Number.isInteger(manifest.dataVersion) || manifest.dataVersion < 1) {
    errors.push(issue("invalid_data_version", "manifest.dataVersion must be a positive integer", {
      value: manifest.dataVersion,
    }));
  }

  if (!Array.isArray(manifest.permissions)) {
    errors.push(issue("invalid_permissions", "manifest.permissions must be an array", {}));
  } else {
    for (const permission of manifest.permissions) {
      if (!PERMISSIONS.has(permission)) {
        errors.push(issue("unknown_permission", "manifest.permissions contains an unknown permission", {
          permission,
        }));
      }
    }
  }

  if (!manifest.capabilities || typeof manifest.capabilities !== "object") {
    errors.push(issue("invalid_capabilities", "manifest.capabilities is required", {}));
  } else {
    for (const key of ["required", "optional"]) {
      if (!Array.isArray(manifest.capabilities[key])) {
        errors.push(issue("invalid_capabilities", `manifest.capabilities.${key} must be an array`, { key }));
      }
    }
  }

  validateResourceBudgetShape(manifest.resourceBudget, errors);
  validateNetworkPolicy(manifest.networkPolicy, errors);
}

function validateResourceBudgetShape(resourceBudget, errors) {
  if (!resourceBudget || typeof resourceBudget !== "object" || Array.isArray(resourceBudget)) {
    errors.push(issue("invalid_resource_budget", "manifest.resourceBudget must be an object", {}));
    return;
  }

  for (const key of REQUIRED_BUDGET_KEYS) {
    if (!Number.isInteger(resourceBudget[key]) || resourceBudget[key] < 0) {
      errors.push(issue("invalid_resource_budget", `manifest.resourceBudget.${key} must be a non-negative integer`, {
        key,
      }));
    }
  }
}

function validateNetworkPolicy(networkPolicy, errors) {
  if (!networkPolicy || typeof networkPolicy !== "object" || Array.isArray(networkPolicy)) {
    errors.push(issue("invalid_network_policy", "manifest.networkPolicy must be an object", {}));
    return;
  }

  if (!Array.isArray(networkPolicy.allow)) {
    errors.push(issue("invalid_network_policy", "manifest.networkPolicy.allow must be an array", {}));
    return;
  }

  for (const entry of networkPolicy.allow) {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
      errors.push(issue("invalid_network_policy", "networkPolicy.allow entries must be objects", {}));
      continue;
    }
    if (typeof entry.origin !== "string" || !/^https:\/\/[^/\s]+(?::\d+)?$/.test(entry.origin)) {
      errors.push(issue("invalid_network_origin", "networkPolicy origin must be https origin", { origin: entry.origin }));
    }
    if (!Array.isArray(entry.methods) || entry.methods.length === 0) {
      errors.push(issue("invalid_network_methods", "networkPolicy methods must be a non-empty array", {
        origin: entry.origin,
      }));
    }
  }
}

function validateHtml(source, errors) {
  if (/<script(?![^>]*\bsrc=["']app\.js["'])[^>]*>/i.test(source)) {
    errors.push(issue("forbidden_inline_script", "index.html may only load app.js", {}));
  }
  if (/<script[^>]+\bsrc=["']https?:\/\//i.test(source)) {
    errors.push(issue("forbidden_remote_script", "remote scripts are forbidden", {}));
  }
  if (/\son[a-z]+\s*=/i.test(source)) {
    errors.push(issue("forbidden_inline_handler", "inline event handlers are forbidden", {}));
  }
  if (/<(iframe|object|embed|applet)\b/i.test(source)) {
    errors.push(issue("forbidden_embedded_context", "generated apps may not create embedded browsing contexts", {}));
  }
  if (/javascript:/i.test(source)) {
    errors.push(issue("forbidden_javascript_url", "javascript: URLs are forbidden", {}));
  }
  for (const match of source.matchAll(/<(button|input|select|textarea|a)\b([^>]*)>/gi)) {
    const tag = match[1].toLowerCase();
    const attrs = match[2] ?? "";
    if (!/\bdata-testid\s*=/.test(attrs)) {
      errors.push(issue("missing_testid", "Interactive HTML elements must declare data-testid", { tag }));
    }
  }
}

function validateCss(source, errors) {
  if (/@import\b/i.test(source)) {
    errors.push(issue("forbidden_css_import", "remote CSS imports are forbidden", {}));
  }
  if (/url\(\s*["']?(?:https?:|data:|\/)/i.test(source)) {
    errors.push(issue("forbidden_css_url", "CSS url() may only reference relative package files", {}));
  }
}

function validateJs(source, errors) {
  for (const [code, pattern] of JS_POLICY) {
    if (pattern.test(source)) {
      errors.push(issue(code, "app.js uses a forbidden JavaScript API", {}));
    }
  }

  const methods = new Set();
  const patterns = [
    /AppRuntime\.call\s*\(\s*["']([^"']+)["']/g,
    /\bcall\s*\(\s*["']([^"']+)["']/g,
  ];
  for (const pattern of patterns) {
    let match;
    while ((match = pattern.exec(source))) {
      methods.add(match[1]);
    }
  }

  for (const method of methods) {
    if (!ALLOWED_METHODS.has(method)) {
      errors.push(issue("forbidden_bridge_method", "app.js calls an unknown bridge method", { method }));
    }
  }

  return [...methods].sort();
}

function validateBridgePermissions(manifest, bridgeMethods, errors) {
  const permissions = new Set(manifest.permissions ?? []);
  for (const method of bridgeMethods) {
    const required = METHOD_PERMISSIONS.get(method);
    if (required && !permissions.has(required)) {
      errors.push(issue("missing_permission", "manifest.permissions does not cover a bridge method used by app.js", {
        method,
        required,
      }));
    }
  }
}

function validateBudgets(manifest, files, errors) {
  const budget = manifest.resourceBudget ?? {};
  let packageBytes = 0;
  for (const [filePath, content] of files.entries()) {
    const bytes = Buffer.byteLength(content);
    packageBytes += bytes;
    if (Number.isInteger(budget.maxFileBytes) && bytes > budget.maxFileBytes) {
      errors.push(issue("resource_budget_exceeded", "Package file exceeds manifest.resourceBudget.maxFileBytes", {
        path: filePath,
        bytes,
        maxFileBytes: budget.maxFileBytes,
      }));
    }
  }
  if (Number.isInteger(budget.maxPackageBytes) && packageBytes > budget.maxPackageBytes) {
    errors.push(issue("resource_budget_exceeded", "Package exceeds manifest.resourceBudget.maxPackageBytes", {
      bytes: packageBytes,
      maxPackageBytes: budget.maxPackageBytes,
    }));
  }
}

function validateMigrations(manifest, files, errors) {
  const migrations = new Map(
    [...files.entries()]
      .filter(([filePath]) => filePath.startsWith("migrations/") && filePath.endsWith(".json"))
      .map(([filePath, content]) => [filePath, parseMigration(filePath, content, errors)])
      .filter(([, migration]) => migration),
  );

  for (let from = 1; from < manifest.dataVersion; from += 1) {
    const filePath = `migrations/${from}_to_${from + 1}.json`;
    if (!migrations.has(filePath)) {
      errors.push(issue("migration_missing", "dataVersion increase requires a consecutive migration file", {
        path: filePath,
        dataVersion: manifest.dataVersion,
      }));
    }
  }

  for (const [filePath, migration] of migrations.entries()) {
    const versionMatch = filePath.match(/^migrations\/(\d+)_to_(\d+)\.json$/);
    if (!versionMatch) {
      errors.push(issue("invalid_migration_filename", "Migration filename must be migrations/<from>_to_<to>.json", { path: filePath }));
      continue;
    }
    const from = Number(versionMatch[1]);
    const to = Number(versionMatch[2]);
    if (migration.appId !== manifest.id) {
      errors.push(issue("invalid_migration_app", "Migration appId must match manifest.id", { path: filePath, appId: migration.appId }));
    }
    if (migration.fromDataVersion !== from || migration.toDataVersion !== to || to !== from + 1) {
      errors.push(issue("invalid_migration_version", "Migration filename and dataVersion fields must describe one consecutive step", {
        path: filePath,
        fromDataVersion: migration.fromDataVersion,
        toDataVersion: migration.toDataVersion,
      }));
    }
    validateMigrationSteps(manifest, filePath, migration, errors);
  }
}

function parseMigration(filePath, content, errors) {
  try {
    return JSON.parse(content);
  } catch (error) {
    errors.push(issue("invalid_migration_json", "Migration file must parse as JSON", { path: filePath, message: error.message }));
    return null;
  }
}

function validateMigrationSteps(manifest, filePath, migration, errors) {
  if (!Array.isArray(migration.steps)) {
    errors.push(issue("invalid_migration", "Migration steps must be an array", { path: filePath }));
    return;
  }
  for (const [index, step] of migration.steps.entries()) {
    if (!step || typeof step !== "object" || Array.isArray(step)) {
      errors.push(issue("invalid_migration", "Migration step must be an object", { path: filePath, index }));
      continue;
    }
    if (!MIGRATION_OPS.has(step.op)) {
      errors.push(issue("invalid_migration_op", "Migration step uses an unknown op", { path: filePath, index, op: step.op }));
    }
    const storageKeyFields = ["key", "keyPattern"];
    if (["renameKey", "moveStorageKey", "copyKey"].includes(step.op)) {
      storageKeyFields.push("from", "to");
    }
    for (const field of storageKeyFields) {
      if (typeof step[field] === "string" && !step[field].startsWith(manifest.storagePrefix)) {
        errors.push(issue("invalid_migration_prefix", "Migration storage keys must stay inside manifest.storagePrefix", {
          path: filePath,
          index,
          field,
          value: step[field],
          storagePrefix: manifest.storagePrefix,
        }));
      }
    }
  }
}

function validationResult(errors, warnings, manifest = null, files = new Map(), bridgeMethods = []) {
  return {
    ok: errors.length === 0,
    errors,
    warnings,
    manifest,
    files,
    bridgeMethods,
  };
}

function issue(code, message, details) {
  return { code, message, details };
}
