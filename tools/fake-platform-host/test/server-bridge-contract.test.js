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
  return ["--dep", "zig_core", "-Mroot=src/main.zig", "-Mzig_core=../zig-core/src/lib.zig"];
}

test(
  "checked-in bridge fixtures match Zig server expected responses",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-server-contract-"));
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
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-server-accessibility-"));
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
  "Zig server requires confirmation before destructive reset controls",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-server-reset-confirm-"));
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
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-server-stylesheet-"));
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
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-server-script-"));
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

function buildServerExecutable(scratch) {
  const targetArgs = targetArgsForHost();
  const executablePath = path.join(scratch, process.platform === "win32" ? "native-ai-server.exe" : "native-ai-server");

  execFileSync("zig", ["build-exe", ...zigServerModuleArgs(), ...targetArgs, "-lc", "-lsqlite3", "-fno-emit-bin"], {
    cwd: serverDir,
    stdio: "ignore",
  });

  if (process.platform === "darwin") {
    assert.equal(hasCc(), true);
    const objectPath = path.join(scratch, "native-ai-server.o");
    execFileSync(
      "zig",
      ["build-obj", ...zigServerModuleArgs(), ...targetArgs, "-lc", `-femit-bin=${objectPath}`],
      { cwd: serverDir, stdio: "ignore" },
    );
    execFileSync("cc", [objectPath, "-lsqlite3", "-o", executablePath], { stdio: "ignore" });
  } else {
    execFileSync(
      "zig",
      ["build-exe", ...zigServerModuleArgs(), ...targetArgs, "-lc", "-lsqlite3", `-femit-bin=${executablePath}`],
      { cwd: serverDir, stdio: "ignore" },
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
      NATIVE_AI_SERVER_DB: dbPath,
      NATIVE_AI_SERVER_CONTROL_TOKEN: controlToken,
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
