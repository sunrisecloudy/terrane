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

    var label = document.createElement("span");
    label.textContent = name;
    root.appendChild(label);

    var meta = document.createElement("small");
    meta.textContent = app && app.has_ui ? id : id + " - no UI";
    root.appendChild(meta);

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

  function showError(message) {
    list.replaceChildren();
    var error = document.createElement("div");
    error.className = "app-error";
    error.textContent = message;
    list.appendChild(error);
  }
})();
