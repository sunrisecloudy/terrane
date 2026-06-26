(function () {
  const engineRoomPreferenceKey = "terrane.engineRoom.visible";
  const engineRoomSectionOrder = [
    { key: "overview", title: "Overview", group: "runtime" },
    // Client-only section: the theme editor renders from local preferences, not
    // from a data snapshot, so it is excluded from the snapshot sectionKeys
    // contract below.
    { key: "appearance", title: "Appearance", group: "appearance", client: true },
    { key: "apps", title: "Apps", group: "runtime" },
    { key: "database", title: "Storage/DB", group: "data" },
    { key: "storage", title: "Storage Rows", group: "data" },
    { key: "bridgeCalls", title: "Bridge/API Calls", group: "activity" },
    { key: "network", title: "Network", group: "activity" },
    { key: "logs", title: "Logs/Telemetry", group: "activity" },
    { key: "core", title: "Core/Replay", group: "activity" },
    { key: "permissions", title: "Permissions/Policy", group: "policy" },
    { key: "tests", title: "Tests/Control", group: "quality" },
    { key: "crdt", title: "CRDT", group: "sync" },
    { key: "sync", title: "Sync", group: "sync" },
  ];
  const engineRoomGroups = [
    ["all", "All"],
    ["runtime", "Runtime"],
    ["appearance", "Appearance"],
    ["data", "Data"],
    ["activity", "Activity"],
    ["policy", "Policy"],
    ["quality", "Quality"],
    ["sync", "Sync"],
  ];

  // Canonical runtime constants. Hosts may override these in their snapshot
  // overview; the runtime fallback reads from here so the values live in one
  // place instead of being duplicated across runtime.js.
  const RUNTIME_VERSION = "0.1.0";
  const MAX_BRIDGE_CALLS_PER_MINUTE = 600;
  // Per-table display caps. Snapshots already bound row counts host-side; these
  // keep a single oversized collection from dominating the panel.
  const ROW_DISPLAY_LIMIT = 25;
  const MAX_TABLE_COLUMNS = 6;

  // Declarative bridge between the two snapshot shapes (rich host SQLite vs.
  // thin in-memory runtime fallback) and the renderer. Each collection lists
  // the candidate field names in priority order, so a single resolver
  // (pickArray) absorbs the drift instead of scattering `??` fallbacks.
  const engineRoomCollections = {
    apps: [
      { label: "Installed", fields: ["installed"], preferred: ["id", "appId", "name", "version"] },
      { label: "Registry", fields: ["rows"], preferred: ["id", "app_id", "name", "version", "status"] },
      { label: "Versions", fields: ["versions"], preferred: ["app_id", "version", "status", "trust_level"] },
      // Premium apps come from the runtime marketplace, not the local host, so
      // this collection is runtime-only and absent from host snapshots.
      { label: "Premium", fields: ["premium"], preferred: ["id", "name", "version"], optional: true },
    ],
    database: [
      { label: "Table counts", fields: ["__tableCounts"], preferred: ["table", "rows"] },
    ],
    storage: [
      { label: "Storage rows", fields: ["rows"], preferred: ["app_id", "appId", "key", "value", "value_json", "updated_at"] },
    ],
    bridgeCalls: [
      { label: "Bridge calls", fields: ["rows"], preferred: ["created_at", "app_id", "method", "params_json", "result_json", "error_json", "text"] },
    ],
    network: [
      { label: "Requests", fields: ["rows"], preferred: ["created_at", "app_id", "method", "params_json", "text"] },
      { label: "Mocks", fields: ["mocks"], preferred: ["app_id", "method", "url"] },
    ],
    logs: [
      { label: "App logs", fields: ["appLogRows", "console"], preferred: ["createdAt", "created_at", "appId", "app_id", "level", "message"] },
      { label: "Runtime sessions", fields: ["runtimeSessions"], preferred: ["app_id", "started_at", "status"] },
    ],
    core: [
      { label: "Events", fields: ["events"], preferred: ["created_at", "appId", "app_id", "type", "name"] },
      { label: "Actions", fields: ["actions"], preferred: ["created_at", "app_id", "type", "name"] },
      { label: "Snapshots", fields: ["snapshots"], preferred: ["snapshot_id", "app_id", "type", "created_at"] },
    ],
    permissions: [
      { label: "Permissions", fields: ["rows", "apps"], preferred: ["appId", "app_id", "permissions", "networkPolicy"] },
      { label: "Install reports", fields: ["installReports"], preferred: ["app_id", "status", "created_at"] },
    ],
    tests: [
      { label: "Test runs", fields: ["runs"], preferred: ["app_id", "status", "created_at"] },
      { label: "Control sessions", fields: ["controlSessions"], preferred: ["control_session_id", "app_id", "created_at"] },
      { label: "Control commands", fields: ["controlCommands"], preferred: ["control_session_id", "tool", "created_at"] },
    ],
    crdt: [
      { label: "Notebooks", fields: ["notebooks"], preferred: ["notebook_id", "app_id", "created_at"] },
      { label: "Documents", fields: ["documents"], preferred: ["document_id", "app_id"] },
      { label: "Updates", fields: ["updates"], preferred: ["app_id", "actor", "created_at"] },
      { label: "Actors", fields: ["actors"], preferred: ["app_id", "actor"] },
      { label: "Proposals", fields: ["proposals"], preferred: ["app_id", "status"] },
    ],
    sync: [
      { label: "Cursors", fields: ["cursors"], preferred: ["app_id", "cursor", "updated_at"] },
    ],
  };

  const columnLabels = {
    app_id: "App",
    appId: "App",
    value_json: "Value",
    params_json: "Params",
    result_json: "Result",
    error_json: "Error",
    bridge_call_id: "Call ID",
    control_session_id: "Session",
    snapshot_id: "Snapshot",
    notebook_id: "Notebook",
    document_id: "Document",
    created_at: "Created",
    updated_at: "Updated",
    started_at: "Started",
    activated_at: "Activated",
    createdAt: "Created",
    updatedAt: "Updated",
    networkPolicy: "Network policy",
  };

  // Neutral light palette used to seed colour pickers and the preview when the
  // active theme leaves a token unset (e.g. the "system" preset), so the editor
  // always shows a concrete colour even before the user overrides anything.
  const THEME_PREVIEW_FALLBACK = {
    accent: "#315efb",
    bg: "#f6f7fb",
    panel: "#ffffff",
    text: "#121826",
    muted: "#667085",
    border: "#dde2eb",
    danger: "#b42318",
  };

  function create(deps) {
    const dom = deps.dom;
    let activeGroup = "all";
    let filterText = "";
    let currentSnapshot = null;
    let selectedAppId = null;
    let appOptions = [];

    function showEngineRoom() {
      const activeMount = deps.getActiveMount();
      if (activeMount) {
        deps.portsByMountToken.delete(activeMount.mountToken);
      }
      deps.clearActiveMount();
      deps.renderAppList();
      dom.reloadButton.disabled = true;
      dom.activeTitle.textContent = "Engine Room";
      dom.activeDescription.textContent = "Inspect raw app and platform debug data.";
      dom.frameWrap.textContent = "";
      dom.frameWrap.appendChild(deps.element("div", "empty-state", "Engine Room is open."));
      document.body.classList.remove("marketplace-mode");
      document.body.classList.add("engine-room-mode");
      deps.setStatus("Engine Room");
      renderSnapshot();
    }

    function applyPreference() {
      const visible = isVisible();
      if (dom.entry) dom.entry.hidden = !visible;
    }

    function isVisible() {
      try {
        return window.localStorage?.getItem(engineRoomPreferenceKey) !== "false";
      } catch (_) {
        return true;
      }
    }

    function setVisible(visible) {
      try {
        if (visible) window.localStorage?.setItem(engineRoomPreferenceKey, "true");
        else window.localStorage?.setItem(engineRoomPreferenceKey, "false");
      } catch (_) {
        // Preference persistence is best-effort in embedded test/runtime contexts.
      }
      applyPreference();
    }

    async function renderSnapshot() {
      if (!dom.sections) return;
      setStatus("Loading");
      try {
        const activeApp = deps.getActiveApp();
        const appId = selectedAppId || (activeApp ? activeApp.id : null);
        currentSnapshot = await snapshot({ appId, limit: 50 });
        if (!selectedAppId) captureAppOptions(currentSnapshot);
        renderSnapshotContent();
        setStatus("Ready");
      } catch (error) {
        dom.sections.textContent = "";
        dom.sections.appendChild(renderCard({ key: "error", title: "Error", group: "runtime" }, {
          code: "engine_room_snapshot_failed",
          message: error && error.message ? error.message : String(error),
        }, [["Status", "Failed"]]));
        setStatus("Error");
      }
    }

    function renderSnapshotContent() {
      if (!dom.sections || !currentSnapshot) return;
      dom.sections.textContent = "";
      dom.sections.appendChild(renderSummary(currentSnapshot));
      dom.sections.appendChild(renderToolbar());
      const grid = deps.element("div", "engine-room-grid");
      let rendered = 0;
      for (const section of engineRoomSectionOrder) {
        if (section.key === "appearance") {
          const themeState = readThemeState();
          if (!themeState) continue;
          const summaryRows = themeSummaryRows(themeState);
          if (!matchesActiveView(section, themeFilterValue(themeState), summaryRows)) continue;
          grid.appendChild(renderThemeCard(section, themeState, summaryRows));
          rendered += 1;
          continue;
        }
        const value = currentSnapshot[section.key] ?? emptySection(section.key);
        const summaryRows = summarizeSection(section.key, value);
        if (!matchesActiveView(section, value, summaryRows)) continue;
        grid.appendChild(renderCard(section, value, summaryRows));
        rendered += 1;
      }
      if (rendered === 0) {
        grid.appendChild(renderEmptyResult());
      }
      dom.sections.appendChild(grid);
    }

    async function snapshot(options) {
      const hostSnapshot = await fetchHostSnapshot(options);
      if (hostSnapshot) return hostSnapshot;
      return runtimeSnapshot(options || {});
    }

    async function fetchHostSnapshot(options) {
      try {
        const response = await deps.fetchJson("/engine-room/snapshot", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(options || {}),
        });
        return response && response.ok === true && response.result ? response.result : response;
      } catch (_) {
        return null;
      }
    }

    function runtimeSnapshot(options) {
      const appId = options && typeof options.appId === "string" ? options.appId : null;
      const appRecords = deps.getApps().map(function (app) {
        return {
          id: app.id,
          name: app.name,
          version: app.version,
          description: app.description,
          contentRating: app.contentRating || null,
          permissions: app.permissions || [],
          capabilities: app.capabilities || [],
          resourceBudget: app.resourceBudget || {},
          networkPolicy: app.networkPolicy || {},
          storagePrefix: app.storagePrefix || null,
        };
      });
      const bridgeRows = dom.bridgeLog ? Array.from(dom.bridgeLog.children || []).map(function (row) {
        return { text: row.textContent || "" };
      }) : [];
      const storage = [];
      for (const [storageAppId, records] of deps.devMockStorageByApp.entries()) {
        if (appId && storageAppId !== appId) continue;
        for (const [key, value] of records.entries()) {
          storage.push({ app_id: storageAppId, key, value });
        }
      }
      const coreEvents = deps.devMockCoreEvents.filter(function (entry) {
        return !appId || entry.appId === appId;
      });
      const networkRows = bridgeRows.filter(function (entry) {
        return entry.text.includes("network.request");
      });
      const logRows = deps.consoleEntries.filter(function (entry) {
        return !appId || entry.appId === appId;
      });
      const activeApp = deps.getActiveApp();
      return {
        generatedAt: new Date().toISOString(),
        overview: {
          source: "runtime-web",
          activeAppId: activeApp ? activeApp.id : null,
          runtimeVersion: RUNTIME_VERSION,
          devMode: window.__APP_RUNTIME_DEV_MOCK__ === true,
          engineRoomVisible: isVisible(),
          hostMode: document.body.classList.contains("native-host-mode"),
          limits: {
            maxBridgeCallsPerMinute: MAX_BRIDGE_CALLS_PER_MINUTE,
          },
        },
        apps: {
          installed: appId ? appRecords.filter((app) => app.id === appId) : appRecords,
          rows: appId ? appRecords.filter((app) => app.id === appId) : appRecords,
          versions: [],
          premium: deps.premiumApps(),
        },
        database: {
          type: "runtime-memory",
          tableCounts: {
            app_storage: storage.length,
            bridge_calls: bridgeRows.length,
            core_events: coreEvents.length,
          },
        },
        storage: { rows: storage },
        bridgeCalls: { rows: bridgeRows },
        network: { rows: networkRows, mocks: [] },
        logs: { appLogRows: logRows, runtimeSessions: [], telemetry: { crashReporting: "not-configured" } },
        core: { events: coreEvents, actions: [], snapshots: [] },
        permissions: {
          rows: appRecords.map(function (app) {
            return { appId: app.id, permissions: app.permissions, networkPolicy: app.networkPolicy, resourceBudget: app.resourceBudget };
          }),
          installReports: [],
        },
        tests: { runs: [], controlSessions: [], controlCommands: [] },
        crdt: emptySection("crdt"),
        sync: emptySection("sync"),
      };
    }

    function emptySection(name) {
      return { status: "empty", rows: [], name };
    }

    function renderSummary(currentSnapshot) {
      const summary = deps.element("section", "engine-room-summary");
      summary.setAttribute("data-testid", "engine-room-summary");
      const overview = currentSnapshot.overview || {};
      const database = currentSnapshot.database || {};
      const apps = currentSnapshot.apps || {};
      const bridgeCalls = currentSnapshot.bridgeCalls || {};
      const logs = currentSnapshot.logs || {};
      summary.appendChild(metric("Source", overview.source || "local runtime"));
      summary.appendChild(metric("Scope", selectedAppId || overview.activeAppId || overview.appId || "all apps"));
      summary.appendChild(metric("Apps", String(countItems(pickArray(apps, ["installed", "rows"])))));
      summary.appendChild(metric("DB rows", String(sumTableCounts(database.tableCounts))));
      summary.appendChild(metric("Bridge calls", String(countItems(bridgeCalls.rows))));
      summary.appendChild(metric("Logs", String(countItems(pickArray(logs, ["appLogRows", "console"])))));
      summary.appendChild(metric("Updated", relativeTime(currentSnapshot.generatedAt)));
      return summary;
    }

    function renderToolbar() {
      const toolbar = deps.element("div", "engine-room-toolbar");
      toolbar.setAttribute("data-testid", "engine-room-toolbar");

      const controls = deps.element("div", "engine-room-controls");
      controls.appendChild(renderAppSelect());

      const search = deps.element("input", "engine-room-filter");
      search.setAttribute("type", "search");
      search.setAttribute("data-testid", "engine-room-filter");
      search.setAttribute("placeholder", "Filter");
      search.setAttribute("aria-label", "Filter Engine Room sections");
      search.value = filterText;
      search.addEventListener("input", function (event) {
        filterText = String(event.target.value || "").trim().toLowerCase();
        renderSnapshotContent();
      });
      controls.appendChild(search);
      controls.appendChild(renderToolbarActions());
      toolbar.appendChild(controls);

      const tabs = deps.element("div", "engine-room-tabs");
      tabs.setAttribute("role", "tablist");
      for (const [group, label] of engineRoomGroups) {
        const button = deps.element("button", group === activeGroup ? "engine-room-tab active" : "engine-room-tab");
        button.setAttribute("type", "button");
        button.setAttribute("role", "tab");
        button.setAttribute("aria-selected", group === activeGroup ? "true" : "false");
        button.textContent = `${label} ${groupCount(group)}`;
        button.addEventListener("click", function () {
          activeGroup = group;
          renderSnapshotContent();
        });
        tabs.appendChild(button);
      }
      toolbar.appendChild(tabs);
      return toolbar;
    }

    function metric(label, value) {
      const item = deps.element("div", "engine-room-metric");
      item.appendChild(deps.element("span", "", label));
      item.appendChild(deps.element("strong", "", String(value == null ? "unknown" : value)));
      return item;
    }

    function summarizeSection(key, value) {
      switch (key) {
      case "overview":
        return [
          ["Source", value.source || "runtime"],
          ["Active app", value.activeAppId || value.appId || "none"],
          ["Runtime", value.runtimeVersion || "unknown"],
          ["Mode", value.devMode ? "developer" : "normal"],
        ];
      case "apps":
        return [
          ["Installed apps", countItems(value.installed)],
          ["Registry rows", countItems(value.rows)],
          ["Versions", countItems(value.versions)],
          ["Package files", countItems(value.packageFiles)],
        ];
      case "database":
        return [
          ["Type", value.type || "unknown"],
          ["Path", value.path || "in-memory"],
          ["Integrity", value.integrity || "not checked"],
          ["Total rows", sumTableCounts(value.tableCounts)],
        ];
      case "storage":
        return [["Storage rows", countItems(value.rows)]];
      case "bridgeCalls":
        return [["Recent calls", countItems(value.rows)]];
      case "network":
        return [["Requests", countItems(value.rows)], ["Mocks", countItems(value.mocks)]];
      case "logs":
        return [
          ["App logs", countItems(pickArray(value, ["appLogRows", "console"]))],
          ["Runtime sessions", countItems(value.runtimeSessions)],
          ["Crash reporting", value.telemetry?.crashReporting || "not configured"],
        ];
      case "core":
        return [
          ["Events", countItems(value.events)],
          ["Actions", countItems(value.actions)],
          ["Snapshots", countItems(value.snapshots)],
        ];
      case "permissions":
        return [
          ["Permission rows", countItems(pickArray(value, ["rows", "apps"]))],
          ["Install reports", countItems(value.installReports)],
        ];
      case "tests":
        return [
          ["Test runs", countItems(value.runs)],
          ["Control sessions", countItems(value.controlSessions)],
          ["Control commands", countItems(value.controlCommands)],
        ];
      case "crdt":
        return [
          ["Notebooks", countItems(value.notebooks)],
          ["Updates", countItems(value.updates)],
          ["Actors", countItems(value.actors)],
          ["Proposals", countItems(value.proposals)],
        ];
      case "sync":
        return [
          ["Cursors", countItems(value.cursors)],
          ["Server", value.server?.status || "not attached"],
        ];
      default:
        return [["Status", value.status || "available"]];
      }
    }

    function renderCard(section, value, summaryRows) {
      const card = deps.element("article", "engine-room-card");
      card.setAttribute("data-testid", `engine-room-${section.title.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`);
      const header = deps.element("div", "engine-room-card-header");
      const titleWrap = deps.element("div", "engine-room-card-title");
      titleWrap.appendChild(deps.element("h3", "", section.title));
      titleWrap.appendChild(deps.element("span", "engine-room-card-group", groupLabel(section.group)));
      header.appendChild(titleWrap);
      const state = sectionState(value);
      const badges = deps.element("div", "engine-room-card-badges");
      badges.appendChild(deps.element("span", `engine-room-state ${state.className}`, state.label));
      badges.appendChild(deps.element("span", "engine-room-card-count", sectionCountLabel(value)));
      header.appendChild(badges);
      card.appendChild(header);
      const summary = deps.element("dl", "engine-room-facts");
      for (const [label, factValue] of summaryRows || []) {
        const row = deps.element("div", "engine-room-fact");
        row.appendChild(deps.element("dt", "", label));
        row.appendChild(deps.element("dd", "", String(factValue == null ? "unknown" : factValue)));
        summary.appendChild(row);
      }
      card.appendChild(summary);
      const tables = renderCollectionTables(section, value);
      if (tables) card.appendChild(tables);
      const details = deps.element("details", "engine-room-raw");
      const rawSummary = deps.element("summary");
      rawSummary.appendChild(deps.element("span", "", "Raw JSON"));
      rawSummary.appendChild(copyButton("Copy", function () {
        copyText(JSON.stringify(value, null, 2));
      }));
      details.appendChild(rawSummary);
      details.appendChild(deps.element("pre", "", JSON.stringify(value, null, 2)));
      card.appendChild(details);
      return card;
    }

    function renderEmptyResult() {
      const empty = deps.element("div", "engine-room-empty-result");
      empty.appendChild(deps.element("strong", "", "No matching sections"));
      empty.appendChild(deps.element("span", "", "Try another filter or section group."));
      return empty;
    }

    function matchesActiveView(section, value, summaryRows) {
      if (activeGroup !== "all" && section.group !== activeGroup) return false;
      if (!filterText) return true;
      const haystack = [
        section.title,
        groupLabel(section.group),
        JSON.stringify(summaryRows),
        JSON.stringify(value),
      ].join(" ").toLowerCase();
      return haystack.includes(filterText);
    }

    function groupCount(group) {
      if (!currentSnapshot) return 0;
      return engineRoomSectionOrder.filter(function (section) {
        if (group !== "all" && section.group !== group) return false;
        if (section.key === "appearance") {
          const themeState = readThemeState();
          if (!themeState) return false;
          return matchesTextOnly(section, themeFilterValue(themeState), themeSummaryRows(themeState));
        }
        const value = currentSnapshot[section.key] ?? emptySection(section.key);
        return matchesTextOnly(section, value, summarizeSection(section.key, value));
      }).length;
    }

    function matchesTextOnly(section, value, summaryRows) {
      if (!filterText) return true;
      const haystack = [
        section.title,
        groupLabel(section.group),
        JSON.stringify(summaryRows),
        JSON.stringify(value),
      ].join(" ").toLowerCase();
      return haystack.includes(filterText);
    }

    function groupLabel(group) {
      const found = engineRoomGroups.find(function (entry) {
        return entry[0] === group;
      });
      return found ? found[1] : group;
    }

    function sectionState(value) {
      if (value && value.code) return { label: "Error", className: "error" };
      const count = countRows(value);
      if (count > 0) return { label: "Active", className: "active" };
      return { label: "Empty", className: "empty" };
    }

    function sectionCountLabel(value) {
      const count = countRows(value);
      if (count === 0) return "No rows";
      if (count === 1) return "1 row";
      return `${count} rows`;
    }

    function countRows(value) {
      if (!value || typeof value !== "object") return 0;
      if (Array.isArray(value)) return value.length;
      let total = 0;
      for (const item of Object.values(value)) {
        if (Array.isArray(item)) total += item.length;
      }
      return total;
    }

    function countItems(value) {
      return Array.isArray(value) ? value.length : 0;
    }

    function sumTableCounts(value) {
      if (!value || typeof value !== "object") return 0;
      return Object.values(value).reduce(function (total, count) {
        return total + (typeof count === "number" ? count : 0);
      }, 0);
    }

    function pickArray(value, fields) {
      if (!value || typeof value !== "object") return [];
      for (const field of fields) {
        if (Array.isArray(value[field])) return value[field];
      }
      return [];
    }

    function captureAppOptions(snapshotValue) {
      const apps = snapshotValue && snapshotValue.apps ? snapshotValue.apps : {};
      const seen = new Set();
      const options = [];
      for (const app of pickArray(apps, ["installed", "rows"])) {
        const id = app && (app.id || app.appId || app.app_id);
        if (!id || seen.has(id)) continue;
        seen.add(id);
        options.push({ id, name: app.name || id });
      }
      appOptions = options;
    }

    function renderAppSelect() {
      const wrap = deps.element("label", "engine-room-scope");
      wrap.appendChild(deps.element("span", "engine-room-scope-label", "App"));
      const select = deps.element("select", "engine-room-select");
      select.setAttribute("data-testid", "engine-room-app-select");
      select.setAttribute("aria-label", "Scope Engine Room to an app");
      appendOption(select, "", "All apps", selectedAppId);
      for (const option of appOptions) {
        appendOption(select, option.id, option.name, selectedAppId);
      }
      select.value = selectedAppId || "";
      select.addEventListener("change", function (event) {
        selectedAppId = String(event.target.value || "") || null;
        renderSnapshot();
      });
      wrap.appendChild(select);
      return wrap;
    }

    function appendOption(select, value, label, current) {
      const option = deps.element("option", "", label);
      option.setAttribute("value", value);
      if ((current || "") === value) option.setAttribute("selected", "selected");
      select.appendChild(option);
    }

    function renderToolbarActions() {
      const actions = deps.element("div", "engine-room-tools");
      actions.appendChild(copyButton("Copy snapshot", function () {
        if (currentSnapshot) copyText(JSON.stringify(currentSnapshot, null, 2));
      }));
      const download = deps.element("button", "engine-room-tool");
      download.setAttribute("type", "button");
      download.setAttribute("data-testid", "engine-room-download");
      download.textContent = "Download";
      download.addEventListener("click", downloadSnapshot);
      actions.appendChild(download);
      return actions;
    }

    function copyButton(label, handler) {
      const button = deps.element("button", "engine-room-tool");
      button.setAttribute("type", "button");
      button.textContent = label;
      button.addEventListener("click", function (event) {
        if (event && typeof event.preventDefault === "function") event.preventDefault();
        handler();
      });
      return button;
    }

    function copyText(text) {
      try {
        if (typeof navigator !== "undefined" && navigator.clipboard && navigator.clipboard.writeText) {
          navigator.clipboard.writeText(text);
        }
      } catch (_) {
        // Clipboard access is best-effort; ignore when unavailable or blocked.
      }
    }

    function downloadSnapshot() {
      if (!currentSnapshot) return;
      const text = JSON.stringify(currentSnapshot, null, 2);
      try {
        if (typeof Blob === "undefined" || typeof URL === "undefined" || !URL.createObjectURL) {
          copyText(text);
          return;
        }
        const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
        const anchor = document.createElement("a");
        anchor.href = url;
        anchor.download = `engine-room-${(selectedAppId || "all").replace(/[^a-z0-9_-]+/gi, "-")}.json`;
        if (typeof anchor.click === "function") anchor.click();
        if (URL.revokeObjectURL) URL.revokeObjectURL(url);
      } catch (_) {
        copyText(text);
      }
    }

    function renderCollectionTables(section, value) {
      const collections = engineRoomCollections[section.key];
      if (!collections) return null;
      const wrap = deps.element("div", "engine-room-tables");
      let rendered = 0;
      for (const collection of collections) {
        const rows = collectionRows(collection, value);
        if (!rows.length) continue;
        wrap.appendChild(renderRowTable(collection, rows));
        rendered += 1;
      }
      return rendered > 0 ? wrap : null;
    }

    function collectionRows(collection, value) {
      if (collection.fields[0] === "__tableCounts") {
        const counts = value && value.tableCounts;
        if (!counts || typeof counts !== "object") return [];
        return Object.keys(counts)
          .filter(function (table) {
            return typeof counts[table] === "number" && counts[table] > 0;
          })
          .map(function (table) {
            return { table, rows: counts[table] };
          });
      }
      return pickArray(value, collection.fields);
    }

    function renderRowTable(collection, rows) {
      const block = deps.element("div", "engine-room-table-block");
      const caption = deps.element("div", "engine-room-table-caption");
      caption.appendChild(deps.element("span", "", collection.label));
      caption.appendChild(deps.element("span", "engine-room-table-count", String(rows.length)));
      block.appendChild(caption);

      const shown = rows.slice(0, ROW_DISPLAY_LIMIT);
      const columns = deriveColumns(shown, collection.preferred);
      const table = deps.element("table", "engine-room-table");
      const thead = deps.element("thead");
      const headRow = deps.element("tr");
      for (const column of columns) {
        headRow.appendChild(deps.element("th", "", columnLabel(column)));
      }
      thead.appendChild(headRow);
      table.appendChild(thead);

      const tbody = deps.element("tbody");
      for (const row of shown) {
        const tr = deps.element("tr");
        for (const column of columns) {
          const td = deps.element("td", "engine-room-cell");
          td.appendChild(formatCell(readField(row, column), column));
          tr.appendChild(td);
        }
        tbody.appendChild(tr);
      }
      table.appendChild(tbody);
      block.appendChild(table);
      if (rows.length > shown.length) {
        block.appendChild(deps.element("div", "engine-room-table-more", `+${rows.length - shown.length} more rows`));
      }
      return block;
    }

    function readField(row, column) {
      if (column === "__value") return row;
      return row ? row[column] : undefined;
    }

    function deriveColumns(rows, preferred) {
      const seen = new Set();
      const order = [];
      let scalarOnly = true;
      for (const row of rows) {
        if (row && typeof row === "object" && !Array.isArray(row)) {
          scalarOnly = false;
          for (const key of Object.keys(row)) {
            if (!seen.has(key)) {
              seen.add(key);
              order.push(key);
            }
          }
        }
      }
      if (scalarOnly || order.length === 0) return ["__value"];
      const columns = [];
      for (const key of preferred || []) {
        if (seen.has(key) && !columns.includes(key)) columns.push(key);
      }
      for (const key of order) {
        if (!columns.includes(key)) columns.push(key);
      }
      return columns.slice(0, MAX_TABLE_COLUMNS);
    }

    function columnLabel(column) {
      if (column === "__value") return "Value";
      if (columnLabels[column]) return columnLabels[column];
      return column
        .replace(/_/g, " ")
        .replace(/([a-z])([A-Z])/g, "$1 $2")
        .replace(/^./, function (char) {
          return char.toUpperCase();
        });
    }

    function isTimestampColumn(column) {
      return /(_at|At)$/.test(column) || column === "generatedAt";
    }

    function formatCell(value, column) {
      if (value === null || value === undefined || value === "") {
        return deps.element("span", "engine-room-cell-empty", "—");
      }
      if (typeof value === "string" && isTimestampColumn(column)) {
        const node = deps.element("span", "engine-room-cell-time", relativeTime(value));
        node.setAttribute("title", value);
        return node;
      }
      if (typeof value === "object") {
        const json = JSON.stringify(value);
        const node = deps.element("code", "engine-room-cell-json", truncate(json, 80));
        if (json.length > 80) node.setAttribute("title", json);
        return node;
      }
      const text = String(value);
      const node = deps.element("span", "", truncate(text, 120));
      if (text.length > 120) node.setAttribute("title", text);
      return node;
    }

    function relativeTime(iso) {
      if (!iso) return "unknown";
      const then = Date.parse(iso);
      if (Number.isNaN(then)) return String(iso);
      const seconds = Math.round((Date.now() - then) / 1000);
      if (seconds < 0) return "just now";
      if (seconds < 5) return "just now";
      if (seconds < 60) return `${seconds}s ago`;
      const minutes = Math.round(seconds / 60);
      if (minutes < 60) return `${minutes}m ago`;
      const hours = Math.round(minutes / 60);
      if (hours < 24) return `${hours}h ago`;
      const days = Math.round(hours / 24);
      return `${days}d ago`;
    }

    function truncate(text, max) {
      return text.length > max ? `${text.slice(0, max - 1)}…` : text;
    }

    // ----- Appearance / custom theme -----

    function themeApi() {
      if (deps.theme) return deps.theme;
      return typeof window !== "undefined" ? window.TerraneTheme || null : null;
    }

    // A render-ready view of the active theme: the persisted choice plus the
    // resolved token map. Returns null when no theme module is available so the
    // Appearance section degrades to hidden rather than throwing.
    function readThemeState() {
      const api = themeApi();
      if (!api) return null;
      try {
        return { api, current: api.get(), tokens: api.resolveTokens() };
      } catch (_) {
        return null;
      }
    }

    function themePresetLabel(state) {
      if (state.current.presetId === state.api.CUSTOM_PRESET_ID) return "Custom";
      const preset = state.api.presetById(state.current.presetId);
      return preset ? preset.name : state.current.presetId;
    }

    function themeSummaryRows(state) {
      return [
        ["Preset", themePresetLabel(state)],
        ["Accent", state.tokens.accent || "app default"],
        ["Background", state.tokens.bg || "app default"],
        ["Custom tokens", Object.keys(state.tokens).length],
      ];
    }

    function themeFilterValue(state) {
      return {
        keywords: "appearance theme colour color palette dark light accent contrast",
        preset: state.current.presetId,
        tokens: state.tokens,
      };
    }

    function renderThemeCard(section, state, summaryRows) {
      const api = state.api;
      const card = deps.element("article", "engine-room-card engine-room-theme-card");
      card.setAttribute("data-testid", "engine-room-appearance");

      const header = deps.element("div", "engine-room-card-header");
      const titleWrap = deps.element("div", "engine-room-card-title");
      titleWrap.appendChild(deps.element("h3", "", section.title));
      titleWrap.appendChild(deps.element("span", "engine-room-card-group", groupLabel(section.group)));
      header.appendChild(titleWrap);
      const overrides = Object.keys(state.tokens).length > 0;
      const badges = deps.element("div", "engine-room-card-badges");
      badges.appendChild(deps.element(
        "span",
        `engine-room-state ${overrides ? "active" : "empty"}`,
        overrides ? "Custom theme" : "App default",
      ));
      header.appendChild(badges);
      card.appendChild(header);

      card.appendChild(deps.element(
        "p",
        "engine-room-theme-intro",
        "Pick a palette or set your own colours. The theme applies to every app you open.",
      ));

      const facts = deps.element("dl", "engine-room-facts");
      for (const [label, factValue] of summaryRows) {
        const row = deps.element("div", "engine-room-fact");
        row.appendChild(deps.element("dt", "", label));
        row.appendChild(deps.element("dd", "", String(factValue == null ? "unknown" : factValue)));
        facts.appendChild(row);
      }
      card.appendChild(facts);

      card.appendChild(renderThemePresets(state));
      card.appendChild(renderThemeColorControls(state));
      card.appendChild(renderThemePreview(state));

      const actions = deps.element("div", "engine-room-theme-actions");
      const reset = deps.element("button", "engine-room-tool", "Reset to system default");
      reset.setAttribute("type", "button");
      reset.setAttribute("data-testid", "engine-room-theme-reset");
      reset.addEventListener("click", function () {
        api.reset();
        renderSnapshotContent();
      });
      actions.appendChild(reset);
      card.appendChild(actions);
      return card;
    }

    function renderThemePresets(state) {
      const api = state.api;
      const wrap = deps.element("div", "engine-room-theme-presets");
      wrap.setAttribute("role", "list");
      for (const preset of api.presets()) {
        const active = preset.id === state.current.presetId;
        const chip = deps.element("button", active ? "engine-room-theme-preset active" : "engine-room-theme-preset");
        chip.setAttribute("type", "button");
        chip.setAttribute("role", "listitem");
        chip.setAttribute("data-testid", `engine-room-theme-preset-${preset.id}`);
        chip.setAttribute("aria-pressed", active ? "true" : "false");
        chip.setAttribute("title", preset.description || preset.name);
        const swatches = deps.element("span", "engine-room-theme-swatches");
        for (const name of ["accent", "bg", "panel", "text"]) {
          const dot = deps.element("span", "engine-room-theme-swatch");
          const value = preset.tokens[name];
          if (value && dot.style) dot.style.background = value;
          swatches.appendChild(dot);
        }
        chip.appendChild(swatches);
        chip.appendChild(deps.element("span", "engine-room-theme-preset-name", preset.name));
        chip.addEventListener("click", function () {
          api.selectPreset(preset.id);
          renderSnapshotContent();
        });
        wrap.appendChild(chip);
      }
      return wrap;
    }

    function renderThemeColorControls(state) {
      const api = state.api;
      const wrap = deps.element("div", "engine-room-theme-colors");
      for (const token of api.tokens()) {
        if (!token.editable) continue;
        const field = deps.element("label", "engine-room-theme-color");
        field.setAttribute("title", token.hint || token.label);
        const input = deps.element("input", "engine-room-theme-color-input");
        input.setAttribute("type", "color");
        input.setAttribute("data-testid", `engine-room-theme-color-${token.name}`);
        input.setAttribute("aria-label", `${token.label} colour`);
        input.value = toHexColor(state.tokens[token.name] || THEME_PREVIEW_FALLBACK[token.name]);
        // `change` (commit) rather than `input` (per-drag) keeps the native
        // colour picker open while dragging instead of re-rendering mid-pick.
        input.addEventListener("change", function (event) {
          const next = event && event.target && event.target.value != null ? event.target.value : input.value;
          api.setToken(token.name, next);
          renderSnapshotContent();
        });
        field.appendChild(input);
        field.appendChild(deps.element("span", "engine-room-theme-color-label", token.label));
        wrap.appendChild(field);
      }
      return wrap;
    }

    function renderThemePreview(state) {
      function tok(name) {
        return state.tokens[name] || THEME_PREVIEW_FALLBACK[name];
      }
      const preview = deps.element("div", "engine-room-theme-preview");
      preview.setAttribute("data-testid", "engine-room-theme-preview");
      preview.setAttribute("aria-hidden", "true");
      if (preview.style) {
        preview.style.background = tok("bg");
        preview.style.borderColor = tok("border");
      }
      const panel = deps.element("div", "engine-room-theme-preview-panel");
      if (panel.style) {
        panel.style.background = tok("panel");
        panel.style.borderColor = tok("border");
      }
      const title = deps.element("div", "engine-room-theme-preview-title", "Aa Preview");
      if (title.style) title.style.color = tok("text");
      const muted = deps.element("div", "engine-room-theme-preview-muted", "Secondary text");
      if (muted.style) muted.style.color = tok("muted");
      const row = deps.element("div", "engine-room-theme-preview-row");
      const primary = deps.element("span", "engine-room-theme-preview-btn", "Primary");
      if (primary.style) {
        primary.style.background = tok("accent");
        primary.style.color = "#ffffff";
      }
      const danger = deps.element("span", "engine-room-theme-preview-btn ghost", "Delete");
      if (danger.style) {
        danger.style.color = tok("danger");
        danger.style.borderColor = tok("border");
      }
      row.appendChild(primary);
      row.appendChild(danger);
      panel.appendChild(title);
      panel.appendChild(muted);
      panel.appendChild(row);
      preview.appendChild(panel);
      return preview;
    }

    function toHexColor(value) {
      if (typeof value !== "string") return "#000000";
      const trimmed = value.trim();
      if (/^#[0-9a-f]{6}$/i.test(trimmed)) return trimmed.toLowerCase();
      if (/^#[0-9a-f]{3}$/i.test(trimmed)) {
        return `#${trimmed.slice(1).split("").map(function (char) { return char + char; }).join("").toLowerCase()}`;
      }
      return "#000000";
    }

    function setStatus(value) {
      if (dom.status) dom.status.textContent = value;
    }

    return {
      applyPreference,
      isVisible,
      renderSnapshot,
      setVisible,
      showEngineRoom,
      snapshot,
    };
  }

  window.TerraneEngineRoom = {
    create,
    // Canonical snapshot contract: the section keys (and their collections) the
    // renderer understands. Both the host snapshot and the runtime fallback are
    // expected to conform to this shape.
    sectionKeys: engineRoomSectionOrder.filter(function (section) {
      return !section.client;
    }).map(function (section) {
      return section.key;
    }),
    groups: engineRoomGroups.map(function (group) {
      return group[0];
    }),
    collections: engineRoomCollections,
  };
})();
