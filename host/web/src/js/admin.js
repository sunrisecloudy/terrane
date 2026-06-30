const state = {
  view: "apps",
  apps: [],
  grants: [],
  agents: [],
  requests: [],
  audit: [],
  session: null,
};

const content = document.getElementById("content");
const title = document.getElementById("view-title");
const authority = document.getElementById("authority");
const lockButton = document.getElementById("lock-toggle");
const adminHeaders = { "X-Terrane-Admin": "local-admin" };

document.querySelectorAll("nav button[data-view]").forEach((button) => {
  button.addEventListener("click", () => {
    state.view = button.dataset.view;
    document.querySelectorAll("nav button[data-view]").forEach((item) => {
      item.classList.toggle("active", item === button);
    });
    render();
  });
});

document.getElementById("refresh").addEventListener("click", refresh);
lockButton.addEventListener("click", async () => {
  const locked = Boolean(state.session && state.session.locked);
  await fetchJson(`/__terrane/admin/local/${locked ? "unlock" : "lock"}`, { method: "POST" });
  await refresh();
});

async function refresh() {
  const [session, apps, grants, agents, requests, audit] = await Promise.all([
    fetchJson("/__terrane/admin/session"),
    fetchJson("/__terrane/admin/apps"),
    fetchJson("/__terrane/admin/grants"),
    fetchJson("/__terrane/admin/agents"),
    fetchJson("/__terrane/admin/requests"),
    fetchJson("/__terrane/admin/audit"),
  ]);
  state.session = session;
  state.apps = apps.apps || [];
  state.grants = grants.grants || [];
  state.agents = agents.agents || [];
  state.requests = requests.requests || [];
  state.audit = audit.entries || [];
  render();
}

async function fetchJson(path, options) {
  const response = await fetch(path, {
    cache: "no-store",
    ...options,
    headers: { ...adminHeaders, ...((options && options.headers) || {}) },
  });
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

function render() {
  const session = state.session || { org: "local", subject: "user:local-owner", source: "local" };
  authority.textContent = `${session.org} / ${session.subject} / ${session.source}${session.locked ? " / locked" : ""}`;
  lockButton.textContent = session.locked ? "Unlock" : "Lock";
  title.textContent = state.view[0].toUpperCase() + state.view.slice(1);
  if (state.view === "grants") return renderGrants();
  if (state.view === "agents") return renderAgents();
  if (state.view === "requests") return renderRequests();
  if (state.view === "audit") return renderAudit();
  renderApps();
}

function renderApps() {
  const table = document.createElement("table");
  table.innerHTML = "<thead><tr><th>App</th><th>Resources</th><th>Actions</th></tr></thead>";
  const body = document.createElement("tbody");
  for (const app of state.apps) {
    const row = document.createElement("tr");
    row.append(cell(app.name || app.id, app.id));
    const resourceCell = document.createElement("td");
    const tokens = document.createElement("div");
    tokens.className = "tokens";
    for (const resource of app.resources || []) {
      const token = document.createElement("span");
      token.className = resource.granted ? "token" : "token missing";
      token.textContent = `${resource.namespace} ${resource.granted ? "granted" : "missing"}`;
      tokens.append(token);
    }
    if (!tokens.childElementCount) {
      const empty = document.createElement("span");
      empty.className = "muted";
      empty.textContent = "No requested resources";
      tokens.append(empty);
    }
    resourceCell.append(tokens);
    row.append(resourceCell);
    const actions = document.createElement("td");
    actions.className = "actions";
    for (const resource of app.resources || []) {
      if (!resource.granted) {
        actions.append(grantButton("Grant owner", app.id, resource.namespace, ""));
      }
      for (const agent of activeAgents()) {
        if (hasGrant(agent.agent, app.id, resource.namespace)) continue;
        actions.append(
          grantButton(`Grant ${agent.display_name || agent.agent}`, app.id, resource.namespace, agent.agent),
        );
      }
    }
    if (!actions.childElementCount) {
      actions.append(muted("Ready"));
    }
    row.append(actions);
    body.append(row);
  }
  table.append(body);
  content.replaceChildren(table);
}

function renderGrants() {
  const table = document.createElement("table");
  table.innerHTML = "<thead><tr><th>App</th><th>Subject</th><th>Resource</th><th>Actions</th></tr></thead>";
  const body = document.createElement("tbody");
  for (const grant of state.grants) {
    const row = document.createElement("tr");
    row.append(cell(grant.app));
    row.append(cell(grant.subject));
    row.append(cell(grant.namespace, grant.resource_id));
    const actions = document.createElement("td");
    const button = document.createElement("button");
    button.textContent = "Revoke";
    button.disabled = Boolean(state.session && state.session.locked);
    button.addEventListener("click", async () => {
      button.disabled = true;
      await fetchJson("/__terrane/admin/grants", {
        method: "DELETE",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          app: grant.app,
          namespace: grant.namespace,
          subject: grant.subject,
        }),
      });
      await refresh();
    });
    actions.append(button);
    row.append(actions);
    body.append(row);
  }
  table.append(body);
  content.replaceChildren(state.grants.length ? table : muted("No grants yet"));
}

function renderAgents() {
  const form = document.createElement("form");
  form.className = "form-row";
  form.innerHTML = `
    <label>Agent id <input name="id" value="codex-local"></label>
    <label>Display name <input name="display_name" value="Codex Local"></label>
    <label>Max role
      <select name="max_role">
        <option value="developer">developer</option>
        <option value="operator">operator</option>
        <option value="member">member</option>
        <option value="viewer">viewer</option>
      </select>
    </label>
  `;
  const controls = document.createElement("div");
  controls.className = "checks";
  for (const [name, label, checked] of [
    ["can_install_apps", "Install apps", true],
    ["can_request_permissions", "Request permissions", true],
    ["can_grant_permissions", "Grant permissions", false],
  ]) {
    const item = document.createElement("label");
    const input = document.createElement("input");
    input.type = "checkbox";
    input.name = name;
    input.checked = checked;
    item.append(input, label);
    controls.append(item);
  }
  const button = document.createElement("button");
  button.className = "primary";
  button.type = "submit";
  button.textContent = "Register";
  button.disabled = Boolean(state.session && state.session.locked);
  controls.append(button);
  form.append(controls);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    button.disabled = true;
    const data = new FormData(form);
    await fetchJson("/__terrane/admin/agents", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        id: data.get("id"),
        display_name: data.get("display_name"),
        max_role: data.get("max_role"),
        can_install_apps: String(form.elements.can_install_apps.checked),
        can_request_permissions: String(form.elements.can_request_permissions.checked),
        can_grant_permissions: String(form.elements.can_grant_permissions.checked),
      }),
    });
    await refresh();
  });

  const table = document.createElement("table");
  table.innerHTML = "<thead><tr><th>Agent</th><th>Delegation</th><th>Status</th><th>Actions</th></tr></thead>";
  const body = document.createElement("tbody");
  for (const agent of state.agents) {
    const row = document.createElement("tr");
    row.append(cell(agent.display_name || agent.agent, agent.agent));
    const flags = [];
    if (agent.can_install_apps) flags.push("install");
    if (agent.can_request_permissions) flags.push("request");
    if (agent.can_grant_permissions) flags.push("grant");
    row.append(cell(agent.max_role, flags.join(", ") || "no delegated actions"));
    row.append(cell(agent.status));
    const actions = document.createElement("td");
    actions.className = "actions";
    if (agent.status === "active") {
      actions.append(agentDelegateButton(agent, "developer"));
      actions.append(agentDelegateButton(agent, "operator"));
      const revoke = document.createElement("button");
      revoke.textContent = "Revoke";
      revoke.disabled = Boolean(state.session && state.session.locked);
      revoke.addEventListener("click", async () => {
        revoke.disabled = true;
        await fetchJson(`/__terrane/admin/agents/${agent.agent}`, { method: "DELETE" });
        await refresh();
      });
      actions.append(revoke);
    } else {
      actions.append(muted("Revoked"));
    }
    row.append(actions);
    body.append(row);
  }
  table.append(body);
  content.replaceChildren(form, state.agents.length ? table : muted("No agents yet"));
}

function renderRequests() {
  const requestId = location.pathname.split("/").pop();
  if (location.pathname.includes("/requests/") && requestId) {
    const request = state.requests.find((item) => item.requestId === requestId);
    if (request) {
      content.replaceChildren(requestTable([request]));
      return;
    }
  }
  content.replaceChildren(state.requests.length ? requestTable(state.requests) : muted("No requests"));
}

function requestTable(requests) {
  const table = document.createElement("table");
  table.innerHTML = "<thead><tr><th>Request</th><th>App</th><th>Resources</th><th>Status</th><th>Actions</th></tr></thead>";
  const body = document.createElement("tbody");
  for (const request of requests) {
    const row = document.createElement("tr");
    row.append(cell(request.requestId, request.subject));
    row.append(cell(request.appName || request.app, `${request.app} / ${request.operation} / ${request.source || "unknown"}`));
    const resourceCell = document.createElement("td");
    const tokens = document.createElement("div");
    tokens.className = "tokens";
    for (const resource of request.resources || []) {
      const token = document.createElement("span");
      token.className = "token missing";
      token.textContent = `${resource.namespace} ${resource.verbs.join("/")}`;
      tokens.append(token);
    }
    resourceCell.append(tokens);
    row.append(resourceCell);
    row.append(cell(request.status, request.decisionReason || request.resumeTokenHash || ""));
    const actions = document.createElement("td");
    if (request.status === "pending") {
      for (const [label, action, className] of [
        ["Approve", "approve", "primary"],
        ["Deny", "deny", ""],
        ["Cancel", "cancel", ""],
      ]) {
        const button = document.createElement("button");
        button.textContent = label;
        button.className = className;
        button.disabled = Boolean(state.session && state.session.locked);
        button.addEventListener("click", async () => {
          button.disabled = true;
          await fetchJson(`/__terrane/admin/requests/${encodeURIComponent(request.requestId)}/${action}`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ reason: action }),
          });
          await refresh();
        });
        actions.append(button);
      }
    } else if (request.status === "approved" && request.source === "preview") {
      const button = document.createElement("button");
      button.textContent = "Promote";
      button.className = "primary";
      button.disabled = Boolean(state.session && state.session.locked);
      button.addEventListener("click", async () => {
        button.disabled = true;
        await fetchJson(`/__terrane/admin/requests/${encodeURIComponent(request.requestId)}/promote`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ reason: "promote", app: previewInstallApp(request.app) }),
        });
        await refresh();
      });
      actions.append(button);
    } else {
      actions.append(muted(request.decidedBy || "Resolved"));
    }
    row.append(actions);
    body.append(row);
  }
  table.append(body);
  return table;
}

function renderAudit() {
  const table = document.createElement("table");
  table.innerHTML = "<thead><tr><th>#</th><th>Event</th></tr></thead>";
  const body = document.createElement("tbody");
  for (const entry of state.audit) {
    const row = document.createElement("tr");
    row.append(cell(String(entry.index)));
    row.append(cell(entry.line));
    body.append(row);
  }
  table.append(body);
  content.replaceChildren(state.audit.length ? table : muted("No audit events yet"));
}

function activeAgents() {
  return state.agents.filter((agent) => agent.status === "active");
}

function hasGrant(subject, app, namespace) {
  return state.grants.some((grant) => (
    grant.subject === subject && grant.app === app && grant.namespace === namespace
  ));
}

function previewInstallApp(previewId) {
  const match = String(previewId || "").match(/^preview-(.*)-[0-9]+$/);
  return match ? match[1] : "";
}

function grantButton(label, app, namespace, subject) {
  const button = document.createElement("button");
  button.className = subject ? "" : "primary";
  button.textContent = `${label} ${namespace}`;
  button.disabled = Boolean(state.session && state.session.locked);
  button.addEventListener("click", async () => {
    button.disabled = true;
    await fetchJson("/__terrane/admin/grants", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ app, namespace, subject }),
    });
    await refresh();
  });
  return button;
}

function agentDelegateButton(agent, role) {
  const button = document.createElement("button");
  button.textContent = role;
  button.disabled = Boolean(state.session && state.session.locked) || agent.max_role === role;
  button.addEventListener("click", async () => {
    button.disabled = true;
    await fetchJson(`/__terrane/admin/agents/${agent.agent}/delegate`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        max_role: role,
        can_install_apps: String(role === "developer"),
        can_request_permissions: "true",
        can_grant_permissions: "false",
      }),
    });
    await refresh();
  });
  return button;
}

function cell(primary, secondary) {
  const td = document.createElement("td");
  const strong = document.createElement("div");
  strong.textContent = primary;
  td.append(strong);
  if (secondary) td.append(muted(secondary));
  return td;
}

function muted(text) {
  const el = document.createElement("div");
  el.className = "muted";
  el.textContent = text;
  return el;
}

refresh().catch((error) => {
  content.replaceChildren(muted(error.message));
});
