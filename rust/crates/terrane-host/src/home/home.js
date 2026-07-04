(function () {
  var list = document.getElementById("home-app-list");
  var adminLink = document.getElementById("home-admin-link");
  var config = readConfig();
  var messages = config.messages && typeof config.messages === "object" ? config.messages : {};

  // Localize the static chrome (the host injects the negotiated locale + the
  // `system` bundle); the English text in the markup is the fallback.
  function t(key, fallback) {
    return Object.prototype.hasOwnProperty.call(messages, key) ? messages[key] : fallback;
  }
  document.documentElement.dir = config.dir === "rtl" ? "rtl" : "ltr";
  Array.prototype.forEach.call(document.querySelectorAll("[data-i18n]"), function (el) {
    el.textContent = t(el.getAttribute("data-i18n"), el.textContent.trim());
  });

  if (adminLink && config.adminHref) {
    adminLink.href = String(config.adminHref);
    adminLink.hidden = false;
  }

  var lastCatalogText = "";

  if (config.catalog) {
    renderCatalogText(String(config.catalog));
  } else if (config.catalogUrl) {
    loadCatalog();
    var pollMs = Number(config.catalogPollMs || 0);
    if (pollMs > 0) setInterval(loadCatalog, pollMs);
  } else {
    showError(t("system.home.loadError", "Cannot load apps"));
  }

  function loadCatalog() {
    fetch(String(config.catalogUrl), { cache: "no-store" })
      .then(function (response) {
        if (!response.ok) throw new Error("cannot load apps");
        return response.text();
      })
      .then(function (text) {
        if (text === lastCatalogText) return;
        lastCatalogText = text;
        renderCatalogText(text);
      })
      .catch(function () {
        if (!lastCatalogText) showError(t("system.home.loadError", "Cannot load apps"));
      });
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
      showError(t("system.home.loadError", "Cannot load apps"));
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
      empty.textContent = t("system.sidebar.empty", "No apps installed");
      list.appendChild(empty);
    }
  }

  function appHref(id) {
    return String(config.appHref || "").replace("{id}", encodeURIComponent(id));
  }

  function appCard(app) {
    var id = app && app.id ? String(app.id) : "";
    var name = app && app.name ? String(app.name) : id || t("system.home.unnamed", "Unnamed app");
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

    root.appendChild(window.terraneAppIcon(app));

    var text = document.createElement("span");
    text.className = "app-card-text";

    var label = document.createElement("span");
    label.textContent = name;
    text.appendChild(label);

    var meta = document.createElement("small");
    meta.textContent = app && app.has_ui ? id : id + " — " + t("system.home.noUi", "no UI");
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
