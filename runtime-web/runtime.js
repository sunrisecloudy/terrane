(function () {
  const EXAMPLE_IDS = ["notes-lite", "task-workbench", "file-transformer", "api-dashboard", "core-replay-lab"];
  const appList = document.getElementById("app-list");
  const statusEl = document.getElementById("runtime-status");
  const frameWrap = document.getElementById("app-frame-wrap");
  const activeTitle = document.getElementById("active-title");
  const activeDescription = document.getElementById("active-description");
  const reloadButton = document.getElementById("reload-app");
  const refreshButton = document.getElementById("refresh-apps");
  const clearDebugButton = document.getElementById("clear-debug");
  const bridgeLog = document.getElementById("bridge-log");
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

  let apps = [];
  let activeApp = null;
  let activeFrame = null;
  const usageByApp = new Map();
  const minuteMs = 60 * 1000;

  refreshButton.addEventListener("click", loadApps);
  reloadButton.addEventListener("click", function () {
    if (activeApp) mountApp(activeApp);
  });
  clearDebugButton.addEventListener("click", function () {
    bridgeLog.textContent = "";
  });

  window.addEventListener("message", function (event) {
    if (!activeFrame || event.source !== activeFrame.contentWindow) return;
    if (!event.data || event.data.type !== "runtime.ready_for_port") return;
    attachBridgePort(event, activeApp.id);
  });

  loadApps();

  async function loadApps() {
    setStatus("Loading apps");
    const loaded = [];
    for (const id of EXAMPLE_IDS) {
      const manifest = await fetchJson(`/webapps/examples/${id}/manifest.json`);
      loaded.push(manifest);
    }
    apps = loaded;
    renderAppList();
    setStatus("Ready");
  }

  function renderAppList() {
    appList.textContent = "";
    for (const app of apps) {
      const button = document.createElement("button");
      button.className = "app-button";
      button.dataset.testid = `open-${app.id}-button`;
      button.innerHTML = `<strong></strong><span></span>`;
      button.querySelector("strong").textContent = app.name;
      button.querySelector("span").textContent = `${app.id} v${app.version}`;
      button.addEventListener("click", function () {
        mountApp(app);
      });
      if (activeApp && activeApp.id === app.id) button.classList.add("active");
      appList.appendChild(button);
    }
  }

  async function mountApp(app) {
    activeApp = app;
    renderAppList();
    reloadButton.disabled = false;
    activeTitle.textContent = app.name;
    activeDescription.textContent = app.description;
    setStatus(`Mounting ${app.id}`);

    const html = await fetchText(`/webapps/examples/${app.id}/index.html`);
    const srcdoc = injectRuntimeBootstrap(app.id, html);
    const frame = document.createElement("iframe");
    frame.title = app.name;
    frame.dataset.testid = "runtime-app-frame";
    frame.setAttribute("sandbox", "allow-scripts");
    frame.setAttribute("referrerpolicy", "no-referrer");
    frame.srcdoc = srcdoc;

    frameWrap.textContent = "";
    frameWrap.appendChild(frame);
    activeFrame = frame;
    setStatus(`Mounted ${app.id}`);
  }

  function injectRuntimeBootstrap(appId, html) {
    const bootstrap = `<base href="/webapps/examples/${appId}/">
<script>
(function () {
  var nextId = 1;
  var port = null;
  var pending = new Map();
  var queued = [];
  function call(method, params) {
    return new Promise(function (resolve, reject) {
      if (typeof method !== "string" || !method) {
        reject({ code: "invalid_request", message: "Bridge method must be a non-empty string", details: {} });
        return;
      }
      var bodyParams = params == null ? {} : params;
      if (typeof bodyParams !== "object" || Array.isArray(bodyParams)) {
        reject({ code: "invalid_request", message: "Bridge params must be an object", details: {} });
        return;
      }
      var id = "app_req_" + nextId++;
      var message = { id: id, method: method, params: bodyParams, timestamp: Date.now() };
      pending.set(id, { resolve: resolve, reject: reject });
      if (port) send(message);
      else queued.push(message);
    });
  }
  window.AppRuntime = {
    call: call,
    capabilities: function () {
      return call("runtime.capabilities", {});
    },
    on: function () {
      return function () {};
    }
  };
  window.addEventListener("message", function (event) {
    if (!event.data || event.data.type !== "runtime.port" || !event.ports || !event.ports[0]) return;
    port = event.ports[0];
    port.onmessage = function (portEvent) {
      var response = portEvent.data;
      var waiter = pending.get(response.id);
      if (!waiter) return;
      pending.delete(response.id);
      if (response.ok) waiter.resolve(response.result);
      else waiter.reject(response.error);
    };
    while (queued.length) send(queued.shift());
  });
  function send(message) {
    port.postMessage(message);
  }
  window.parent.postMessage({ type: "runtime.ready_for_port" }, "*");
})();
</script>`;

    if (/<head[^>]*>/i.test(html)) {
      return html.replace(/<head([^>]*)>/i, `<head$1>${bootstrap}`);
    }
    return `${bootstrap}${html}`;
  }

  function attachBridgePort(event, appId) {
    const channel = new MessageChannel();
    channel.port1.onmessage = async function (portEvent) {
      const request = portEvent.data;
      const runtimeError = validateRuntimeBridgeRequest(activeApp, request);
      if (runtimeError) {
        addBridgeLog(appId, request && request.method ? request.method : "unknown", runtimeError.code);
        channel.port1.postMessage({
          id: request && typeof request.id === "string" ? request.id : null,
          ok: false,
          error: runtimeError,
        });
        return;
      }
      addBridgeLog(appId, request.method, "pending");
      try {
        const response = await fetchJson("/bridge", {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "x-app-id": appId,
          },
          body: JSON.stringify(request),
        });
        addBridgeLog(appId, request.method, response.ok ? "ok" : response.error.code);
        channel.port1.postMessage(response);
      } catch (error) {
        addBridgeLog(appId, request.method, "runtime_error");
        channel.port1.postMessage({
          id: request.id,
          ok: false,
          error: { code: "runtime_error", message: error.message, details: {} },
        });
      }
    };
    event.source.postMessage({ type: "runtime.port" }, "*", [channel.port2]);
  }

  function validateRuntimeBridgeRequest(app, request) {
    if (!request || typeof request !== "object" || Array.isArray(request)) {
      return bridgeError("invalid_request", "Bridge request must be an object");
    }
    const fields = Object.keys(request);
    for (const field of fields) {
      if (field !== "id" && field !== "method" && field !== "params" && field !== "timestamp") {
        return bridgeError("invalid_request", "Bridge request contains unknown top-level fields", { fields: [field] });
      }
    }
    if (typeof request.id !== "string" || request.id.length === 0) {
      return bridgeError("invalid_request", "Bridge request id must be a non-empty string");
    }
    if (!isKnownRuntimeBridgeMethod(request.method)) {
      return bridgeError("unknown_method", `Unknown bridge method: ${request.method}`, { method: request.method });
    }
    if (!request.params || typeof request.params !== "object" || Array.isArray(request.params)) {
      return bridgeError("invalid_request", "Bridge request params must be an object");
    }
    if ("timestamp" in request && !Number.isFinite(request.timestamp)) {
      return bridgeError("invalid_request", "Bridge request timestamp must be a finite number");
    }
    const permission = permissionForBridgeMethod(request.method);
    if (permission && !(app.permissions || []).includes(permission)) {
      return bridgeError("permission_denied", `App ${app.id} cannot call ${request.method}`, {
        appId: app.id,
        method: request.method,
        requiredPermission: permission,
      });
    }
    const paramsError = validateMethodParams(app, request.method, request.params);
    if (paramsError) return paramsError;
    const budgetError = validateAndRecordBudget(app, request.method);
    if (budgetError) return budgetError;
    return null;
  }

  function validateMethodParams(app, method, params) {
    if (method === "storage.get" || method === "storage.set" || method === "storage.remove") {
      if (typeof params.key !== "string") {
        return bridgeError("invalid_request", `${method} requires key`);
      }
      if (!params.key.startsWith(app.storagePrefix)) {
        return bridgeError("permission_denied", `Storage key must begin with ${app.storagePrefix}`, {
          key: params.key,
          prefix: app.storagePrefix,
          appId: app.id,
        });
      }
    }
    if (method === "storage.list") {
      if (typeof params.prefix !== "string") {
        return bridgeError("invalid_request", "storage.list requires prefix");
      }
      if (!params.prefix.startsWith(app.storagePrefix)) {
        return bridgeError("permission_denied", `Storage key must begin with ${app.storagePrefix}`, {
          key: params.prefix,
          prefix: app.storagePrefix,
          appId: app.id,
        });
      }
    }
    if (method === "notification.toast") {
      if (typeof params.message !== "string") {
        return bridgeError("invalid_request", "notification.toast requires message");
      }
      if (params.level != null && !["info", "success", "warning", "error"].includes(params.level)) {
        return bridgeError("invalid_request", "notification.toast level must be info, success, warning, or error");
      }
    }
    if (method === "app.log") {
      if (!["debug", "info", "warn", "error"].includes(params.level)) {
        return bridgeError("invalid_request", "app.log level must be debug, info, warn, or error");
      }
      if (typeof params.message !== "string") {
        return bridgeError("invalid_request", "app.log requires message");
      }
    }
    if (method === "network.request") {
      return validateNetworkRequest(app, params);
    }
    return null;
  }

  function validateNetworkRequest(app, params) {
    if (typeof params.url !== "string") {
      return bridgeError("invalid_request", "network.request requires url");
    }
    let url;
    try {
      url = new URL(params.url);
    } catch (_) {
      return bridgeError("invalid_request", "network.request url must be absolute");
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      return bridgeError("network_policy_denied", "network.request protocol is not allowed");
    }
    const method = (params.method || "GET").toUpperCase();
    const headers = params.headers == null ? {} : params.headers;
    if (!headers || typeof headers !== "object" || Array.isArray(headers)) {
      return bridgeError("invalid_request", "network.request headers must be an object");
    }
    const headerNames = [];
    for (const [name, value] of Object.entries(headers)) {
      if (typeof value !== "string") {
        return bridgeError("invalid_request", "network.request headers must be strings");
      }
      const normalized = name.toLowerCase();
      if (normalized === "cookie" || normalized === "set-cookie") {
        return bridgeError("network_policy_denied", "network.request credential headers are not allowed");
      }
      headerNames.push(normalized);
    }
    const body = params.body == null ? null : params.body;
    if (body != null && typeof body !== "string") {
      return bridgeError("invalid_request", "network.request body must be a string or null");
    }
    const policy = app.networkPolicy && Array.isArray(app.networkPolicy.allow) ? app.networkPolicy.allow : [];
    const rule = policy.find(function (candidate) {
      const methods = Array.isArray(candidate.methods) ? candidate.methods.map(function (item) { return item.toUpperCase(); }) : [];
      const allowedHeaders = Array.isArray(candidate.allowedHeaders) ? candidate.allowedHeaders.map(function (item) { return item.toLowerCase(); }) : [];
      return candidate.origin === url.origin &&
        methods.includes(method) &&
        headerNames.every(function (name) { return allowedHeaders.includes(name); });
    });
    if (!rule) {
      return bridgeError("network_policy_denied", "network.request is outside manifest.networkPolicy", {
        origin: url.origin,
        method: method,
      });
    }
    if (body != null && Number.isInteger(rule.maxRequestBytes) && utf8Bytes(body) > rule.maxRequestBytes) {
      return bridgeError("network_policy_denied", "network.request body exceeds manifest.networkPolicy maxRequestBytes");
    }
    return null;
  }

  function validateAndRecordBudget(app, method) {
    const budget = app.resourceBudget || {};
    const usage = usageForApp(app.id);
    const now = Date.now();
    pruneUsage(usage.bridgeCalls, now);
    pruneUsage(usage.networkCalls, now);
    pruneUsage(usage.logLines, now);
    const bridgeLimit = budget.maxBridgeCallsPerMinute;
    if (Number.isInteger(bridgeLimit) && usage.bridgeCalls.length >= bridgeLimit) {
      return bridgeError("resource_budget_exceeded", "Bridge call rate exceeds manifest.resourceBudget.maxBridgeCallsPerMinute", {
        appId: app.id,
        limit: bridgeLimit,
      });
    }
    if (method === "network.request") {
      const networkLimit = budget.maxNetworkRequestsPerMinute;
      if (Number.isInteger(networkLimit) && usage.networkCalls.length >= networkLimit) {
        return bridgeError("resource_budget_exceeded", "Network request rate exceeds manifest.resourceBudget.maxNetworkRequestsPerMinute", {
          appId: app.id,
          limit: networkLimit,
        });
      }
    }
    if (method === "app.log") {
      const logLimit = budget.maxLogLinesPerMinute;
      if (Number.isInteger(logLimit) && usage.logLines.length >= logLimit) {
        return bridgeError("resource_budget_exceeded", "Log rate exceeds manifest.resourceBudget.maxLogLinesPerMinute", {
          appId: app.id,
          limit: logLimit,
        });
      }
    }
    usage.bridgeCalls.push(now);
    if (method === "network.request") usage.networkCalls.push(now);
    if (method === "app.log") usage.logLines.push(now);
    return null;
  }

  function usageForApp(appId) {
    if (!usageByApp.has(appId)) {
      usageByApp.set(appId, { bridgeCalls: [], networkCalls: [], logLines: [] });
    }
    return usageByApp.get(appId);
  }

  function pruneUsage(items, now) {
    while (items.length && now - items[0] > minuteMs) {
      items.shift();
    }
  }

  function utf8Bytes(value) {
    return new TextEncoder().encode(value).length;
  }

  function permissionForBridgeMethod(method) {
    return METHOD_PERMISSION.get(method) || null;
  }

  function isKnownRuntimeBridgeMethod(method) {
    return METHOD_PERMISSION.has(method) || method === "app.log" || method === "runtime.capabilities";
  }

  function bridgeError(code, message, details) {
    return { code, message, details: details || {} };
  }

  function addBridgeLog(appId, method, status) {
    const item = document.createElement("li");
    item.textContent = `${new Date().toISOString()} ${appId} ${method} ${status}`;
    bridgeLog.prepend(item);
  }

  async function fetchJson(url, options) {
    const response = await fetch(url, options);
    if (!response.ok) throw new Error(`${url} returned HTTP ${response.status}`);
    return response.json();
  }

  async function fetchText(url) {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url} returned HTTP ${response.status}`);
    return response.text();
  }

  function setStatus(value) {
    statusEl.textContent = value;
  }
})();
