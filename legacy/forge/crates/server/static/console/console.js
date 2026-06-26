(function () {
  "use strict";

  const state = {
    catalog: null,
    selected: null,
    schema: null,
    schemaMode: "raw",
    requestCounter: 0,
  };

  const els = {
    status: document.getElementById("console-status"),
    tierFilter: document.getElementById("tier-filter"),
    authToken: document.getElementById("auth-token"),
    reloadCatalog: document.getElementById("reload-catalog"),
    drainEvents: document.getElementById("drain-events"),
    catalogVersion: document.getElementById("catalog-version"),
    commandNav: document.getElementById("command-nav"),
    selectedCommand: document.getElementById("selected-command"),
    selectedSummary: document.getElementById("selected-summary"),
    commandBadges: document.getElementById("command-badges"),
    commandForm: document.getElementById("command-form"),
    schemaForm: document.getElementById("schema-form"),
    payloadJson: document.getElementById("payload-json"),
    runCommand: document.getElementById("run-command"),
    responseStatus: document.getElementById("response-status"),
    responseJson: document.getElementById("response-json"),
    eventsJson: document.getElementById("events-json"),
    confirmDialog: document.getElementById("confirm-dialog"),
    confirmMessage: document.getElementById("confirm-message"),
  };

  init();

  function init() {
    els.tierFilter.addEventListener("change", loadCatalog);
    els.reloadCatalog.addEventListener("click", loadCatalog);
    els.drainEvents.addEventListener("click", drainEvents);
    els.commandForm.addEventListener("submit", onSubmit);
    els.payloadJson.addEventListener("input", syncRawPayload);
    loadCatalog();
  }

  async function loadCatalog() {
    setStatus("Loading catalog…");
    try {
      const tier = els.tierFilter.value;
      const response = await bridge({
        name: "system.describe",
        payload: { tier },
      });
      if (!response.ok) {
        throw new Error(formatError(response.error));
      }
      state.catalog = response.payload;
      renderCatalog();
      setStatus(`Catalog loaded (${state.catalog.commands.length} commands)`, "ok");
    } catch (error) {
      state.catalog = null;
      els.commandNav.textContent = "";
      setStatus(error.message || String(error), "error");
    }
  }

  function renderCatalog() {
    const groups = new Map();
    for (const command of state.catalog.commands) {
      const bucket = groups.get(command.namespace) || [];
      bucket.push(command);
      groups.set(command.namespace, bucket);
    }

    els.catalogVersion.textContent = state.catalog.catalogVersion || "";
    els.commandNav.textContent = "";

    const namespaces = [...groups.keys()].sort();
    for (const namespace of namespaces) {
      const section = document.createElement("section");
      section.className = "namespace-group";
      section.dataset.namespace = namespace;

      const title = document.createElement("h3");
      title.className = "namespace-title";
      title.textContent = namespace;
      section.appendChild(title);

      const commands = groups.get(namespace).slice().sort((a, b) => a.name.localeCompare(b.name));
      for (const command of commands) {
        const button = document.createElement("button");
        button.type = "button";
        button.className = "command-button";
        button.dataset.commandName = command.name;
        button.innerHTML = `<strong>${escapeHtml(command.name)}</strong><span>${escapeHtml(command.summary)}</span>`;
        button.addEventListener("click", () => selectCommand(command));
        section.appendChild(button);
      }
      els.commandNav.appendChild(section);
    }

    if (state.selected) {
      const stillVisible = state.catalog.commands.some((entry) => entry.name === state.selected.name);
      if (stillVisible) {
        highlightSelected(state.selected.name);
        return;
      }
    }
    clearSelection();
  }

  async function selectCommand(command) {
    state.selected = command;
    highlightSelected(command.name);
    els.selectedCommand.textContent = command.name;
    els.selectedSummary.textContent = command.summary;
    renderBadges(command);
    els.runCommand.disabled = false;
    els.payloadJson.value = "{}";
    await buildFormForCommand(command);
  }

  function clearSelection() {
    state.selected = null;
    state.schema = null;
    state.schemaMode = "raw";
    els.selectedCommand.textContent = "Select a command";
    els.selectedSummary.textContent = "Choose a command from the catalog to build its payload.";
    els.commandBadges.textContent = "";
    els.schemaForm.textContent = "";
    els.payloadJson.value = "{}";
    els.runCommand.disabled = true;
  }

  function highlightSelected(name) {
    for (const button of els.commandNav.querySelectorAll(".command-button")) {
      button.setAttribute("aria-current", button.dataset.commandName === name ? "true" : "false");
    }
  }

  function renderBadges(command) {
    els.commandBadges.textContent = "";
    const badges = [
      ["tier", command.visibility],
      command.mutates ? ["mutates", "mutates"] : null,
      command.effectful ? ["effectful", "effectful"] : null,
      ["stability", command.stability],
    ].filter(Boolean);

    for (const [kind, label] of badges) {
      const span = document.createElement("span");
      span.className = `badge ${kind}`;
      span.textContent = label;
      els.commandBadges.appendChild(span);
    }
  }

  async function buildFormForCommand(command) {
    els.schemaForm.textContent = "";
    state.schema = null;
    state.schemaMode = "raw";

    if (!command.payload_schema) {
      return;
    }

    try {
      const response = await fetch(`/${command.payload_schema}`);
      if (!response.ok) {
        return;
      }
      state.schema = await response.json();
      state.schemaMode = "schema";
      const form = buildSchemaForm(state.schema, []);
      els.schemaForm.appendChild(form);
      syncSchemaToRaw();
    } catch (_error) {
      // Fall back to raw JSON textarea.
    }
  }

  function buildSchemaForm(schema, path, rootSchema) {
    rootSchema = rootSchema || schema;
    const resolved = resolveSchema(schema, rootSchema);
    const container = document.createElement("div");
    container.className = "schema-root";
    container.dataset.path = path.join(".");

    if (resolved.type === "object" || resolved.properties) {
      const fieldset = document.createElement("fieldset");
      fieldset.className = "fieldset";
      const legend = document.createElement("legend");
      legend.textContent = path.length ? path[path.length - 1] : "Payload";
      fieldset.appendChild(legend);

      const required = new Set(resolved.required || []);
      const propertyNames = Object.keys(resolved.properties || {}).sort();
      for (const name of propertyNames) {
        const childPath = path.concat(name);
        const childSchema = resolved.properties[name];
        const field = buildField(name, childSchema, childPath, rootSchema, required.has(name));
        fieldset.appendChild(field);
      }
      container.appendChild(fieldset);
      return container;
    }

    container.appendChild(buildWidget(resolved, path, rootSchema));
    return container;
  }

  function buildField(name, schema, path, rootSchema, required) {
    const wrapper = document.createElement("div");
    wrapper.className = "field" + (required ? " required" : "");

    const resolved = resolveSchema(schema, rootSchema);
    if (resolved.type === "object" || resolved.properties) {
      const nested = buildSchemaForm(resolved, path, rootSchema);
      wrapper.appendChild(nested);
      return wrapper;
    }

    if (resolved.type === "array") {
      const label = document.createElement("label");
      label.textContent = name;
      wrapper.appendChild(label);
      wrapper.appendChild(buildArrayEditor(resolved, path, rootSchema));
      return wrapper;
    }

    const label = document.createElement("label");
    label.textContent = name;
    label.htmlFor = pathId(path);
    wrapper.appendChild(label);
    wrapper.appendChild(buildWidget(resolved, path, rootSchema, name));
    return wrapper;
  }

  function buildArrayEditor(schema, path, rootSchema) {
    const resolved = resolveSchema(schema, rootSchema);
    const rows = document.createElement("div");
    rows.className = "array-rows";
    rows.dataset.path = path.join(".");

    const addButton = document.createElement("button");
    addButton.type = "button";
    addButton.textContent = "Add item";
    addButton.addEventListener("click", () => {
      rows.insertBefore(buildArrayRow(resolved.items || { type: "string" }, path, rootSchema), addButton);
      syncSchemaToRaw();
    });

    rows.appendChild(buildArrayRow(resolved.items || { type: "string" }, path, rootSchema));
    rows.appendChild(addButton);
    return rows;
  }

  function buildArrayRow(itemSchema, path, rootSchema) {
    const row = document.createElement("div");
    row.className = "array-row";
    const widgetWrap = document.createElement("div");
    widgetWrap.appendChild(buildWidget(resolveSchema(itemSchema, rootSchema), path, rootSchema));
    row.appendChild(widgetWrap);

    const remove = document.createElement("button");
    remove.type = "button";
    remove.textContent = "Remove";
    remove.addEventListener("click", () => {
      row.remove();
      syncSchemaToRaw();
    });
    row.appendChild(remove);
    return row;
  }

  function buildWidget(schema, path, rootSchema, name) {
    const resolved = resolveSchema(schema, rootSchema);
    const id = pathId(path);
    let input;

    if (Array.isArray(resolved.enum)) {
      input = document.createElement("select");
      input.id = id;
      input.dataset.path = path.join(".");
      for (const value of resolved.enum) {
        const option = document.createElement("option");
        option.value = String(value);
        option.textContent = String(value);
        input.appendChild(option);
      }
    } else if (resolved.type === "boolean") {
      input = document.createElement("input");
      input.type = "checkbox";
      input.id = id;
      input.dataset.path = path.join(".");
    } else if (resolved.type === "integer" || resolved.type === "number") {
      input = document.createElement("input");
      input.type = "number";
      input.id = id;
      input.dataset.path = path.join(".");
      if (resolved.type === "integer") {
        input.step = "1";
      }
    } else {
      input = document.createElement("input");
      input.type = "text";
      input.id = id;
      input.dataset.path = path.join(".");
      if (name) {
        input.placeholder = name;
      }
    }

    input.addEventListener("input", syncSchemaToRaw);
    input.addEventListener("change", syncSchemaToRaw);
    return input;
  }

  function resolveSchema(schema, rootSchema) {
    if (!schema || typeof schema !== "object") {
      return { type: "string" };
    }
    if (schema.$ref) {
      const key = schema.$ref.replace(/^#\//, "").split("/");
      let node = rootSchema;
      for (const part of key) {
        node = node && node[part];
      }
      return resolveSchema(node || { type: "string" }, rootSchema);
    }
    return schema;
  }

  function syncSchemaToRaw() {
    if (state.schemaMode !== "schema" || !state.schema) {
      return;
    }
    const value = readSchemaValue(state.schema, []);
    els.payloadJson.value = JSON.stringify(value, null, 2);
  }

  function readSchemaValue(schema, path, rootSchema) {
    rootSchema = rootSchema || schema;
    const resolved = resolveSchema(schema, rootSchema);

    if (resolved.type === "object" || resolved.properties) {
      const out = {};
      for (const [name, childSchema] of Object.entries(resolved.properties || {})) {
        const childPath = path.concat(name);
        const field = document.querySelector(`[data-path="${cssEscape(childPath.join("."))}"]`);
        if (!field) {
          continue;
        }
        if (field.classList && field.classList.contains("array-rows")) {
          out[name] = readArrayValue(childSchema, childPath, rootSchema);
          continue;
        }
        const nestedObject = field.querySelector(":scope > .schema-root .fieldset");
        if (nestedObject) {
          out[name] = readSchemaValue(childSchema, childPath, rootSchema);
          continue;
        }
        const widget = field.querySelector("input, select, textarea");
        if (!widget) {
          continue;
        }
        const value = readWidgetValue(widget);
        if (value !== undefined && value !== "") {
          out[name] = value;
        }
      }
      return out;
    }

    const widget = document.querySelector(`[data-path="${cssEscape(path.join("."))}"]`);
    return widget ? readWidgetValue(widget) : undefined;
  }

  function readArrayValue(schema, path, rootSchema) {
    const rows = document.querySelector(`[data-path="${cssEscape(path.join("."))}"]`);
    if (!rows) {
      return [];
    }
    const values = [];
    for (const row of rows.querySelectorAll(":scope > .array-row")) {
      const widget = row.querySelector("input, select, textarea");
      if (!widget) {
        continue;
      }
      const value = readWidgetValue(widget);
      if (value !== undefined && value !== "") {
        values.push(value);
      }
    }
    return values;
  }

  function readWidgetValue(widget) {
    if (widget.type === "checkbox") {
      return widget.checked;
    }
    if (widget.type === "number") {
      if (widget.value === "") {
        return undefined;
      }
      return widget.step === "1" ? parseInt(widget.value, 10) : Number(widget.value);
    }
    return widget.value;
  }

  function syncRawPayload() {
    if (state.schemaMode === "schema") {
      return;
    }
    // Raw mode only.
  }

  async function onSubmit(event) {
    event.preventDefault();
    if (!state.selected) {
      return;
    }

    let payload;
    try {
      payload = JSON.parse(els.payloadJson.value || "{}");
    } catch (error) {
      setStatus(`Invalid payload JSON: ${error.message}`, "error");
      return;
    }

    if (state.selected.mutates || state.selected.effectful) {
      const confirmed = await confirmCommand(state.selected);
      if (!confirmed) {
        return;
      }
    }

    setStatus(`Running ${state.selected.name}…`);
    try {
      const response = await bridge({
        name: state.selected.name,
        payload,
      });
      renderResponse(response);
      setStatus(response.ok ? `Ran ${state.selected.name}` : `Command failed`, response.ok ? "ok" : "error");
    } catch (error) {
      setStatus(error.message || String(error), "error");
    }
  }

  function confirmCommand(command) {
    const bits = [];
    if (command.mutates) {
      bits.push("mutates durable state");
    }
    if (command.effectful) {
      bits.push("may touch host effects");
    }
    els.confirmMessage.textContent = `${command.name} ${bits.join(" and ")}. Continue?`;
    els.confirmDialog.showModal();
    return new Promise((resolve) => {
      els.confirmDialog.addEventListener(
        "close",
        () => {
          resolve(els.confirmDialog.returnValue === "confirm");
        },
        { once: true },
      );
    });
  }

  async function drainEvents() {
    setStatus("Draining events…");
    try {
      const response = await fetch("/events/drain", {
        method: "POST",
        headers: authHeaders(),
      });
      const body = await response.json();
      els.eventsJson.textContent = JSON.stringify(body.events || [], null, 2);
      setStatus(`Drained ${Array.isArray(body.events) ? body.events.length : 0} events`, "ok");
    } catch (error) {
      setStatus(error.message || String(error), "error");
    }
  }

  function renderResponse(response) {
    els.responseStatus.textContent = response.ok ? "ok" : "error";
    els.responseJson.textContent = JSON.stringify(response, null, 2);
  }

  async function bridge(command) {
    const response = await fetch("/bridge", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        ...authHeaders(),
      },
      body: JSON.stringify({
        request_id: `console-${++state.requestCounter}`,
        actor: { actor: "console", role: "owner" },
        workspace_id: "console",
        applet_id: null,
        name: command.name,
        payload: command.payload || {},
      }),
    });
    return response.json();
  }

  function authHeaders() {
    const token = els.authToken.value.trim();
    if (!token) {
      return {};
    }
    return { authorization: `Bearer ${token}` };
  }

  function setStatus(message, kind) {
    els.status.textContent = message;
    els.status.classList.remove("ok", "error");
    if (kind) {
      els.status.classList.add(kind);
    }
  }

  function formatError(error) {
    if (!error) {
      return "unknown error";
    }
    if (typeof error === "string") {
      return error;
    }
    if (error.detail) {
      return `${error.kind || "error"}: ${error.detail}`;
    }
    return JSON.stringify(error);
  }

  function pathId(path) {
    return `field-${path.join("-")}`;
  }

  function cssEscape(value) {
    if (window.CSS && typeof window.CSS.escape === "function") {
      return window.CSS.escape(value);
    }
    return value.replace(/"/g, '\\"');
  }

  function escapeHtml(value) {
    return String(value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;");
  }
})();