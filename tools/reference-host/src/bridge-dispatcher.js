import { referenceHostCapabilities } from "./capabilities.js";
import { bridgeError, bridgeOk, errorBody, PlatformError } from "./errors.js";
import { id as makeId } from "./util.js";

const METHOD_PERMISSION = new Map([
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

export class BridgeDispatcher {
  constructor({ database, core, runtimeVersion = "0.1.0", allowRuntimeMismatch = false, capabilityOverrides = {} }) {
    this.database = database;
    this.core = core;
    this.runtimeVersion = runtimeVersion;
    this.allowRuntimeMismatch = allowRuntimeMismatch;
    this.capabilityOverrides = capabilityOverrides;
    this.faults = [];
  }

  addFault({ appId = null, method, code = "fault_injected", message = "Injected bridge fault", details = {}, once = true } = {}) {
    if (typeof method !== "string" || method.length === 0) {
      throw new PlatformError("invalid_request", "runtime.fault_inject requires a bridge method", { method });
    }
    if (!isKnownMethod(method)) {
      throw new PlatformError("unknown_method", `Unknown bridge method: ${method}`, { method });
    }
    const fault = {
      faultId: makeId("fault"),
      appId,
      method,
      code,
      message,
      details,
      once: once !== false,
    };
    this.faults.push(fault);
    return { ok: true, ...fault };
  }

  async dispatch(request, context = {}) {
    const started = Date.now();
    const id = request && typeof request === "object" && !Array.isArray(request) && typeof request.id === "string" ? request.id : null;
    const appId = context.appId ?? null;
    const sessionId = context.sessionId ?? this.database.createRuntimeSession({ appId });
    let method = "unknown";
    let params = {};
    const active = appId ? this.database.activeInstall(appId) : null;

    try {
      assertBridgeRequestShape(request);
      method = request.method;
      params = request.params;
      assertNoAppIdParam(params);
      if (!appId) {
        throw new PlatformError("bridge.unauthorized_channel", "Bridge calls require a channel-derived app id");
      }

      const result = await this.call(method, params, { appId, sessionId, active });
      const response = bridgeOk(id, result);
      this.database.logBridgeCall({
        sessionId,
        appId,
        installId: active?.installId ?? null,
        method,
        params,
        result: response,
        durationMs: Date.now() - started,
      });
      return response;
    } catch (error) {
      const response = bridgeError(id, error);
      this.database.logBridgeCall({
        sessionId,
        appId,
        installId: active?.installId ?? null,
        method: method ?? "unknown",
        params,
        error: response.error,
        durationMs: Date.now() - started,
      });
      this.quarantineAfterRepeatedBudgetViolations({ appId, active, error: response.error });
      return response;
    }
  }

  async call(method, params, context) {
    if (!isKnownMethod(method)) {
      throw new PlatformError("unknown_method", `Unknown bridge method: ${method}`, { method });
    }

    this.assertRuntimeCompatibility(context);
    this.throwInjectedFault(method, context);
    this.assertPermission(method, context);
    this.assertCapability(method, context);
    this.assertResourceBudget(method, params, context);

    if (method.startsWith("storage.")) {
      return this.storage(method, params, context);
    }

    if (method === "core.step") {
      if (params?.app && params.app !== context.appId) {
        throw new PlatformError("permission_denied", "core.step app field does not match the channel-derived app id", {
          requestedApp: params.app,
          channelApp: context.appId,
        });
      }
      const result = this.core.step(context.appId, params.event);
      this.database.logCoreStep({
        sessionId: context.sessionId,
        appId: context.appId,
        installId: context.active?.installId ?? null,
        event: params.event,
        result,
      });
      return result;
    }

    if (method === "dialog.openFile") {
      const mock = this.database.findDialogMock({ sessionId: context.sessionId, appId: context.appId, dialogType: "openFile" });
      if (!mock) throw new PlatformError("dialog.mock_missing", "No dialog.openFile mock is registered", {});
      return mock;
    }

    if (method === "dialog.saveFile") {
      const mock = this.database.findDialogMock({ sessionId: context.sessionId, appId: context.appId, dialogType: "saveFile" });
      return mock ?? { ok: true };
    }

    if (method === "notification.toast") {
      assertNotificationToastParams(params);
      return { ok: true };
    }

    if (method === "network.request") {
      const policyEntry = this.assertNetworkPolicy(params, context);
      const mock = this.database.findNetworkMock({
        sessionId: context.sessionId,
        appId: context.appId,
        method: params.method ?? "GET",
        url: params.url,
      });
      if (!mock) throw new PlatformError("network.mock_missing", "No network mock is registered for request", {
        method: params.method ?? "GET",
        url: params.url,
      });
      this.assertNetworkResponsePolicy(mock, policyEntry, params, context);
      return networkResponsePayload(mock);
    }

    if (method === "app.log") {
      assertAppLogParams(params);
      return { ok: true };
    }

    if (method === "runtime.capabilities") {
      return this.capabilities(context.appId);
    }

    throw new PlatformError("unknown_method", `Unknown bridge method: ${method}`, { method });
  }

  capabilities(appId = null) {
    return referenceHostCapabilities({
      appId,
      runtimeVersion: this.runtimeVersion,
      featureOverrides: this.capabilityOverrides,
    });
  }

  assertRuntimeCompatibility(context) {
    if (context.active?.status === "quarantined") {
      throw new PlatformError("package_quarantined", `App is quarantined: ${context.appId}`, { appId: context.appId });
    }
    const appRuntimeVersion = context.active?.manifest?.runtimeVersion;
    if (!appRuntimeVersion || this.allowRuntimeMismatch) return;
    const runtime = parseSemver(this.runtimeVersion);
    const app = parseSemver(appRuntimeVersion);
    const ok = Boolean(runtime && app && app.major === runtime.major && app.minor <= runtime.minor);
    if (!ok) {
      throw new PlatformError("runtime_version_incompatible", "App runtimeVersion is not compatible with the reference-host runtime", {
        runtimeVersion: this.runtimeVersion,
        appRuntimeVersion,
        allowRuntimeMismatch: this.allowRuntimeMismatch,
      });
    }
  }

  quarantineAfterRepeatedBudgetViolations({ appId, active, error }) {
    if (!appId || !active?.installId || error?.code !== "resource_budget_exceeded") return;

    const since = new Date(Date.now() - 60_000).toISOString();
    const count = this.database.countBridgeErrorsSince({
      appId,
      installId: active.installId,
      since,
      code: "resource_budget_exceeded",
    });
    if (count < 3) return;

    this.database.quarantineWebapp(appId, active.installId, "resource_budget_exceeded", {
      restorePrevious: true,
      actor: "reference-host-runtime",
    });
  }

  assertPermission(method, context) {
    const permission = METHOD_PERMISSION.get(method);
    if (!permission) return;

    const permissions = this.database.approvedPermissions(context.appId);
    if (!permissions.has(permission)) {
      throw new PlatformError("permission_denied", `App ${context.appId} cannot call ${method}`, {
        appId: context.appId,
        method,
        requiredPermission: permission,
      });
    }
  }

  assertCapability(method, context) {
    const features = this.capabilities(context.appId).features ?? {};
    const capability = METHOD_PERMISSION.get(method) ?? method;
    if (features[capability] === false) {
      throw new PlatformError("capability_unavailable", `${capability} is unavailable on reference-host`, {
        appId: context.appId,
        method,
        capability,
      });
    }
  }

  throwInjectedFault(method, context) {
    const index = this.faults.findIndex((fault) => fault.method === method && (!fault.appId || fault.appId === context.appId));
    if (index === -1) return;
    const fault = this.faults[index];
    if (fault.once) {
      this.faults.splice(index, 1);
    }
    throw new PlatformError(fault.code, fault.message, {
      ...fault.details,
      faultId: fault.faultId,
      appId: context.appId,
      method,
    });
  }

  assertResourceBudget(method, params, context) {
    const budget = context.active?.manifest?.resourceBudget ?? {};
    const since = new Date(Date.now() - 60_000).toISOString();
    const bridgeLimit = budget.maxBridgeCallsPerMinute;
    if (Number.isInteger(bridgeLimit)) {
      const count = this.database.countBridgeCallsSince({ appId: context.appId, installId: context.active?.installId ?? null, since });
      if (count >= bridgeLimit) {
        throw new PlatformError("resource_budget_exceeded", "Bridge call rate exceeds manifest.resourceBudget.maxBridgeCallsPerMinute", {
          appId: context.appId,
          budget: "maxBridgeCallsPerMinute",
          current: count + 1,
          max: bridgeLimit,
          limit: bridgeLimit,
          count,
        });
      }
    }

    if (method === "network.request" && Number.isInteger(budget.maxNetworkRequestsPerMinute)) {
      const count = this.database.countBridgeCallsSince({
        appId: context.appId,
        installId: context.active?.installId ?? null,
        since,
        method: "network.request",
      });
      if (count >= budget.maxNetworkRequestsPerMinute) {
        throw new PlatformError("resource_budget_exceeded", "Network request rate exceeds manifest.resourceBudget.maxNetworkRequestsPerMinute", {
          appId: context.appId,
          budget: "maxNetworkRequestsPerMinute",
          current: count + 1,
          max: budget.maxNetworkRequestsPerMinute,
          limit: budget.maxNetworkRequestsPerMinute,
          count,
        });
      }
    }

    if (method === "app.log" && Number.isInteger(budget.maxLogLinesPerMinute)) {
      const count = this.database.countBridgeCallsSince({
        appId: context.appId,
        installId: context.active?.installId ?? null,
        since,
        method: "app.log",
      });
      if (count >= budget.maxLogLinesPerMinute) {
        throw new PlatformError("resource_budget_exceeded", "Log rate exceeds manifest.resourceBudget.maxLogLinesPerMinute", {
          appId: context.appId,
          budget: "maxLogLinesPerMinute",
          current: count + 1,
          max: budget.maxLogLinesPerMinute,
          limit: budget.maxLogLinesPerMinute,
          count,
        });
      }
    }

    if (method === "storage.set" && Number.isInteger(budget.maxStorageBytes)) {
      const projectedBytes = this.database.storageBytesAfterSet(context.appId, params.key, params.value);
      if (projectedBytes > budget.maxStorageBytes) {
        throw new PlatformError("resource_budget_exceeded", "Storage write exceeds manifest.resourceBudget.maxStorageBytes", {
          appId: context.appId,
          key: params.key,
          budget: "maxStorageBytes",
          current: projectedBytes,
          max: budget.maxStorageBytes,
          limit: budget.maxStorageBytes,
          projectedBytes,
        });
      }
    }
  }

  storage(method, params, context) {
    const prefix = context.active?.manifest?.storagePrefix ?? `${context.appId}:`;
    const key = params.key ?? params.prefix;
    if (typeof key !== "string" || !key.startsWith(prefix)) {
      throw new PlatformError("permission_denied", `Storage key must begin with ${prefix}`, {
        key,
        prefix,
      });
    }

    if (method === "storage.get") {
      return { value: this.database.storageGet(context.appId, params.key, params.defaultValue ?? null) };
    }
    if (method === "storage.set") {
      return { ok: true, bytesWritten: this.database.storageSet(context.appId, params.key, params.value) };
    }
    if (method === "storage.remove") {
      this.database.storageRemove(context.appId, params.key);
      return { ok: true };
    }
    if (method === "storage.list") {
      return { keys: this.database.storageList(context.appId, params.prefix) };
    }
    throw new PlatformError("unknown_method", `Unknown storage method: ${method}`, { method });
  }

  assertNetworkPolicy(params, context) {
    let url;
    try {
      url = new URL(params.url);
    } catch {
      throw new PlatformError("invalid_request", "network.request url must be absolute", { url: params.url });
    }

    const method = (params.method ?? "GET").toUpperCase();
    const networkPolicy = context.active?.manifest?.networkPolicy ?? {};
    assertPrivateNetworkPolicy(networkPolicy, url, "network.request private network targets are denied");
    const allow = Array.isArray(networkPolicy.allow) ? networkPolicy.allow : [];
    const matching = findMatchingNetworkPolicy(allow, url, method);

    if (!matching) {
      throw new PlatformError("network_policy_denied", "network.request is outside manifest.networkPolicy", {
        origin: url.origin,
        method,
      });
    }

    assertNetworkHeaders(params.headers ?? {}, matching);
    assertNetworkCredentials(params);
    assertNetworkRequestBody(params.body, matching);
    return matching;
  }

  assertNetworkResponsePolicy(response, policyEntry, params, context) {
    assertNetworkTimeout(response, policyEntry, params);
    assertNetworkResponseSize(response, policyEntry, context);
    assertNetworkRedirect(response, context.active?.manifest?.networkPolicy ?? {}, params);
  }
}

export function controlResponse(result) {
  return { ok: true, result };
}

export function controlError(error) {
  return { ok: false, error: errorBody(error) };
}

function isKnownMethod(method) {
  return (
    METHOD_PERMISSION.has(method) ||
    method === "app.log" ||
    method === "runtime.capabilities"
  );
}

function assertNotificationToastParams(params) {
  if (typeof params.message !== "string") {
    throw new PlatformError("invalid_request", "notification.toast requires message", {});
  }
  if (params.level != null && typeof params.level !== "string") {
    throw new PlatformError("invalid_request", "notification.toast level must be a string", {});
  }
  if (typeof params.level === "string" && !["info", "success", "warning", "error"].includes(params.level)) {
    throw new PlatformError("invalid_request", "notification.toast level must be info, success, warning, or error", {
      level: params.level,
    });
  }
}

function assertAppLogParams(params) {
  if (typeof params.level !== "string") {
    throw new PlatformError("invalid_request", "app.log requires level", {});
  }
  if (!["debug", "info", "warn", "error"].includes(params.level)) {
    throw new PlatformError("invalid_request", "app.log level must be debug, info, warn, or error", {
      level: params.level,
    });
  }
  if (typeof params.message !== "string" || params.message.length === 0) {
    throw new PlatformError("invalid_request", "app.log requires message", {});
  }
}

function parseSemver(version) {
  const match = String(version ?? "").match(/^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/);
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
  };
}

function assertBridgeRequestShape(request) {
  if (!request || typeof request !== "object" || Array.isArray(request)) {
    throw new PlatformError("invalid_request", "Bridge request must be an object");
  }

  const allowed = new Set(["id", "method", "params", "timestamp"]);
  const extra = Object.keys(request).filter((key) => !allowed.has(key));
  if (extra.length > 0) {
    throw new PlatformError("invalid_request", "Bridge request contains unknown top-level fields", { fields: extra });
  }

  if (typeof request.id !== "string" || request.id.length === 0) {
    throw new PlatformError("invalid_request", "Bridge request id must be a non-empty string");
  }
  if (typeof request.method !== "string") {
    throw new PlatformError("invalid_request", "Bridge request method must be a string");
  }
  if (!request.params || typeof request.params !== "object" || Array.isArray(request.params)) {
    throw new PlatformError("invalid_request", "Bridge request params must be an object");
  }
  if ("timestamp" in request && !Number.isFinite(request.timestamp)) {
    throw new PlatformError("invalid_request", "Bridge request timestamp must be a finite number");
  }
}

function assertNoAppIdParam(params) {
  if (Object.prototype.hasOwnProperty.call(params, "appId")) {
    throw new PlatformError("invalid_request", "Bridge params must not include appId; app id is channel-derived", {
      field: "appId",
    });
  }
}

function findMatchingNetworkPolicy(allow, url, method) {
  return allow.find((entry) => {
    if (entry.origin !== url.origin) return false;
    if (!entry.methods.includes(method)) return false;
    if (entry.pathPrefix && !url.pathname.startsWith(entry.pathPrefix)) return false;
    return true;
  });
}

function assertPrivateNetworkPolicy(policy, url, message) {
  if (!networkPolicyDeniesPrivateNetwork(policy) || !isPrivateNetworkHost(url.hostname)) return;
  throw new PlatformError("network_policy_denied", message, {
    origin: url.origin,
    host: normalizedNetworkHost(url.hostname),
  });
}

function networkPolicyDeniesPrivateNetwork(policy) {
  return !policy || policy.denyPrivateNetwork !== false;
}

function isPrivateNetworkHost(hostname) {
  const host = normalizedNetworkHost(hostname);
  if (!host) return false;
  if (host === "localhost" || host.endsWith(".localhost")) return true;
  const ipv4 = parseIpv4Host(host);
  if (ipv4) {
    return isPrivateIpv4Octets(ipv4);
  }
  if (host === "::1") return true;
  if (host.startsWith("fc") || host.startsWith("fd")) return true;
  if (host.startsWith("fe8") || host.startsWith("fe9") || host.startsWith("fea") || host.startsWith("feb")) return true;
  if (host.startsWith("::ffff:")) {
    return isPrivateIpv4MappedHost(host.slice("::ffff:".length));
  }
  return false;
}

function normalizedNetworkHost(hostname) {
  let host = String(hostname ?? "").trim().toLowerCase();
  if (host.startsWith("[") && host.endsWith("]")) {
    host = host.slice(1, -1);
  }
  const zoneIndex = host.indexOf("%");
  return zoneIndex === -1 ? host : host.slice(0, zoneIndex);
}

function parseIpv4Host(host) {
  const parts = host.split(".");
  if (parts.length !== 4) return null;
  const octets = [];
  for (const part of parts) {
    if (!/^[0-9]{1,3}$/.test(part)) return null;
    const value = Number(part);
    if (!Number.isInteger(value) || value < 0 || value > 255) return null;
    octets.push(value);
  }
  return octets;
}

function isPrivateIpv4MappedHost(tail) {
  const dotted = parseIpv4Host(tail);
  if (dotted) return isPrivateIpv4Octets(dotted);
  const parts = tail.split(":");
  if (parts.length !== 2) return false;
  const high = parseHex16(parts[0]);
  const low = parseHex16(parts[1]);
  if (high == null || low == null) return false;
  return isPrivateIpv4Octets([
    (high >> 8) & 255,
    high & 255,
    (low >> 8) & 255,
    low & 255,
  ]);
}

function parseHex16(value) {
  if (!/^[0-9a-f]{1,4}$/.test(value)) return null;
  return Number.parseInt(value, 16);
}

function isPrivateIpv4Octets(octets) {
  const first = octets[0];
  const second = octets[1];
  return first === 0 ||
    first === 10 ||
    first === 127 ||
    (first === 100 && second >= 64 && second <= 127) ||
    (first === 169 && second === 254) ||
    (first === 172 && second >= 16 && second <= 31) ||
    (first === 192 && second === 168);
}

function assertNetworkHeaders(headers, policyEntry) {
  if (!headers || typeof headers !== "object" || Array.isArray(headers)) {
    throw new PlatformError("invalid_request", "network.request headers must be an object", {});
  }
  const allowed = new Set((policyEntry.allowedHeaders ?? []).map((header) => String(header).toLowerCase()));
  for (const name of Object.keys(headers)) {
    const normalized = name.toLowerCase();
    if (normalized === "cookie" || normalized === "set-cookie") {
      throw new PlatformError("network_policy_denied", "network.request credential headers are not allowed", {
        header: name,
      });
    }
    if (!allowed.has(normalized)) {
      throw new PlatformError("network_policy_denied", "network.request header is outside manifest.networkPolicy", {
        header: name,
        allowedHeaders: [...allowed],
      });
    }
  }
}

function assertNetworkCredentials(params) {
  if (Object.prototype.hasOwnProperty.call(params, "credentials") && params.credentials != null) {
    throw new PlatformError("network_policy_denied", "network.request credentials are not allowed", {
      credentials: params.credentials,
    });
  }
}

function assertNetworkRequestBody(body, policyEntry) {
  if (!Number.isInteger(policyEntry.maxRequestBytes)) return;
  const bytes = payloadBytes(body);
  if (bytes > policyEntry.maxRequestBytes) {
    throw new PlatformError("network_policy_denied", "network.request body exceeds manifest.networkPolicy.maxRequestBytes", {
      maxRequestBytes: policyEntry.maxRequestBytes,
      bytes,
    });
  }
}

function assertNetworkTimeout(response, policyEntry, params) {
  if ("timeoutMs" in params && (!Number.isInteger(params.timeoutMs) || params.timeoutMs <= 0)) {
    throw new PlatformError("invalid_request", "network.request timeoutMs must be a positive integer", { timeoutMs: params.timeoutMs });
  }
  if (!Number.isInteger(response?.delayMs)) return;

  const policyTimeout = Number.isInteger(policyEntry.timeoutMs) ? policyEntry.timeoutMs : null;
  const requestedTimeout = Number.isInteger(params.timeoutMs) ? params.timeoutMs : null;
  const effectiveTimeout = policyTimeout && requestedTimeout
    ? Math.min(policyTimeout, requestedTimeout)
    : policyTimeout ?? requestedTimeout;
  if (effectiveTimeout && response.delayMs > effectiveTimeout) {
    throw new PlatformError("timeout", "network.request timed out", {
      timeoutMs: effectiveTimeout,
      delayMs: response.delayMs,
    });
  }
}

function assertNetworkResponseSize(response, policyEntry, context) {
  const policyLimit = Number.isInteger(policyEntry.maxResponseBytes) ? policyEntry.maxResponseBytes : null;
  const budgetLimit = Number.isInteger(context.active?.manifest?.resourceBudget?.maxNetworkResponseBytes)
    ? context.active.manifest.resourceBudget.maxNetworkResponseBytes
    : null;
  const limit = policyLimit !== null && budgetLimit !== null ? Math.min(policyLimit, budgetLimit) : policyLimit ?? budgetLimit;
  if (!Number.isInteger(limit)) return;

  const bytes = payloadBytes(response?.bodyText ?? response?.body ?? "");
  if (bytes > limit) {
    throw new PlatformError("network_policy_denied", "network.response exceeds allowed byte limit", {
      maxResponseBytes: limit,
      bytes,
    });
  }
}

function assertNetworkRedirect(response, policy, params) {
  const status = Number(response?.status ?? 0);
  if (status < 300 || status >= 400) return;
  const location = headerValue(response?.headers, "location");
  if (!location) return;

  let redirectUrl;
  try {
    redirectUrl = new URL(location, params.url);
  } catch {
    throw new PlatformError("network_policy_denied", "network.response redirect location is invalid", { location });
  }
  assertPrivateNetworkPolicy(policy, redirectUrl, "network.response redirect targets private network");
  const method = (params.method ?? "GET").toUpperCase();
  const allow = Array.isArray(policy.allow) ? policy.allow : [];
  if (!findMatchingNetworkPolicy(allow, redirectUrl, method)) {
    throw new PlatformError("network_policy_denied", "network.response redirect is outside manifest.networkPolicy", {
      origin: redirectUrl.origin,
      method,
    });
  }
}

function headerValue(headers, name) {
  if (!headers || typeof headers !== "object" || Array.isArray(headers)) return null;
  const wanted = name.toLowerCase();
  for (const [key, value] of Object.entries(headers)) {
    if (key.toLowerCase() === wanted) return String(value);
  }
  return null;
}

function payloadBytes(value) {
  if (value == null) return 0;
  if (typeof value === "string") return Buffer.byteLength(value);
  return Buffer.byteLength(JSON.stringify(value));
}

function networkResponsePayload(response) {
  if (!response || typeof response !== "object" || Array.isArray(response)) return response;
  const { delayMs, ...payload } = response;
  return payload;
}
