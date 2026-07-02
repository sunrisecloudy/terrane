(function () {
  var currentId = currentAppId();
  var list = document.getElementById("app-list");
  var title = document.getElementById("app-title");
  var frame = document.getElementById("app-frame");
  var infoButton = document.getElementById("desktop-info-button");
  var infoPanel = document.getElementById("desktop-info-panel");
  var infoClose = document.getElementById("desktop-info-close");

  if (!currentId) {
    showError("No app selected");
    return;
  }

  bindDesktopInfo();
  bindBridge();
  frame.src = "/apps/" + encodeURIComponent(currentId) + "/__terrane/frame/";

  fetch("/apps", { cache: "no-store" })
    .then(function (response) {
      if (!response.ok) throw new Error("cannot load apps");
      return response.json();
    })
    .then(function (catalog) {
      renderCatalog(Array.isArray(catalog.apps) ? catalog.apps : []);
    })
    .catch(function () {
      showError("Cannot load apps");
    });

  function currentAppId() {
    var match = window.location.pathname.match(/^\/apps\/([^/]+)/);
    return match ? decodeURIComponent(match[1]) : "";
  }

  function renderCatalog(apps) {
    var current = null;
    list.replaceChildren();

    apps.forEach(function (app) {
      if (app && app.id === currentId) current = app;
      list.appendChild(appLink(app));
    });

    if (!apps.length) {
      var empty = document.createElement("div");
      empty.className = "app-empty";
      empty.textContent = "No apps installed";
      list.appendChild(empty);
    }

    if (!current) {
      showError("App not found");
      return;
    }

    setTitle(current.name || current.id);
  }

  function appLink(app) {
    var id = app && app.id ? String(app.id) : "";
    var name = app && app.name ? String(app.name) : id || "Unnamed app";
    var root = app && app.has_ui
      ? document.createElement("a")
      : document.createElement("div");
    root.className = "app-link";
    if (id === currentId) {
      root.className += " selected";
      root.setAttribute("aria-current", "page");
    }
    if (app && app.has_ui) {
      root.href = "/apps/" + encodeURIComponent(id) + "/";
    } else {
      root.className += " disabled";
    }

    root.appendChild(window.terraneAppIcon(id));

    var text = document.createElement("span");
    text.className = "app-link-text";

    var label = document.createElement("span");
    label.textContent = name;
    text.appendChild(label);

    var meta = document.createElement("small");
    meta.textContent = app && app.has_ui ? id : id + " - no UI";
    text.appendChild(meta);

    root.appendChild(text);
    return root;
  }

  function setTitle(name) {
    var pageTitle = name + " - Terrane";
    document.title = pageTitle;
    title.textContent = name;
    frame.title = name;
  }

  function bindDesktopInfo() {
    if (!infoButton || !infoPanel) return;

    infoButton.addEventListener("click", function () {
      setInfoPanelOpen(infoPanel.hidden);
    });

    if (infoClose) {
      infoClose.addEventListener("click", function () {
        setInfoPanelOpen(false);
        infoButton.focus();
      });
    }

    document.addEventListener("keydown", function (event) {
      if (event.key === "Escape" && !infoPanel.hidden) {
        setInfoPanelOpen(false);
        infoButton.focus();
      }
    });
  }

  function setInfoPanelOpen(open) {
    infoPanel.hidden = !open;
    infoButton.setAttribute("aria-expanded", open ? "true" : "false");
  }

  function bindBridge() {
    window.addEventListener("message", function (event) {
      if (!frame || event.source !== frame.contentWindow) return;
      var message = event.data || {};
      if (!message || message.type !== "terrane:bridge:request") return;

      var route = bridgeRoute(message.kind);
      if (!route) {
        sendBridgeResponse(message.id, false, { error: "unsupported bridge request" });
        return;
      }

      postJson(route, message.body || {})
        .then(function (result) {
          sendBridgeResponse(message.id, result.ok, result.body);
        })
        .catch(function (error) {
          sendBridgeResponse(message.id, false, { error: errorMessage(error) });
        });
    });
  }

  function bridgeRoute(kind) {
    if (kind === "invoke") return "/apps/" + encodeURIComponent(currentId) + "/invoke";
    if (kind === "preview") return "/__terrane/previews";
    if (kind === "builderGenerate") return "/__terrane/builder/generate";
    return "";
  }

  function postJson(url, body) {
    return fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body || {}),
    })
      .then(function (response) {
        return response.text().then(function (text) {
          var parsed = {};
          if (text) {
            try {
              parsed = JSON.parse(text);
            } catch (error) {
              parsed = { error: text };
            }
          }
          if (!response.ok && !parsed.error) parsed.error = "HTTP " + response.status;
          return { ok: response.ok, body: parsed };
        });
      });
  }

  function sendBridgeResponse(id, ok, body) {
    if (!id || !frame || !frame.contentWindow) return;
    frame.contentWindow.postMessage(
      {
        type: "terrane:bridge:response",
        id: id,
        ok: !!ok,
        body: body || {},
      },
      "*"
    );
  }

  function errorMessage(error) {
    return error && error.message ? error.message : String(error || "request failed");
  }

  function showError(message) {
    list.replaceChildren();
    var error = document.createElement("div");
    error.className = "app-error";
    error.textContent = message;
    list.appendChild(error);
  }
})();
