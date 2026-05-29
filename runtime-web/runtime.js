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
  let activeMount = null;
  const mountsByFrame = new WeakMap();
  const mountsByPort = new WeakMap();
  const androidBridgePending = new Map();
  let androidBridgeHandlerAttached = false;
  const webview2BridgePending = new Map();
  let webview2BridgeHandlerAttached = false;
  const usageByApp = new Map();
  const devMockStorageByApp = new Map();
  const devMockCoreVersions = new Map();
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
    if (!event.data || event.data.type !== "runtime.ready_for_port") {
      addBridgeLog(activeMount ? activeMount.appId : "unknown", "postMessage", "bridge.unauthorized_channel");
      return;
    }
    const mount = mountsByFrame.get(activeFrame);
    if (!mount || !activeMount || mount.mountToken !== activeMount.mountToken) {
      addBridgeLog(mount ? mount.appId : "unknown", "runtime.ready_for_port", "bridge.unauthorized_channel");
      return;
    }
    attachBridgePort(event, mount);
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
    const mount = {
      app: app,
      appId: app.id,
      mountToken: createMountToken(),
      createdAt: Date.now(),
    };
    activeApp = app;
    activeMount = mount;
    renderAppList();
    reloadButton.disabled = false;
    activeTitle.textContent = app.name;
    activeDescription.textContent = app.description;
    setStatus(`Mounting ${app.id}`);

    const html = await fetchText(`/webapps/examples/${app.id}/index.html`);
    const srcdoc = injectRuntimeBootstrap(app, html);
    const frame = document.createElement("iframe");
    frame.title = app.name;
    frame.dataset.testid = "runtime-app-frame";
    frame.setAttribute("sandbox", "allow-scripts");
    frame.setAttribute("referrerpolicy", "no-referrer");
    frame.srcdoc = srcdoc;

    frameWrap.textContent = "";
    frameWrap.appendChild(frame);
    activeFrame = frame;
    mountsByFrame.set(frame, mount);
    setStatus(`Mounted ${app.id}`);
  }

  function injectRuntimeBootstrap(app, html) {
    const appId = app.id;
    const bootstrap = `<base href="/webapps/examples/${appId}/">
<script>
(function () {
  var runtimeAppId = ${JSON.stringify(appId)};
  var resourceBudget = ${JSON.stringify(app.resourceBudget || {})};
  var knownEvents = new Set(["runtime.ready", "runtime.suspend", "runtime.resume", "app.error", "app.budget_warning", "app.permission_revoked"]);
  var eventHandlers = new Map();
  var nextId = 1;
  var port = null;
  var pending = new Map();
  var queued = [];
  var nativeSetTimeout = window.setTimeout.bind(window);
  var nativeClearTimeout = window.clearTimeout.bind(window);
  var nativeSetInterval = window.setInterval.bind(window);
  var nativeClearInterval = window.clearInterval.bind(window);
  var activeTimers = new Map();
  var budgetSignals = new Set();
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
  function on(eventName, handler) {
    if (!knownEvents.has(eventName) || typeof handler !== "function") {
      return function () {};
    }
    if (!eventHandlers.has(eventName)) {
      eventHandlers.set(eventName, new Set());
    }
    var handlers = eventHandlers.get(eventName);
    handlers.add(handler);
    return function () {
      handlers.delete(handler);
    };
  }
  function emit(eventName, payload) {
    var handlers = eventHandlers.get(eventName);
    if (!handlers || !handlers.size) return;
    Array.from(handlers).forEach(function (handler) {
      try {
        handler(payload);
      } catch (error) {
        console.error("AppRuntime event handler failed", error);
      }
    });
  }
  function emitAppError(error, source) {
    emit("app.error", {
      code: error && error.code ? error.code : "runtime_error",
      message: error && error.message ? error.message : String(error || "Unknown runtime error"),
      source: source
    });
  }
  window.AppRuntime = {
    call: call,
    capabilities: function () {
      return call("runtime.capabilities", {});
    },
    on: on
  };
  installBudgetGuards();
  window.addEventListener("error", function (event) {
    emitAppError({ code: "app.error", message: event.message || "Unhandled app error" }, "window.error");
  });
  window.addEventListener("unhandledrejection", function (event) {
    var reason = event.reason || {};
    emitAppError({ code: reason.code || "app.unhandled_rejection", message: reason.message || String(reason) }, "unhandledrejection");
  });
  window.addEventListener("message", function (event) {
    if (!event.data || event.data.type !== "runtime.port" || !event.ports || !event.ports[0]) return;
    port = event.ports[0];
    port.onmessage = function (portEvent) {
      var response = portEvent.data;
      if (response && response.type === "runtime.event") {
        emit(response.eventName, response.payload || {});
        return;
      }
      var waiter = pending.get(response.id);
      if (!waiter) return;
      pending.delete(response.id);
      if (response.ok) waiter.resolve(response.result);
      else {
        emitAppError(response.error, "bridge");
        waiter.reject(response.error);
      }
    };
    while (queued.length) send(queued.shift());
    call("runtime.capabilities", {}).then(function (capabilities) {
      emit("runtime.ready", {
        runtimeVersion: capabilities.runtimeVersion || "0.1.0",
        appId: runtimeAppId,
        capabilities: capabilities
      });
    }).catch(function (error) {
      emitAppError(error, "runtime.ready");
    });
  });
  function send(message) {
    port.postMessage(message);
  }
  function installBudgetGuards() {
    installTimerBudgetGuard();
    installDomBudgetGuard();
  }
  function installTimerBudgetGuard() {
    var maxTimers = budgetLimit("maxTimers");
    if (maxTimers == null) return;
    window.setTimeout = function (handler, delay) {
      var args = Array.prototype.slice.call(arguments, 2);
      assertTimerBudget("setTimeout", maxTimers);
      var nativeId = nativeSetTimeout(function () {
        activeTimers.delete(nativeId);
        if (typeof handler === "function") {
          handler.apply(window, args);
        }
      }, delay);
      activeTimers.set(nativeId, "timeout");
      warnBudget("maxTimers", activeTimers.size, maxTimers);
      return nativeId;
    };
    window.clearTimeout = function (nativeId) {
      activeTimers.delete(nativeId);
      return nativeClearTimeout(nativeId);
    };
    window.setInterval = function (handler, delay) {
      var args = Array.prototype.slice.call(arguments, 2);
      assertTimerBudget("setInterval", maxTimers);
      var nativeId = nativeSetInterval(function () {
        if (typeof handler === "function") {
          handler.apply(window, args);
        }
      }, delay);
      activeTimers.set(nativeId, "interval");
      warnBudget("maxTimers", activeTimers.size, maxTimers);
      return nativeId;
    };
    window.clearInterval = function (nativeId) {
      activeTimers.delete(nativeId);
      return nativeClearInterval(nativeId);
    };
  }
  function installDomBudgetGuard() {
    var maxDomNodes = budgetLimit("maxDomNodes");
    if (maxDomNodes == null) return;
    var scheduled = false;
    function scheduleCheck() {
      if (scheduled) return;
      scheduled = true;
      nativeSetTimeout(checkDomBudget, 0);
    }
    function checkDomBudget() {
      scheduled = false;
      var count = document.getElementsByTagName("*").length;
      warnBudget("maxDomNodes", count, maxDomNodes);
      if (count > maxDomNodes) {
        signalBudget("maxDomNodes", "error", count, maxDomNodes);
      }
    }
    if (window.MutationObserver && document.documentElement) {
      new MutationObserver(scheduleCheck).observe(document.documentElement, { childList: true, subtree: true });
    }
    nativeSetInterval(checkDomBudget, 250);
    scheduleCheck();
  }
  function assertTimerBudget(source, maxTimers) {
    if (activeTimers.size < maxTimers) return;
    signalBudget("maxTimers", "error", activeTimers.size + 1, maxTimers);
    throw new Error("resource_budget_exceeded: " + source + " would exceed maxTimers");
  }
  function warnBudget(budget, current, max) {
    if (max <= 0) return;
    if (current >= Math.ceil(max * 0.8)) {
      signalBudget(budget, "warning", current, max);
    }
  }
  function signalBudget(budget, level, current, max) {
    var key = budget + ":" + level;
    if (budgetSignals.has(key)) return;
    budgetSignals.add(key);
    var payload = { budget: budget, current: current, max: max, appId: runtimeAppId };
    if (level === "warning") {
      emit("app.budget_warning", payload);
      return;
    }
    emitAppError({
      code: "resource_budget_exceeded",
      message: budget + " exceeded",
      details: payload
    }, "resource_budget");
  }
  function budgetLimit(name) {
    return Number.isInteger(resourceBudget[name]) ? resourceBudget[name] : null;
  }
  window.parent.postMessage({ type: "runtime.ready_for_port" }, "*");
})();
</script>`;

    if (/<head[^>]*>/i.test(html)) {
      return html.replace(/<head([^>]*)>/i, `<head$1>${bootstrap}`);
    }
    return `${bootstrap}${html}`;
  }

  function attachBridgePort(event, mount) {
    const channel = new MessageChannel();
    mountsByPort.set(channel.port1, mount);
    channel.port1.onmessage = async function (portEvent) {
      const portMount = mountsByPort.get(channel.port1);
      if (!portMount || portMount.mountToken !== mount.mountToken) {
        addBridgeLog(mount.appId, "port.message", "bridge.unauthorized_channel");
        channel.port1.postMessage({
          id: portEvent.data && typeof portEvent.data.id === "string" ? portEvent.data.id : null,
          ok: false,
          error: bridgeError("bridge.unauthorized_channel", "Bridge message arrived on an unauthorized channel"),
        });
        return;
      }
      const request = portEvent.data;
      const runtimeError = validateRuntimeBridgeRequest(portMount.app, request, channel.port1);
      if (runtimeError) {
        addBridgeLog(portMount.appId, request && request.method ? request.method : "unknown", runtimeError.code);
        channel.port1.postMessage({
          id: request && typeof request.id === "string" ? request.id : null,
          ok: false,
          error: runtimeError,
        });
        return;
      }
      addBridgeLog(portMount.appId, request.method, "pending");
      try {
        const response = await dispatchBridgeRequest(request, portMount);
        addBridgeLog(portMount.appId, request.method, response.ok ? "ok" : response.error.code);
        channel.port1.postMessage(response);
      } catch (error) {
        addBridgeLog(portMount.appId, request.method, "runtime_error");
        channel.port1.postMessage({
          id: request.id,
          ok: false,
          error: { code: "runtime_error", message: error.message, details: {} },
        });
      }
    };
    event.source.postMessage({ type: "runtime.port" }, "*", [channel.port2]);
  }

  async function dispatchBridgeRequest(request, mount) {
    if (window.__APP_RUNTIME_DEV_MOCK__ === true) {
      return dispatchDevMockBridgeRequest(request, mount);
    }

    const webkitHandler = webkitNativeBridgeHandler();
    if (webkitHandler) {
      const response = await webkitHandler.postMessage({
        appId: mount.appId,
        mountToken: mount.mountToken,
        request: request,
      });
      return normalizeHostBridgeResponse(response, request.id);
    }

    const androidHandler = androidNativeBridgeHandler();
    if (androidHandler) {
      const response = await androidHandler.postMessage({
        appId: mount.appId,
        mountToken: mount.mountToken,
        request: request,
      });
      return normalizeHostBridgeResponse(response, request.id);
    }

    const webview2Handler = webview2NativeBridgeHandler();
    if (webview2Handler) {
      const response = await webview2Handler.postMessage({
        appId: mount.appId,
        mountToken: mount.mountToken,
        request: request,
      });
      return normalizeHostBridgeResponse(response, request.id);
    }

    return fetchJson("/bridge", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-app-id": mount.appId,
        "x-mount-token": mount.mountToken,
      },
      body: JSON.stringify(request),
    });
  }

  async function dispatchDevMockBridgeRequest(request, mount) {
    const result = devMockBridgeResult(request, mount);
    if (result && result.error) {
      return { id: request.id, ok: false, error: result.error };
    }
    return { id: request.id, ok: true, result: result };
  }

  function devMockBridgeResult(request, mount) {
    const method = request.method;
    const params = request.params || {};
    if (method === "runtime.capabilities") {
      return {
        runtimeVersion: "0.1.0",
        platform: "browser",
        target: "runtime-dev-mock",
        devMode: true,
        features: {
          "core.step": true,
          "runtime.capabilities": true,
          "storage.read": true,
          "storage.write": true,
          "storage.get": true,
          "storage.set": true,
          "storage.remove": true,
          "storage.list": true,
          "dialog.openFile": true,
          "dialog.saveFile": true,
          "notification.toast": true,
          "network.request": true,
          "app.log": true,
        },
        limits: {
          maxBodyBytes: 1048576,
          maxStorageBytes: 5242880,
          maxBridgeCallsPerMinute: 600,
          maxPackageBytes: 1048576,
          maxFileBytes: 524288,
        },
      };
    }
    if (method === "core.step") {
      return devMockCoreStep(mount.appId, params.event);
    }
    if (method === "storage.get") {
      const storage = devMockStorageForApp(mount.appId);
      return { value: storage.has(params.key) ? cloneJson(storage.get(params.key)) : cloneJson(params.defaultValue) };
    }
    if (method === "storage.set") {
      const storage = devMockStorageForApp(mount.appId);
      const value = "value" in params ? params.value : null;
      storage.set(params.key, cloneJson(value));
      return { ok: true, bytesWritten: utf8Bytes(JSON.stringify(value)) };
    }
    if (method === "storage.remove") {
      devMockStorageForApp(mount.appId).delete(params.key);
      return { ok: true };
    }
    if (method === "storage.list") {
      const keys = Array.from(devMockStorageForApp(mount.appId).keys())
        .filter(function (key) { return key.startsWith(params.prefix); })
        .sort();
      return { keys: keys };
    }
    if (method === "dialog.openFile") {
      return { error: bridgeError("dialog.mock_missing", "No dialog.openFile mock is registered") };
    }
    if (method === "dialog.saveFile") {
      return { ok: true };
    }
    if (method === "notification.toast") {
      return { ok: true };
    }
    if (method === "network.request") {
      return { status: 200, headers: {}, bodyText: "{}" };
    }
    if (method === "app.log") {
      if (params.level === "error") console.error("[app.log]", mount.appId, params.message);
      else console.log("[app.log]", mount.appId, params.message);
      return { ok: true };
    }
    return { error: bridgeError("unknown_method", `Unknown bridge method: ${method}`, { method: method }) };
  }

  function devMockCoreStep(appId, event) {
    const validationError = validateDevMockCoreEvent(event);
    if (validationError) {
      return { ok: false, error: validationError, actions: [] };
    }
    const stateVersion = (devMockCoreVersions.get(appId) || 0) + 1;
    devMockCoreVersions.set(appId, stateVersion);
    return {
      ok: true,
      stateVersion: stateVersion,
      actions: devMockActionsForEvent(event),
    };
  }

  function validateDevMockCoreEvent(event) {
    if (event === undefined) return { code: "invalid_event", message: "core.step input requires event" };
    if (!event || typeof event !== "object" || Array.isArray(event)) return { code: "invalid_event", message: "event must be an object" };
    if (!("type" in event)) return { code: "invalid_event", message: "event.type is required" };
    if (typeof event.type !== "string") return { code: "invalid_event", message: "event.type must be a string" };
    return null;
  }

  function devMockActionsForEvent(event) {
    if (event.type === "CreateTask") {
      const payload = event.payload || {};
      return [
        {
          type: "TaskAccepted",
          title: typeof payload.title === "string" ? payload.title : "",
          priority: typeof payload.priority === "string" ? payload.priority : "medium",
        },
        { type: "Toast", message: "Task accepted" },
      ];
    }
    if (event.type === "ToggleTask") {
      const payload = event.payload || {};
      return [{ type: "TaskToggled", id: payload.id || null }];
    }
    if (event.type === "TransformText") {
      const payload = event.payload || {};
      const text = typeof payload.text === "string" ? payload.text : "";
      const mode = typeof payload.mode === "string" ? payload.mode : "uppercase";
      return [{ type: "TransformText", text: devMockTransformText(text, mode) }];
    }
    if (event.type === "NetworkSnapshotReceived") {
      return [{ type: "NetworkSnapshotStored", received: true }];
    }
    return [{ type: "EventAccepted", eventType: event.type || "UnknownEvent" }];
  }

  function devMockTransformText(text, mode) {
    if (mode === "lowercase") return text.toLowerCase();
    if (mode === "reverse-lines") return text.split(/\r?\n/).reverse().join("\n");
    if (mode === "word-count") {
      const words = text.trim() ? text.trim().split(/\s+/).length : 0;
      const lines = text ? text.split(/\r?\n/).length : 0;
      return `Words: ${words}\nLines: ${lines}\nCharacters: ${text.length}`;
    }
    return text.toUpperCase();
  }

  function devMockStorageForApp(appId) {
    if (!devMockStorageByApp.has(appId)) {
      devMockStorageByApp.set(appId, new Map());
    }
    return devMockStorageByApp.get(appId);
  }

  function cloneJson(value) {
    if (value === undefined) return undefined;
    return JSON.parse(JSON.stringify(value));
  }

  function webkitNativeBridgeHandler() {
    const handlers = window.webkit && window.webkit.messageHandlers;
    const handler = handlers && handlers.NativeAIPlatformBridge;
    if (!handler || typeof handler.postMessage !== "function") return null;
    return handler;
  }

  function androidNativeBridgeHandler() {
    const handler = window.NativeAIPlatformBridge;
    if (!handler || typeof handler.postMessage !== "function") return null;
    attachAndroidBridgeHandler(handler);
    return {
      postMessage: function (envelope) {
        return new Promise(function (resolve, reject) {
          const requestId = envelope && envelope.request && envelope.request.id;
          if (typeof requestId !== "string" || requestId.length === 0) {
            reject(new Error("Android native bridge envelope requires a request id"));
            return;
          }
          androidBridgePending.set(requestId, { resolve: resolve, reject: reject });
          try {
            handler.postMessage(JSON.stringify(envelope));
          } catch (error) {
            androidBridgePending.delete(requestId);
            reject(error);
          }
        });
      },
    };
  }

  function attachAndroidBridgeHandler(handler) {
    if (androidBridgeHandlerAttached) return;
    const previousHandler = typeof handler.onmessage === "function" ? handler.onmessage : null;
    handler.onmessage = function (event) {
      if (previousHandler) previousHandler.call(handler, event);
      const response = typeof event.data === "string" ? parseJsonOrNull(event.data) : event.data;
      const responseId = response && typeof response.id === "string" ? response.id : null;
      if (!responseId || !androidBridgePending.has(responseId)) return;
      const waiter = androidBridgePending.get(responseId);
      androidBridgePending.delete(responseId);
      waiter.resolve(response);
    };
    androidBridgeHandlerAttached = true;
  }

  function webview2NativeBridgeHandler() {
    const handler = window.chrome && window.chrome.webview;
    if (!handler || typeof handler.postMessage !== "function" || typeof handler.addEventListener !== "function") return null;
    attachWebView2BridgeHandler(handler);
    return {
      postMessage: function (envelope) {
        return new Promise(function (resolve, reject) {
          const requestId = envelope && envelope.request && envelope.request.id;
          if (typeof requestId !== "string" || requestId.length === 0) {
            reject(new Error("WebView2 native bridge envelope requires a request id"));
            return;
          }
          webview2BridgePending.set(requestId, { resolve: resolve, reject: reject });
          try {
            handler.postMessage(JSON.stringify(envelope));
          } catch (error) {
            webview2BridgePending.delete(requestId);
            reject(error);
          }
        });
      },
    };
  }

  function attachWebView2BridgeHandler(handler) {
    if (webview2BridgeHandlerAttached) return;
    handler.addEventListener("message", function (event) {
      const response = typeof event.data === "string" ? parseJsonOrNull(event.data) : event.data;
      const responseId = response && typeof response.id === "string" ? response.id : null;
      if (!responseId || !webview2BridgePending.has(responseId)) return;
      const waiter = webview2BridgePending.get(responseId);
      webview2BridgePending.delete(responseId);
      waiter.resolve(response);
    });
    webview2BridgeHandlerAttached = true;
  }

  function normalizeHostBridgeResponse(response, requestId) {
    const parsed = typeof response === "string" ? parseJsonOrNull(response) : response;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return {
        id: requestId,
        ok: false,
        error: bridgeError("invalid_response", "Host bridge response must be an object"),
      };
    }
    if (typeof parsed.ok !== "boolean") {
      return {
        id: requestId,
        ok: false,
        error: bridgeError("invalid_response", "Host bridge response must include ok"),
      };
    }
    return parsed;
  }

  function validateRuntimeBridgeRequest(app, request, eventPort) {
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
    if ("appId" in request.params) {
      return bridgeError("invalid_request", "Bridge params must not include appId; app id is channel-derived", {
        field: "appId",
      });
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
    const budgetError = validateAndRecordBudget(app, request.method, eventPort);
    if (budgetError) return budgetError;
    return null;
  }

  function validateMethodParams(app, method, params) {
    if (method === "core.step") {
      if ("app" in params && typeof params.app !== "string") {
        return bridgeError("invalid_request", "core.step app field must be a string when present");
      }
      if (typeof params.app === "string" && params.app !== app.id) {
        return bridgeError("permission_denied", "core.step app field does not match the channel-derived app id", {
          requestedApp: params.app,
          channelApp: app.id,
        });
      }
    }
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
    const networkPolicy = app.networkPolicy && typeof app.networkPolicy === "object" ? app.networkPolicy : {};
    if (networkPolicyDeniesPrivateNetwork(networkPolicy) && isPrivateNetworkHost(url.hostname)) {
      return bridgeError("network_policy_denied", "network.request private network targets are denied", {
        origin: url.origin,
        host: normalizedNetworkHost(url.hostname),
      });
    }
    const method = (params.method || "GET").toUpperCase();
    const headers = params.headers == null ? {} : params.headers;
    if (!headers || typeof headers !== "object" || Array.isArray(headers)) {
      return bridgeError("invalid_request", "network.request headers must be an object");
    }
    if ("credentials" in params && params.credentials != null) {
      return bridgeError("network_policy_denied", "network.request credentials are not allowed");
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
    const policy = Array.isArray(networkPolicy.allow) ? networkPolicy.allow : [];
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
    let host = String(hostname || "").trim().toLowerCase();
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

  function validateAndRecordBudget(app, method, eventPort) {
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
        budget: "maxBridgeCallsPerMinute",
        current: usage.bridgeCalls.length + 1,
        max: bridgeLimit,
        limit: bridgeLimit,
      });
    }
    if (method === "network.request") {
      const networkLimit = budget.maxNetworkRequestsPerMinute;
      if (Number.isInteger(networkLimit) && usage.networkCalls.length >= networkLimit) {
        return bridgeError("resource_budget_exceeded", "Network request rate exceeds manifest.resourceBudget.maxNetworkRequestsPerMinute", {
          appId: app.id,
          budget: "maxNetworkRequestsPerMinute",
          current: usage.networkCalls.length + 1,
          max: networkLimit,
          limit: networkLimit,
        });
      }
    }
    if (method === "app.log") {
      const logLimit = budget.maxLogLinesPerMinute;
      if (Number.isInteger(logLimit) && usage.logLines.length >= logLimit) {
        return bridgeError("resource_budget_exceeded", "Log rate exceeds manifest.resourceBudget.maxLogLinesPerMinute", {
          appId: app.id,
          budget: "maxLogLinesPerMinute",
          current: usage.logLines.length + 1,
          max: logLimit,
          limit: logLimit,
        });
      }
    }
    usage.bridgeCalls.push(now);
    maybeWarnRuntimeBudget(usage, eventPort, app.id, "maxBridgeCallsPerMinute", usage.bridgeCalls.length, bridgeLimit);
    if (method === "network.request") {
      usage.networkCalls.push(now);
      maybeWarnRuntimeBudget(usage, eventPort, app.id, "maxNetworkRequestsPerMinute", usage.networkCalls.length, budget.maxNetworkRequestsPerMinute);
    }
    if (method === "app.log") {
      usage.logLines.push(now);
      maybeWarnRuntimeBudget(usage, eventPort, app.id, "maxLogLinesPerMinute", usage.logLines.length, budget.maxLogLinesPerMinute);
    }
    return null;
  }

  function usageForApp(appId) {
    if (!usageByApp.has(appId)) {
      usageByApp.set(appId, { bridgeCalls: [], networkCalls: [], logLines: [], budgetWarnings: new Set() });
    }
    return usageByApp.get(appId);
  }

  function maybeWarnRuntimeBudget(usage, eventPort, appId, budget, current, max) {
    if (!Number.isInteger(max) || max <= 0 || !eventPort || typeof eventPort.postMessage !== "function") return;
    const warningAt = Math.ceil(max * 0.8);
    if (current < warningAt) {
      usage.budgetWarnings.delete(budget);
      return;
    }
    if (usage.budgetWarnings.has(budget)) return;
    usage.budgetWarnings.add(budget);
    eventPort.postMessage({
      type: "runtime.event",
      eventName: "app.budget_warning",
      payload: { budget: budget, current: current, max: max, appId: appId },
    });
  }

  function createMountToken() {
    const bytes = new Uint8Array(16);
    if (!window.crypto || !window.crypto.getRandomValues) {
      throw new Error("Web Crypto getRandomValues is required for runtime mount tokens");
    }
    window.crypto.getRandomValues(bytes);
    let binary = "";
    for (const byte of bytes) binary += String.fromCharCode(byte);
    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
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

  function parseJsonOrNull(text) {
    try {
      return JSON.parse(text);
    } catch (_) {
      return null;
    }
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
