import fs from "node:fs";
import { BridgeDispatcher, controlError, controlResponse } from "./bridge-dispatcher.js";
import { fakeHostCapabilities } from "./capabilities.js";
import { CoreEngine } from "./core.js";
import { PlatformError } from "./errors.js";
import { examplesDir, repoRoot, resolveInside, runtimeWebDir } from "./paths.js";
import { packageHashes, readPackage, validatePackage } from "./package-validator.js";
import { PlatformDatabase } from "./platform-database.js";
import { createPlatformKeypair, signPackage, verifyInstalledPackage } from "./signing.js";
import { TestRunner } from "./test-runner.js";
import { canonicalJson, prettyJson, sha256 } from "./util.js";

export class FakePlatformHost {
  constructor({ dbFile = ":memory:", controlToken = "dev-token-change-me", runtimeVersion = "0.1.0", allowRuntimeMismatch = false } = {}) {
    this.database = new PlatformDatabase({ dbFile });
    this.core = new CoreEngine();
    this.bridge = new BridgeDispatcher({ database: this.database, core: this.core });
    this.testRunner = new TestRunner({
      database: this.database,
      runControlCommand: (tool, args) => this.runControlCommand(tool, args),
    });
    this.keypair = createPlatformKeypair();
    this.controlToken = controlToken;
    this.dbFile = dbFile;
    this.runtimeVersion = runtimeVersion;
    this.allowRuntimeMismatch = allowRuntimeMismatch;
  }

  close() {
    this.database.close();
  }

  health() {
    return {
      ok: true,
      version: this.runtimeVersion,
      db: this.dbFile === ":memory:" ? "sqlite-mem" : "sqlite-file",
      keyId: this.keypair.keyId,
      capabilities: fakeHostCapabilities(),
    };
  }

  installPackage(packageDir, { trustLevel = "developer" } = {}) {
    const pkg = readPackage(packageDir);
    const signed = signPackage({ manifest: pkg.manifest, files: pkg.files, trustLevel, keypair: this.keypair });
    const smokeTest = this.evaluateBundledSmokeTests(pkg);
    const compatibility = this.evaluateRuntimeCompatibility(pkg.manifest.runtimeVersion);
    const approval = this.evaluateUpdateApproval(pkg.manifest);
    const canActivate = smokeTest.ok && compatibility.ok && !approval.requiresUserApproval;
    const blockedByFailure = !smokeTest.ok || !compatibility.ok;
    const install = this.database.insertInstalledPackage({
      manifest: pkg.manifest,
      files: pkg.files,
      hashes: signed.hashes,
      validation: summarizeValidation(pkg.validation),
      signature: signed.signature,
      contentHashesDocument: signed.contentHashesDocument,
      trustLevel,
      smokeTest,
      compatibility,
      approval,
      activate: canActivate,
      versionStatus: canActivate ? "enabled" : blockedByFailure ? "quarantined" : "installed",
      reportStatus: canActivate ? "accepted" : blockedByFailure ? "failed" : "requires-approval",
    });
    this.database.recordTestRun({
      microTestId: `smoke:${pkg.manifest.id}`,
      name: `${pkg.manifest.id} bundled smoke tests`,
      appId: pkg.manifest.id,
      spec: smokeTest.spec ?? [],
      status: smokeTest.ok ? "passed" : "failed",
      result: smokeTest,
    });
    return {
      ...install,
      status: canActivate ? "enabled" : blockedByFailure ? "quarantined" : "requires-approval",
      smokeTest,
      compatibility,
      approval,
    };
  }

  signPackage(packageDir, { trustLevel = "developer" } = {}) {
    const pkg = readPackage(packageDir);
    return signPackage({ manifest: pkg.manifest, files: pkg.files, trustLevel, keypair: this.keypair });
  }

  verifyInstalledApp(appId) {
    const installed = this.database.activeInstallPackage(appId);
    if (!installed) {
      throw new PlatformError("app_not_installed", `App is not installed: ${appId}`, { appId });
    }
    if (installed.status === "quarantined") {
      throw new PlatformError("package_quarantined", `App is quarantined: ${appId}`, { appId });
    }
    const compatibility = this.evaluateRuntimeCompatibility(installed.manifest.runtimeVersion);
    if (!compatibility.ok && !this.allowRuntimeMismatch) {
      throw new PlatformError("runtime_version_incompatible", "App runtimeVersion is not compatible with the fake-host runtime", compatibility);
    }
    return verifyInstalledPackage({
      manifest: installed.manifest,
      files: installed.files,
      signature: installed.signature,
      publicKey: this.keypair.publicKey,
    });
  }

  validatePackage(packageDir) {
    const result = validatePackage(packageDir);
    return summarizeValidation(result);
  }

  evaluateBundledSmokeTests(pkg) {
    const smokeText = pkg.files.get("smoke-tests.json");
    if (!smokeText) {
      return {
        ok: true,
        status: "not-run",
        appId: pkg.manifest.id,
        total: 0,
        failures: [],
        spec: [],
      };
    }
    try {
      const tests = JSON.parse(smokeText);
      const result = this.testRunner.evaluateSmokeTests({
        appId: pkg.manifest.id,
        tests,
        html: pkg.files.get("index.html") ?? "",
        appJs: pkg.files.get("app.js") ?? "",
      });
      return { ...result, status: result.ok ? "passed" : "failed", spec: tests };
    } catch (error) {
      return {
        ok: false,
        status: "failed",
        appId: pkg.manifest.id,
        total: 0,
        failures: [{ code: "package.invalid", message: error.message, path: "smoke-tests.json" }],
        spec: [],
      };
    }
  }

  evaluateRuntimeCompatibility(appRuntimeVersion) {
    const runtime = parseSemver(this.runtimeVersion);
    const app = parseSemver(appRuntimeVersion);
    const ok = Boolean(runtime && app && app.major === runtime.major && app.minor <= runtime.minor);
    return {
      ok,
      runtimeVersion: this.runtimeVersion,
      appRuntimeVersion,
      allowRuntimeMismatch: this.allowRuntimeMismatch,
    };
  }

  evaluateUpdateApproval(manifest) {
    const active = this.database.activeInstall(manifest.id);
    if (!active) {
      return { requiresUserApproval: false, reasons: [] };
    }
    const previous = active.manifest;
    const checks = [
      ["permissions", sortedStrings(previous.permissions), sortedStrings(manifest.permissions)],
      ["networkPolicy", previous.networkPolicy ?? {}, manifest.networkPolicy ?? {}],
      ["resourceBudget", previous.resourceBudget ?? {}, manifest.resourceBudget ?? {}],
      ["capabilities", previous.capabilities ?? {}, manifest.capabilities ?? {}],
      ["dataVersion", previous.dataVersion, manifest.dataVersion],
    ];
    const reasons = checks
      .filter(([, before, after]) => canonicalJson(before) !== canonicalJson(after))
      .map(([field]) => field);
    return {
      requiresUserApproval: reasons.length > 0,
      reasons,
      previousInstallId: active.installId,
    };
  }

  async dispatchBridge(request, context) {
    return this.bridge.dispatch(request, context);
  }

  runtimeSnapshot(appId) {
    const pkg = this.activeRuntimePackage(appId);
    const html = pkg.files.get("index.html") ?? "";
    return {
      appId,
      installId: pkg.installId,
      version: pkg.version,
      title: html.match(/<title>([^<]+)<\/title>/i)?.[1] ?? pkg.manifest.name,
      testIds: extractTestIds(html),
      text: htmlToText(html),
      accessibilityTree: accessibilitySnapshotFromHtml(appId, html),
      resourceUsage: this.database.resourceUsage(appId),
    };
  }

  runtimeScreenshot(args) {
    const appId = requiredArg(args, "appId");
    const snapshot = this.runtimeSnapshot(appId);
    return {
      ok: true,
      appId,
      label: args.label ?? null,
      format: "static-html-summary",
      title: snapshot.title,
      textHash: `sha256:${sha256(snapshot.text)}`,
      testIds: snapshot.testIds,
    };
  }

  runtimeQuery(args) {
    const appId = requiredArg(args, "appId");
    const pkg = this.activeRuntimePackage(appId);
    const html = pkg.files.get("index.html") ?? "";
    const query = args.testId ? `[data-testid="${args.testId}"]` : args.selector ?? args.text ?? "";
    const matches = queryMatches(html, args);
    return { ok: matches.length > 0, appId, query, matches };
  }

  validateRuntimeTarget(tool, args) {
    if (tool === "runtime.press_key") {
      return { ok: true, key: args.key ?? null };
    }
    const result = this.runtimeQuery(args);
    if (!result.ok) {
      throw new PlatformError("selector.not_found", "Runtime target was not found in installed package HTML", {
        appId: args.appId,
        testId: args.testId,
        selector: args.selector,
        text: args.text,
      });
    }
    return { ok: true, tool, target: result.matches[0] };
  }

  assertRuntimeVisible(args) {
    const result = this.runtimeQuery(args);
    if (!result.ok) {
      throw new PlatformError("selector.not_found", "Expected runtime target is not visible", {
        appId: args.appId,
        testId: args.testId,
        selector: args.selector,
        text: args.text,
      });
    }
    return { ok: true, matches: result.matches.length };
  }

  assertRuntimeText(args) {
    const appId = requiredArg(args, "appId");
    const text = requiredArg(args, "text");
    const pkg = this.activeRuntimePackage(appId);
    const html = pkg.files.get("index.html") ?? "";
    if (!htmlToText(html).includes(text)) {
      throw new PlatformError("text.not_found", "Expected text was not found in installed package HTML", { appId, text });
    }
    return { ok: true, text };
  }

  accessibilitySnapshot(appId) {
    const pkg = this.activeRuntimePackage(appId);
    return accessibilitySnapshotFromHtml(appId, pkg.files.get("index.html") ?? "");
  }

  runAccessibilityAudit(appId) {
    const snapshot = this.accessibilitySnapshot(appId);
    const checks = [
      accessibilityCheck("document_title", snapshot.title.length > 0, "Document must include a non-empty <title>."),
      accessibilityCheck("main_landmark", snapshot.landmarks.some((landmark) => landmark.role === "main"), "Page must include a <main> landmark."),
      accessibilityCheck("screen_title", snapshot.headings.some((heading) => heading.level === 1), "Page must include an h1 screen title."),
      accessibilityCheck(
        "no_unlabeled_controls",
        snapshot.controls.every((control) => control.name.length > 0),
        "Every interactive control must have an accessible name.",
        snapshot.controls.find((control) => control.name.length === 0)?.selector,
      ),
    ];
    const status = checks.some((check) => check.status === "fail") ? "fail" : checks.some((check) => check.status === "warn") ? "warn" : "pass";
    return {
      appId,
      checkedAt: new Date().toISOString(),
      status,
      checks,
    };
  }

  assertAccessibility(args) {
    const report = this.runAccessibilityAudit(requiredArg(args, "appId"));
    const rule = args.rule ?? null;
    const failures = report.checks.filter((check) => check.status === "fail" && (!rule || check.id === rule));
    if (failures.length > 0) {
      throw new PlatformError("accessibility_failed", "Accessibility assertion failed", {
        appId: report.appId,
        rule,
        failures,
      });
    }
    return { ok: true, appId: report.appId, rule, report };
  }

  activeRuntimePackage(appId) {
    this.verifyInstalledApp(appId);
    const pkg = this.database.activeInstallPackage(appId);
    if (!pkg) {
      throw new PlatformError("app_not_installed", `App is not installed: ${appId}`, { appId });
    }
    return pkg;
  }

  async runControlCommand(tool, args = {}) {
    switch (tool) {
      case "platform.health":
        return this.health();
      case "platform.list_targets":
        return {
          targets: [
            { id: "fake-host", platform: "fake-host", status: "available", runtimeVersion: this.runtimeVersion },
            { id: "macos", platform: "macos", status: "not-attached" },
            { id: "server", platform: "server", status: "not-attached" },
          ],
        };
      case "runtime.capabilities":
        return fakeHostCapabilities(args.appId ?? null);
      case "platform.validate_package":
        return this.validatePackage(packagePathArg(args));
      case "platform.sign_webapp_package":
        return this.signPackage(packagePathArg(args), {
          trustLevel: args.trustLevel ?? "developer",
        });
      case "platform.run_policy_audit":
        return this.validatePackage(packagePathArg(args));
      case "platform.install_webapp_package":
        return this.installPackage(packagePathArg(args), {
          trustLevel: args.trustLevel ?? "developer",
        });
      case "platform.list_webapp_versions":
        return this.database.listWebappVersions(requiredArg(args, "appId"));
      case "platform.rollback_webapp":
        return this.database.rollbackWebapp(requiredArg(args, "appId"), args.installId ?? null);
      case "platform.approve_webapp_update":
        return this.database.approveWebappUpdate(requiredArg(args, "appId"), requiredArg(args, "installId"));
      case "platform.quarantine_webapp":
        return this.database.quarantineWebapp(requiredArg(args, "appId"), args.installId ?? null, args.reason ?? "manual quarantine");
      case "platform.install_report":
        return this.database.installReport(requiredArg(args, "appId"), args.installId ?? null);
      case "platform.create_snapshot":
        return this.database.createSnapshot({ appId: requiredArg(args, "appId"), type: args.type ?? "manual", sessionId: args.sessionId ?? null });
      case "platform.restore_snapshot":
        return this.database.restoreSnapshot(requiredArg(args, "snapshotId"));
      case "platform.migration_dry_run":
        return this.database.runMigration({ migration: requiredArg(args, "migration"), mode: "dry-run" });
      case "platform.migration_apply":
        return this.database.runMigration({ migration: requiredArg(args, "migration"), mode: "apply" });
      case "platform.open_webapp":
        this.verifyInstalledApp(requiredArg(args, "appId"));
        return {
          sessionId: this.database.createRuntimeSession({ appId: args.appId }),
          appId: args.appId,
        };
      case "runtime.snapshot":
        return this.runtimeSnapshot(requiredArg(args, "appId"));
      case "runtime.screenshot":
        return this.runtimeScreenshot(args);
      case "runtime.query":
        return this.runtimeQuery(args);
      case "runtime.click":
      case "runtime.type":
      case "runtime.set_value":
      case "runtime.press_key":
        return this.validateRuntimeTarget(tool, args);
      case "runtime.wait_for":
        return { ok: true, kind: args.kind ?? "idle" };
      case "runtime.assert_visible":
        return this.assertRuntimeVisible(args);
      case "runtime.assert_text":
        return this.assertRuntimeText(args);
      case "runtime.accessibility_snapshot":
        return this.accessibilitySnapshot(requiredArg(args, "appId"));
      case "runtime.run_accessibility_audit":
        return this.runAccessibilityAudit(requiredArg(args, "appId"));
      case "runtime.assert_accessibility":
        return this.assertAccessibility(args);
      case "platform.reset_webapp":
      case "runtime.storage_reset":
        return this.database.resetWebapp(requiredArg(args, "appId"));
      case "runtime.resource_usage":
        return this.database.resourceUsage(requiredArg(args, "appId"));
      case "runtime.clear_logs":
        return this.database.clearRuntimeLogs(args.appId ?? null);
      case "runtime.assert_bridge_call":
        return this.database.assertBridgeCall({
          appId: requiredArg(args, "appId"),
          method: requiredArg(args, "method"),
        });
      case "runtime.assert_no_console_errors":
        return { ok: true, errors: 0 };
      case "runtime.core_step":
        return this.bridge.dispatch(
          { id: args.id ?? "control_core_step", method: "core.step", params: { event: requiredArg(args, "event") } },
          { appId: requiredArg(args, "appId"), sessionId: args.sessionId },
        );
      case "runtime.storage_get":
        return this.bridge.dispatch(
          {
            id: args.id ?? "control_storage_get",
            method: "storage.get",
            params: { key: requiredArg(args, "key"), defaultValue: args.defaultValue ?? null },
          },
          { appId: requiredArg(args, "appId"), sessionId: args.sessionId },
        );
      case "runtime.storage_set":
        return this.bridge.dispatch(
          {
            id: args.id ?? "control_storage_set",
            method: "storage.set",
            params: { key: requiredArg(args, "key"), value: args.value },
          },
          { appId: requiredArg(args, "appId"), sessionId: args.sessionId },
        );
      case "runtime.bridge_calls":
      case "db.query_bridge_calls":
        return this.database.queryBridgeCalls(args.appId ?? null);
      case "runtime.run_smoke_tests":
        return this.testRunner.runSmokeTests(requiredArg(args, "appId"));
      case "runtime.run_microtest":
        return this.testRunner.runMicroTest({ spec: args.spec, microtestPath: args.microtestPath });
      case "platform.run_platform_smoke":
        return this.testRunner.runPlatformSmokeTest({ spec: args.spec, smokePath: args.smokePath, platform: args.platform ?? "fake-host" });
      case "runtime.network_mock_set":
        this.database.addNetworkMock(normalizeNetworkMockArgs(args));
        return { ok: true };
      case "runtime.dialog_mock_set":
        this.database.addDialogMock(normalizeDialogMockArgs(args));
        return { ok: true };
      case "db.snapshot":
        return this.database.snapshot();
      case "db.export_backup":
        return this.database.exportBackup({ type: "backup", runtimeCapabilities: fakeHostCapabilities() });
      case "db.export_debug_bundle":
        return this.database.exportBackup({ type: "debug-bundle", runtimeCapabilities: fakeHostCapabilities(), includeDebug: true });
      case "db.import_backup":
        return this.importBackup(requiredArg(args, "backup"));
      case "db.query_app_storage":
        return this.database.queryAppStorage(requiredArg(args, "appId"));
      case "db.query_app_versions":
        return this.database.queryAppVersions(requiredArg(args, "appId"));
      case "db.query_core_events":
        return this.database.queryCoreEvents(args.appId ?? null);
      case "db.query_test_runs":
        return this.database.queryTestRuns(args.appId ?? null);
      default:
        throw new PlatformError("unknown_tool", `Unknown control tool: ${tool}`, { tool });
    }
  }

  async handleHttp(req, res) {
    try {
      const url = new URL(req.url, "http://127.0.0.1");

      if (req.method === "GET" && url.pathname === "/health") {
        return sendJson(res, 200, this.health());
      }

      if (req.method === "GET" && url.pathname === "/") {
        return this.serveStatic(res, runtimeWebDir, "index.html");
      }

      if (req.method === "GET" && url.pathname === "/webapps/examples.json") {
        return sendJson(res, 200, this.exampleManifestList());
      }

      if (req.method === "GET" && url.pathname.startsWith("/webapps/examples/")) {
        return this.serveStatic(res, examplesDir, url.pathname.replace("/webapps/examples/", ""));
      }

      if (req.method === "GET" && url.pathname.startsWith("/runtime/")) {
        return this.serveStatic(res, runtimeWebDir, url.pathname.replace("/runtime/", ""));
      }

      if (req.method === "POST" && url.pathname === "/bridge") {
        const body = await readBodyJson(req);
        const appId = req.headers["x-app-id"];
        const sessionId = req.headers["x-runtime-session-id"];
        return sendJson(res, 200, await this.dispatchBridge(body, { appId, sessionId }));
      }

      const sessionRoute = parseControlSessionRoute(url.pathname);
      if (sessionRoute) {
        try {
          this.requireControlToken(req);
        } catch (error) {
          return sendJson(res, 401, controlError(error));
        }
        return this.handleControlSessionRoute(req, res, sessionRoute);
      }

      if (req.method === "POST" && url.pathname === "/control/command") {
        try {
          this.requireControlToken(req);
        } catch (error) {
          return sendJson(res, 401, controlError(error));
        }
        const body = await readBodyJson(req);
        try {
          const result = await this.runControlCommand(body.tool, body.args ?? {});
          return sendJson(res, 200, controlResponse(result));
        } catch (error) {
          return sendJson(res, 400, controlError(error));
        }
      }

      return sendJson(res, 404, { ok: false, error: { code: "not_found", message: "Route not found", details: {} } });
    } catch (error) {
      return sendJson(res, 500, controlError(error));
    }
  }

  async handleControlSessionRoute(req, res, route) {
    try {
      if (req.method === "POST" && route.kind === "collection") {
        const body = await readBodyJson(req);
        const session = this.database.createControlSession({
          target: body.target ?? "fake-host",
          appId: body.appId ?? null,
          actor: body.actor ?? "codex",
          metadata: body.metadata ?? {},
          tokenHash: body.tokenHash ?? null,
        });
        return sendJson(res, 200, controlResponse(session));
      }

      if (req.method === "DELETE" && route.kind === "item") {
        return sendJson(res, 200, controlResponse(this.database.endControlSession(route.controlSessionId)));
      }

      if (req.method === "GET" && route.kind === "subresource") {
        const session = this.database.controlSession(route.controlSessionId);
        if (route.subresource === "snapshot") {
          const snapshot = session.appId ? this.runtimeSnapshot(session.appId) : this.database.snapshot();
          return sendJson(res, 200, controlResponse({ controlSessionId: session.controlSessionId, snapshot }));
        }
        if (route.subresource === "events") {
          return sendJson(res, 200, controlResponse({
            controlSessionId: session.controlSessionId,
            runtimeSessionId: session.runtimeSessionId,
            appId: session.appId,
            bridgeCalls: this.database.queryBridgeCalls(session.appId),
            coreEvents: this.database.queryCoreEvents(session.appId),
          }));
        }
        if (route.subresource === "capabilities") {
          return sendJson(res, 200, controlResponse(fakeHostCapabilities(session.appId)));
        }
      }

      return sendJson(res, 404, controlError(new PlatformError("not_found", "Control session route not found", {})));
    } catch (error) {
      return sendJson(res, 400, controlError(error));
    }
  }

  serveStatic(res, root, relPath) {
    const filePath = resolveInside(root, relPath);
    if (!fs.existsSync(filePath) || !fs.statSync(filePath).isFile()) {
      return sendJson(res, 404, { ok: false, error: { code: "not_found", message: "File not found", details: {} } });
    }
    sendText(res, 200, fs.readFileSync(filePath, "utf8"), contentType(filePath));
  }

  exampleManifestList() {
    return fs
      .readdirSync(examplesDir, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => {
        const manifest = JSON.parse(fs.readFileSync(resolveInside(examplesDir, `${entry.name}/manifest.json`), "utf8"));
        return {
          id: manifest.id,
          name: manifest.name,
          version: manifest.version,
          description: manifest.description,
        };
      })
      .sort((a, b) => a.id.localeCompare(b.id));
  }

  requireControlToken(req) {
    const expected = `Bearer ${this.controlToken}`;
    if (req.headers.authorization !== expected) {
      throw new PlatformError("control.unauthorized", "Missing or invalid control token", {});
    }
  }

  importBackup(backup) {
    const result = this.database.importBackup(backup);
    for (const version of backup.appVersions ?? []) {
      const installId = version.install_id ?? version.installId;
      const installed = this.database.installedPackageByInstallId(installId);
      if (!installed) continue;
      const signed = signPackage({
        manifest: installed.manifest,
        files: installed.files,
        trustLevel: installed.trustLevel ?? "developer",
        keypair: this.keypair,
      });
      this.database.updateInstalledSignature({
        installId,
        signature: signed.signature,
        hashes: signed.hashes,
      });
    }
    return result;
  }
}

export function sendJson(res, status, value) {
  sendText(res, status, prettyJson(value), "application/json");
}

function sendText(res, status, body, contentTypeValue) {
  res.writeHead(status, {
    "content-type": `${contentTypeValue}; charset=utf-8`,
    "content-length": Buffer.byteLength(body),
  });
  res.end(body);
}

function contentType(filePath) {
  if (filePath.endsWith(".html")) return "text/html";
  if (filePath.endsWith(".css")) return "text/css";
  if (filePath.endsWith(".js")) return "text/javascript";
  if (filePath.endsWith(".json")) return "application/json";
  return "text/plain";
}

async function readBodyJson(req) {
  let body = "";
  for await (const chunk of req) {
    body += chunk;
  }
  return body ? JSON.parse(body) : {};
}

function requiredArg(args, name) {
  if (!(name in args)) {
    throw new PlatformError("invalid_request", `Missing required argument: ${name}`, { name });
  }
  return args[name];
}

function packagePathArg(args) {
  const packagePath = args.packagePath ?? args.path;
  if (!packagePath) {
    throw new PlatformError("invalid_request", "Missing required argument: packagePath", { aliases: ["packagePath", "path"] });
  }
  return packagePath.startsWith("/") ? packagePath : resolveInside(repoRoot, packagePath);
}

function normalizeNetworkMockArgs(args) {
  const match = args.match ?? {};
  return {
    sessionId: args.sessionId ?? null,
    appId: args.appId ?? null,
    method: args.method ?? match.method ?? "GET",
    urlPattern: args.urlPattern ?? match.url ?? match.urlPattern,
    response: args.response,
  };
}

function normalizeDialogMockArgs(args) {
  const method = args.method ?? "";
  const dialogType = args.dialogType ?? method.replace(/^dialog\./, "");
  return {
    sessionId: args.sessionId ?? null,
    appId: args.appId ?? null,
    dialogType,
    response: args.response ?? {
      files: args.files ?? [],
      selectedPath: args.selectedPath ?? null,
      cancelled: args.cancelled ?? false,
    },
  };
}

function parseControlSessionRoute(pathname) {
  const normalized = pathname.startsWith("/control/sessions")
    ? pathname.replace(/^\/control/, "")
    : pathname;
  if (normalized === "/sessions") {
    return { kind: "collection" };
  }
  const match = normalized.match(/^\/sessions\/([^/]+)(?:\/([^/]+))?$/);
  if (!match) return null;
  return {
    kind: match[2] ? "subresource" : "item",
    controlSessionId: decodeURIComponent(match[1]),
    subresource: match[2] ? decodeURIComponent(match[2]) : null,
  };
}

function parseSemver(version) {
  const match = String(version ?? "").match(/^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/);
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
  };
}

function sortedStrings(values) {
  return [...new Set(Array.isArray(values) ? values : [])].sort();
}

function extractTestIds(html) {
  return [...html.matchAll(/\bdata-testid=["']([^"']+)["']/g)].map((match) => match[1]).sort();
}

function queryMatches(html, args) {
  if (args.testId) {
    const tag = tagForAttribute(html, "data-testid", args.testId);
    return tag ? [{ kind: "testId", value: args.testId, tag }] : [];
  }
  if (args.selector?.startsWith("#")) {
    const id = args.selector.slice(1);
    const tag = tagForAttribute(html, "id", id);
    return tag ? [{ kind: "selector", value: args.selector, tag }] : [];
  }
  const testId = args.selector?.match(/\[data-testid=["']([^"']+)["']\]/)?.[1];
  if (testId) {
    const tag = tagForAttribute(html, "data-testid", testId);
    return tag ? [{ kind: "selector", value: args.selector, tag }] : [];
  }
  if (args.text) {
    return htmlToText(html).includes(args.text) ? [{ kind: "text", value: args.text }] : [];
  }
  if (/^[a-z][a-z0-9-]*$/i.test(args.selector ?? "")) {
    const tag = args.selector.toLowerCase();
    return new RegExp(`<${escapeRegExp(tag)}\\b`, "i").test(html) ? [{ kind: "selector", value: args.selector, tag }] : [];
  }
  return [];
}

function tagForAttribute(html, attr, value) {
  const pattern = new RegExp(`<([a-z0-9-]+)\\b[^>]*\\b${attr}=["']${escapeRegExp(value)}["'][^>]*>`, "i");
  return html.match(pattern)?.[1] ?? null;
}

function htmlToText(html) {
  return html
    .replace(/<script\b[\s\S]*?<\/script>/gi, " ")
    .replace(/<style\b[\s\S]*?<\/style>/gi, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function accessibilitySnapshotFromHtml(appId, html) {
  return {
    appId,
    title: html.match(/<title>([^<]+)<\/title>/i)?.[1]?.trim() ?? "",
    landmarks: /<main\b/i.test(html) ? [{ role: "main", selector: "main" }] : [],
    headings: [...html.matchAll(/<h([1-6])\b[^>]*>([\s\S]*?)<\/h\1>/gi)].map((match) => ({
      level: Number(match[1]),
      name: htmlToText(match[2]),
    })),
    controls: extractControls(html),
  };
}

function accessibilityCheck(id, ok, message, selector = undefined) {
  return {
    id,
    status: ok ? "pass" : "fail",
    message,
    ...(selector ? { selector } : {}),
  };
}

function extractControls(html) {
  const controls = [];
  const paired = /<(button|select|textarea|a)\b([^>]*)>([\s\S]*?)<\/\1>/gi;
  let match;
  while ((match = paired.exec(html))) {
    const tag = match[1].toLowerCase();
    const attrs = parseAttrs(match[2] ?? "");
    controls.push(controlRecord({ html, tag, attrs, innerHtml: match[3] ?? "" }));
  }
  const inputs = /<input\b([^>]*)>/gi;
  while ((match = inputs.exec(html))) {
    const attrs = parseAttrs(match[1] ?? "");
    const type = (attrs.type ?? "text").toLowerCase();
    if (type === "hidden") continue;
    controls.push(controlRecord({ html, tag: "input", attrs, innerHtml: "" }));
  }
  return controls.sort((a, b) => a.selector.localeCompare(b.selector));
}

function controlRecord({ html, tag, attrs, innerHtml }) {
  const testId = attrs["data-testid"] ?? "";
  const id = attrs.id ?? "";
  const selector = testId ? `[data-testid="${testId}"]` : id ? `#${id}` : tag;
  return {
    tag,
    type: attrs.type ?? null,
    testId,
    selector,
    name: accessibleName({ html, tag, attrs, innerHtml }),
  };
}

function accessibleName({ html, tag, attrs, innerHtml }) {
  for (const attr of ["aria-label", "title"]) {
    if (attrs[attr]?.trim()) return attrs[attr].trim();
  }
  if ((tag === "button" || tag === "a") && htmlToText(innerHtml).trim()) {
    return htmlToText(innerHtml).trim();
  }
  if (attrs.id) {
    const explicit = labelForId(html, attrs.id);
    if (explicit) return explicit;
    const wrapped = wrappingLabelForControl(html, tag, attrs.id);
    if (wrapped) return wrapped;
  }
  return "";
}

function labelForId(html, id) {
  const match = html.match(new RegExp(`<label\\b[^>]*\\bfor=["']${escapeRegExp(id)}["'][^>]*>([\\s\\S]*?)<\\/label>`, "i"));
  return match ? htmlToText(match[1]).trim() : "";
}

function wrappingLabelForControl(html, tag, id) {
  const match = html.match(new RegExp(`<label\\b[^>]*>([\\s\\S]*?<${tag}\\b[^>]*\\bid=["']${escapeRegExp(id)}["'][^>]*>[\\s\\S]*?)<\\/label>`, "i"));
  if (!match) return "";
  return htmlToText(match[1].replace(new RegExp(`<${tag}\\b[\\s\\S]*`, "i"), "")).trim();
}

function parseAttrs(attrsText) {
  const attrs = {};
  for (const match of attrsText.matchAll(/\b([a-zA-Z_:][-a-zA-Z0-9_:.]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))/g)) {
    attrs[match[1].toLowerCase()] = match[2] ?? match[3] ?? match[4] ?? "";
  }
  return attrs;
}

function escapeRegExp(value) {
  return String(value).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function summarizeValidation(result) {
  return {
    ok: result.ok,
    errors: result.errors,
    warnings: result.warnings,
    manifest: result.manifest
      ? {
          id: result.manifest.id,
          version: result.manifest.version,
          runtimeVersion: result.manifest.runtimeVersion,
          dataVersion: result.manifest.dataVersion,
        }
      : null,
    bridgeMethods: result.bridgeMethods,
  };
}
