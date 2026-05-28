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

  let apps = [];
  let activeApp = null;
  let activeFrame = null;

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
  window.AppRuntime = {
    call: function (method, params) {
      return new Promise(function (resolve, reject) {
        var id = "app_req_" + nextId++;
        var message = { id: id, method: method, params: params || {}, timestamp: Date.now() };
        pending.set(id, { resolve: resolve, reject: reject });
        if (port) send(message);
        else queued.push(message);
      });
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
