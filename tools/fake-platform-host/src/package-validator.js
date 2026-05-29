import fs from "node:fs";
import path from "node:path";
import { PlatformError } from "./errors.js";
import { canonicalPackageHashes } from "./signing.js";
import { readJsonFile } from "./util.js";

const REQUIRED_FILES = ["manifest.json", "index.html", "styles.css", "app.js"];
const OPTIONAL_FILES = new Set(["smoke-tests.json", "README.md"]);
const PLATFORM_GENERATED_FILES = new Set(["signature.json", "install-report.json", "content-hashes.json"]);
const MAX_PACKAGE_FILES = 32;
const MAX_MIGRATION_FILES = 16;
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
const NETWORK_POLICY_KEYS = new Set(["allow", "denyPrivateNetwork", "allowCredentials"]);
const NETWORK_POLICY_ENTRY_KEYS = new Set([
  "origin",
  "methods",
  "pathPrefix",
  "allowedHeaders",
  "maxRequestBytes",
  "maxResponseBytes",
  "timeoutMs",
]);
const RESOURCE_HINT_RELS = new Set(["dns-prefetch", "modulepreload", "preconnect", "prefetch", "preload", "prerender"]);
const URL_ATTRIBUTE_TAGS_HANDLED_ELSEWHERE = new Set(["base", "form", "link", "script"]);
const NETWORK_METHODS = new Set(["GET", "POST", "PUT", "PATCH", "DELETE"]);
const CONTENT_RATING_MINIMUM_AGE = new Map([
  ["4+", 4],
  ["9+", 9],
  ["12+", 12],
  ["17+", 17],
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
  ["forbidden_service_worker", /\bnavigator\.serviceWorker\b|\bserviceWorker\.register\b/],
  ["forbidden_trusted_types_policy", /\btrustedTypes\.createPolicy\s*\(/],
  ["forbidden_network_api", /\bfetch\s*\(/],
  ["forbidden_network_api", /\bXMLHttpRequest\b/],
  ["forbidden_network_api", /\bWebSocket\b|\bEventSource\b|\bnavigator\.sendBeacon\b/],
  ["forbidden_storage_api", /\blocalStorage\b|\bsessionStorage\b|\bindexedDB\b|\bdocument\.cookie\b|\bcookieStore\b/],
  ["forbidden_sql_api", /\bopenDatabase\s*\(|\bexecuteSql\s*\(|\bSQLDatabase\b|\bsqlite3\b/],
  ["forbidden_native_bridge", /\bwebkit\.messageHandlers\b|\bchrome\.webview\b|\bAndroid\.|\bnative\.exec\b|\bNativeAIPlatformBridge\b/],
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
  validateCss(files.get("styles.css"), errors, files);
  validateSmokeTests(files.get("smoke-tests.json"), errors);
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

  if (files.size > MAX_PACKAGE_FILES) {
    throw new PlatformError("resource_budget_exceeded", "Package exceeds hard file count cap", {
      files: files.size,
      maxFiles: MAX_PACKAGE_FILES,
    });
  }

  for (const filePath of files.keys()) {
    if (PLATFORM_GENERATED_FILES.has(filePath)) {
      throw new PlatformError("platform_generated_artifact", "AI packages must not include platform-generated artifacts", {
        path: filePath,
      });
    }

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

  if (manifest.trust?.level === "bundled" && !manifest.contentRating) {
    errors.push(issue("missing_content_rating", "bundled manifests require contentRating for iOS app indexing", {
      field: "contentRating",
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
    const permissions = new Set(Array.isArray(manifest.permissions) ? manifest.permissions : []);
    for (const key of ["required", "optional"]) {
      if (!Array.isArray(manifest.capabilities[key])) {
        errors.push(issue("invalid_capabilities", `manifest.capabilities.${key} must be an array`, { key }));
        continue;
      }
      for (const capability of manifest.capabilities[key]) {
        if (typeof capability !== "string") {
          errors.push(issue("invalid_capabilities", `manifest.capabilities.${key} entries must be strings`, { key }));
        } else if (!capability.startsWith("runtime.") && !permissions.has(capability)) {
          errors.push(issue("invalid_capabilities", "Bridge capabilities must be covered by manifest.permissions", {
            capability,
            key,
          }));
        }
      }
    }
  }

  if ("contentRating" in manifest) {
    validateContentRating(manifest.contentRating, errors);
  }
  validateResourceBudgetShape(manifest.resourceBudget, errors);
  validateNetworkPolicy(manifest.networkPolicy, errors);
}

function validateContentRating(contentRating, errors) {
  if (!contentRating || typeof contentRating !== "object" || Array.isArray(contentRating)) {
    errors.push(issue("invalid_content_rating", "manifest.contentRating must be an object", {}));
    return;
  }

  for (const key of ["scheme", "label", "minimumAge", "descriptors"]) {
    if (!(key in contentRating)) {
      errors.push(issue("invalid_content_rating", `manifest.contentRating.${key} is required`, { key }));
    }
  }
  if (contentRating.scheme !== "app-store") {
    errors.push(issue("invalid_content_rating", "manifest.contentRating.scheme must be app-store", {
      value: contentRating.scheme,
    }));
  }
  const expectedMinimumAge = CONTENT_RATING_MINIMUM_AGE.get(contentRating.label);
  if (!expectedMinimumAge) {
    errors.push(issue("invalid_content_rating", "manifest.contentRating.label must be an App Store age band", {
      value: contentRating.label,
    }));
  } else if (contentRating.minimumAge !== expectedMinimumAge) {
    errors.push(issue("invalid_content_rating", "manifest.contentRating.minimumAge must match label", {
      label: contentRating.label,
      expected: expectedMinimumAge,
      actual: contentRating.minimumAge,
    }));
  }
  validateUniqueStringArray(
    contentRating.descriptors,
    "invalid_content_rating",
    "manifest.contentRating.descriptors must be a unique string array",
    errors,
  );
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

  for (const key of Object.keys(networkPolicy)) {
    if (!NETWORK_POLICY_KEYS.has(key)) {
      errors.push(issue("invalid_network_policy", "manifest.networkPolicy contains an unknown field", { key }));
    }
  }
  for (const key of ["denyPrivateNetwork", "allowCredentials"]) {
    if (key in networkPolicy && typeof networkPolicy[key] !== "boolean") {
      errors.push(issue("invalid_network_policy", `manifest.networkPolicy.${key} must be a boolean`, { key }));
    }
  }
  if (networkPolicy.allowCredentials === true) {
    errors.push(issue("invalid_network_policy", "manifest.networkPolicy.allowCredentials must be false in v0.4", {
      key: "allowCredentials",
    }));
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
    for (const key of Object.keys(entry)) {
      if (!NETWORK_POLICY_ENTRY_KEYS.has(key)) {
        errors.push(issue("invalid_network_policy", "networkPolicy.allow entry contains an unknown field", { key }));
      }
    }
    if (typeof entry.origin !== "string" || !/^https:\/\/[^/\s]+(?::\d+)?$/.test(entry.origin)) {
      errors.push(issue("invalid_network_origin", "networkPolicy origin must be https origin", { origin: entry.origin }));
    }
    if (!Array.isArray(entry.methods) || entry.methods.length === 0) {
      errors.push(issue("invalid_network_methods", "networkPolicy methods must be a non-empty array", {
        origin: entry.origin,
      }));
    } else {
      validateUniqueStringArray(entry.methods, "invalid_network_methods", "networkPolicy methods must be unique allowed HTTP methods", errors, {
        origin: entry.origin,
        allowed: [...NETWORK_METHODS],
      }, NETWORK_METHODS);
    }
    if ("pathPrefix" in entry && typeof entry.pathPrefix !== "string") {
      errors.push(issue("invalid_network_policy", "networkPolicy pathPrefix must be a string", { origin: entry.origin }));
    }
    if ("allowedHeaders" in entry) {
      validateUniqueStringArray(entry.allowedHeaders, "invalid_network_policy", "networkPolicy allowedHeaders must be a unique string array", errors, {
        origin: entry.origin,
      });
      for (const header of entry.allowedHeaders) {
        if (typeof header === "string" && isCredentialHeader(header)) {
          errors.push(issue("invalid_network_policy", "networkPolicy cannot allow credential headers", {
            origin: entry.origin,
            header,
          }));
        }
      }
    }
    for (const key of ["maxRequestBytes", "maxResponseBytes"]) {
      if (key in entry && (!Number.isInteger(entry[key]) || entry[key] < 0)) {
        errors.push(issue("invalid_network_policy", `networkPolicy ${key} must be a non-negative integer`, {
          origin: entry.origin,
          key,
        }));
      }
    }
    if ("timeoutMs" in entry && (!Number.isInteger(entry.timeoutMs) || entry.timeoutMs < 1 || entry.timeoutMs > 120000)) {
      errors.push(issue("invalid_network_policy", "networkPolicy timeoutMs must be an integer from 1 to 120000", {
        origin: entry.origin,
      }));
    }
  }
}

function isCredentialHeader(name) {
  const normalized = name.toLowerCase();
  return normalized === "cookie" || normalized === "set-cookie";
}

function validateUniqueStringArray(value, code, message, errors, details = {}, allowed = null) {
  if (!Array.isArray(value)) {
    errors.push(issue(code, message, details));
    return;
  }
  const seen = new Set();
  for (const item of value) {
    if (typeof item !== "string" || seen.has(item) || (allowed && !allowed.has(item))) {
      errors.push(issue(code, message, { ...details, value: item }));
      return;
    }
    seen.add(item);
  }
}

function validateHtml(source, errors) {
  validateCsp(source, errors);
  validateScriptTags(source, errors);
  validateStylesheetLinks(source, errors);
  validateHtmlUrlAttributes(source, errors);
  if (/<style\b/i.test(source) || /\sstyle\s*=/i.test(source)) {
    errors.push(issue("forbidden_inline_style", "generated apps must use styles.css instead of inline styles", {}));
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
  if (/<meta\b[^>]*\bhttp-equiv\s*=\s*["']refresh["']/i.test(source)) {
    errors.push(issue("forbidden_meta_refresh", "meta refresh is forbidden", {}));
  }
  for (const match of source.matchAll(/<base\b([^>]*)>/gi)) {
    const href = htmlAttr(match[1] ?? "", "href") ?? "";
    if (!href || /^(?:https?:|data:|javascript:|\/\/|\/)/i.test(href)) {
      errors.push(issue("forbidden_base_href", "base href must not escape package-relative URLs", { href }));
    }
  }
  for (const match of source.matchAll(/<form\b([^>]*)>/gi)) {
    const action = htmlAttr(match[1] ?? "", "action");
    if (action && action !== "#") {
      errors.push(issue("forbidden_form_action", "generated app forms must not submit directly", { action }));
    }
  }
  for (const match of source.matchAll(/<(button|input|select|textarea|a)\b([^>]*)>/gi)) {
    const tag = match[1].toLowerCase();
    const attrs = match[2] ?? "";
    if (!/\bdata-testid\s*=/.test(attrs)) {
      errors.push(issue("missing_testid", "Interactive HTML elements must declare data-testid", { tag }));
    }
  }
}

function validateHtmlUrlAttributes(source, errors) {
  for (const match of source.matchAll(/<([a-zA-Z][a-zA-Z0-9:-]*)\b([^>]*)>/g)) {
    const tag = match[1].toLowerCase();
    if (URL_ATTRIBUTE_TAGS_HANDLED_ELSEWHERE.has(tag)) continue;
    const attrs = match[2] ?? "";

    for (const attrName of ["href", "src", "poster"]) {
      const value = htmlAttr(attrs, attrName);
      if (value && isForbiddenHtmlUrl(value)) {
        errors.push(issue("forbidden_external_resource", "generated app HTML URLs must be package-relative", { tag, attribute: attrName, value }));
      }
    }

    const srcset = htmlAttr(attrs, "srcset");
    if (srcset && srcset.split(",").some((candidate) => isForbiddenHtmlUrl(candidate.trim().split(/\s+/)[0] ?? ""))) {
      errors.push(issue("forbidden_external_resource", "generated app HTML srcset URLs must be package-relative", { tag, attribute: "srcset" }));
    }
  }
}

function isForbiddenHtmlUrl(value) {
  const trimmed = value.trim();
  if (!trimmed || trimmed.startsWith("#")) return false;
  return /^(?:[a-z][a-z0-9+.-]*:|\/\/|\/)/i.test(trimmed);
}

function validateStylesheetLinks(source, errors) {
  let stylesheetCount = 0;
  for (const match of source.matchAll(/<link\b([^>]*)>/gi)) {
    const attrs = match[1] ?? "";
    const rel = htmlAttr(attrs, "rel") ?? "";
    const relTokens = rel.toLowerCase().split(/\s+/).filter(Boolean);
    if (!relTokens.includes("stylesheet")) {
      if (relTokens.some((token) => RESOURCE_HINT_RELS.has(token))) {
        errors.push(issue("forbidden_resource_hint", "generated apps must not create network/resource hints", { rel }));
      } else {
        errors.push(issue("forbidden_link_tag", "index.html may only use a link tag for styles.css", { rel }));
      }
      continue;
    }
    const href = htmlAttr(attrs, "href") ?? "";
    if (/^https?:\/\//i.test(href)) {
      errors.push(issue("forbidden_remote_stylesheet", "remote stylesheets are forbidden", {}));
      continue;
    }
    if (href !== "styles.css") {
      errors.push(issue("forbidden_stylesheet_href", "index.html may only load styles.css", { href }));
      continue;
    }
    stylesheetCount += 1;
    const disallowedAttrs = htmlAttrNames(attrs).filter((name) => name !== "rel" && name !== "href");
    if (disallowedAttrs.length > 0) {
      errors.push(issue("forbidden_stylesheet_attribute", "styles.css link tag must only declare rel and href", {
        attributes: disallowedAttrs,
      }));
    }
  }

  if (stylesheetCount === 0) {
    errors.push(issue("missing_stylesheet", "index.html must load styles.css", {}));
  } else if (stylesheetCount > 1) {
    errors.push(issue("invalid_stylesheet_count", "index.html must load styles.css exactly once", {
      count: stylesheetCount,
    }));
  }
}

function validateScriptTags(source, errors) {
  const scripts = [...source.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)];
  if (scripts.length === 0) {
    errors.push(issue("missing_app_script", "index.html must load app.js", {}));
    return;
  }

  let appScriptCount = 0;
  for (const match of scripts) {
    const attrs = match[1] ?? "";
    const body = match[2] ?? "";
    const src = htmlAttr(attrs, "src");
    if (!src) {
      errors.push(issue("forbidden_inline_script", "index.html may only load app.js", {}));
      continue;
    }
    if (/^https?:\/\//i.test(src)) {
      errors.push(issue("forbidden_remote_script", "remote scripts are forbidden", {}));
      continue;
    }
    if (src !== "app.js") {
      errors.push(issue("forbidden_app_script_src", "index.html may only load app.js", { src }));
      continue;
    }
    appScriptCount += 1;
    const disallowedAttrs = htmlAttrNames(attrs).filter((name) => name !== "src");
    if (disallowedAttrs.length > 0) {
      errors.push(issue("forbidden_app_script_attribute", "app.js script tag must only declare src", {
        attributes: disallowedAttrs,
      }));
    }
    if (body.trim()) {
      errors.push(issue("forbidden_inline_script", "app.js script tag must not contain inline script body", {}));
    }
  }

  if (appScriptCount !== 1) {
    errors.push(issue("invalid_app_script_count", "index.html must load app.js exactly once", {
      count: appScriptCount,
    }));
  }
}

function validateCsp(source, errors) {
  for (const match of source.matchAll(/<meta\b([^>]*)>/gi)) {
    const attrs = match[1] ?? "";
    const httpEquiv = htmlAttr(attrs, "http-equiv") ?? "";
    if (httpEquiv.toLowerCase() !== "content-security-policy") continue;
    const content = htmlAttr(attrs, "content") ?? "";
    const directives = parseCspDirectives(content);
    const styleSrc = directives.get("style-src") ?? directives.get("default-src") ?? [];
    if (styleSrc.includes("'unsafe-inline'")) {
      errors.push(issue("forbidden_inline_style_csp", "Content-Security-Policy must not allow inline styles", {
        directive: "style-src",
      }));
    }
    const scriptSrc = directives.get("script-src") ?? directives.get("default-src") ?? [];
    if (scriptSrc.includes("'unsafe-inline'")) {
      errors.push(issue("forbidden_inline_script_csp", "Content-Security-Policy must not allow inline scripts", {
        directive: "script-src",
      }));
    }
  }
}

function parseCspDirectives(content) {
  const directives = new Map();
  for (const rawDirective of content.split(";")) {
    const parts = rawDirective.trim().split(/\s+/).filter(Boolean);
    if (parts.length === 0) continue;
    directives.set(parts[0].toLowerCase(), parts.slice(1));
  }
  return directives;
}

function htmlAttr(attrs, name) {
  const match = attrs.match(new RegExp(`\\b${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)')`, "i"));
  return match?.[1] ?? match?.[2] ?? null;
}

function htmlAttrNames(attrs) {
  return [...attrs.matchAll(/\b([a-zA-Z_:][-a-zA-Z0-9_:.]*)\b(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'>`]+))?/g)]
    .map((match) => match[1].toLowerCase());
}

function validateCss(source, errors, files = new Map()) {
  if (/@import\b/i.test(source)) {
    errors.push(issue("forbidden_css_import", "remote CSS imports are forbidden", {}));
  }
  if (/@font-face\b/i.test(source)) {
    errors.push(issue("forbidden_external_font", "external fonts are forbidden before v0.5", {}));
  }
  if (/\bposition\s*:\s*fixed\b/i.test(source)) {
    errors.push(issue("forbidden_fixed_position", "generated app CSS must not escape the host viewport", {}));
  }
  for (const value of cssUrlValues(source)) {
    if (isForbiddenCssUrl(value, files)) {
      errors.push(issue("forbidden_css_url", "CSS url() may only reference relative package files", { value }));
      break;
    }
  }
}

function cssUrlValues(source) {
  return [...source.matchAll(/url\(\s*(?:"([^"]*)"|'([^']*)'|([^'")\s]+))\s*\)/gi)].map(
    (match) => match[1] ?? match[2] ?? match[3] ?? "",
  );
}

function isForbiddenCssUrl(value, files) {
  const trimmed = value.trim();
  if (!trimmed || trimmed.startsWith("#")) return false;
  if (isForbiddenHtmlUrl(trimmed)) return true;
  const packagePath = trimmed.split(/[?#]/, 1)[0];
  return !files.has(packagePath);
}

function validateSmokeTests(source, errors) {
  if (!source) return;
  let tests;
  try {
    tests = JSON.parse(source);
  } catch (error) {
    errors.push(issue("invalid_smoke_tests", "smoke-tests.json must parse as JSON", { message: error.message }));
    return;
  }
  if (!Array.isArray(tests)) {
    errors.push(issue("invalid_smoke_tests", "smoke-tests.json must be an array", {}));
    return;
  }
  for (const testCase of tests) {
    if (!testCase || typeof testCase !== "object" || Array.isArray(testCase)) {
      errors.push(issue("invalid_smoke_tests", "Each smoke test must be an object", {}));
      continue;
    }
    if (testCase.steps !== undefined && !Array.isArray(testCase.steps)) {
      errors.push(issue("invalid_smoke_tests", "Smoke test steps must be an array", {}));
      continue;
    }
    for (const step of testCase.steps ?? []) {
      if (!step || typeof step !== "object" || Array.isArray(step)) {
        errors.push(issue("invalid_smoke_tests", "Smoke test steps must be objects", {}));
        continue;
      }
      if ("selector" in step && !isDataTestIdSelector(step.selector)) {
        errors.push(issue("invalid_smoke_selector", "Smoke test selectors must use data-testid", { selector: step.selector }));
      }
    }
  }
}

function isDataTestIdSelector(selector) {
  return typeof selector === "string" && /^\[data-testid=(["'])[^"']+\1\]$/.test(selector.trim());
}

function validateJs(source, errors) {
  for (const [code, pattern] of JS_POLICY) {
    if (pattern.test(source)) {
      errors.push(issue(code, "app.js uses a forbidden JavaScript API", {}));
    }
  }
  if (hasBridgeAppIdParam(source)) {
    errors.push(issue("forbidden_appid_param", "AppRuntime.call params must not include appId", {}));
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

function hasBridgeAppIdParam(source) {
  const patterns = [
    /\bAppRuntime\s*\.\s*call\s*\(\s*["'][^"']+["']\s*,\s*\{[^)]*(?:\bappId\b|["']appId["'])\s*:/m,
    /\bcall\s*\(\s*["'][^"']+["']\s*,\s*\{[^)]*(?:\bappId\b|["']appId["'])\s*:/m,
  ];
  return patterns.some((pattern) => pattern.test(source));
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
  const migrationFileCount = [...files.keys()].filter((filePath) => filePath.startsWith("migrations/")).length;
  if (migrationFileCount > MAX_MIGRATION_FILES) {
    errors.push(issue("resource_budget_exceeded", "Package exceeds hard migration file count cap", {
      files: migrationFileCount,
      maxMigrationFiles: MAX_MIGRATION_FILES,
    }));
  }

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
