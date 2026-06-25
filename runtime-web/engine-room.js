(function () {
  const engineRoomPreferenceKey = "terrane.engineRoom.visible";
  const engineRoomSectionOrder = [
    { key: "overview", title: "Overview", group: "runtime" },
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
    ["data", "Data"],
    ["activity", "Activity"],
    ["policy", "Policy"],
    ["quality", "Quality"],
    ["sync", "Sync"],
  ];

  function create(deps) {
    const dom = deps.dom;
    let activeGroup = "all";
    let filterText = "";
    let currentSnapshot = null;

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
        currentSnapshot = await snapshot({ appId: activeApp ? activeApp.id : null, limit: 50 });
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
          storage.push({ appId: storageAppId, key, value });
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
          runtimeVersion: "0.1.0",
          devMode: window.__APP_RUNTIME_DEV_MOCK__ === true,
          engineRoomVisible: isVisible(),
          hostMode: document.body.classList.contains("native-host-mode"),
          limits: {
            maxBridgeCallsPerMinute: 600,
          },
        },
        apps: {
          installed: appId ? appRecords.filter((app) => app.id === appId) : appRecords,
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
        logs: { console: logRows, telemetry: { crashReporting: "not-configured" } },
        core: { events: coreEvents, actions: [], snapshots: [] },
        permissions: {
          apps: appRecords.map(function (app) {
            return { appId: app.id, permissions: app.permissions, networkPolicy: app.networkPolicy, resourceBudget: app.resourceBudget };
          }),
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
      summary.appendChild(metric("Active app", overview.activeAppId || overview.appId || "none"));
      summary.appendChild(metric("Apps", String(countItems(apps.rows) + countItems(apps.installed))));
      summary.appendChild(metric("DB rows", String(sumTableCounts(database.tableCounts))));
      summary.appendChild(metric("Bridge calls", String(countItems(bridgeCalls.rows))));
      summary.appendChild(metric("Logs", String(countItems(logs.appLogRows) + countItems(logs.console))));
      return summary;
    }

    function renderToolbar() {
      const toolbar = deps.element("div", "engine-room-toolbar");
      toolbar.setAttribute("data-testid", "engine-room-toolbar");
      const search = deps.element("input", "engine-room-filter");
      search.setAttribute("type", "search");
      search.setAttribute("placeholder", "Filter");
      search.setAttribute("aria-label", "Filter Engine Room sections");
      search.value = filterText;
      search.addEventListener("input", function (event) {
        filterText = String(event.target.value || "").trim().toLowerCase();
        renderSnapshotContent();
      });
      toolbar.appendChild(search);

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
          ["App logs", countItems(value.appLogRows) + countItems(value.console)],
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
          ["Permission rows", countItems(value.rows)],
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
      const details = deps.element("details", "engine-room-raw");
      details.appendChild(deps.element("summary", "", "Raw JSON"));
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

  window.TerraneEngineRoom = { create };
})();
