(function () {
  var shellMode = window.__terraneShellMode || "app";
  var isAdmin = shellMode === "admin";
  var appFrameOrigin = window.__terraneAppFrameOrigin || "";
  var currentId = currentAppId();
  var list = document.getElementById("app-list");
  var adminLink =
    document.getElementById("admin-console-link") ||
    document.getElementById("admin-link");
  var title = document.getElementById("app-title");
  var frame = document.getElementById("app-frame");
  var adminPanel = document.getElementById("admin-panel");
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
  var menuPremium = document.getElementById("menu-premium");
  var premiumSection = document.getElementById("premium-section");
  var premiumList = document.getElementById("premium-list");
  var settingsPanel = document.getElementById("settings-panel");
  var settingsClose = document.getElementById("settings-close");
  var sttMicButton = document.getElementById("stt-mic-button");
  var sttListeningBadge = document.getElementById("stt-listening-badge");

  var DOC_KEY = "terrane.doc." + (currentId || "admin");
  var STT_CONSENT_KEY = "terrane.stt.consent";
  var STT_NOTE_KEY = "terrane.stt.note." + (currentId || "app");
  var THEME_KEY = "terrane.theme";
  var SIGNED_OUT_KEY = "terrane.signedOut";
  // Optional Terrane Premium sign-in (Google). The host injects the control
  // plane URL; unset keeps the shell local-only. The session payload is the
  // one premium's /auth/google/callback postMessages back to this origin.
  var PREMIUM_SESSION_KEY = "terranePremiumSession";
  var premiumUrl =
    typeof window.__terranePremiumUrl === "string"
      ? window.__terranePremiumUrl.replace(/\/+$/, "")
      : "";
  var premiumSession = null;
  // The premium catalog is public metadata; entries already installed on
  // this host stay in the local list only, the rest render as a "Premium"
  // sidebar section linking to the control plane.
  var premiumApps = [];
  var localAppIds = {};
  var activePremiumAppId = "";
  var appDisplayName = currentId;
  var settingsOpen = false;
  var currentTheme = "system";
  var identity = { name: "Local user", subject: "", source: "", locked: null };
  // A fresh nonce per frame load, handed to the app via the frame URL. The
  // app echoes it on every message; a page the app navigates its own frame to
  // loads without it, so it cannot drive the bridge or the breadcrumb.
  var frameNonce = "";

  // Localization: the server injects the negotiated locale, its direction, the
  // shell-chrome bundle, and the app frame's bundle. shellT() localizes chrome;
  // the app frame's bundle is pushed to it over the same channel as the theme.
  var shellLocale =
    typeof window.__terraneLocale === "string" ? window.__terraneLocale : "en";
  var shellDir = window.__terraneDir === "rtl" ? "rtl" : "ltr";
  var shellMessages =
    window.__terraneMessages && typeof window.__terraneMessages === "object"
      ? window.__terraneMessages
      : {};
  var appMessages =
    window.__terraneAppMessages && typeof window.__terraneAppMessages === "object"
      ? window.__terraneAppMessages
      : {};
  // Endonyms for the language picker; codes must match terrane-i18n::SUPPORTED.
  var LANGUAGES = [
    ["en", "English"],
    ["es", "Español"],
    ["zh-Hans", "简体中文"],
    ["ar", "العربية"],
    ["pt-BR", "Português (Brasil)"],
    ["fr", "Français"],
    ["de", "Deutsch"],
    ["ja", "日本語"],
    ["id", "Bahasa Indonesia"],
    ["th-TH", "ไทย"],
    ["ko", "한국어"],
    ["vi", "Tiếng Việt"],
  ];

  function shellT(key, fallback) {
    return Object.prototype.hasOwnProperty.call(shellMessages, key)
      ? shellMessages[key]
      : fallback == null
        ? key
        : fallback;
  }

  // One-pass sweep: every [data-i18n] node's text becomes its localized string.
  // The English text stays in the HTML as the pre-bundle fallback.
  function localizeChrome() {
    var nodes = document.querySelectorAll("[data-i18n]");
    for (var i = 0; i < nodes.length; i++) {
      var key = nodes[i].getAttribute("data-i18n");
      nodes[i].textContent = shellT(key, nodes[i].textContent.trim());
    }
  }

  registerProtocolHandler();
  if (consumeOpenHash()) return;
  if (!currentId && !isAdmin) {
    showError("No app selected");
    return;
  }

  var lastCatalogText = "";

  localizeChrome();
  bindDesktopInfo();
  bindBridge();
  bindTopbar();
  bindSttMic();
  bindAgents();
  bindPremium();
  bindLanguagePicker();
  setAdminMode(isAdmin);
  if (isAdmin) {
    setTitle(shellT("system.sidebar.admin", "Admin Console"));
  } else {
    loadFrame();
  }

  function loadFrame() {
    activePremiumAppId = "";
    frameNonce = randomNonce();
    frame.src =
      appFrameOrigin +
      "/apps/" +
      encodeURIComponent(currentId) +
      "/__terrane/frame/?__terrane_n=" +
      encodeURIComponent(frameNonce);
  }

  function randomNonce() {
    try {
      var bytes = new Uint8Array(16);
      window.crypto.getRandomValues(bytes);
      return Array.prototype.map
        .call(bytes, function (b) {
          return ("0" + b.toString(16)).slice(-2);
        })
        .join("");
    } catch (_) {
      return "n" + String(Date.now()) + String(Math.floor(Math.random() * 1e9));
    }
  }

  loadCatalog();
  // Dev iteration: keep the sidebar in sync with the catalog (new dev apps
  // appear, renames apply) and reload the frame when the app's bundle
  // changes. The frame watches from the shell because the sandboxed iframe
  // has an opaque origin and cannot fetch live-version itself.
  if (window.__terraneLiveReload) {
    setInterval(loadCatalog, 3000);
    if (!isAdmin) setInterval(watchAppVersion, 1000);
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
          loadFrame();
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

  function consumeOpenHash() {
    var match = window.location.hash.match(/^#open\/([^/?#]+)/);
    if (!match) return false;
    var app = decodeURIComponent(match[1]);
    if (!/^[A-Za-z0-9_-]+$/.test(app)) {
      showError("Invalid app link");
      return true;
    }
    window.location.replace("/apps/" + encodeURIComponent(app) + "/");
    return true;
  }

  function registerProtocolHandler() {
    if (!navigator.registerProtocolHandler) return;
    try {
      navigator.registerProtocolHandler(
        "web+terrane",
        window.location.origin + "/#open/%s"
      );
    } catch (_) {}
  }

  function renderCatalog(apps) {
    var current = null;
    list.replaceChildren();
    localAppIds = {};

    apps.forEach(function (app) {
      if (app && app.id === currentId) current = app;
      if (app && app.id) localAppIds[app.id] = true;
      list.appendChild(appLink(app));
    });

    if (!apps.length) {
      var empty = document.createElement("div");
      empty.className = "app-empty";
      empty.textContent = shellT("system.sidebar.empty", "No apps installed");
      list.appendChild(empty);
    }

    // A premium app already installed locally drops out of the premium
    // section, so re-render it whenever the local catalog changes.
    renderPremiumCatalog();

    if (activePremiumAppId) return;

    if (!current) {
      if (isAdmin) {
        setTitle(shellT("system.sidebar.admin", "Admin Console"));
        return;
      }
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
    if (!activePremiumAppId && id === currentId) {
      root.className += " selected";
      root.setAttribute("aria-current", "page");
    }
    if (app && app.has_ui) {
      root.href = "/apps/" + encodeURIComponent(id) + "/";
    } else {
      root.className += " disabled";
    }

    root.appendChild(window.terraneAppIcon(app));

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

  function setAdminMode(admin) {
    if (adminLink) {
      adminLink.classList.toggle("selected", admin);
      if (admin) {
        adminLink.setAttribute("aria-current", "page");
      } else {
        adminLink.removeAttribute("aria-current");
      }
    }
    if (adminPanel) adminPanel.hidden = !admin;
    if (frame) frame.hidden = admin;
    if (crumbDoc) crumbDoc.hidden = admin;
    if (crumbSep) crumbSep.hidden = admin;
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
    if (isAdmin) return;
    window.addEventListener("message", function (event) {
      if (!frame || event.source !== frame.contentWindow) return;
      var message = event.data || {};
      // Initial sync: reply with the current theme + document only to a frame
      // that proves it is the one we loaded (carries the per-load nonce), so a
      // navigated-to page cannot solicit the user's document name and theme.
      if (message && message.type === "terrane:hello") {
        if (message.nonce !== frameNonce) return;
        sendToFrame({ type: "terrane:theme", theme: currentTheme });
        sendToFrame({ type: "terrane:document", name: storedDocName() });
        sendFrameLocale();
        return;
      }
      // App-driven messages must carry the per-load nonce.
      if (
        message &&
        (message.type === "terrane:document:set" ||
          message.type === "terrane:bridge:request") &&
        message.nonce !== frameNonce
      ) {
        return;
      }
      if (message && message.type === "terrane:document:set") {
        setDocName(message.name, true);
        return;
      }
      if (message && message.type === "terrane:stt:deliver") {
        deliverSttSink(message.sink, message.text);
        return;
      }
      if (!message || message.type !== "terrane:bridge:request") return;

      var body = message.body || {};
      var route;
      if (message.kind === "previewInvoke") {
        // A relayed preview-frame invoke: route to that preview's backend,
        // not the current app's.
        var previewId = String(body.previewId || "");
        if (!previewId) {
          sendBridgeResponse(message.id, false, { error: "missing previewId" });
          return;
        }
        route = "/__terrane/previews/" + encodeURIComponent(previewId) + "/invoke";
        body = { verb: String(body.verb || ""), args: body.args || [] };
      } else {
        route = bridgeRoute(message.kind);
      }
      if (!route) {
        sendBridgeResponse(message.id, false, { error: "unsupported bridge request" });
        return;
      }

      postJson(route, body)
        .then(function (result) {
          if (isPermissionRequired(result)) {
            // Host-owned elicitation: hold the app's promise, ask the user,
            // and on approval retry the same request so the app just gets
            // its output. Progress tells the waiting frame to extend its
            // bridge timeout — a human decision doesn't fit inside 30s.
            sendBridgeProgress(message.id);
            return promptForPermission(result, function () {
              return postJson(route, body);
            });
          }
          if (isPickRequired(result)) {
            // Powerbox: the app asked to hand off over an interface with no
            // chosen target. Render the candidate apps, record the pick, retry.
            sendBridgeProgress(message.id);
            return promptForPick(result, function () {
              return postJson(route, body);
            });
          }
          return result;
        })
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

  function sendBridgeProgress(id) {
    if (!id || !frame || !frame.contentWindow) return;
    frame.contentWindow.postMessage(
      { type: "terrane:bridge:progress", id: id },
      "*"
    );
  }

  function errorMessage(error) {
    return error && error.message ? error.message : String(error || "request failed");
  }

  // ---- In-session permission prompts -------------------------------------
  // A 403 permission_required from an invoke opens a host-owned bar.
  // Approve grants via the admin route and retries the original request;
  // deny answers the app with the original permission error.

  var permDialog = document.getElementById("perm-dialog");
  var permApp = document.getElementById("perm-app");
  var permResources = document.getElementById("perm-resources");
  var permError = document.getElementById("perm-error");
  var permApprove = document.getElementById("perm-approve");
  var permDeny = document.getElementById("perm-deny");
  var permQueue = [];
  var permActive = null;

  function isPermissionRequired(result) {
    return (
      !result.ok &&
      result.body &&
      result.body.type === "permission_required" &&
      typeof result.body.requestId === "string" &&
      result.body.requestId !== ""
    );
  }

  function promptForPermission(result, retry) {
    return new Promise(function (resolve) {
      var entry = { required: result.body, original: result, retry: retry, resolve: resolve };
      var same =
        permActive && permActive.required.requestId === entry.required.requestId
          ? permActive
          : null;
      for (var i = 0; !same && i < permQueue.length; i++) {
        if (permQueue[i].required.requestId === entry.required.requestId) same = permQueue[i];
      }
      if (same) {
        same.followers.push(entry);
        return;
      }
      entry.followers = [];
      permQueue.push(entry);
      pumpPermissionQueue();
    });
  }

  function pumpPermissionQueue() {
    if (permActive || !permQueue.length || !permDialog) return;
    permActive = permQueue.shift();
    var required = permActive.required;
    permApp.textContent = required.appName || required.app || "This app";
    permResources.textContent = (required.missingResources || []).join(", ") || "a resource";
    permError.hidden = true;
    permError.textContent = "";
    setPermBusy(false);
    permDialog.hidden = false;
    permApprove.focus();
  }

  function setPermBusy(busy) {
    permApprove.disabled = busy;
    permDeny.disabled = busy;
  }

  function settlePermission(makeResult) {
    var entry = permActive;
    permActive = null;
    permDialog.hidden = true;
    var all = [entry].concat(entry.followers);
    all.forEach(function (follower) {
      makeResult(follower).then(follower.resolve);
    });
    pumpPermissionQueue();
  }

  function decideActivePermission(action) {
    if (!permActive) return;
    setPermBusy(true);
    var requestId = permActive.required.requestId;
    fetch("/__terrane/admin/requests/" + encodeURIComponent(requestId) + "/" + action, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "X-Terrane-Admin": "local-admin",
      },
      body: "{}",
    })
      .then(function (response) {
        if (!response.ok) {
          return response.text().then(function (text) {
            var detail = "";
            try {
              detail = (JSON.parse(text) || {}).error || "";
            } catch (_) {}
            throw new Error(detail || "HTTP " + response.status);
          });
        }
        if (action === "approve") {
          settlePermission(function (follower) {
            return follower.retry();
          });
        } else {
          settlePermission(function (follower) {
            return Promise.resolve(follower.original);
          });
        }
      })
      .catch(function (error) {
        setPermBusy(false);
        permError.textContent = "Cannot " + action + ": " + errorMessage(error);
        permError.hidden = false;
      });
  }

  if (permDialog) {
    permApprove.addEventListener("click", function () {
      decideActivePermission("approve");
    });
    permDeny.addEventListener("click", function () {
      decideActivePermission("deny");
    });
    document.addEventListener("keydown", function (event) {
      if (event.key === "Escape" && !permDialog.hidden && !permApprove.disabled) {
        decideActivePermission("deny");
      }
    });
  }

  // ---- In-session interop picker (powerbox) ------------------------------
  // A 403 interop_pick_required from an invoke opens a host-owned chooser.
  // Choosing records the caller -> interface -> target grant and retries;
  // cancel answers the app with the original pick-required error.

  var pickDialog = document.getElementById("pick-dialog");
  var pickApp = document.getElementById("pick-app");
  var pickInterface = document.getElementById("pick-interface");
  var pickList = document.getElementById("pick-list");
  var pickEmpty = document.getElementById("pick-empty");
  var pickError = document.getElementById("pick-error");
  var pickCancel = document.getElementById("pick-cancel");
  var pickConfirm = document.getElementById("pick-confirm");
  var pickQueue = [];
  var pickActive = null;
  var pickSelected = "";

  function isPickRequired(result) {
    return (
      !result.ok &&
      result.body &&
      result.body.type === "interop_pick_required" &&
      typeof result.body.interface === "string" &&
      result.body.interface !== ""
    );
  }

  function promptForPick(result, retry) {
    return new Promise(function (resolve) {
      pickQueue.push({ required: result.body, original: result, retry: retry, resolve: resolve });
      pumpPickQueue();
    });
  }

  function pumpPickQueue() {
    if (pickActive || !pickQueue.length || !pickDialog) return;
    pickActive = pickQueue.shift();
    var required = pickActive.required;
    pickApp.textContent = required.app || "An app";
    pickInterface.textContent = required.interface || "an interface";
    pickError.hidden = true;
    pickError.textContent = "";
    pickSelected = "";
    renderPickOptions(required.candidates || []);
    setPickBusy(false);
    pickDialog.hidden = false;
    if (pickCancel) pickCancel.focus();
  }

  function renderPickOptions(candidates) {
    pickList.textContent = "";
    var hasCandidates = candidates.length > 0;
    pickEmpty.hidden = hasCandidates;
    pickConfirm.disabled = !hasCandidates;
    candidates.forEach(function (candidate) {
      var id = String(candidate.id || "");
      if (!id) return;
      var option = document.createElement("button");
      option.type = "button";
      option.className = "pick-option";
      option.setAttribute("role", "radio");
      option.setAttribute("aria-checked", "false");
      option.dataset.target = id;
      option.textContent = candidate.name ? candidate.name + " (" + id + ")" : id;
      option.addEventListener("click", function () {
        selectPickOption(id);
      });
      pickList.appendChild(option);
    });
  }

  function selectPickOption(target) {
    pickSelected = target;
    var options = pickList.querySelectorAll(".pick-option");
    for (var i = 0; i < options.length; i++) {
      options[i].setAttribute(
        "aria-checked",
        options[i].dataset.target === target ? "true" : "false"
      );
    }
    pickConfirm.disabled = !target;
  }

  function setPickBusy(busy) {
    pickConfirm.disabled = busy || !pickSelected;
    pickCancel.disabled = busy;
  }

  function settlePick(makeResult) {
    var entry = pickActive;
    pickActive = null;
    pickDialog.hidden = true;
    makeResult(entry).then(entry.resolve);
    pumpPickQueue();
  }

  function confirmActivePick() {
    if (!pickActive || !pickSelected) return;
    setPickBusy(true);
    var required = pickActive.required;
    fetch("/__terrane/admin/interop/pick", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "X-Terrane-Admin": "local-admin",
      },
      body: JSON.stringify({
        app: required.app,
        interface: required.interface,
        target: pickSelected,
      }),
    })
      .then(function (response) {
        if (!response.ok) {
          return response.text().then(function (text) {
            var detail = "";
            try {
              detail = (JSON.parse(text) || {}).error || "";
            } catch (_) {}
            throw new Error(detail || "HTTP " + response.status);
          });
        }
        settlePick(function (entry) {
          return entry.retry();
        });
      })
      .catch(function (error) {
        setPickBusy(false);
        pickError.textContent = "Cannot choose: " + errorMessage(error);
        pickError.hidden = false;
      });
  }

  function cancelActivePick() {
    if (!pickActive) return;
    settlePick(function (entry) {
      return Promise.resolve(entry.original);
    });
  }

  if (pickDialog) {
    pickConfirm.addEventListener("click", confirmActivePick);
    pickCancel.addEventListener("click", cancelActivePick);
    document.addEventListener("keydown", function (event) {
      if (event.key === "Escape" && !pickDialog.hidden && !pickCancel.disabled) {
        cancelActivePick();
      }
    });
  }

  // Top bar <-> app protocol (postMessage, best-effort):
  //   shell -> app: {type: "terrane:theme", theme: "system"|"light"|"dark"}
  //                 {type: "terrane:document", name}  (sent on frame load and
  //                 on rename)
  //   app -> shell: {type: "terrane:document:set", name}  (rename the crumb)
  // ---- Shell-owned STT mic capture (outside the sandboxed iframe) ----------
  var sttCapture = null;

  function bindSttMic() {
    if (!sttMicButton || isAdmin || !currentId) return;
    sttMicButton.hidden = false;
    sttMicButton.addEventListener("click", function () {
      if (sttCapture) stopSttMic("stopped");
      else startSttMic();
    });
    window.addEventListener("beforeunload", function () {
      stopSttMic("host-exit");
    });
    window.addEventListener("pagehide", function () {
      stopSttMic("host-exit");
    });
  }

  function ensureSttConsent() {
    return new Promise(function (resolve, reject) {
      try {
        if (window.localStorage.getItem(STT_CONSENT_KEY) === "granted") {
          resolve();
          return;
        }
      } catch (_) {}
      var ok = window.confirm(
        "Terrane will use your microphone for on-device speech transcription. " +
          "Only finalized text is recorded in the log — not raw audio. Allow listening?"
      );
      if (!ok) {
        reject(new Error("stt consent denied"));
        return;
      }
      try {
        window.localStorage.setItem(STT_CONSENT_KEY, "granted");
      } catch (_) {}
      resolve();
    });
  }

  function deliverSttSink(sink, text) {
    var value = String(text == null ? "" : text);
    if (!value) return;
    var kind = String(sink || "").trim();
    if (kind === "clipboard") {
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(value).catch(function () {});
      }
      return;
    }
    if (kind === "field") {
      sendToFrame({ type: "terrane:stt:field", text: value });
      return;
    }
    if (kind.indexOf("app:") === 0) {
      var targetId = kind.slice(4).trim();
      if (!targetId) return;
      postJson("/apps/" + encodeURIComponent(targetId) + "/invoke", {
        verb: "sttDeliver",
        args: [value],
      }).catch(function () {});
      return;
    }
    if (kind === "note") {
      try {
        window.localStorage.setItem(STT_NOTE_KEY, value);
      } catch (_) {}
      return;
    }
  }

  function startSttMic() {
    var sessionId = randomNonce();
    sttMicButton.disabled = true;
    ensureSttConsent()
      .then(function () {
        return fetchSttConfig();
      })
      .then(function (config) {
        return openSttSession(sessionId).then(function (open) {
          return {
            wsUrl: open.wsUrl || config.wsUrl,
            sessionId: open.sessionId || sessionId,
          };
        });
      })
      .then(function (opened) {
        return navigator.mediaDevices.getUserMedia({ audio: true }).then(function (stream) {
          return startSttAudio(opened, stream);
        });
      })
      .then(function (capture) {
        sttCapture = capture;
        sttMicButton.disabled = false;
        sttMicButton.setAttribute("aria-pressed", "true");
        sttMicButton.title = "Disable microphone";
        if (sttListeningBadge) sttListeningBadge.hidden = false;
        sendToFrame({ type: "terrane:stt", status: "open", sessionId: capture.sessionId });
      })
      .catch(function (error) {
        sttMicButton.disabled = false;
        console.warn("stt mic failed:", errorMessage(error));
      });
  }

  function fetchSttConfig() {
    return fetch("/__terrane/stt/config", { cache: "no-store" }).then(function (response) {
      if (!response.ok) throw new Error("stt config");
      return response.json();
    });
  }

  function openSttSession(sessionId) {
    return postJsonAdmin("/__terrane/admin/stt/open", {
      app: currentId,
      sessionId: sessionId,
    }).then(function (result) {
      if (!result.ok) throw new Error(errorMessage(result.body && result.body.error));
      return result.body || {};
    });
  }

  function startSttAudio(opened, stream) {
    var audioContext = new AudioContext({ sampleRate: 16000 });
    return audioContext.audioWorklet
      .addModule("/__terrane/stt/worklet.js")
      .then(function () {
        var source = audioContext.createMediaStreamSource(stream);
        var worklet = new AudioWorkletNode(audioContext, "stt-capture-processor");
        var ws = new WebSocket(
          opened.wsUrl + "?session=" + encodeURIComponent(opened.sessionId)
        );
        ws.binaryType = "arraybuffer";
        worklet.port.onmessage = function (event) {
          if (!ws || ws.readyState !== WebSocket.OPEN) return;
          ws.send(event.data);
        };
        source.connect(worklet);
        return {
          sessionId: opened.sessionId,
          ws: ws,
          stream: stream,
          context: audioContext,
          worklet: worklet,
        };
      });
  }

  function stopSttMic(reason) {
    if (!sttCapture) return;
    var capture = sttCapture;
    sttCapture = null;
    try {
      if (capture.ws && capture.ws.readyState === WebSocket.OPEN) capture.ws.close();
    } catch (_) {}
    try {
      if (capture.worklet) capture.worklet.disconnect();
    } catch (_) {}
    try {
      if (capture.context) capture.context.close();
    } catch (_) {}
    if (capture.stream) {
      capture.stream.getTracks().forEach(function (track) {
        track.stop();
      });
    }
    postJsonAdmin("/__terrane/admin/stt/close", {
      app: currentId,
      sessionId: capture.sessionId,
      reason: reason || "stopped",
    }).catch(function () {});
    if (sttMicButton) {
      sttMicButton.setAttribute("aria-pressed", "false");
      sttMicButton.title = "Enable microphone";
    }
    if (sttListeningBadge) sttListeningBadge.hidden = true;
    sendToFrame({ type: "terrane:stt", status: "closed", sessionId: capture.sessionId });
  }

  function postJsonAdmin(route, body) {
    return fetch(route, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Terrane-Admin": "local-admin",
      },
      body: JSON.stringify(body || {}),
    }).then(function (response) {
      return response.text().then(function (text) {
        var parsed = {};
        if (text) {
          try {
            parsed = JSON.parse(text);
          } catch (_) {
            parsed = { error: text };
          }
        }
        return { ok: response.ok, body: parsed };
      });
    });
  }

  function bindTopbar() {
    if (!topbarApp || !crumbDoc || !userButton) return;
    if (window.terraneAppIcon) topbarApp.appendChild(window.terraneAppIcon({ id: isAdmin ? "admin" : currentId }));
    crumbApp.textContent = appDisplayName;
    crumbDoc.textContent = isAdmin ? "" : storedDocName();
    if (!isAdmin) bindDocEditing();
    bindUserMenu();
    bindSettings();
    initTheme();
    loadIdentity();
    updateAuthUi();
    // The app requests the current theme + document via a nonce-checked hello
    // once it loads (see bindBridge); that is what performs the initial sync.
  }

  // ---- Agents ------------------------------------------------------------
  // A stack of assistant avatars in the top bar. Left-click opens an assist
  // panel that runs the agent against the current app; right-click opens the
  // agent's setup; "+" creates a new agent. Definitions come from the host's
  // `agent` capability via /__terrane/agents.
  function bindAgents() {
    var widget = document.getElementById("agents-widget");
    var stack = document.getElementById("agent-stack");
    var addButton = document.getElementById("agent-add");
    var assistPanel = document.getElementById("agent-assist");
    var setupPanel = document.getElementById("agent-setup");
    if (!widget || !stack || !assistPanel || !setupPanel) return;

    var assistAvatar = document.getElementById("assist-avatar");
    var assistName = document.getElementById("assist-name");
    var assistPersonality = document.getElementById("assist-personality");
    var assistModel = document.getElementById("assist-model");
    var assistInput = document.getElementById("assist-input");
    var assistRun = document.getElementById("assist-run");
    var assistStatus = document.getElementById("assist-status");
    var assistSetup = document.getElementById("assist-setup");

    var setupTitle = document.getElementById("setup-title");
    var setupIdField = document.getElementById("setup-id-field");
    var setupId = document.getElementById("setup-id");
    var setupName = document.getElementById("setup-name");
    var setupPersonality = document.getElementById("setup-personality");
    var setupModel = document.getElementById("setup-model");
    var setupColor = document.getElementById("setup-color");
    var setupError = document.getElementById("setup-error");
    var setupSave = document.getElementById("setup-save");

    var agents = [];
    var defaults = { model: "", harness: "opencode" };
    var activeId = null; // agent whose assist panel is open
    var editingId = null; // agent being edited (null = creating)
    var assisting = false;

    loadAgents();

    function loadAgents() {
      fetch("/__terrane/agents", { cache: "no-store" })
        .then(function (r) {
          return r.ok ? r.json() : null;
        })
        .then(function (data) {
          if (!data) return;
          agents = data.agents || [];
          defaults.model = data.default_model || "";
          defaults.harness = data.default_harness || "opencode";
          renderStack();
        })
        .catch(function () {});
    }

    function initials(name) {
      var parts = String(name || "?").trim().split(/\s+/);
      var text = parts[0] ? parts[0].charAt(0) : "?";
      if (parts.length > 1) text += parts[parts.length - 1].charAt(0);
      return text.slice(0, 2);
    }

    function findAgent(id) {
      for (var i = 0; i < agents.length; i++) {
        if (agents[i].id === id) return agents[i];
      }
      return null;
    }

    function renderStack() {
      widget.hidden = false;
      stack.textContent = "";
      agents.forEach(function (agent) {
        var el = document.createElement("button");
        el.type = "button";
        el.className = "agent-avatar";
        el.style.background = agent.color || "#6b7bff";
        el.textContent = initials(agent.name);
        el.title = agent.name;
        el.setAttribute("aria-label", "Agent " + agent.name);
        if (agent.id === activeId) el.classList.add("selected");
        el.addEventListener("click", function (event) {
          event.stopPropagation();
          openAssist(agent.id);
        });
        el.addEventListener("contextmenu", function (event) {
          event.preventDefault();
          event.stopPropagation();
          openSetup(agent.id);
        });
        stack.appendChild(el);
      });
    }

    function closePanels() {
      assistPanel.hidden = true;
      setupPanel.hidden = true;
      activeId = null;
      renderStackSelection();
    }

    function renderStackSelection() {
      var chips = stack.querySelectorAll(".agent-avatar");
      for (var i = 0; i < chips.length; i++) {
        var agent = agents[i];
        if (agent && agent.id === activeId) {
          chips[i].classList.add("selected");
        } else {
          chips[i].classList.remove("selected");
        }
      }
    }

    function openAssist(id) {
      var agent = findAgent(id);
      if (!agent) return;
      setupPanel.hidden = true;
      activeId = id;
      renderStackSelection();
      assistAvatar.style.background = agent.color || "#6b7bff";
      assistAvatar.textContent = initials(agent.name);
      assistName.textContent = agent.name;
      assistPersonality.textContent = agent.personality || "";
      assistModel.textContent = agent.model || defaults.model;
      assistInput.value = "";
      setAssistStatus("", false);
      assistPanel.hidden = false;
      assistInput.focus();
    }

    function setAssistStatus(text, isError) {
      assistStatus.textContent = text || "";
      assistStatus.hidden = !text;
      if (isError) {
        assistStatus.classList.add("error");
      } else {
        assistStatus.classList.remove("error");
      }
    }

    function openSetup(id) {
      assistPanel.hidden = true;
      editingId = id || null;
      setupError.textContent = "";
      if (editingId) {
        var agent = findAgent(editingId);
        if (!agent) return;
        setupTitle.textContent = "Edit " + agent.name;
        setupIdField.hidden = true;
        setupId.value = agent.id;
        setupName.value = agent.name;
        setupPersonality.value = agent.personality || "";
        setupModel.value = agent.model || "";
        setupColor.value = agent.color || "";
        setupSave.textContent = "Save";
      } else {
        setupTitle.textContent = "New agent";
        setupIdField.hidden = false;
        setupId.value = "";
        setupName.value = "";
        setupPersonality.value = "";
        setupModel.value = defaults.model;
        setupColor.value = "#6b7bff";
        setupSave.textContent = "Create";
      }
      setupPanel.hidden = false;
      (editingId ? setupName : setupId).focus();
    }

    function saveSetup() {
      var body = {
        name: setupName.value.trim(),
        personality: setupPersonality.value.trim(),
        model: setupModel.value.trim(),
        color: setupColor.value.trim(),
      };
      var url = "/__terrane/agents";
      if (editingId) {
        url = "/__terrane/agents/" + encodeURIComponent(editingId);
      } else {
        body.id = setupId.value.trim().toLowerCase().replace(/[^a-z0-9_-]/g, "-");
        if (!body.id) {
          setupError.textContent = "id is required";
          return;
        }
      }
      setupSave.disabled = true;
      fetch(url, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      })
        .then(function (r) {
          return r.json().then(function (data) {
            return { ok: r.ok, data: data };
          });
        })
        .then(function (res) {
          setupSave.disabled = false;
          if (!res.ok) {
            setupError.textContent = (res.data && res.data.error) || "could not save";
            return;
          }
          agents = res.data.agents || agents;
          renderStack();
          setupPanel.hidden = true;
        })
        .catch(function () {
          setupSave.disabled = false;
          setupError.textContent = "could not save";
        });
    }

    function runAssist() {
      if (assisting || !activeId) return;
      var message = assistInput.value.trim();
      if (!message) {
        assistInput.focus();
        return;
      }
      assisting = true;
      assistRun.disabled = true;
      assistRun.textContent = "Working…";
      setAssistStatus("Starting " + assistName.textContent + "…", false);
      fetch("/__terrane/agents/" + encodeURIComponent(activeId) + "/assist", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ app: currentId, message: message }),
      })
        .then(function (r) {
          return r.json().then(function (data) {
            return { ok: r.ok, data: data };
          });
        })
        .then(function (res) {
          if (!res.ok || !res.data || !res.data.job) {
            throw new Error((res.data && res.data.error) || "could not start agent");
          }
          pollAssist(res.data.job);
        })
        .catch(function (err) {
          finishAssist(err.message || "agent failed", true);
        });
    }

    function pollAssist(job) {
      fetch("/__terrane/agents/assist/status", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ job: job }),
      })
        .then(function (r) {
          return r.json().then(function (data) {
            return { ok: r.ok, data: data };
          });
        })
        .then(function (res) {
          var data = res.data || {};
          if (data.status === "running") {
            setAssistStatus("Working… " + (data.note || ""), false);
            setTimeout(function () {
              pollAssist(job);
            }, 1500);
            return;
          }
          if (data.status === "done") {
            finishAssist(data.transcript || "Done.", false);
            reloadFrame();
            return;
          }
          finishAssist((data && data.error) || "agent failed", true);
        })
        .catch(function () {
          finishAssist("lost contact with the agent", true);
        });
    }

    function finishAssist(text, isError) {
      assisting = false;
      assistRun.disabled = false;
      assistRun.textContent = "Ask";
      setAssistStatus(text, isError);
    }

    function reloadFrame() {
      // The agent drove the app's backend through the host's own tools; the
      // sandboxed frame reads its state on load, so reload it (with a fresh
      // per-load nonce, like the initial load) to show the work.
      if (frame && currentId) loadFrame();
    }

    // Wiring.
    if (addButton) {
      addButton.addEventListener("click", function (event) {
        event.stopPropagation();
        openSetup(null);
      });
    }
    assistRun.addEventListener("click", runAssist);
    assistInput.addEventListener("keydown", function (event) {
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
        event.preventDefault();
        runAssist();
      }
    });
    assistSetup.addEventListener("click", function () {
      openSetup(activeId);
    });
    setupSave.addEventListener("click", saveSetup);

    document.addEventListener("click", function (event) {
      if (assistPanel.hidden && setupPanel.hidden) return;
      if (widget.contains(event.target)) return;
      closePanels();
      setupPanel.hidden = true;
    });
    document.addEventListener("keydown", function (event) {
      if (event.key === "Escape" && (!assistPanel.hidden || !setupPanel.hidden)) {
        closePanels();
        setupPanel.hidden = true;
      }
    });
    window.addEventListener("blur", function () {
      // Focus moving into the app frame should not dismiss an in-flight run.
      if (!assisting && !setupPanel.hidden) setupPanel.hidden = true;
    });
  }

  function storedDocName() {
    var stored = "";
    try {
      stored = window.localStorage.getItem(DOC_KEY) || "";
    } catch (_) {}
    return stored || shellT("system.doc.untitled", "Untitled");
  }

  function setDocName(raw, fromApp) {
    // Strip control/format characters (bidi overrides, zero-width) so an
    // app-supplied name cannot spoof the trusted breadcrumb chrome.
    var name = String(raw == null ? "" : raw)
      .replace(/[\u0000-\u001f\u007f-\u009f\u200b-\u200f\u2028-\u202e\u2066-\u2069\ufeff]/g, "")
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, 120);
    if (!name) name = shellT("system.doc.untitled", "Untitled");
    crumbDoc.textContent = name;
    if (name !== storedDocName()) {
      try {
        window.localStorage.setItem(DOC_KEY, name);
      } catch (_) {}
    }
    // Hand the canonical (sanitized) name back to the app — including when the
    // app itself set it, whose optimistic getDocument() value may differ after
    // sanitization — so getDocument()/onDocument converge with what we stored.
    void fromApp;
    sendToFrame({ type: "terrane:document", name: name });
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

  // Shell -> app pushes (theme/document). The frame's opaque sandbox origin
  // can't be pinned, so these post with "*". The automatic on-load push was
  // removed in favor of the nonce-checked hello, closing the drive-by leak;
  // a residual remains: if the app navigates its own frame away and the user
  // then changes theme or renames the document, that value (both low-
  // sensitivity: user-chosen) reaches whatever now occupies the frame. The
  // bridge (invoke/permission) and breadcrumb writes stay fully nonce-gated.
  function sendToFrame(message) {
    if (frame && frame.contentWindow) frame.contentWindow.postMessage(message, "*");
  }

  // Push the negotiated locale + the app's merged bundle to the frame, over the
  // same channel as theme/document. Sent on the nonce-checked hello.
  function sendFrameLocale() {
    sendToFrame({
      type: "terrane:locale",
      locale: shellLocale,
      dir: shellDir,
      messages: appMessages,
    });
  }

  // The in-app language picker: choosing a language stores a cookie the server
  // reads on the next render (overriding Accept-Language) and reloads so the
  // whole shell + frame come back in the chosen language.
  function bindLanguagePicker() {
    var select = document.getElementById("menu-language");
    if (!select) return;
    select.replaceChildren();
    for (var i = 0; i < LANGUAGES.length; i++) {
      var opt = document.createElement("option");
      opt.value = LANGUAGES[i][0];
      opt.textContent = LANGUAGES[i][1];
      if (LANGUAGES[i][0] === shellLocale) opt.selected = true;
      select.appendChild(opt);
    }
    select.addEventListener("change", function () {
      document.cookie =
        "terrane_lang=" +
        encodeURIComponent(select.value) +
        "; path=/; max-age=31536000; samesite=lax";
      window.location.reload();
    });
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
    menuAuth.textContent = out
      ? shellT("system.menu.login", "Log in")
      : shellT("system.menu.logout", "Log out");
    userName.textContent = out ? shellT("system.auth.signedOut", "Signed out") : identity.name;
    userSubject.textContent = out
      ? shellT("system.auth.localOnly", "Local session only")
      : identity.subject;
    setSettingsField(
      "settings-user",
      out ? shellT("system.auth.signedOut", "Signed out") : identity.name
    );
    setSettingsField("settings-subject", out ? "-" : identity.subject || "-");
    setSettingsField("settings-source", out ? "-" : identity.source || "-");
    setSettingsField(
      "settings-session",
      out || identity.locked == null ? "-" : identity.locked ? "Locked" : "Unlocked"
    );
    updatePremiumUi();
  }

  // --- Terrane Premium (Google) sign-in — optional, host-configured ---

  function bindPremium() {
    if (!premiumUrl || !menuPremium) return;
    premiumSession = loadPremiumSession();
    menuPremium.hidden = false;
    menuPremium.addEventListener("click", function () {
      if (premiumSession) {
        storePremiumSession(null);
      } else {
        startPremiumSignIn();
      }
      setDropdownOpen(false);
    });
    window.addEventListener("message", function (event) {
      if (event.origin !== premiumOrigin()) return;
      var data = event.data;
      if (!data || data.type !== "terrane-premium-session") return;
      storePremiumSession(data);
    });
    if (premiumSession) refreshPremiumAccount();
    updatePremiumUi();
    loadPremiumCatalog();
  }

  // The premium catalog is public metadata served by the control plane; it
  // needs no session, so list it whenever a premium URL is configured.
  function loadPremiumCatalog() {
    if (!premiumUrl) return;
    fetch(premiumUrl + "/marketplace/premium-apps", { cache: "no-store" })
      .then(function (response) {
        return response.ok ? response.json() : null;
      })
      .then(function (body) {
        if (!body || body.ok !== true || !body.result) return;
        premiumApps = Array.isArray(body.result.apps) ? body.result.apps : [];
        renderPremiumCatalog();
      })
      .catch(function (_) {});
  }

  function renderPremiumCatalog() {
    if (!premiumSection || !premiumList) return;
    premiumList.replaceChildren();
    var shown = 0;
    premiumApps.forEach(function (app) {
      if (!app || !app.id) return;
      // Installed premium apps already run from the local catalog; only the
      // rest belong in this "get it from Premium" section.
      if (localAppIds[app.id]) return;
      premiumList.appendChild(premiumLink(app));
      shown += 1;
    });
    premiumSection.hidden = shown === 0;
  }

  function premiumLink(app) {
    var id = String(app.id);
    var name = app.name ? String(app.name) : id;
    var root = document.createElement("a");
    root.className = "app-link";
    if (id === activePremiumAppId) {
      root.className += " selected";
      root.setAttribute("aria-current", "page");
    }
    root.href = premiumUrl + "/apps.html#" + encodeURIComponent(id);
    root.addEventListener("click", function (event) {
      event.preventDefault();
      openPremiumApp(app);
    });

    root.appendChild(window.terraneAppIcon(app));

    var text = document.createElement("span");
    text.className = "app-link-text";

    var label = document.createElement("span");
    label.textContent = name;
    text.appendChild(label);

    var meta = document.createElement("small");
    meta.textContent = app.publisher ? String(app.publisher) : "Premium";
    text.appendChild(meta);

    root.appendChild(text);
    return root;
  }

  function openPremiumApp(app) {
    var id = String(app.id);
    var name = app.name ? String(app.name) : id;
    activePremiumAppId = id;
    frameNonce = "";
    setAdminMode(false);
    frame.hidden = false;
    frame.src = premiumUrl + "/apps.html#" + encodeURIComponent(id);
    setTitle(name);
    renderPremiumCatalog();
    renderLocalSelection();
  }

  function renderLocalSelection() {
    var links = list ? list.querySelectorAll(".app-link") : [];
    for (var i = 0; i < links.length; i++) {
      links[i].classList.remove("selected");
      links[i].removeAttribute("aria-current");
    }
  }

  function premiumOrigin() {
    try {
      return new URL(premiumUrl).origin;
    } catch (_) {
      return "";
    }
  }

  function loadPremiumSession() {
    try {
      var raw = window.localStorage.getItem(PREMIUM_SESSION_KEY);
      var data = raw ? JSON.parse(raw) : null;
      return data && data.user && data.session ? data : null;
    } catch (_) {
      return null;
    }
  }

  function storePremiumSession(data) {
    premiumSession = data;
    try {
      if (data) {
        window.localStorage.setItem(PREMIUM_SESSION_KEY, JSON.stringify(data));
      } else {
        window.localStorage.removeItem(PREMIUM_SESSION_KEY);
      }
    } catch (_) {}
    updatePremiumUi();
  }

  function startPremiumSignIn() {
    fetch(premiumUrl + "/auth/google/start", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ returnOrigin: window.location.origin }),
    })
      .then(function (response) {
        return response.json();
      })
      .then(function (body) {
        if (!body || body.ok !== true) {
          throw new Error((body && body.error && body.error.message) || "sign-in unavailable");
        }
        window.open(body.result.authUrl, "terrane-google-signin", "width=480,height=640");
      })
      .catch(function (error) {
        setSettingsField("settings-premium", "Sign-in unavailable: " + error.message);
      });
  }

  // Validate a restored session against the control plane; drop it if revoked.
  function refreshPremiumAccount() {
    fetch(premiumUrl + "/account/me", {
      cache: "no-store",
      headers: { Authorization: "Bearer " + premiumSession.session.token },
    })
      .then(function (response) {
        if (response.status === 401) {
          storePremiumSession(null);
          return null;
        }
        return response.ok ? response.json() : null;
      })
      .then(function (body) {
        if (!body || body.ok !== true || !premiumSession) return;
        premiumSession.user = body.result.user;
        var org = body.result.organizations && body.result.organizations[0];
        if (org) premiumSession.organization = { id: org.id, name: org.name };
        storePremiumSession(premiumSession);
      })
      .catch(function (_) {});
  }

  function updatePremiumUi() {
    if (!premiumUrl || !menuPremium) return;
    var signedIn = !!premiumSession;
    menuPremium.textContent = signedIn
      ? "Sign out of Premium (" + premiumSession.user.email + ")"
      : "Sign in with Google";
    setSettingsField(
      "settings-premium",
      signedIn
        ? premiumSession.user.email +
            (premiumSession.organization ? " · " + premiumSession.organization.name : "")
        : "Not signed in"
    );
    if (signedIn && !isSignedOut()) {
      userSubject.textContent = premiumSession.user.email;
    }
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
    frame.hidden = open || isAdmin;
    if (adminPanel) adminPanel.hidden = open || !isAdmin;
    crumbApp.textContent = open ? shellT("system.menu.settings", "Settings") : appDisplayName;
    crumbSep.hidden = open;
    crumbDoc.hidden = open || isAdmin;
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
