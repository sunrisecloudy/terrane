#!/usr/bin/env node
/**
 * Build the Forge public API documentation page from live contract sources:
 * - forge/data/commands.json (core command catalog)
 * - forge/std/forge-std.d.ts (applet host API)
 * - forge/data/bundled-apps.json (example applets)
 *
 * Output: forge/docs/public-api/{index.html,styles.css,app.js}
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const outDir = path.join(repoRoot, "forge/docs/public-api");

const APPLET_API = [
  {
    id: "ctx.db",
    title: "Database (`ctx.db`)",
    summary: "Collection-scoped JSON records with deterministic replay.",
    methods: [
      { name: "insert(collection, record)", returns: "Promise<{ id: string }>", example: "webapps/examples/notes-lite" },
      { name: "get(collection, id)", returns: "Promise<DbRecord | null>", example: "webapps/examples/task-workbench" },
      { name: "list(collection)", returns: "Promise<DbRecord[]>", example: "webapps/examples/notes-lite" },
      { name: "query(plan)", returns: "Promise<DbRecord[]>", example: "webapps/examples/task-workbench" },
      { name: "query(collection, plan)", returns: "Promise<DbRecord[]>", example: "webapps/examples/task-workbench" },
    ],
    snippet: `const id = await ctx.db.insert("notes", { title: "Hello" });
const rows = await ctx.db.list("notes");
const row = await ctx.db.get("notes", id);`,
    test: "cargo run -p forge-cli -- demo",
  },
  {
    id: "ctx.storage",
    title: "Storage (`ctx.storage`)",
    summary: "Per-applet key/value storage scoped by manifest grants.",
    methods: [
      { name: "get(key)", returns: "Promise<string | null>", example: "webapps/examples/task-workbench" },
      { name: "set(key, value)", returns: "Promise<void>", example: "webapps/examples/core-replay-lab" },
      { name: "delete(key)", returns: "Promise<void>", example: "webapps/examples/core-replay-lab" },
      { name: "list(prefix)", returns: "Promise<string[]>", example: "webapps/examples/core-replay-lab" },
    ],
    snippet: `await ctx.storage.set("app/last-run", runId);
const value = await ctx.storage.get("app/last-run");`,
    test: "node --test tools/reference-host/test/example-load-acceptance.test.js",
  },
  {
    id: "ctx.net",
    title: "Network (`ctx.net`)",
    summary: "Manifest-gated HTTP fetch; replay serves recorded responses.",
    methods: [{ name: "fetch(request)", returns: "Promise<NetResponse>", example: "webapps/examples/api-dashboard" }],
    snippet: `const response = await ctx.net.fetch({
  method: "GET",
  url: "https://api.example.com/public/weather",
  response_content_type: "application/json"
});`,
    test: "node --test tools/reference-host/test/example-load-acceptance.test.js",
  },
  {
    id: "ctx.files",
    title: "Files (`ctx.files`)",
    summary: "Sandboxed read/write via trusted host handles (never raw paths).",
    methods: [
      { name: "read(request)", returns: "Promise<FileReadResponse>", example: "webapps/examples/file-transformer" },
      { name: "write(request)", returns: "Promise<FileWriteResponse>", example: "webapps/examples/file-transformer" },
    ],
    snippet: `await ctx.files.write({
  handle: "workspace_data",
  path: "out/summary.txt",
  bytes_base64: Buffer.from("hello").toString("base64"),
  mode: "create_or_truncate"
});`,
    test: "node --test tools/reference-host/test/example-load-acceptance.test.js",
  },
  {
    id: "ctx.ui",
    title: "UI (`ctx.ui`)",
    summary: "Declarative component trees rendered by the host.",
    methods: [{ name: "render(tree)", returns: "void", example: "webapps/examples/notes-lite" }],
    snippet: `ctx.ui.render({
  type: "Stack",
  testId: "root",
  direction: "v",
  children: [
    { type: "Text", testId: "title", text: "Notes", variant: "title" }
  ]
});`,
    test: "cargo test -p forge-cli --test e2e",
  },
  {
    id: "ctx.time",
    title: "Time (`ctx.time`)",
    summary: "Deterministic logical clock for replayable runs.",
    methods: [{ name: "now()", returns: "number", example: "webapps/examples/notes-lite" }],
    snippet: "const createdAt = ctx.time.now();",
    test: "cargo run -p forge-cli -- demo",
  },
  {
    id: "ctx.random",
    title: "Random (`ctx.random`)",
    summary: "Seeded deterministic PRNG recorded into the run trace.",
    methods: [{ name: "next()", returns: "number", example: "webapps/examples/core-replay-lab" }],
    snippet: "const roll = ctx.random.next();",
    test: "node --test tools/reference-host/test/example-load-acceptance.test.js",
  },
  {
    id: "main",
    title: "Entrypoint (`main`)",
    summary: "Every applet exports async main(ctx, input) returning AppResult.",
    methods: [{ name: "main(ctx, input)", returns: "Promise<AppResult>", example: "forge/fixtures/demo-notes-lite" }],
    snippet: `export async function main(ctx: AppContext, input: unknown): Promise<AppResult> {
  return { ok: true, value: { received: input } };
}`,
    test: "cargo run -p forge-cli -- demo",
  },
];

const CLI_COMMANDS = [
  { name: "forge commands", summary: "List catalog commands (tier/namespace filters, --json)." },
  { name: "forge describe <name>", summary: "Show one command descriptor and schema paths." },
  { name: "forge run <name>", summary: "Execute any outer command locally or via --server." },
  { name: "forge trace <run_id>", summary: "Read redacted host-call journal via system.trace." },
  { name: "forge demo", summary: "M0a spine: install notes-lite, run, replay byte-identically." },
  { name: "forge help", summary: "Catalog-generated help grouped by namespace." },
];

const HTTP_ROUTES = [
  { method: "GET", path: "/health", summary: "Server liveness and console flag." },
  { method: "GET", path: "/docs", summary: "This public API reference page." },
  { method: "GET", path: "/console", summary: "Catalog-driven operator command console." },
  { method: "GET", path: "/schemas/commands/*.json", summary: "Per-command JSON Schema assets." },
  { method: "POST", path: "/bridge", summary: "CoreCommand JSON envelope in, CoreResponse out." },
  { method: "POST", path: "/events/drain", summary: "Drain emitted CoreEvent batch from the workspace." },
];

function readJson(relPath) {
  return JSON.parse(fs.readFileSync(path.join(repoRoot, relPath), "utf8"));
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function badge(text, kind = "default") {
  return `<span class="badge badge-${kind}">${escapeHtml(text)}</span>`;
}

function renderAppletSection() {
  return APPLET_API.map((entry) => {
    const methods = entry.methods
      .map(
        (m) =>
          `<tr><td><code>${escapeHtml(m.name)}</code></td><td><code>${escapeHtml(m.returns)}</code></td><td><a href="#example-${m.example.split("/").pop()}">${escapeHtml(m.example)}</a></td></tr>`,
      )
      .join("");
    return `<article class="api-card" id="${entry.id}">
      <h3>${escapeHtml(entry.title)}</h3>
      <p>${escapeHtml(entry.summary)}</p>
      <table class="method-table"><thead><tr><th>Method</th><th>Returns</th><th>Example</th></tr></thead><tbody>${methods}</tbody></table>
      <pre class="code-block"><code>${escapeHtml(entry.snippet)}</code></pre>
      <p class="meta">Test: <code>${escapeHtml(entry.test)}</code></p>
    </article>`;
  }).join("\n");
}

function renderCommandSection(commands) {
  const byNs = new Map();
  for (const cmd of commands) {
    if (cmd.surface !== "outer") continue;
    const ns = cmd.namespace ?? cmd.name.split(".")[0];
    if (!byNs.has(ns)) byNs.set(ns, []);
    byNs.get(ns).push(cmd);
  }

  return [...byNs.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([ns, cmds]) => {
      const rows = cmds
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((cmd) => {
          const flags = [
            cmd.mutates ? badge("mutates", "warn") : "",
            cmd.effectful ? badge("effectful", "danger") : "",
            badge(cmd.stability, cmd.stability === "stable" ? "good" : "default"),
            badge(cmd.visibility, "tier"),
          ]
            .filter(Boolean)
            .join(" ");
          const schema = cmd.payload_schema
            ? `<a href="/schemas/commands/${escapeHtml(path.basename(cmd.payload_schema))}">request</a>`
            : "—";
          const response = cmd.response_schema
            ? `<a href="/schemas/commands/${escapeHtml(path.basename(cmd.response_schema))}">response</a>`
            : "—";
          return `<tr data-name="${escapeHtml(cmd.name)}" data-summary="${escapeHtml(cmd.summary)}">
            <td><code class="cmd-name">${escapeHtml(cmd.name)}</code></td>
            <td>${escapeHtml(cmd.summary)}</td>
            <td>${flags}</td>
            <td>${escapeHtml((cmd.required_roles ?? []).join(", "))}</td>
            <td>${schema} / ${response}</td>
          </tr>`;
        })
        .join("");
      return `<section class="ns-section" id="commands-${ns}">
        <h3>${escapeHtml(ns)}</h3>
        <table class="command-table"><thead><tr><th>Command</th><th>Summary</th><th>Flags</th><th>Roles</th><th>Schemas</th></tr></thead><tbody>${rows}</tbody></table>
      </section>`;
    })
    .join("\n");
}

function renderExamples(apps) {
  return apps
    .map((app) => {
      const appJsPath = path.join(repoRoot, "webapps/examples", app.id, "app.js");
      let snippet = "";
      if (fs.existsSync(appJsPath)) {
        const source = fs.readFileSync(appJsPath, "utf8");
        const lines = source.split("\n").slice(0, 28);
        snippet = lines.join("\n").trim();
        if (source.split("\n").length > 28) snippet += "\n// ...";
      }
      return `<article class="example-card" id="example-${app.id}">
        <h3>${escapeHtml(app.name)} <code>${escapeHtml(app.id)}</code></h3>
        <p>${escapeHtml(app.description)}</p>
        <p class="meta">Path: <code>webapps/examples/${escapeHtml(app.id)}/</code></p>
        <pre class="code-block"><code>${escapeHtml(snippet || "(see example on disk)")}</code></pre>
        <p class="meta">Tests: <code>node --test tools/reference-host/test/package-validator.test.js</code></p>
      </article>`;
    })
    .join("\n");
}

function renderCliSection() {
  return CLI_COMMANDS.map(
    (c) =>
      `<article class="api-card"><h3><code>${escapeHtml(c.name)}</code></h3><p>${escapeHtml(c.summary)}</p></article>`,
  ).join("\n");
}

function renderHttpSection() {
  const rows = HTTP_ROUTES.map(
    (r) =>
      `<tr><td><code>${escapeHtml(r.method)}</code></td><td><code>${escapeHtml(r.path)}</code></td><td>${escapeHtml(r.summary)}</td></tr>`,
  ).join("");
  return `<table class="method-table"><thead><tr><th>Method</th><th>Path</th><th>Summary</th></tr></thead><tbody>${rows}</tbody></table>`;
}

function buildHtml({ catalog, apps }) {
  const generatedAt = new Date().toISOString();
  const commandCount = catalog.commands.filter((c) => c.surface === "outer").length;
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Forge Public API Reference</title>
  <link rel="stylesheet" href="styles.css">
</head>
<body>
  <header class="hero">
    <div class="hero-inner">
      <p class="eyebrow">Terrane Forge v1</p>
      <h1>Public API Reference</h1>
      <p class="lede">Applet host surface (<code>@forge/std</code>), ${commandCount} core commands, CLI, and HTTP bridge — generated from the live catalog.</p>
      <div class="hero-meta">
        <span>Catalog <code>${escapeHtml(catalog.catalogVersion ?? "unknown")}</code></span>
        <span>Generated <time datetime="${generatedAt}">${generatedAt}</time></span>
      </div>
    </div>
  </header>

  <div class="layout">
    <nav class="sidebar" aria-label="API sections">
      <label class="search-field">Filter
        <input id="api-filter" type="search" placeholder="Search commands & APIs…" autocomplete="off">
      </label>
      <ul>
        <li><a href="#overview">Overview</a></li>
        <li><a href="#applet-api">Applet API</a></li>
        <li><a href="#core-commands">Core Commands</a></li>
        <li><a href="#cli">CLI</a></li>
        <li><a href="#http">HTTP Bridge</a></li>
        <li><a href="#examples">Examples</a></li>
        <li><a href="#validation">Validation</a></li>
      </ul>
    </nav>

    <main class="content">
      <section id="overview" class="section">
        <h2>Overview</h2>
        <p>Forge exposes one command facade (<code>WorkspaceCore::handle</code>) to every shell, agent, CLI, and HTTP bridge. Generated applets never call that facade directly — they use the typed <code>ctx.*</code> host API declared in <code>forge/std/forge-std.d.ts</code>.</p>
        <div class="card-grid">
          <div class="info-card"><h3>Applet authors</h3><p>Import <code>@forge/std</code>, export <code>main(ctx, input)</code>, declare capabilities in <code>manifest.json</code>.</p></div>
          <div class="info-card"><h3>Operators</h3><p>Use <code>forge run</code>, the web console at <code>/console</code>, or POST <code>/bridge</code>.</p></div>
          <div class="info-card"><h3>Integrators</h3><p>Pin <code>artifacts/public-contract.json</code> and verify with <code>tools/verify-public-contract.mjs</code>.</p></div>
        </div>
      </section>

      <section id="applet-api" class="section">
        <h2>Applet API (<code>@forge/std</code>)</h2>
        <p>Source: <code>forge/std/forge-std.d.ts</code>. Only these <code>ctx</code> surfaces are reachable from applet TypeScript.</p>
        ${renderAppletSection()}
      </section>

      <section id="core-commands" class="section">
        <h2>Core Commands</h2>
        <p>Source: <code>forge/data/commands.json</code> via <code>system.describe</code>. Inner <code>ctx.*</code> host calls are reference-only and listed under debug describe with <code>include_inner</code>.</p>
        ${renderCommandSection(catalog.commands)}
      </section>

      <section id="cli" class="section">
        <h2>CLI</h2>
        <p>Binary: <code>forge</code> from <code>forge-cli</code>. Every outer command is reachable through <code>forge run &lt;name&gt;</code>.</p>
        ${renderCliSection()}
      </section>

      <section id="http" class="section">
        <h2>HTTP Bridge</h2>
        <p><code>forge-server</code> exposes a narrow JSON HTTP surface over the same core.</p>
        ${renderHttpSection()}
      </section>

      <section id="examples" class="section">
        <h2>Example Applets</h2>
        <p>All bundled apps under <code>webapps/examples/</code> (reference-host validation + smoke/micro tests).</p>
        ${renderExamples(apps)}
      </section>

      <section id="validation" class="section">
        <h2>Validation</h2>
        <pre class="code-block"><code>node --no-warnings tools/build-forge-api-docs.mjs
node --test tools/test/forge-api-docs.test.mjs
node --test --no-warnings tools/reference-host/test/package-validator.test.js
cd forge && cargo run -p forge-cli -- demo
node --no-warnings tools/export-public-contract.mjs --out artifacts/public-contract.json
node --no-warnings tools/verify-public-contract.mjs --contract artifacts/public-contract.json --root .</code></pre>
      </section>
    </main>
  </div>
  <script src="app.js"></script>
</body>
</html>`;
}

const STYLES = `:root {
  color-scheme: light dark;
  --bg: #f6f7fb;
  --panel: #ffffff;
  --text: #121826;
  --muted: #5b6475;
  --border: #dde2eb;
  --accent: #315efb;
  --accent-soft: #eef2ff;
  --good: #147d56;
  --warn: #b25e09;
  --danger: #b42318;
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  line-height: 1.5;
  background: var(--bg);
  color: var(--text);
}
* { box-sizing: border-box; }
body { margin: 0; }
a { color: var(--accent); }
code { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 0.92em; }
.hero { background: linear-gradient(135deg, #eef2ff, #f6f7fb 55%, #ffffff); border-bottom: 1px solid var(--border); }
.hero-inner { max-width: 1200px; margin: 0 auto; padding: 2.5rem 1.5rem 2rem; }
.eyebrow { text-transform: uppercase; letter-spacing: 0.08em; font-size: 0.75rem; color: var(--muted); margin: 0 0 0.5rem; }
.hero h1 { margin: 0 0 0.75rem; font-size: 2.2rem; }
.lede { max-width: 70ch; color: var(--muted); margin: 0 0 1rem; }
.hero-meta { display: flex; flex-wrap: wrap; gap: 1rem; color: var(--muted); font-size: 0.9rem; }
.layout { display: grid; grid-template-columns: 260px 1fr; gap: 0; max-width: 1200px; margin: 0 auto; }
.sidebar { position: sticky; top: 0; align-self: start; padding: 1.25rem 1rem 2rem; border-right: 1px solid var(--border); min-height: 100vh; background: rgba(255,255,255,0.7); }
.sidebar ul { list-style: none; padding: 0; margin: 1rem 0 0; }
.sidebar li { margin: 0.35rem 0; }
.sidebar a { text-decoration: none; color: var(--text); }
.sidebar a:hover { color: var(--accent); }
.search-field { display: grid; gap: 0.35rem; font-size: 0.85rem; color: var(--muted); }
.search-field input { width: 100%; border: 1px solid var(--border); border-radius: 8px; padding: 0.55rem 0.7rem; }
.content { padding: 1.5rem 1.75rem 3rem; }
.section { margin-bottom: 2.5rem; scroll-margin-top: 1rem; }
.section h2 { margin-top: 0; font-size: 1.6rem; border-bottom: 1px solid var(--border); padding-bottom: 0.5rem; }
.api-card, .example-card, .info-card { background: var(--panel); border: 1px solid var(--border); border-radius: 12px; padding: 1rem 1.1rem; margin: 1rem 0; box-shadow: 0 8px 24px rgba(16,24,40,0.04); }
.card-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 1rem; }
.method-table, .command-table { width: 100%; border-collapse: collapse; font-size: 0.92rem; }
.method-table th, .method-table td, .command-table th, .command-table td { border-bottom: 1px solid var(--border); padding: 0.55rem 0.4rem; text-align: left; vertical-align: top; }
.code-block { background: #0f172a; color: #e2e8f0; border-radius: 10px; padding: 1rem; overflow: auto; font-size: 0.85rem; }
.meta { color: var(--muted); font-size: 0.88rem; }
.badge { display: inline-block; border-radius: 999px; padding: 0.1rem 0.45rem; font-size: 0.72rem; margin-right: 0.25rem; border: 1px solid var(--border); }
.badge-good { background: #e8f8f1; color: var(--good); border-color: #b7e4d2; }
.badge-warn { background: #fff6e8; color: var(--warn); }
.badge-danger { background: #fdecec; color: var(--danger); }
.badge-tier { background: var(--accent-soft); color: var(--accent); }
.is-hidden { display: none !important; }
@media (max-width: 900px) {
  .layout { grid-template-columns: 1fr; }
  .sidebar { position: static; min-height: auto; border-right: none; border-bottom: 1px solid var(--border); }
}`;

const APP_JS = `const filter = document.getElementById("api-filter");
if (filter) {
  filter.addEventListener("input", () => {
    const q = filter.value.trim().toLowerCase();
    document.querySelectorAll(".command-table tbody tr").forEach((row) => {
      const hay = (row.dataset.name + " " + row.dataset.summary).toLowerCase();
      row.classList.toggle("is-hidden", q.length > 0 && !hay.includes(q));
    });
    document.querySelectorAll(".api-card, .example-card").forEach((card) => {
      const hay = card.textContent.toLowerCase();
      card.classList.toggle("is-hidden", q.length > 0 && !hay.includes(q));
    });
  });
}`;

function main() {
  const catalog = readJson("forge/data/commands.json");
  const apps = readJson("forge/data/bundled-apps.json");
  fs.mkdirSync(outDir, { recursive: true });
  fs.writeFileSync(path.join(outDir, "index.html"), buildHtml({ catalog, apps }));
  fs.writeFileSync(path.join(outDir, "styles.css"), STYLES);
  fs.writeFileSync(path.join(outDir, "app.js"), APP_JS);
  console.log(`Wrote Forge API docs to ${outDir}`);
  console.log(`  commands: ${catalog.commands.length}, examples: ${apps.length}`);
}

main();