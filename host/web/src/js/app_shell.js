(function () {
  var currentId = currentAppId();
  var list = document.getElementById("app-list");
  var title = document.getElementById("app-title");
  var frame = document.getElementById("app-frame");
  var infoButton = document.getElementById("desktop-info-button");
  var infoPanel = document.getElementById("desktop-info-panel");
  var infoClose = document.getElementById("desktop-info-close");
  var topbarApp = document.getElementById("topbar-app");
  var crumbApp = document.getElementById("crumb-app");
  var crumbSep = document.getElementById("crumb-sep");
  var crumbDoc = document.getElementById("crumb-doc");
  var userButton = document.getElementById("user-button");
  var userDropdown = document.getElementById("user-dropdown");
  var userName = document.getElementById("user-name");
  var userSubject = document.getElementById("user-subject");
  var menuSettings = document.getElementById("menu-settings");
  var menuAuth = document.getElementById("menu-auth");
  var settingsPanel = document.getElementById("settings-panel");
  var settingsClose = document.getElementById("settings-close");

  var DOC_KEY = "terrane.doc." + currentId;
  var THEME_KEY = "terrane.theme";
  var SIGNED_OUT_KEY = "terrane.signedOut";
  var appDisplayName = currentId;
  var settingsOpen = false;
  var currentTheme = "system";
  var identity = { name: "Local user", subject: "", source: "", locked: null };

  if (!currentId) {
    showError("No app selected");
    return;
  }

  var lastCatalogText = "";

  bindDesktopInfo();
  bindBridge();
  bindTopbar();
  frame.src = "/apps/" + encodeURIComponent(currentId) + "/__terrane/frame/";

  loadCatalog();
  // Dev iteration: keep the sidebar in sync with the catalog (new dev apps
  // appear, renames apply) and reload the frame when the app's bundle
  // changes. The frame watches from the shell because the sandboxed iframe
  // has an opaque origin and cannot fetch live-version itself.
  if (window.__terraneLiveReload) {
    setInterval(loadCatalog, 3000);
    setInterval(watchAppVersion, 1000);
  }

  var appVersion = null;
  function watchAppVersion() {
    fetch("/apps/" + encodeURIComponent(currentId) + "/__terrane/live-version", {
      cache: "no-store",
    })
      .then(function (response) {
        if (!response.ok) throw new Error("live-version");
        return response.json();
      })
      .then(function (payload) {
        if (!payload.version) return;
        if (appVersion === null) {
          appVersion = payload.version;
          return;
        }
        if (appVersion !== payload.version) {
          appVersion = payload.version;
          frame.src = "/apps/" + encodeURIComponent(currentId) + "/__terrane/frame/";
        }
      })
      .catch(function () {});
  }

  function loadCatalog() {
    fetch("/apps", { cache: "no-store" })
      .then(function (response) {
        if (!response.ok) throw new Error("cannot load apps");
        return response.text();
      })
      .then(function (text) {
        if (text === lastCatalogText) return;
        lastCatalogText = text;
        var catalog = {};
        try {
          catalog = JSON.parse(text) || {};
        } catch (_) {
          catalog = {};
        }
        renderCatalog(Array.isArray(catalog.apps) ? catalog.apps : []);
      })
      .catch(function () {
        if (!lastCatalogText) showError("Cannot load apps");
      });
  }

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
    appDisplayName = name;
    if (!settingsOpen) crumbApp.textContent = name;
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
      if (message && message.type === "terrane:document:set") {
        setDocName(message.name, true);
        return;
      }
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
    if (kind === "builderStatus") return "/__terrane/builder/status";
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

  // Top bar <-> app protocol (postMessage, best-effort):
  //   shell -> app: {type: "terrane:theme", theme: "system"|"light"|"dark"}
  //                 {type: "terrane:document", name}  (sent on frame load and
  //                 on rename)
  //   app -> shell: {type: "terrane:document:set", name}  (rename the crumb)
  function bindTopbar() {
    if (!topbarApp || !crumbDoc || !userButton) return;
    if (window.terraneAppIcon) topbarApp.appendChild(window.terraneAppIcon(currentId));
    crumbApp.textContent = appDisplayName;
    crumbDoc.textContent = storedDocName();
    bindDocEditing();
    bindUserMenu();
    bindSettings();
    initTheme();
    loadIdentity();
    updateAuthUi();
    // The first applyTheme/setDocName run before the frame navigates; hand the
    // current state to the app once its document is actually loaded.
    frame.addEventListener("load", function () {
      sendToFrame({ type: "terrane:theme", theme: currentTheme });
      sendToFrame({ type: "terrane:document", name: storedDocName() });
    });
  }

  function storedDocName() {
    var stored = "";
    try {
      stored = window.localStorage.getItem(DOC_KEY) || "";
    } catch (_) {}
    return stored || "Untitled";
  }

  function setDocName(raw, fromApp) {
    // Strip control/format characters (bidi overrides, zero-width) so an
    // app-supplied name cannot spoof the trusted breadcrumb chrome.
    var name = String(raw == null ? "" : raw)
      .replace(/[\u0000-\u001f\u007f-\u009f\u200b-\u200f\u2028-\u202e\u2066-\u2069\ufeff]/g, "")
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, 120);
    if (!name) name = "Untitled";
    var changed = name !== storedDocName();
    crumbDoc.textContent = name;
    if (!changed) return;
    try {
      window.localStorage.setItem(DOC_KEY, name);
    } catch (_) {}
    if (!fromApp) sendToFrame({ type: "terrane:document", name: name });
  }

  function bindDocEditing() {
    crumbDoc.addEventListener("keydown", function (event) {
      if (event.isComposing || event.keyCode === 229) return;
      if (event.key === "Enter") {
        event.preventDefault();
        crumbDoc.blur();
      }
      if (event.key === "Escape") {
        crumbDoc.textContent = storedDocName();
        crumbDoc.blur();
      }
    });
    crumbDoc.addEventListener("blur", function () {
      setDocName(crumbDoc.textContent);
    });
  }

  function sendToFrame(message) {
    if (frame && frame.contentWindow) frame.contentWindow.postMessage(message, "*");
  }

  function bindUserMenu() {
    userButton.addEventListener("click", function (event) {
      event.stopPropagation();
      setDropdownOpen(userDropdown.hidden);
    });
    document.addEventListener("click", function (event) {
      if (userDropdown.hidden) return;
      if (userDropdown.contains(event.target) || event.target === userButton) return;
      setDropdownOpen(false);
    });
    document.addEventListener("keydown", function (event) {
      if (event.key === "Escape" && !userDropdown.hidden) {
        setDropdownOpen(false);
        userButton.focus();
      }
    });
    // Clicks inside the app iframe never reach this document; closing on
    // window blur covers focus moving into the frame.
    window.addEventListener("blur", function () {
      if (!userDropdown.hidden) setDropdownOpen(false);
    });
    menuAuth.addEventListener("click", function () {
      setSignedOut(!isSignedOut());
      setDropdownOpen(false);
    });
  }

  function setDropdownOpen(open) {
    userDropdown.hidden = !open;
    userButton.setAttribute("aria-expanded", open ? "true" : "false");
  }

  function isSignedOut() {
    try {
      return window.localStorage.getItem(SIGNED_OUT_KEY) === "1";
    } catch (_) {
      return false;
    }
  }

  function setSignedOut(out) {
    try {
      if (out) {
        window.localStorage.setItem(SIGNED_OUT_KEY, "1");
      } else {
        window.localStorage.removeItem(SIGNED_OUT_KEY);
      }
    } catch (_) {}
    updateAuthUi();
  }

  function updateAuthUi() {
    var out = isSignedOut();
    userButton.textContent = out ? "?" : (identity.name || "L").charAt(0);
    userButton.dataset.signedOut = out ? "true" : "false";
    menuAuth.textContent = out ? "Log in" : "Log out";
    userName.textContent = out ? "Signed out" : identity.name;
    userSubject.textContent = out ? "Local session only" : identity.subject;
    setSettingsField("settings-user", out ? "Signed out" : identity.name);
    setSettingsField("settings-subject", out ? "-" : identity.subject || "-");
    setSettingsField("settings-source", out ? "-" : identity.source || "-");
    setSettingsField(
      "settings-session",
      out || identity.locked == null ? "-" : identity.locked ? "Locked" : "Unlocked"
    );
  }

  function loadIdentity() {
    fetch("/__terrane/admin/session", {
      cache: "no-store",
      headers: { "X-Terrane-Admin": "local-admin" },
    })
      .then(function (response) {
        if (!response.ok) throw new Error("no session");
        return response.json();
      })
      .then(function (session) {
        identity.subject = String(session.subject || "");
        identity.source = String(session.source || "");
        identity.locked = !!session.locked;
        identity.name = displayName(identity.subject);
        updateAuthUi();
      })
      .catch(function () {
        updateAuthUi();
      });
  }

  function displayName(subject) {
    var raw = subject.indexOf(":") >= 0 ? subject.slice(subject.indexOf(":") + 1) : subject;
    if (!raw) return "Local user";
    return raw.replace(/[-_]+/g, " ").replace(/\b\w/g, function (c) {
      return c.toUpperCase();
    });
  }

  function setSettingsField(id, value) {
    var field = document.getElementById(id);
    if (field) field.textContent = value;
  }

  function bindSettings() {
    menuSettings.addEventListener("click", function () {
      setDropdownOpen(false);
      setSettingsOpen(true);
    });
    settingsClose.addEventListener("click", function () {
      setSettingsOpen(false);
    });
  }

  function setSettingsOpen(open) {
    settingsOpen = open;
    settingsPanel.hidden = !open;
    frame.hidden = open;
    crumbApp.textContent = open ? "Settings" : appDisplayName;
    crumbSep.hidden = open;
    crumbDoc.hidden = open;
  }

  function initTheme() {
    var saved = "";
    try {
      saved = window.localStorage.getItem(THEME_KEY) || "";
    } catch (_) {}
    applyTheme(saved === "light" || saved === "dark" ? saved : "system");
    Array.prototype.forEach.call(
      document.querySelectorAll(".theme-option"),
      function (button) {
        button.addEventListener("click", function () {
          applyTheme(button.dataset.theme || "system");
        });
      }
    );
  }

  function applyTheme(theme) {
    currentTheme = theme;
    document.documentElement.style.colorScheme = theme === "system" ? "" : theme;
    try {
      if (theme === "system") {
        window.localStorage.removeItem(THEME_KEY);
      } else {
        window.localStorage.setItem(THEME_KEY, theme);
      }
    } catch (_) {}
    Array.prototype.forEach.call(
      document.querySelectorAll(".theme-option"),
      function (button) {
        button.classList.toggle("active", (button.dataset.theme || "system") === theme);
      }
    );
    sendToFrame({ type: "terrane:theme", theme: theme });
  }

  function showError(message) {
    list.replaceChildren();
    var error = document.createElement("div");
    error.className = "app-error";
    error.textContent = message;
    list.appendChild(error);
  }
})();
