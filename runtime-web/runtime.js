(function () {
  const FALLBACK_EXAMPLE_IDS = ["notes-lite", "task-workbench", "file-transformer", "api-dashboard", "core-replay-lab", "calendar-planner"];
  const appList = document.getElementById("app-list");
  const statusEl = document.getElementById("runtime-status");
  const frameWrap = document.getElementById("app-frame-wrap");
  const activeTitle = document.getElementById("active-title");
  const activeDescription = document.getElementById("active-description");
  const engineRoomEntry = document.getElementById("engine-room-entry");
  const openEngineRoomButton = document.getElementById("open-engine-room");
  const refreshEngineRoomButton = document.getElementById("refresh-engine-room");
  const engineRoomSections = document.getElementById("engine-room-sections");
  const engineRoomStatus = document.getElementById("engine-room-status");
  const reloadButton = document.getElementById("reload-app");
  const refreshButton = document.getElementById("refresh-apps");
  const refreshPremiumButton = document.getElementById("refresh-premium-catalog");
  const freeAppsList = document.getElementById("free-apps-list");
  const premiumAppsList = document.getElementById("premium-apps-list");
  const premiumMarketplaceStatus = document.getElementById("premium-marketplace-status");
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
    ["notebook.open", "notebook.read"],
    ["notebook.apply_local", "notebook.write"],
    ["notebook.propose_ai_patch", "notebook.propose"],
    ["notebook.accept_proposal", "notebook.approve"],
    ["notebook.reject_proposal", "notebook.approve"],
    ["notebook.snapshot", "notebook.read"],
    ["notebook.checkout", "notebook.read"],
    ["notebook.sync_pull", "notebook.sync"],
    ["notebook.sync_push", "notebook.sync"],
    ["notebook.subscribe", "notebook.read"],
  ]);
  const GENERATED_APP_CSP = "default-src 'none'; script-src 'self' app-runtime:; style-src 'self' app-runtime:; img-src 'self' app-runtime: data: blob:; font-src 'self' app-runtime:; connect-src 'none'; frame-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'; object-src 'none'; require-trusted-types-for 'script'; trusted-types runtime-default;";

  let apps = [];
  let activeApp = null;
  let activeFrame = null;
  let activeMount = null;
  let premiumCatalog = null;
  let lastPremiumCatalogEndpoint = null;
  const mountsByFrame = new WeakMap();
  const mountsByPort = new WeakMap();
  const portsByMountToken = new Map();
  const androidBridgePending = new Map();
  let androidBridgeHandlerAttached = false;
  const webview2BridgePending = new Map();
  const webview2MountPending = new Map();
  let webview2BridgeHandlerAttached = false;
  let nextWebView2MountRequestId = 1;
  const usageByApp = new Map();
  const devMockStorageByApp = new Map();
  const devMockCoreVersions = new Map();
  const devMockCoreEvents = [];
  const consoleEntries = [];
  const minuteMs = 60 * 1000;
  const engineRoom = window.TerraneEngineRoom.create({
    clearActiveMount() {
      activeApp = null;
      activeMount = null;
      activeFrame = null;
    },
    consoleEntries,
    devMockCoreEvents,
    devMockStorageByApp,
    dom: {
      activeDescription,
      activeTitle,
      bridgeLog,
      entry: engineRoomEntry,
      frameWrap,
      reloadButton,
      sections: engineRoomSections,
      status: engineRoomStatus,
    },
    element,
    fetchJson,
    getActiveApp() {
      return activeApp;
    },
    getActiveMount() {
      return activeMount;
    },
    getApps() {
      return apps;
    },
    portsByMountToken,
    premiumApps() {
      return window.TerraneRuntimeHost ? window.TerraneRuntimeHost.premiumApps() : [];
    },
    renderAppList,
    setStatus,
  });

  refreshButton.addEventListener("click", loadApps);
  engineRoom.applyPreference();
  if (openEngineRoomButton) {
    openEngineRoomButton.addEventListener("click", engineRoom.showEngineRoom);
  }
  if (refreshEngineRoomButton) {
    refreshEngineRoomButton.addEventListener("click", function () {
      engineRoom.renderSnapshot();
    });
  }
  if (refreshPremiumButton) {
    refreshPremiumButton.addEventListener("click", function () {
      loadPremiumCatalog();
    });
  }
  reloadButton.addEventListener("click", function () {
    if (activeApp) mountApp(activeApp);
  });
  clearDebugButton.addEventListener("click", function () {
    bridgeLog.textContent = "";
  });

  window.addEventListener("message", function (event) {
    if (!activeFrame) return;
    const sourceWindow = event.source || activeFrame.contentWindow;
    if (event.source && event.source !== activeFrame.contentWindow) return;
    if (!event.data || event.data.type !== "runtime.ready_for_port") {
      addBridgeLog(activeMount ? activeMount.appId : "unknown", "postMessage", "bridge.unauthorized_channel");
      emitRuntimeEvent(activeMount, "app.error", {
        code: "bridge.unauthorized_channel",
        message: "Bridge message arrived outside the assigned MessageChannel",
        source: "postMessage"
      });
      return;
    }
    const mount = mountsByFrame.get(activeFrame);
    if (!mount || !activeMount || mount.mountToken !== activeMount.mountToken) {
      addBridgeLog(mount ? mount.appId : "unknown", "runtime.ready_for_port", "bridge.unauthorized_channel");
      emitRuntimeEvent(mount || activeMount, "app.error", {
        code: "bridge.unauthorized_channel",
        message: "Bridge port request did not match the active mount",
        source: "runtime.ready_for_port"
      });
      return;
    }
    attachBridgePort(sourceWindow, mount);
  });

  installRuntimeDevtools();
  const appsReady = loadApps();
  installTerraneRuntimeHost();

  async function loadApps() {
    setStatus("Loading apps");
    const appIndex = await fetchAppIndex();
    loadPremiumCatalog(appIndex);
    const appIndexRecords = new Map((appIndex?.apps || []).map((record) => [record.id, record]));
    const appIds = appIndex ? appIndex.apps.map((record) => record.id) : FALLBACK_EXAMPLE_IDS;
    const loaded = [];
    for (const id of appIds) {
      const manifest = await fetchJson(`/webapps/examples/${id}/manifest.json`);
      loaded.push({ ...manifest, ...(appIndexRecords.get(id) || {}) });
    }
    apps = loaded;
    renderAppList();
    renderFreeAppCatalog();
    setStatus("Ready");
    return apps;
  }

  function installTerraneRuntimeHost() {
    window.TerraneRuntimeHost = {
      activeAppId() {
        return activeApp ? activeApp.id : null;
      },
      apps() {
        return apps.map(function (app) {
          return {
            id: app.id,
            name: app.name,
            version: app.version,
            description: app.description,
          };
        });
      },
      premiumApps() {
        return (premiumCatalog?.apps || []).map(function (app) {
          return {
            id: app.id,
            name: app.name,
            publisher: app.publisher,
            version: app.version,
            category: app.category,
            price: app.price,
            summary: app.summary,
          };
        });
      },
      async mountApp(appId) {
        await appsReady;
        const app = apps.find(function (candidate) {
          return candidate.id === appId;
        });
        if (!app) {
          throw new Error(`Unknown Terrane app: ${appId}`);
        }
        await mountApp(app);
        return { ok: true, appId: app.id };
      },
      async showMarketplace() {
        await appsReady;
        showMarketplace();
        return { ok: true, view: "marketplace" };
      },
      async showEngineRoom() {
        await appsReady;
        engineRoom.showEngineRoom();
        return { ok: true, view: "engine-room" };
      },
      setEngineRoomVisible(visible) {
        engineRoom.setVisible(visible === true);
        return { ok: true, visible: engineRoom.isVisible() };
      },
      async engineRoomSnapshot(options) {
        await appsReady;
        return engineRoom.snapshot(options || {});
      },
      async reload() {
        if (!activeApp) return { ok: false, reason: "no-active-app" };
        await mountApp(activeApp);
        return { ok: true, appId: activeApp.id };
      },
      setHostMode(enabled) {
        const active = enabled === true;
        if (document.body && document.body.classList) {
          document.body.classList.toggle("native-host-mode", active);
        }
        return { ok: true, enabled: active };
      },
    };
  }

  async function fetchAppIndex() {
    try {
      const appIndex = await fetchJson("/runtime/app-index.json");
      if (!appIndex || !Array.isArray(appIndex.apps)) return null;
      const apps = appIndex.apps.filter(function (record) {
        return record && typeof record.id === "string" && record.id;
      });
      return { ...appIndex, apps: apps };
    } catch (_) {
      return null;
    }
  }

  async function loadPremiumCatalog(appIndex) {
    if (!premiumAppsList) return null;
    setPremiumStatus("Loading");
    const endpoint = resolvePremiumCatalogEndpoint(appIndex);
    if (endpoint) {
      lastPremiumCatalogEndpoint = endpoint;
      try {
        const rawCatalog = await fetchJson(endpoint, { credentials: "omit", cache: "no-store" });
        const catalog = normalizePremiumCatalog(rawCatalog, "premium-server", "Premium catalog");
        premiumCatalog = catalog.apps.length ? catalog : fallbackPremiumCatalog();
        renderPremiumCatalog(premiumCatalog, premiumCatalog === catalog ? "Premium catalog" : "Static fallback");
        renderFreeAppCatalog();
        return premiumCatalog;
      } catch (_) {
        premiumCatalog = fallbackPremiumCatalog();
        renderPremiumCatalog(premiumCatalog, "Static fallback");
        renderFreeAppCatalog();
        return premiumCatalog;
      }
    }
    premiumCatalog = fallbackPremiumCatalog();
    renderPremiumCatalog(premiumCatalog, "Static fallback");
    renderFreeAppCatalog();
    return premiumCatalog;
  }

  function resolvePremiumCatalogEndpoint(appIndex) {
    const candidates = [
      window.__TERRANE_PREMIUM_CATALOG_URL__,
      appIndex?.premiumCatalogUrl,
      appIndex?.premiumCatalog?.url,
      appIndex?.marketplace?.premiumCatalogUrl,
      appIndex?.marketplace?.catalogUrl,
      appIndex?.premium?.catalogUrl,
      lastPremiumCatalogEndpoint,
    ];
    for (const candidate of candidates) {
      const endpoint = normalizeCatalogEndpoint(candidate);
      if (endpoint) return endpoint;
    }
    return null;
  }

  function normalizeCatalogEndpoint(value) {
    if (typeof value !== "string") return null;
    const trimmed = value.trim();
    if (!trimmed || trimmed.length > 2048 || /[\u0000-\u001f\u007f]/.test(trimmed)) return null;
    let url;
    let base;
    try {
      base = new URL(runtimeBaseUrl());
      url = new URL(trimmed, base.href);
    } catch (_) {
      return null;
    }
    if (url.username || url.password) return null;
    if (url.origin === base.origin) {
      return `${url.pathname}${url.search}`;
    }
    if (url.protocol === "https:" || (url.protocol === "http:" && /^(?:127\.0\.0\.1|localhost)$/.test(url.hostname))) {
      return `${url.origin}${url.pathname}${url.search}`;
    }
    return null;
  }

  function runtimeBaseUrl() {
    const location = window.location || {};
    const protocol = location.protocol === "http:" || location.protocol === "https:" ? location.protocol : "https:";
    const hostname = typeof location.hostname === "string" && location.hostname ? location.hostname : "runtime.local.platform";
    const port = typeof location.port === "string" && location.port ? `:${location.port}` : "";
    const pathname = typeof location.pathname === "string" && location.pathname ? location.pathname : "/runtime/index.html";
    return `${protocol}//${hostname}${port}${pathname}`;
  }

  function fallbackPremiumCatalog() {
    return normalizePremiumCatalog({
      source: "static-fallback",
      apps: [
        {
          id: "premium-todo",
          name: "Premium Todo",
          subtitle: "Local-first tasks with hosted sync",
          summary: "A Terrane Premium todo app with encrypted local storage, optional hosted sync, and backup entitlement support.",
          publisher: "Terrane Premium",
          category: "Productivity",
          version: "0.1.0",
          price: "Included",
          contentRating: { label: "4+" },
          rating: { value: 4.8, count: 24 },
          compatibility: "Terrane Runtime 0.1+",
          updatedAt: "2026-06-16",
          permissions: ["storage.read", "storage.write", "notification.toast"],
          privacy: ["Local-first data", "Hosted sync optional"],
        },
      ],
    }, "static-fallback", "Static fallback");
  }

  function normalizePremiumCatalog(rawCatalog, source, sourceLabel) {
    const envelope = rawCatalog && typeof rawCatalog === "object" && !Array.isArray(rawCatalog) ? rawCatalog : {};
    const raw = envelope.ok === true && envelope.result && typeof envelope.result === "object" && !Array.isArray(envelope.result)
      ? envelope.result
      : envelope;
    const records = Array.isArray(raw.apps) ? raw.apps : Array.isArray(raw.packages) ? raw.packages : [];
    const apps = records.map(normalizePremiumApp).filter(Boolean).slice(0, 8);
    return {
      source: source,
      sourceLabel: safeText(raw.sourceLabel || raw.source || sourceLabel, sourceLabel, 48),
      apps: apps,
    };
  }

  function normalizePremiumApp(record) {
    if (!record || typeof record !== "object" || Array.isArray(record)) return null;
    const id = safeText(record.id || record.appId || record.manifestId, "", 64);
    const name = safeText(record.name || record.title, "", 80);
    if (!id || !name) return null;
    const summary = safeText(record.summary || record.description || record.subtitle, "Premium app for Terrane workspaces.", 220);
    const rating = normalizeRating(record.rating);
    const contentRating = record.contentRating && typeof record.contentRating === "object"
      ? safeText(record.contentRating.label, "4+", 16)
      : safeText(record.contentRating || record.ageRating, "4+", 16);
    return {
      id: id,
      name: name,
      subtitle: safeText(record.subtitle || record.tagline || record.category, "Premium app", 96),
      summary: summary,
      publisher: safeText(record.publisher || record.author || record.authorId, "Terrane Premium", 80),
      category: safeText(record.category, "Productivity", 48),
      version: safeText(record.version, "0.1.0", 32),
      price: safeText(record.price || record.requiredPlan || record.entitlement || record.plan, "Premium", 48),
      contentRating: contentRating,
      ratingValue: rating.value,
      ratingCount: rating.count,
      compatibility: safeText(record.compatibility || record.runtimeCompatibility || record.minimumRuntimeVersion || (record.serverRequired ? "Terrane Premium server" : null), "Terrane Runtime", 80),
      updatedAt: safeText(record.updatedAt || record.releaseDate, "Current", 32),
      permissions: safeTextList(record.permissions || record.entitlementFeatures, 4, 40),
      privacy: safeTextList(record.privacy || record.privacyLabels, 3, 48),
      benefits: safeTextList(record.benefits, 3, 96),
      serverRequired: record.serverRequired === true,
    };
  }

  function normalizeRating(value) {
    if (typeof value === "number" && Number.isFinite(value)) {
      return { value: clampRating(value), count: null };
    }
    if (value && typeof value === "object" && !Array.isArray(value)) {
      const ratingValue = Number(value.value ?? value.average ?? value.score);
      const count = Number(value.count ?? value.ratings ?? value.reviews);
      return {
        value: Number.isFinite(ratingValue) ? clampRating(ratingValue) : null,
        count: Number.isFinite(count) && count >= 0 ? Math.floor(count) : null,
      };
    }
    return { value: null, count: null };
  }

  function clampRating(value) {
    return Math.max(0, Math.min(5, value));
  }

  function safeTextList(value, maxItems, maxLength) {
    if (typeof value === "string") return [safeText(value, "", maxLength)].filter(Boolean);
    if (!Array.isArray(value)) return [];
    return value
      .map(function (item) {
        return safeText(item, "", maxLength);
      })
      .filter(Boolean)
      .slice(0, maxItems);
  }

  function safeText(value, fallback, maxLength) {
    const text = value == null ? "" : String(value);
    const cleaned = text.replace(/[\u0000-\u001f\u007f]/g, " ").replace(/\s+/g, " ").trim();
    if (!cleaned) return fallback || "";
    return cleaned.slice(0, maxLength);
  }

  function renderPremiumCatalog(catalog, statusLabel) {
    premiumAppsList.textContent = "";
    const apps = catalog && Array.isArray(catalog.apps) ? catalog.apps : [];
    if (!apps.length) {
      premiumAppsList.appendChild(element("div", "empty-state", "No Premium apps available."));
      setPremiumStatus(statusLabel || "Static fallback");
      return;
    }
    for (const app of apps) {
      premiumAppsList.appendChild(renderPremiumAppCard(app));
    }
    setPremiumStatus(statusLabel || catalog.sourceLabel || "Premium catalog");
  }

  function renderFreeAppCatalog() {
    if (!freeAppsList) return;
    freeAppsList.textContent = "";
    const premiumIds = new Set((premiumCatalog?.apps || []).map(function (app) {
      return app.id;
    }));
    const freeApps = apps.filter(function (app) {
      return app && app.id && !premiumIds.has(app.id);
    });
    if (!freeApps.length) {
      freeAppsList.appendChild(element("div", "empty-state", "No free apps available."));
      return;
    }
    for (const app of freeApps) {
      freeAppsList.appendChild(renderFreeAppCard(app));
    }
  }

  function renderFreeAppCard(app) {
    const card = element("article", "premium-app-card marketplace-app-card free-app-card");
    card.setAttribute("data-testid", `free-app-${app.id}`);

    const icon = element("div", "premium-app-icon free-app-icon", appInitials(app.name));
    card.appendChild(icon);

    const body = element("div", "premium-app-body");
    const topline = element("div", "premium-app-topline");
    const title = element("div", "premium-app-title");
    title.appendChild(element("h3", "", app.name));
    title.appendChild(element("p", "", `${app.id} - Included with Terrane`));
    topline.appendChild(title);
    const action = element("button", "marketplace-app-action", "Open");
    action.type = "button";
    action.setAttribute("data-testid", `marketplace-open-${app.id}`);
    action.setAttribute("aria-label", `Open ${app.name}`);
    action.addEventListener("click", function () {
      mountApp(app);
    });
    topline.appendChild(action);
    body.appendChild(topline);

    body.appendChild(element("p", "premium-app-summary", app.description || "Bundled local Terrane app."));
    body.appendChild(renderFreeAppMeta(app));
    body.appendChild(renderFreeAppPills(app));
    card.appendChild(body);
    return card;
  }

  function renderPremiumAppCard(app) {
    const card = element("article", "premium-app-card");
    card.setAttribute("data-testid", `premium-app-${app.id}`);

    const icon = element("div", "premium-app-icon", appInitials(app.name));
    card.appendChild(icon);

    const body = element("div", "premium-app-body");
    const topline = element("div", "premium-app-topline");
    const title = element("div", "premium-app-title");
    title.appendChild(element("h3", "", app.name));
    title.appendChild(element("p", "", `${app.subtitle} - ${app.publisher}`));
    topline.appendChild(title);
    const price = element("span", "premium-app-price", app.price);
    const installedApp = apps.find(function (candidate) {
      return candidate.id === app.id;
    });
    if (installedApp) {
      const action = element("button", "marketplace-app-action premium-app-action", "Open");
      action.type = "button";
      action.setAttribute("data-testid", `marketplace-open-${app.id}`);
      action.setAttribute("aria-label", `Open ${app.name}`);
      action.addEventListener("click", function () {
        mountApp(installedApp);
      });
      topline.appendChild(action);
    } else {
      price.setAttribute("aria-label", `${app.name} availability`);
      topline.appendChild(price);
    }
    body.appendChild(topline);

    body.appendChild(element("p", "premium-app-summary", app.summary));
    if (app.benefits.length) body.appendChild(renderPremiumBenefits(app));
    body.appendChild(renderPremiumMeta(app));
    body.appendChild(renderPremiumPills(app));
    card.appendChild(body);
    return card;
  }

  function renderFreeAppMeta(app) {
    const meta = element("dl", "premium-app-meta");
    addPremiumMeta(meta, "Plan", "Free");
    addPremiumMeta(meta, "Version", safeText(app.version, "0.1.0", 32));
    addPremiumMeta(meta, "Age", app.contentRating && app.contentRating.label ? app.contentRating.label : "4+");
    addPremiumMeta(meta, "Works With", `Terrane Runtime ${safeText(app.runtimeVersion, "0.1+", 32)}`);
    return meta;
  }

  function renderFreeAppPills(app) {
    const pills = element("div", "premium-app-pills");
    for (const item of ["Free", "Installed"].concat(safeTextList(app.permissions, 4, 40))) {
      if (item) pills.appendChild(element("span", "premium-app-pill", item));
    }
    return pills;
  }

  function renderPremiumBenefits(app) {
    const list = element("ul", "premium-app-benefits");
    for (const benefit of app.benefits) {
      list.appendChild(element("li", "", benefit));
    }
    return list;
  }

  function renderPremiumMeta(app) {
    const meta = element("dl", "premium-app-meta");
    addPremiumMeta(meta, "Rating", formatPremiumRating(app));
    addPremiumMeta(meta, "Age", app.contentRating);
    addPremiumMeta(meta, "Version", app.version);
    addPremiumMeta(meta, "Updated", app.updatedAt);
    addPremiumMeta(meta, "Works With", app.compatibility);
    return meta;
  }

  function addPremiumMeta(meta, label, value) {
    const group = element("div", "");
    group.appendChild(element("dt", "", label));
    group.appendChild(element("dd", "", value));
    meta.appendChild(group);
  }

  function renderPremiumPills(app) {
    const pills = element("div", "premium-app-pills");
    for (const item of [app.category, app.serverRequired ? "Premium server required" : null].concat(app.permissions, app.privacy)) {
      if (item) pills.appendChild(element("span", "premium-app-pill", item));
    }
    return pills;
  }

  function formatPremiumRating(app) {
    if (typeof app.ratingValue !== "number") return "New";
    const count = typeof app.ratingCount === "number" ? ` (${app.ratingCount})` : "";
    return `${app.ratingValue.toFixed(1)}${count}`;
  }

  function appInitials(name) {
    const words = String(name || "").trim().split(/\s+/).filter(Boolean);
    return words.slice(0, 2).map(function (word) {
      return word[0].toUpperCase();
    }).join("") || "P";
  }

  function element(tagName, className, text) {
    const node = document.createElement(tagName);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  }

  function setPremiumStatus(value) {
    if (premiumMarketplaceStatus) {
      premiumMarketplaceStatus.textContent = value;
    }
  }

  function renderAppList() {
    appList.textContent = "";
    for (const app of apps) {
      const button = document.createElement("button");
      button.className = "app-button";
      button.dataset.testid = `open-${app.id}-button`;
      button.innerHTML = `<strong></strong><span></span>`;
      button.querySelector("strong").textContent = app.name;
      button.querySelector("span").textContent = app.contentRating && app.contentRating.label
        ? `${app.id} v${app.version} · ${app.contentRating.label}`
        : `${app.id} v${app.version}`;
      button.addEventListener("click", function () {
        mountApp(app);
      });
      if (activeApp && activeApp.id === app.id) button.classList.add("active");
      appList.appendChild(button);
    }
  }

  async function mountApp(app) {
    let mountToken;
    try {
      mountToken = await createRuntimeMountToken(app);
    } catch (error) {
      setStatus(`Mount failed: ${error && error.message ? error.message : String(error)}`);
      throw error;
    }
    if (activeMount) {
      portsByMountToken.delete(activeMount.mountToken);
    }
    const mount = {
      app: app,
      appId: app.id,
      mountToken: mountToken,
      createdAt: Date.now(),
    };
    activeApp = app;
    activeMount = mount;
    document.body.classList.remove("marketplace-mode");
    document.body.classList.remove("engine-room-mode");
    renderAppList();
    reloadButton.disabled = false;
    activeTitle.textContent = app.name;
    activeDescription.textContent = app.description;
    notifyNativeActiveAppChanged(app.id);
    setStatus(`Mounting ${app.id}`);

    const html = rewritePackageResourceUrls(app.id, await fetchText(`/webapps/examples/${app.id}/index.html`));
    const srcdoc = injectRuntimeBootstrap(app, html);
    const frame = document.createElement("iframe");
    frame.title = app.name;
    frame.dataset.testid = "runtime-app-frame";
    frame.setAttribute("allow", "");
    frame.setAttribute("sandbox", "allow-scripts");
    frame.setAttribute("csp", GENERATED_APP_CSP);
    frame.setAttribute("referrerpolicy", "no-referrer");

    frameWrap.textContent = "";
    activeFrame = frame;
    mountsByFrame.set(frame, mount);
    if (usesWebKitNativeAppFrames()) {
      frame.src = `app-runtime://${encodeURIComponent(app.id)}/index.html?mountToken=${encodeURIComponent(mount.mountToken)}`;
    } else {
      frame.srcdoc = srcdoc;
    }
    frameWrap.appendChild(frame);
    setStatus(`Mounted ${app.id}`);
  }

  function showMarketplace() {
    if (activeMount) {
      portsByMountToken.delete(activeMount.mountToken);
    }
    activeApp = null;
    activeMount = null;
    activeFrame = null;
    renderAppList();
    reloadButton.disabled = true;
    activeTitle.textContent = "Marketplace";
    activeDescription.textContent = "Browse Terrane Premium apps.";
    frameWrap.textContent = "";
    frameWrap.appendChild(element("div", "empty-state", "Marketplace is open."));
    document.body.classList.add("marketplace-mode");
    document.body.classList.remove("engine-room-mode");
    setStatus("Marketplace");
    loadPremiumCatalog();
  }

  function notifyNativeActiveAppChanged(appId) {
    const handler = window.webkit?.messageHandlers?.TerraneNativeShell;
    if (!handler || typeof handler.postMessage !== "function") return;
    try {
      handler.postMessage({ type: "active_app_changed", appId: appId });
    } catch (_) {
      // Native shell notifications are best-effort UI synchronization only.
    }
  }

  function usesWebKitNativeAppFrames() {
    return Boolean(webkitNativeBridgeHandler()) && window.location && window.location.protocol === "app-runtime:";
  }

  function rewritePackageResourceUrls(appId, html) {
    return html
      .replace(/\s(href|src|poster)=(["'])([^"']*)\2/gi, function (_match, attribute, quote, value) {
        return ` ${attribute}=${quote}${packageResourceUrl(appId, value)}${quote}`;
      })
      .replace(/\ssrcset=(["'])([^"']*)\1/gi, function (_match, quote, value) {
        return ` srcset=${quote}${rewriteSrcset(appId, value)}${quote}`;
      });
  }

  function rewriteSrcset(appId, value) {
    return value.split(",").map(function (candidate) {
      const trimmed = candidate.trim();
      if (!trimmed) return trimmed;
      const parts = trimmed.split(/\s+/);
      parts[0] = packageResourceUrl(appId, parts[0]);
      return parts.join(" ");
    }).join(", ");
  }

  function packageResourceUrl(appId, value) {
    const trimmed = String(value || "").trim();
    if (
      trimmed === "" ||
      trimmed[0] === "#" ||
      trimmed[0] === "/" ||
      /^[a-z][a-z0-9+.-]*:/i.test(trimmed)
    ) {
      return value;
    }
    return `/webapps/examples/${encodeURIComponent(appId)}/${trimmed.replace(/^\.\//, "")}`;
  }

  function injectRuntimeBootstrap(app, html) {
    const appId = app.id;
    const bootstrap = `<script>
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

  function attachBridgePort(targetWindow, mount) {
    const channel = new MessageChannel();
    mountsByPort.set(channel.port1, mount);
    portsByMountToken.set(mount.mountToken, channel.port1);
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
    targetWindow.postMessage({ type: "runtime.port" }, "*", [channel.port2]);
  }

  function emitRuntimeEvent(mount, eventName, payload) {
    if (!mount) return;
    const port = portsByMountToken.get(mount.mountToken);
    if (!port || typeof port.postMessage !== "function") return;
    port.postMessage({
      type: "runtime.event",
      eventName: eventName,
      payload: payload || {}
    });
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
          "notebook.read": true,
          "notebook.write": true,
          "notebook.propose": true,
          "notebook.approve": true,
          "notebook.sync": true,
          "notebook.open": true,
          "notebook.apply_local": true,
          "notebook.propose_ai_patch": true,
          "notebook.accept_proposal": true,
          "notebook.reject_proposal": true,
          "notebook.snapshot": true,
          "notebook.checkout": true,
          "notebook.sync_pull": true,
          "notebook.sync_push": true,
          "notebook.subscribe": true,
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
      consoleEntries.push({
        appId: mount.appId,
        level: params.level || "info",
        message: params.message || "",
        createdAt: new Date().toISOString(),
      });
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
    const result = {
      ok: true,
      stateVersion: stateVersion,
      actions: devMockActionsForEvent(event),
    };
    devMockCoreEvents.push({
      appId: appId,
      event: cloneJson(event),
      result: cloneJson(result),
      createdAt: new Date().toISOString(),
    });
    return result;
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
      return [
        {
          type: "Toast",
          message: `Task accepted: ${devMockPayloadString(event.payload, "title") || "task"}`,
          level: "success",
        },
        { type: "Log", message: "CreateTask handled" },
      ];
    }
    if (event.type === "TransformText") {
      const text = devMockPayloadString(event.payload, "text") || "";
      const mode = devMockPayloadString(event.payload, "mode") || "uppercase";
      return [{ type: "TransformText", text: devMockTransformText(text, mode) }];
    }
    if (event.type === "NetworkSnapshotReceived") {
      return [{ type: "RenderHint", hint: "network-snapshot-received" }];
    }
    return [{ type: "Log", message: `Unhandled event: ${event.type}` }];
  }

  function devMockPayloadString(payload, field) {
    if (!payload || typeof payload !== "object" || Array.isArray(payload)) return null;
    return typeof payload[field] === "string" ? payload[field] : null;
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
    const handler = handlers && handlers.TerranePlatformBridge;
    if (!handler || typeof handler.postMessage !== "function") return null;
    return handler;
  }

  function androidNativeBridgeHandler() {
    const handler = window.TerranePlatformBridge;
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
      if (response && response.type === "runtime.mount_response" && responseId && webview2MountPending.has(responseId)) {
        const waiter = webview2MountPending.get(responseId);
        webview2MountPending.delete(responseId);
        clearTimeout(waiter.timeoutId);
        if (response.ok === true && typeof response.mountToken === "string" && response.mountToken.length > 0) {
          waiter.resolve(response.mountToken);
        } else {
          const error = response.error && response.error.message
            ? response.error.message
            : "WebView2 native mount token request failed";
          waiter.reject(new Error(error));
        }
        return;
      }
      if (!responseId || !webview2BridgePending.has(responseId)) return;
      const waiter = webview2BridgePending.get(responseId);
      webview2BridgePending.delete(responseId);
      waiter.resolve(response);
    });
    webview2BridgeHandlerAttached = true;
  }

  async function createRuntimeMountToken(app) {
    const webview2Token = await requestWebView2RuntimeMountToken(app.id);
    return webview2Token || createMountToken();
  }

  function requestWebView2RuntimeMountToken(appId) {
    const handler = window.chrome && window.chrome.webview;
    if (!handler || typeof handler.postMessage !== "function" || typeof handler.addEventListener !== "function") {
      return Promise.resolve(null);
    }
    attachWebView2BridgeHandler(handler);
    const id = `runtime_mount_${nextWebView2MountRequestId++}`;
    return new Promise(function (resolve, reject) {
      const timeoutId = setTimeout(function () {
        webview2MountPending.delete(id);
        reject(new Error("WebView2 native mount token request timed out"));
      }, 2000);
      webview2MountPending.set(id, { resolve: resolve, reject: reject, timeoutId: timeoutId });
      try {
        handler.postMessage(JSON.stringify({
          type: "runtime.mount_request",
          id: id,
          appId: appId,
        }));
      } catch (error) {
        clearTimeout(timeoutId);
        webview2MountPending.delete(id);
        reject(error);
      }
    });
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

  function installRuntimeDevtools() {
    if (!runtimeDevtoolsEnabled()) {
      try {
        delete window.__APP_RUNTIME_DEVTOOLS__;
      } catch (_) {
        window.__APP_RUNTIME_DEVTOOLS__ = undefined;
      }
      return;
    }

    window.__APP_RUNTIME_DEVTOOLS__ = {
      snapshot: runtimeDevtoolsSnapshot,
      query: runtimeDevtoolsQuery,
      bridgeLog: runtimeDevtoolsBridgeLog,
      consoleLog: runtimeDevtoolsConsoleLog,
      storageSnapshot: runtimeDevtoolsStorageSnapshot,
      coreEventLog: runtimeDevtoolsCoreEventLog,
      reset: runtimeDevtoolsReset,
    };
  }

  function runtimeDevtoolsEnabled() {
    if (window.__APP_RUNTIME_DEVTOOLS_ENABLED__ === true || window.__APP_RUNTIME_DEV_MOCK__ === true) {
      return true;
    }
    return false;
  }

  function runtimeDevtoolsSnapshot() {
    return {
      status: statusEl.textContent,
      activeApp: activeApp ? {
        appId: activeApp.id,
        name: activeApp.name,
        version: activeApp.version,
        description: activeApp.description,
      } : null,
      mounted: Boolean(activeFrame && activeMount),
      testIds: activeFrame ? testIdsFromHtml(activeFrame.srcdoc || "") : [],
      bridgeCalls: runtimeDevtoolsBridgeLog(),
      console: runtimeDevtoolsConsoleLog(),
    };
  }

  function runtimeDevtoolsQuery(query) {
    const html = activeFrame ? activeFrame.srcdoc || "" : "";
    const requestedTestId = typeof query === "string" && query[0] !== "[" ? query : query && query.testId;
    const selector = query && typeof query === "object" ? query.selector : null;
    const testId = requestedTestId || testIdFromSelector(typeof query === "string" ? query : selector);
    if (!testId) {
      return { count: 0, matches: [] };
    }
    const pattern = new RegExp("<([a-z0-9-]+)([^>]*\\bdata-testid\\s*=\\s*[\"']" + escapeRegExp(testId) + "[\"'][^>]*)>([\\s\\S]*?)<\\/\\1>", "i");
    const match = html.match(pattern);
    if (!match) {
      return { count: 0, matches: [] };
    }
    return {
      count: 1,
      matches: [{
        testId: testId,
        tagName: match[1].toLowerCase(),
        text: stripTags(match[3]).trim(),
      }],
    };
  }

  function runtimeDevtoolsBridgeLog() {
    return Array.from(bridgeLog.children || []).map(function (item) {
      return item.textContent;
    });
  }

  function runtimeDevtoolsConsoleLog() {
    return consoleEntries.map(cloneJson);
  }

  function runtimeDevtoolsStorageSnapshot(appId) {
    const targetAppId = appId || (activeApp && activeApp.id);
    const storage = targetAppId ? devMockStorageByApp.get(targetAppId) : null;
    return {
      appId: targetAppId || null,
      entries: storage
        ? Array.from(storage.entries()).map(function ([key, value]) {
            return { key: key, value: cloneJson(value) };
          })
        : [],
    };
  }

  function runtimeDevtoolsCoreEventLog(appId) {
    return devMockCoreEvents
      .filter(function (entry) {
        return !appId || entry.appId === appId;
      })
      .map(cloneJson);
  }

  function runtimeDevtoolsReset(appId) {
    const targetAppId = appId || (activeApp && activeApp.id);
    if (targetAppId) {
      devMockStorageByApp.delete(targetAppId);
      devMockCoreVersions.delete(targetAppId);
      for (let index = devMockCoreEvents.length - 1; index >= 0; index -= 1) {
        if (devMockCoreEvents[index].appId === targetAppId) {
          devMockCoreEvents.splice(index, 1);
        }
      }
    }
    bridgeLog.textContent = "";
    consoleEntries.length = 0;
    return { ok: true, appId: targetAppId || null };
  }

  function testIdsFromHtml(html) {
    return Array.from(html.matchAll(/\bdata-testid\s*=\s*["']([^"']+)["']/gi), function (match) {
      return match[1];
    }).sort();
  }

  function testIdFromSelector(selector) {
    if (typeof selector !== "string") return null;
    const match = selector.match(/\[data-testid=["']([^"']+)["']\]/);
    return match ? match[1] : null;
  }

  function stripTags(value) {
    return String(value || "").replace(/<[^>]*>/g, "");
  }

  function escapeRegExp(value) {
    return String(value).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
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
