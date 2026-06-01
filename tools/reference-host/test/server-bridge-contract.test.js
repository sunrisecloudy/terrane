import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { examplesDir, repoRoot } from "../src/paths.js";
import { readPackage } from "../src/package-validator.js";

const testDir = path.dirname(fileURLToPath(import.meta.url));
const serverDir = path.resolve(testDir, "../../..", "server");
const controlToken = "server-contract-token";

function hasZig() {
  try {
    execFileSync("zig", ["version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasCc() {
  try {
    execFileSync("cc", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function targetArgsForHost() {
  if (process.platform !== "darwin") return [];
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  return ["-target", `${arch}-macos.15.0.0`];
}

function zigServerModuleArgs() {
  return ["--dep", "zig_core", "--dep", "zig_crdt", "-Mroot=src/main.zig", "-Mzig_core=../zig-core/src/lib.zig", "-Mzig_crdt=../zig-crdt/src/lib.zig"];
}

test(
  "checked-in bridge fixtures match Zig server expected responses",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-server-contract-"));
    try {
      const executablePath = buildServerExecutable(scratch);
      const fixturesDir = path.join(repoRoot, "tests", "fixtures", "bridge");
      const files = fs.readdirSync(fixturesDir).filter((fileName) => fileName.endsWith(".json")).sort();

      for (const fileName of files) {
        const fixture = JSON.parse(fs.readFileSync(path.join(fixturesDir, fileName), "utf8"));
        if (!fixture.platforms.includes("server")) continue;
        const expected = expectedForPlatform(fixture, "server");

        const started = await startServer(executablePath, scratch, fileName);
        try {
          const install = await controlCommand(started.url, "platform.install_webapp_package", {
            package: packageForFixture(fixture),
            activate: true,
            trustLevel: "developer",
          });
          assert.equal(install.ok, true, `${fileName} install`);

          const opened = await controlCommand(started.url, "platform.open_webapp", {
            appId: fixture.context.appId,
          });
          if (!opened.ok && expected?.errorCode === "runtime_version_incompatible") {
            assert.equal(install.result.activated, false, `${fileName} install refused activation`);
            assert.equal(opened.error?.code, "app_not_installed", `${fileName} open rejected inactive package`);
            continue;
          }
          assert.equal(opened.ok, true, `${fileName} open`);
          const sessionId = opened.result.sessionId;
          await applyBridgeFixturePreconditions(started.url, fixture, sessionId);

          const {
            context: _context,
            preconditions: _preconditions,
            expected: _expected,
            expectedByPlatform: _expectedByPlatform,
            platforms: _platforms,
            ...request
          } = fixture;
          const response = await bridgeCall(started.url, fixture.context.appId, sessionId, request);
          assertBridgeExpected(response, expected, fileName);
        } finally {
          await stopServer(started);
        }
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Zig server install gate quarantines packages that fail accessibility",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-server-accessibility-"));
    try {
      const executablePath = buildServerExecutable(scratch);
      const started = await startServer(executablePath, scratch, "accessibility-gate");
      try {
        const first = await controlCommand(started.url, "platform.install_webapp_package", {
          package: packageForApp("notes-lite"),
          activate: true,
          trustLevel: "developer",
        });
        assert.equal(first.ok, true);
        assert.equal(first.result.status, "enabled");

        const failed = await controlCommand(started.url, "platform.install_webapp_package", {
          package: packageForApp("notes-lite", (html) => html
            .replace(/<main\b([^>]*)>/i, "<section$1>")
            .replace(/<\/main>/i, "</section>")),
          activate: true,
          trustLevel: "developer",
        });
        assert.equal(failed.ok, true);
        assert.equal(failed.result.status, "quarantined");
        assert.equal(failed.result.activated, false);

        const report = await controlCommand(started.url, "platform.install_report", {
          appId: "notes-lite",
          installId: failed.result.installId,
        });
        assert.equal(report.ok, true);
        assert.equal(report.result.length, 1);
        assert.equal(report.result[0].status, "failed");
        const security = JSON.parse(report.result[0].security_json);
        assert.equal(security.ok, false);
        assert.equal(security.accessibility.status, "fail");
        assert.equal(security.accessibility.checks.some((check) => check.id === "main_landmark" && check.status === "fail"), true);
      } finally {
        await stopServer(started);
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Zig server notebook bridge persists CRDT edits, approvals, and rejected AI writes",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-server-notebook-"));
    try {
      const executablePath = buildServerExecutable(scratch);
      const started = await startServer(executablePath, scratch, "notebook-crdt");
      try {
        const appId = "notes-lite";
        const install = await controlCommand(started.url, "platform.install_webapp_package", {
          package: packageForNotebookApp(appId),
          activate: true,
          trustLevel: "developer",
        });
        assert.equal(install.ok, true, JSON.stringify(install));

        const openedApp = await controlCommand(started.url, "platform.open_webapp", { appId });
        assert.equal(openedApp.ok, true, JSON.stringify(openedApp));
        const sessionId = openedApp.result.sessionId;
        const notebookId = "server_notebook_contract";

        const open = await bridgeCall(started.url, appId, sessionId, {
          id: "req_notebook_open",
          method: "notebook.open",
          params: { notebookId, title: "Server notebook" },
        });
        assert.equal(open.ok, true);
        assert.equal(open.result.notebookId, notebookId);

        const insert = await bridgeCall(started.url, appId, sessionId, {
          id: "req_notebook_insert",
          method: "notebook.apply_local",
          params: {
            notebookId,
            operation: { opId: "op_server_cell", type: "cell.insert", cellId: "cell_intro", cellType: "markdown", source: "Hello" },
          },
        });
        assert.equal(insert.ok, true);

        const proposal = await bridgeCall(started.url, appId, sessionId, {
          id: "req_notebook_propose",
          method: "notebook.propose_ai_patch",
          params: {
            notebookId,
            opId: "op_server_proposal",
            proposalId: "proposal_server_1",
            modelId: "reference-model",
            promptContextHash: "sha256:server-context",
            affectedCellIds: ["cell_intro"],
            proposedSource: "Accepted server proposal",
          },
        });
        assert.equal(proposal.ok, true);
        assert.equal(proposal.result.notebook.proposals.proposal_server_1.status, "pending");

        const accepted = await bridgeCall(started.url, appId, sessionId, {
          id: "req_notebook_accept",
          method: "notebook.accept_proposal",
          params: { notebookId, opId: "op_server_accept", proposalId: "proposal_server_1" },
        });
        assert.equal(accepted.ok, true);
        assert.equal(accepted.result.notebook.cells[0].source, "Accepted server proposal");

        const denied = await bridgeCall(started.url, appId, sessionId, {
          id: "req_notebook_denied",
          method: "notebook.apply_local",
          params: {
            notebookId,
            actorId: "actor_reference_ai",
            actorKind: "ai",
            operation: { opId: "op_server_ai_write", type: "text.insert", cellId: "cell_intro", index: 0, text: "Nope. " },
          },
        });
        assert.equal(denied.ok, false);
        assert.equal(denied.error.code, "permission_denied");

        const snapshot = await controlCommand(started.url, "db.snapshot", {});
        assert.equal(snapshot.ok, true);
        for (const tableName of [
          "crdt_notebooks",
          "crdt_documents",
          "crdt_updates",
          "crdt_heads",
          "crdt_actors",
          "crdt_permissions",
          "crdt_proposals",
          "crdt_sync_cursors",
        ]) {
          assert.equal(Array.isArray(snapshot.result[tableName]), true, `db.snapshot includes ${tableName}`);
        }
        const updates = snapshot.result.crdt_updates.filter((row) => row.notebook_id === notebookId);
        assert.equal(updates.filter((row) => row.status === "accepted").length, 3);
        assert.equal(updates.some((row) => row.status === "rejected" && row.error_code === "permission_denied"), true);
        assert.equal(snapshot.result.crdt_documents.some((row) => row.notebook_id === notebookId), true);
        assert.equal(snapshot.result.crdt_actors.some((row) => row.actor_id === "actor_reference_ai"), true);
        assert.equal(snapshot.result.crdt_permissions.some((row) => row.permission === "notebook.write"), true);

        const backup = await controlCommand(started.url, "db.export_backup", {});
        assert.equal(backup.ok, true);
        for (const field of [
          "crdtNotebooks",
          "crdtDocuments",
          "crdtUpdates",
          "crdtHeads",
          "crdtActors",
          "crdtPermissions",
          "crdtProposals",
          "crdtSyncCursors",
        ]) {
          assert.equal(Array.isArray(backup.result[field]), true, `db.export_backup includes ${field}`);
        }
        assert.equal(backup.result.crdtUpdates.filter((row) => row.notebook_id === notebookId).length, 4);

        const debugBundle = await controlCommand(started.url, "db.export_debug_bundle", {});
        assert.equal(debugBundle.ok, true);
        assert.equal(Array.isArray(debugBundle.result.crdtUpdates), true);

        const importedServer = await startServer(executablePath, scratch, "notebook-crdt-import");
        try {
          const imported = await controlCommand(importedServer.url, "db.import_backup", { backup: backup.result });
          assert.equal(imported.ok, true, JSON.stringify(imported));
          assert.equal(imported.result.crdtUpdates, 4);
          const importedSnapshot = await controlCommand(importedServer.url, "db.snapshot", {});
          assert.equal(importedSnapshot.ok, true);
          assert.equal(importedSnapshot.result.crdt_updates.filter((row) => row.notebook_id === notebookId).length, 4);
          assert.equal(importedSnapshot.result.crdt_heads.some((row) => row.notebook_id === notebookId && row.version === 3), true);
        } finally {
          await stopServer(importedServer);
        }
      } finally {
        await stopServer(started);
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Zig server requires confirmation before destructive reset controls",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-server-reset-confirm-"));
    try {
      const executablePath = buildServerExecutable(scratch);
      const started = await startServer(executablePath, scratch, "reset-confirm");
      try {
        const install = await controlCommand(started.url, "platform.install_webapp_package", {
          package: packageForApp("notes-lite"),
          activate: true,
          trustLevel: "developer",
        });
        assert.equal(install.ok, true);

        const seeded = await controlCommand(started.url, "runtime.storage_set", {
          appId: "notes-lite",
          key: "notes-lite:server-reset-confirm",
          value: { title: "Seeded by server contract" },
        });
        assert.equal(seeded.ok, true);

        const platformResetWithoutConfirm = await controlCommand(started.url, "platform.reset_webapp", {
          appId: "notes-lite",
        });
        assert.equal(platformResetWithoutConfirm.ok, false);
        assert.equal(platformResetWithoutConfirm.error.code, "confirmation_required");

        const storageResetWithoutConfirm = await controlCommand(started.url, "runtime.storage_reset", {
          appId: "notes-lite",
        });
        assert.equal(storageResetWithoutConfirm.ok, false);
        assert.equal(storageResetWithoutConfirm.error.code, "confirmation_required");

        const storageReset = await controlCommand(started.url, "runtime.storage_reset", {
          appId: "notes-lite",
          confirm: true,
        });
        assert.equal(storageReset.ok, true);
        assert.equal(storageReset.result.storageRowsDeleted, 1);
      } finally {
        await stopServer(started);
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Zig server package validation enforces plain stylesheet links",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-server-stylesheet-"));
    try {
      const executablePath = buildServerExecutable(scratch);
      const started = await startServer(executablePath, scratch, "stylesheet-validation");
      try {
        const missing = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace('<link rel="stylesheet" href="styles.css">', "")));
        assert.equal(missing.status, 200);
        assert.equal(missing.body.ok, false);
        assert.equal(missing.body.errors.includes("missing_stylesheet"), true);

        const nonPlain = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace('href="styles.css"', 'href="styles.css" media="print"')));
        assert.equal(nonPlain.status, 200);
        assert.equal(nonPlain.body.ok, false);
        assert.equal(nonPlain.body.errors.includes("forbidden_stylesheet_attribute"), true);

        const duplicate = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace("</head>", '<link rel="stylesheet" href="styles.css"></head>')));
        assert.equal(duplicate.status, 200);
        assert.equal(duplicate.body.ok, false);
        assert.equal(duplicate.body.errors.includes("invalid_stylesheet_count"), true);

        const resourceHint = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace("</head>", '<link rel="preconnect" href="https://tracker.example"></head>')));
        assert.equal(resourceHint.status, 200);
        assert.equal(resourceHint.body.ok, false);
        assert.equal(resourceHint.body.errors.includes("forbidden_resource_hint"), true);

        const externalResource = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace("</main>", '<img src="https://tracker.example/pixel.png" alt=""></main>')));
        assert.equal(externalResource.status, 200);
        assert.equal(externalResource.body.ok, false);
        assert.equal(externalResource.body.errors.includes("forbidden_external_resource"), true);

        const brittleSmoke = packageForApp("notes-lite");
        const smokeFile = brittleSmoke.files.find((file) => file.path === "smoke-tests.json");
        smokeFile.content = JSON.stringify([{ name: "brittle selector", steps: [{ type: "click", selector: "#new-note" }] }]);
        const smokeSelector = await validateWebappPackage(started.url, brittleSmoke);
        assert.equal(smokeSelector.status, 200);
        assert.equal(smokeSelector.body.ok, false);
        assert.equal(smokeSelector.body.errors.includes("invalid_smoke_selector"), true);

        const unsafeSmoke = packageForApp("notes-lite");
        const unsafeSmokeFile = unsafeSmoke.files.find((file) => file.path === "smoke-tests.json");
        unsafeSmokeFile.content = JSON.stringify([{ name: "uses mock", steps: [{ tool: "runtime.network_mock_set", args: {} }] }]);
        const smokeCommand = await validateWebappPackage(started.url, unsafeSmoke);
        assert.equal(smokeCommand.status, 200);
        assert.equal(smokeCommand.body.ok, false);
        assert.equal(smokeCommand.body.errors.includes("invalid_smoke_tests"), true);
      } finally {
        await stopServer(started);
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Zig server package validation enforces plain app script tags",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-server-script-"));
    try {
      const executablePath = buildServerExecutable(scratch);
      const started = await startServer(executablePath, scratch, "script-validation");
      try {
        const missing = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace('<script src="app.js"></script>', "")));
        assert.equal(missing.status, 200);
        assert.equal(missing.body.ok, false);
        assert.equal(missing.body.errors.includes("missing_app_script"), true);

        const alternate = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace('src="app.js"', 'src="other.js"')));
        assert.equal(alternate.status, 200);
        assert.equal(alternate.body.ok, false);
        assert.equal(alternate.body.errors.includes("forbidden_app_script_src"), true);

        const nonPlain = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace('src="app.js"', 'src="app.js" type="module"')));
        assert.equal(nonPlain.status, 200);
        assert.equal(nonPlain.body.ok, false);
        assert.equal(nonPlain.body.errors.includes("forbidden_app_script_attribute"), true);

        const duplicate = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace("</body>", '<script src="app.js"></script></body>')));
        assert.equal(duplicate.status, 200);
        assert.equal(duplicate.body.ok, false);
        assert.equal(duplicate.body.errors.includes("invalid_app_script_count"), true);
      } finally {
        await stopServer(started);
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Zig server package validation rejects inline style policy drift",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-server-inline-style-"));
    try {
      const executablePath = buildServerExecutable(scratch);
      const started = await startServer(executablePath, scratch, "inline-style-validation");
      try {
        const styleAttribute = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace("<main", '<main style="color:red"')));
        assert.equal(styleAttribute.status, 200);
        assert.equal(styleAttribute.body.ok, false);
        assert.equal(styleAttribute.body.errors.includes("forbidden_inline_style"), true);

        const styleElement = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace("</head>", "<style>body { color: red; }</style></head>")));
        assert.equal(styleElement.status, 200);
        assert.equal(styleElement.body.ok, false);
        assert.equal(styleElement.body.errors.includes("forbidden_inline_style"), true);

        const styleCsp = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace("style-src 'self'", "style-src 'self' 'unsafe-inline'")));
        assert.equal(styleCsp.status, 200);
        assert.equal(styleCsp.body.ok, false);
        assert.equal(styleCsp.body.errors.includes("forbidden_inline_style_csp"), true);

        const scriptCsp = await validateWebappPackage(started.url, packageForApp("notes-lite", (html) => html
          .replace("script-src 'self'", "script-src 'self' 'unsafe-inline'")));
        assert.equal(scriptCsp.status, 200);
        assert.equal(scriptCsp.body.ok, false);
        assert.equal(scriptCsp.body.errors.includes("forbidden_inline_script_csp"), true);

        const missingCssResourcePackage = packageForApp("notes-lite");
        const cssFile = missingCssResourcePackage.files.find((file) => file.path === "styles.css");
        cssFile.content = `.logo { background-image: url(icon.png); }\n${cssFile.content}`;
        const missingCssResource = await validateWebappPackage(started.url, missingCssResourcePackage);
        assert.equal(missingCssResource.status, 200);
        assert.equal(missingCssResource.body.ok, false);
        assert.equal(missingCssResource.body.errors.includes("forbidden_css_url"), true);
      } finally {
        await stopServer(started);
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Zig server package validation rejects platform-generated artifacts",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-server-platform-artifact-"));
    try {
      const executablePath = buildServerExecutable(scratch);
      const started = await startServer(executablePath, scratch, "platform-artifact-validation");
      try {
        for (const generatedFile of ["signature.json", "install-report.json", "content-hashes.json"]) {
          const packageBody = packageForApp("notes-lite");
          packageBody.files.push({ path: generatedFile, content: "{}" });

          const result = await validateWebappPackage(started.url, packageBody);
          assert.equal(result.status, 200, generatedFile);
          assert.equal(result.body.ok, false, generatedFile);
          assert.equal(result.body.errors.includes("platform_generated_artifact"), true, generatedFile);
        }
      } finally {
        await stopServer(started);
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Zig server package validation rejects bridge appId params",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-server-appid-param-"));
    try {
      const executablePath = buildServerExecutable(scratch);
      const started = await startServer(executablePath, scratch, "appid-param-validation");
      try {
        const packageBody = packageForApp("notes-lite");
        const appScript = packageBody.files.find((file) => file.path === "app.js");
        appScript.content += '\nAppRuntime.call("storage.get", { appId: "other-app", key: "notes-lite:notes" });\n';

        const result = await validateWebappPackage(started.url, packageBody);
        assert.equal(result.status, 200);
        assert.equal(result.body.ok, false);
        assert.equal(result.body.errors.includes("forbidden_appid_param"), true);
      } finally {
        await stopServer(started);
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

function buildServerExecutable(scratch) {
  const targetArgs = targetArgsForHost();
  const executablePath = path.join(scratch, process.platform === "win32" ? "terrane-server.exe" : "terrane-server");
  const zigEnv = {
    ...process.env,
    ZIG_GLOBAL_CACHE_DIR: path.join(scratch, "zig-global-cache"),
    ZIG_LOCAL_CACHE_DIR: path.join(scratch, "zig-local-cache"),
  };

  execFileSync("zig", ["build-exe", ...zigServerModuleArgs(), ...targetArgs, "-lc", "-lsqlite3", "-fno-emit-bin"], {
    cwd: serverDir,
    env: zigEnv,
    stdio: "ignore",
  });

  if (process.platform === "darwin") {
    assert.equal(hasCc(), true);
    const objectPath = path.join(scratch, "terrane-server.o");
    execFileSync(
      "zig",
      ["build-obj", ...zigServerModuleArgs(), ...targetArgs, "-lc", `-femit-bin=${objectPath}`],
      { cwd: serverDir, env: zigEnv, stdio: "ignore" },
    );
    execFileSync("cc", [objectPath, "-lsqlite3", "-o", executablePath], { stdio: "ignore" });
  } else {
    execFileSync(
      "zig",
      ["build-exe", ...zigServerModuleArgs(), ...targetArgs, "-lc", "-lsqlite3", `-femit-bin=${executablePath}`],
      { cwd: serverDir, env: zigEnv, stdio: "ignore" },
    );
  }

  return executablePath;
}

async function startServer(executablePath, scratch, fileName) {
  const port = await freePort();
  const dbPath = path.join(scratch, `${fileName}.sqlite`);
  const tokenFile = path.join(scratch, `${fileName}.token`);
  const child = spawn(executablePath, ["--port", String(port), "--token-file", tokenFile], {
    env: {
      ...process.env,
      TERRANE_SERVER_DB: dbPath,
      TERRANE_SERVER_CONTROL_TOKEN: controlToken,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });

  const output = [];
  child.stdout.on("data", (chunk) => output.push(String(chunk)));
  child.stderr.on("data", (chunk) => output.push(String(chunk)));

  await waitForServer(child, `http://127.0.0.1:${port}`, output, fileName);
  return { child, output, url: `http://127.0.0.1:${port}` };
}

async function stopServer(started) {
  if (started.child.exitCode != null) return;
  started.child.kill("SIGTERM");
  await new Promise((resolve) => {
    const timeout = setTimeout(resolve, 1000);
    started.child.once("exit", () => {
      clearTimeout(timeout);
      resolve();
    });
  });
}

async function waitForServer(child, url, output, fileName) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (child.exitCode != null) {
      throw new Error(`${fileName} server exited early: ${output.join("")}`);
    }
    try {
      const response = await fetch(`${url}/health`);
      if (response.ok) return;
    } catch {
      // Try again until the process has bound the port.
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`${fileName} server did not start: ${output.join("")}`);
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => resolve(address.port));
    });
  });
}

async function controlCommand(baseUrl, tool, args = {}) {
  const response = await postJson(`${baseUrl}/control/command`, { tool, args }, {
    "x-platform-control-token": controlToken,
  });
  return response.body;
}

async function validateWebappPackage(baseUrl, packageBody) {
  return postJson(`${baseUrl}/webapps/validate`, packageBody);
}

async function bridgeCall(baseUrl, appId, sessionId, request) {
  const response = await postJson(`${baseUrl}/bridge`, request, {
    "x-app-id": appId,
    "x-runtime-session-id": sessionId,
    "x-mount-token": `mount-${sessionId}`,
  });
  assert.equal(response.status, 200);
  return response.body;
}

async function postJson(url, body, headers = {}) {
  const response = await fetch(url, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...headers,
    },
    body: JSON.stringify(body),
  });
  const text = await response.text();
  return {
    status: response.status,
    body: JSON.parse(text),
  };
}

function packageForFixture(fixture) {
  const pkg = readPackage(path.join(examplesDir, fixture.context.appId));
  const manifest = {
    ...pkg.manifest,
    ...(fixture.preconditions?.manifestPatch ?? {}),
    resourceBudget: {
      ...pkg.manifest.resourceBudget,
      ...(fixture.preconditions?.resourceBudget ?? {}),
    },
  };

  const files = [...pkg.files.entries()].map(([filePath, content]) => ({
    path: filePath,
    content: filePath === "manifest.json" ? `${JSON.stringify(manifest, null, 2)}\n` : content,
  }));
  return { manifest, files };
}

function packageForApp(appId, mapIndexHtml = (html) => html) {
  const pkg = readPackage(path.join(examplesDir, appId));
  const files = [...pkg.files.entries()].map(([filePath, content]) => ({
    path: filePath,
    content: filePath === "index.html" ? mapIndexHtml(content) : content,
  }));
  return { manifest: pkg.manifest, files };
}

function packageForNotebookApp(appId) {
  const pkg = readPackage(path.join(examplesDir, appId));
  const notebookPermissions = ["notebook.read", "notebook.write", "notebook.propose", "notebook.approve", "notebook.sync"];
  const manifest = {
    ...pkg.manifest,
    permissions: [...new Set([...pkg.manifest.permissions, ...notebookPermissions])],
    capabilities: {
      required: [...new Set([...pkg.manifest.capabilities.required, "notebook.read"])],
      optional: [...new Set([...pkg.manifest.capabilities.optional, "notebook.write", "notebook.propose", "notebook.approve", "notebook.sync"])],
    },
  };
  const files = [...pkg.files.entries()].map(([filePath, content]) => ({
    path: filePath,
    content: filePath === "manifest.json" ? `${JSON.stringify(manifest, null, 2)}\n` : content,
  }));
  return { manifest, files };
}

async function applyBridgeFixturePreconditions(baseUrl, fixture, sessionId) {
  const appId = fixture.context.appId;
  for (const mock of fixture.preconditions?.networkMocks ?? []) {
    const set = await controlCommand(baseUrl, "runtime.network_mock_set", {
      appId,
      sessionId,
      method: mock.method ?? "GET",
      urlPattern: mock.urlPattern,
      response: mock.response,
    });
    assert.equal(set.ok, true, `${fixture.id} network mock`);
  }

  for (const mock of fixture.preconditions?.dialogMocks ?? []) {
    const set = await controlCommand(baseUrl, "runtime.dialog_mock_set", {
      appId,
      sessionId,
      dialogType: mock.dialogType,
      response: mock.response,
    });
    assert.equal(set.ok, true, `${fixture.id} dialog mock`);
  }
}

function expectedForPlatform(fixture, platform) {
  return fixture.expectedByPlatform?.[platform] ?? fixture.expected;
}

function assertBridgeExpected(response, expected, label) {
  assert.equal(response.ok, expected?.ok, label);
  if (expected?.errorCode) {
    assert.equal(response.error.code, expected.errorCode, label);
  }
  if ("resultOk" in (expected ?? {})) {
    assert.equal(response.result?.ok, expected.resultOk, label);
  }
  if (expected?.resultErrorCode) {
    assert.equal(response.result?.error?.code, expected.resultErrorCode, label);
  }
  if (expected?.resultSubset) {
    assertDeepSubset(response.result, expected.resultSubset, `${label} result`);
  }
  if (expected?.errorDetailsSubset) {
    assertDeepSubset(response.error?.details, expected.errorDetailsSubset, `${label} error details`);
  }
}

function assertDeepSubset(actual, expected, label) {
  if (Array.isArray(expected)) {
    assert.deepEqual(actual, expected, label);
    return;
  }
  if (expected && typeof expected === "object") {
    assert.equal(Boolean(actual && typeof actual === "object" && !Array.isArray(actual)), true, label);
    for (const [key, value] of Object.entries(expected)) {
      assertDeepSubset(actual[key], value, `${label}.${key}`);
    }
    return;
  }
  assert.deepEqual(actual, expected, label);
}
