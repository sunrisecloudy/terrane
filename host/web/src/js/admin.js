const state = {
  view: "apps",
  apps: [],
  grants: [],
  session: null,
};

const content = document.getElementById("content");
const title = document.getElementById("view-title");
const authority = document.getElementById("authority");
const lockButton = document.getElementById("lock-toggle");

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
  const [session, apps, grants] = await Promise.all([
    fetchJson("/__terrane/admin/session"),
    fetchJson("/__terrane/admin/apps"),
    fetchJson("/__terrane/admin/grants"),
  ]);
  state.session = session;
  state.apps = apps.apps || [];
  state.grants = grants.grants || [];
  render();
}

async function fetchJson(path, options) {
  const response = await fetch(path, { cache: "no-store", ...options });
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

function render() {
  const session = state.session || { org: "local", subject: "user:local-owner", source: "local" };
  authority.textContent = `${session.org} / ${session.subject} / ${session.source}${session.locked ? " / locked" : ""}`;
  lockButton.textContent = session.locked ? "Unlock" : "Lock";
  title.textContent = state.view[0].toUpperCase() + state.view.slice(1);
  if (state.view === "grants") return renderGrants();
  if (state.view === "requests") return renderRequests();
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
    for (const resource of app.resources || []) {
      if (resource.granted) continue;
      const button = document.createElement("button");
      button.className = "primary";
      button.textContent = `Grant ${resource.namespace}`;
      button.disabled = Boolean(state.session && state.session.locked);
      button.addEventListener("click", async () => {
        button.disabled = true;
        await fetchJson("/__terrane/admin/grants", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ app: app.id, namespace: resource.namespace }),
        });
        await refresh();
      });
      actions.append(button);
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

function renderRequests() {
  const requestId = location.pathname.split("/").pop();
  if (location.pathname.includes("/requests/") && requestId) {
    content.replaceChildren(muted(`Request ${requestId}`));
    return;
  }
  content.replaceChildren(muted("No pending requests"));
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
