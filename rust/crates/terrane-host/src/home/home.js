(function () {
  var list = document.getElementById("home-app-list");
  var adminLink = document.getElementById("home-admin-link");
  var config = readConfig();

  if (adminLink && config.adminHref) {
    adminLink.href = String(config.adminHref);
    adminLink.hidden = false;
  }

  if (config.catalog) {
    renderCatalogText(String(config.catalog));
  } else if (config.catalogUrl) {
    fetch(String(config.catalogUrl), { cache: "no-store" })
      .then(function (response) {
        if (!response.ok) throw new Error("cannot load apps");
        return response.text();
      })
      .then(renderCatalogText)
      .catch(function () {
        showError("Cannot load apps");
      });
  } else {
    showError("Cannot load apps");
  }

  function readConfig() {
    var node = document.getElementById("home-config");
    try {
      return JSON.parse(node ? node.textContent : "{}") || {};
    } catch (_) {
      return {};
    }
  }

  function renderCatalogText(text) {
    var catalog;
    try {
      catalog = JSON.parse(text) || {};
    } catch (_) {
      showError("Cannot load apps");
      return;
    }
    renderCatalog(Array.isArray(catalog.apps) ? catalog.apps : []);
  }

  function renderCatalog(apps) {
    list.replaceChildren();

    apps.forEach(function (app) {
      list.appendChild(appCard(app));
    });

    if (!apps.length) {
      var empty = document.createElement("div");
      empty.className = "app-empty";
      empty.textContent = "No apps installed";
      list.appendChild(empty);
    }
  }

  function appHref(id) {
    return String(config.appHref || "").replace("{id}", encodeURIComponent(id));
  }

  function appCard(app) {
    var id = app && app.id ? String(app.id) : "";
    var name = app && app.name ? String(app.name) : id || "Unnamed app";
    var openable = !!(app && app.has_ui && id && config.appHref);
    var root = openable
      ? document.createElement("a")
      : document.createElement("div");
    root.className = "app-card";
    if (openable) {
      root.href = appHref(id);
    } else {
      root.className += " disabled";
    }

    root.appendChild(window.terraneAppIcon(id));

    var text = document.createElement("span");
    text.className = "app-card-text";

    var label = document.createElement("span");
    label.textContent = name;
    text.appendChild(label);

    var meta = document.createElement("small");
    meta.textContent = app && app.has_ui ? id : id + " - no UI";
    text.appendChild(meta);

    root.appendChild(text);
    return root;
  }

  function showError(message) {
    list.replaceChildren();
    var error = document.createElement("div");
    error.className = "app-error";
    error.textContent = message;
    list.appendChild(error);
  }
})();
