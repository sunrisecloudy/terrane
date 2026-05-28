import fs from "node:fs";
import { BrowserSmokeRunner } from "./browser-smoke-runner.js";
import { BridgeDispatcher, controlError, controlResponse } from "./bridge-dispatcher.js";
import { fakeHostCapabilities } from "./capabilities.js";
import { CoreEngine } from "./core.js";
import { bridgeError, errorBody, PlatformError } from "./errors.js";
import { examplesDir, repoRoot, resolveInside, runtimeWebDir } from "./paths.js";
import { packageHashes, readPackage, validatePackage } from "./package-validator.js";
import { PlatformDatabase } from "./platform-database.js";
import { createPlatformKeypair, signPackage, verifyInstalledPackage } from "./signing.js";
import { TestRunner } from "./test-runner.js";
import { canonicalJson, prettyJson, sha256 } from "./util.js";

export class FakePlatformHost {
  constructor({
    dbFile = ":memory:",
    controlToken = "dev-token-change-me",
    runtimeVersion = "0.1.0",
    allowRuntimeMismatch = false,
    browserSmokeRunner = null,
    smokeRunner = process.env.NATIVE_AI_SMOKE_RUNNER ?? "static",
  } = {}) {
    this.database = new PlatformDatabase({ dbFile });
    this.core = new CoreEngine();
    this.bridge = new BridgeDispatcher({ database: this.database, core: this.core, runtimeVersion, allowRuntimeMismatch });
    this.testRunner = new TestRunner({
      database: this.database,
      runControlCommand: (tool, args) => this.runControlCommand(tool, args),
      browserSmokeRunner:
        browserSmokeRunner ??
        new BrowserSmokeRunner({
          database: this.database,
          dispatchBridge: (request, context) => this.dispatchBridge(request, context),
        }),
      smokeRunner,
    });
    this.keypair = createPlatformKeypair();
    this.controlToken = controlToken;
    this.dbFile = dbFile;
    this.runtimeVersion = runtimeVersion;
    this.allowRuntimeMismatch = allowRuntimeMismatch;
    this.auditControlSessionId = null;
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

  assertRuntimeStorage(args) {
    const appId = requiredArg(args, "appId");
    const key = requiredArg(args, "key");
    const expected = requiredArg(args, "value");
    const actual = this.database.storageGet(appId, key, null);
    if (canonicalJson(actual) !== canonicalJson(expected)) {
      throw new PlatformError("storage.assert_failed", "Storage value did not match expected JSON", { appId, key, expected, actual });
    }
    return { ok: true, appId, key, value: actual };
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

  assertCoreAction(args) {
    const appId = requiredArg(args, "appId");
    const expectedType = args.type ?? null;
    const expectedMatch = args.match ?? {};
    const matches = this.database
      .queryCoreActions(appId)
      .map((row) => ({ row, action: JSON.parse(row.action_json) }))
      .filter(({ action }) => (!expectedType || action.type === expectedType) && matchesJsonSubset(action, expectedMatch));
    if (matches.length === 0) {
      throw new PlatformError("core_action.not_found", "Expected core action was not found", { appId, type: expectedType, match: expectedMatch });
    }
    return { ok: true, appId, count: matches.length, actions: matches.map((match) => match.action) };
  }

  compareSnapshots(args) {
    const left = args.left ?? (args.leftSnapshotId ? this.database.runtimeSnapshotById(args.leftSnapshotId).snapshot : null);
    const right = args.right ?? (args.rightSnapshotId ? this.database.runtimeSnapshotById(args.rightSnapshotId).snapshot : null);
    if (!left || !right) {
      throw new PlatformError("invalid_request", "runtime.compare_snapshot requires left/right snapshots or snapshot ids", {});
    }
    const equal = canonicalJson(left) === canonicalJson(right);
    return {
      ok: equal,
      equal,
      leftHash: `sha256:${sha256(canonicalJson(left))}`,
      rightHash: `sha256:${sha256(canonicalJson(right))}`,
    };
  }

  controlAuditSession() {
    if (!this.auditControlSessionId) {
      this.auditControlSessionId = this.database.createControlSession({
        target: "fake-host",
        actor: "control-audit",
        metadata: { implicit: true },
      }).controlSessionId;
    }
    return this.auditControlSessionId;
  }

  resolveAuditControlSession(controlSessionId) {
    if (controlSessionId) {
      try {
        return this.database.controlSession(controlSessionId);
      } catch {
        return this.database.controlSession(this.controlAuditSession());
      }
    }
    return this.database.controlSession(this.controlAuditSession());
  }

  auditControlRequest({ req, path, tool, args = null, result = null, error = null, startedAt, controlSessionId = null, runtimeSessionId = null }) {
    const session = this.resolveAuditControlSession(controlSessionId);
    const errorPayload = error ? errorBody(error) : null;
    this.database.logControlCommand({
      controlSessionId: session.controlSessionId,
      runtimeSessionId: runtimeSessionId ?? session.runtimeSessionId ?? null,
      tool: tool ?? `${req.method} ${path}`,
      args,
      result,
      error: errorPayload,
      durationMs: Math.max(0, Date.now() - startedAt),
      httpMethod: req.method,
      path,
      decision: errorPayload ? "rejected" : "accepted",
      errorCode: errorPayload?.code ?? null,
    });
  }

  listWebapps(args = {}) {
    const installed = this.database.listWebapps({ includeUninstalled: args.includeUninstalled === true });
    const installedIds = new Set(installed.map((app) => app.appId));
    const bundled = this.exampleManifestList()
      .filter((app) => !installedIds.has(app.id))
      .map((app) => ({
        appId: app.id,
        name: app.name,
        version: app.version,
        description: app.description,
        status: "bundled",
        bundled: true,
        installed: false,
      }));
    return {
      apps: [
        ...installed.map((app) => ({ ...app, bundled: false, installed: true })),
        ...bundled,
      ],
    };
  }

  async runRepairLoop(args = {}) {
    const startedAt = new Date().toISOString();
    const packagePath = packagePathArg(args);
    const maxAttempts = Math.max(1, Math.min(args.maxAttempts ?? 1, 3));
    const microtestPaths = Array.isArray(args.microtestPaths)
      ? args.microtestPaths
      : args.microtestPath
        ? [args.microtestPath]
        : [];
    const attemptReports = [];
    const snapshots = [];
    const testsRun = [];
    let appId = null;
    let finalStatus = "failed";
    let remainingWarnings = [];

    for (let index = 0; index < maxAttempts; index += 1) {
      const steps = [];
      const validation = this.validatePackage(packagePath);
      remainingWarnings = validation.warnings ?? [];
      steps.push(repairStep("platform.validate_package", validation.ok ? "passed" : "failed", validation));
      if (!validation.ok) {
        attemptReports.push({ index: index + 1, status: "failed", steps });
        break;
      }

      const signed = this.signPackage(packagePath, { trustLevel: args.trustLevel ?? "developer" });
      steps.push(repairStep("platform.sign_webapp_package", "passed", signed));

      const install = this.installPackage(packagePath, { trustLevel: args.trustLevel ?? "developer" });
      appId = install.appId;
      steps.push(repairStep("platform.install_webapp_package", install.status === "enabled" ? "passed" : install.status, install));
      if (install.approval?.requiresUserApproval) {
        finalStatus = "requires-approval";
        attemptReports.push({ index: index + 1, status: finalStatus, steps, diagnostics: this.repairDiagnostics(appId) });
        break;
      }
      if (install.status !== "enabled") {
        attemptReports.push({ index: index + 1, status: "failed", steps, diagnostics: this.repairDiagnostics(appId) });
        break;
      }

      const opened = await this.runControlCommand("platform.open_webapp", { appId });
      steps.push(repairStep("platform.open_webapp", "passed", opened));
      steps.push(repairStep("runtime.capabilities", "passed", fakeHostCapabilities(appId)));
      steps.push(repairStep("runtime.snapshot", "passed", this.runtimeSnapshot(appId)));

      const snapshot = this.database.createSnapshot({ appId, type: "post-test" });
      snapshots.push(snapshot.snapshotId);
      steps.push(repairStep("platform.create_snapshot", "passed", snapshot));

      let smokeOk = true;
      if (args.runSmokeTests !== false) {
        const smoke = await this.testRunner.runSmokeTests(appId, { runner: args.smokeRunner ?? args.runner });
        testsRun.push(smoke.microTestId);
        smokeOk = smoke.status === "passed";
        steps.push(repairStep("runtime.run_smoke_tests", smokeOk ? "passed" : "failed", smoke));
      }

      let microOk = true;
      for (const microtestPath of microtestPaths) {
        const micro = await this.testRunner.runMicroTest({ microtestPath });
        testsRun.push(micro.microTestId);
        microOk &&= micro.status === "passed";
        steps.push(repairStep("runtime.run_microtest", micro.status === "passed" ? "passed" : "failed", micro));
      }

      const accessibility = this.runAccessibilityAudit(appId);
      steps.push(repairStep("runtime.run_accessibility_audit", accessibility.status === "fail" ? "failed" : "passed", accessibility));
      steps.push(repairStep("runtime.resource_usage", "passed", this.database.resourceUsage(appId)));
      steps.push(repairStep("platform.install_report", "passed", this.database.installReport(appId)));

      finalStatus = smokeOk && microOk && accessibility.status !== "fail" ? "passed" : "failed";
      attemptReports.push({ index: index + 1, status: finalStatus, steps, diagnostics: this.repairDiagnostics(appId) });
      if (finalStatus === "passed") break;
    }

    return {
      ok: finalStatus === "passed",
      appId,
      startedAt,
      attempts: attemptReports.length,
      finalStatus,
      changedFiles: [],
      testsRun,
      snapshots,
      remainingWarnings,
      attemptReports,
    };
  }

  repairDiagnostics(appId) {
    if (!appId) return {};
    return {
      installReport: this.database.installReport(appId),
      appVersions: this.database.queryAppVersions(appId),
      appStorage: this.database.queryAppStorage(appId),
      bridgeCalls: this.database.queryBridgeCalls(appId),
      coreEvents: this.database.queryCoreEvents(appId),
      testRuns: this.database.queryTestRuns(appId),
    };
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
      case "platform.launch":
        return { ok: true, target: args.target ?? "fake-host", status: "running", url: `http://127.0.0.1:${args.port ?? 7878}` };
      case "platform.stop":
        return { ok: true, target: args.target ?? "fake-host", status: "stopped" };
      case "platform.reload_runtime":
        return { ok: true, target: args.target ?? "fake-host", status: "reloaded" };
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
      case "platform.list_webapps":
        return this.listWebapps(args);
      case "platform.uninstall_webapp":
        return this.database.uninstallWebapp(requiredArg(args, "appId"), { confirm: args.confirm === true, actor: args.actor ?? "codex" });
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
      case "runtime.drag":
        return this.validateRuntimeTarget(tool, args);
      case "runtime.wait_for":
        return { ok: true, kind: args.kind ?? "idle" };
      case "runtime.timer_advance":
        return { ok: true, advancedMs: args.ms ?? args.milliseconds ?? 0 };
      case "runtime.fault_inject":
        return this.bridge.addFault(normalizeFaultArgs(args));
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
      case "runtime.assert_storage":
        return this.assertRuntimeStorage(args);
      case "runtime.assert_core_action":
        return this.assertCoreAction(args);
      case "platform.reset_webapp":
      case "runtime.storage_reset":
        return this.database.resetWebapp(requiredArg(args, "appId"));
      case "runtime.resource_usage":
        return this.database.resourceUsage(requiredArg(args, "appId"));
      case "runtime.console_logs":
        return { appId: args.appId ?? null, logs: [] };
      case "runtime.event_log":
        return {
          appId: args.appId ?? null,
          bridgeCalls: this.database.queryBridgeCalls(args.appId ?? null),
          coreEvents: this.database.queryCoreEvents(args.appId ?? null),
        };
      case "runtime.clear_logs":
        return this.database.clearRuntimeLogs(args.appId ?? null);
      case "runtime.assert_bridge_call":
        return this.database.assertBridgeCall({
          appId: requiredArg(args, "appId"),
          method: requiredArg(args, "method"),
        });
      case "runtime.assert_no_console_errors":
        return { ok: true, errors: 0 };
      case "runtime.call_bridge":
        return this.bridge.dispatch(
          { id: args.id ?? "control_call_bridge", method: requiredArg(args, "method"), params: args.params ?? {} },
          { appId: requiredArg(args, "appId"), sessionId: args.sessionId },
        );
      case "runtime.core_step":
        return this.bridge.dispatch(
          { id: args.id ?? "control_core_step", method: "core.step", params: { event: requiredArg(args, "event") } },
          { appId: requiredArg(args, "appId"), sessionId: args.sessionId },
        );
      case "runtime.core_snapshot":
        return this.core.snapshot(args.appId ?? null);
      case "runtime.replay_events":
        return {
          ok: true,
          appId: requiredArg(args, "appId"),
          replay: this.core.replay(args.appId, requiredArg(args, "events")),
        };
      case "runtime.compare_snapshot":
        return this.compareSnapshots(args);
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
        return this.testRunner.runSmokeTests(requiredArg(args, "appId"), { runner: args.runner ?? args.mode });
      case "runtime.run_microtest":
        return this.testRunner.runMicroTest({ spec: args.spec, microtestPath: args.microtestPath });
      case "platform.run_platform_smoke":
        return this.testRunner.runPlatformSmokeTest({ spec: args.spec, smokePath: args.smokePath, platform: args.platform ?? "fake-host" });
      case "platform.run_repair_loop":
        return this.runRepairLoop(args);
      case "runtime.network_mock_set":
        this.database.addNetworkMock(normalizeNetworkMockArgs(args));
        return { ok: true };
      case "runtime.network_mock_reset":
        return this.database.resetNetworkMocks({ sessionId: args.sessionId ?? null, appId: args.appId ?? null });
      case "runtime.dialog_mock_set":
        this.database.addDialogMock(normalizeDialogMockArgs(args));
        return { ok: true };
      case "runtime.notification_capture":
        return {
          appId: args.appId ?? null,
          notifications: this.bridge.notifications.filter((notification) => !args.appId || notification.appId === args.appId),
        };
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
        const mountToken = req.headers["x-mount-token"];
        if (!mountToken) {
          const id = body && typeof body === "object" && !Array.isArray(body) && typeof body.id === "string" ? body.id : null;
          return sendJson(
            res,
            200,
            bridgeError(id, new PlatformError("bridge.unauthorized_channel", "Bridge calls require a channel-derived mount token")),
          );
        }
        return sendJson(res, 200, await this.dispatchBridge(body, { appId, sessionId, mountToken }));
      }

      const sessionRoute = parseControlSessionRoute(url.pathname);
      if (sessionRoute) {
        const startedAt = Date.now();
        try {
          this.requireControlToken(req);
        } catch (error) {
          this.auditControlRequest({ req, path: url.pathname, tool: `${req.method} ${url.pathname}`, error, startedAt });
          return sendJson(res, 401, controlError(error));
        }
        return this.handleControlSessionRoute(req, res, sessionRoute, startedAt, url.pathname);
      }

      const directRoute = parseDirectControlRoute(url.pathname);
      if (directRoute) {
        const startedAt = Date.now();
        try {
          this.requireControlToken(req);
        } catch (error) {
          this.auditControlRequest({ req, path: url.pathname, tool: directRoute.tool, args: directRoute.args, error, startedAt });
          return sendJson(res, 401, controlError(error));
        }
        return this.handleDirectControlRoute(req, res, directRoute, startedAt, url.pathname);
      }

      if (req.method === "POST" && url.pathname === "/control/command") {
        const startedAt = Date.now();
        try {
          this.requireControlToken(req);
        } catch (error) {
          this.auditControlRequest({ req, path: url.pathname, tool: "control.command", error, startedAt });
          return sendJson(res, 401, controlError(error));
        }
        let body = {};
        try {
          body = await readBodyJson(req);
          const result = await this.runControlCommand(body.tool, body.args ?? {});
          this.auditControlRequest({ req, path: url.pathname, tool: body.tool, args: body.args ?? {}, result, startedAt });
          return sendJson(res, 200, controlResponse(result));
        } catch (error) {
          this.auditControlRequest({ req, path: url.pathname, tool: body.tool ?? "control.command", args: body.args ?? {}, error, startedAt });
          return sendJson(res, 400, controlError(error));
        }
      }

      return sendJson(res, 404, { ok: false, error: { code: "not_found", message: "Route not found", details: {} } });
    } catch (error) {
      return sendJson(res, 500, controlError(error));
    }
  }

  async handleDirectControlRoute(req, res, route, startedAt = Date.now(), requestPath = null) {
    const path = requestPath ?? new URL(req.url, "http://127.0.0.1").pathname;
    if (!route.methods.includes(req.method)) {
      const error = new PlatformError("not_found", "Control route not found", {});
      this.auditControlRequest({ req, path, tool: route.tool, args: route.args, error, startedAt });
      return sendJson(res, 404, controlError(error));
    }
    let args = route.args;
    try {
      const body = req.method === "POST" ? await readBodyJson(req) : {};
      args = { ...route.args, ...body };
      const result = await this.runControlCommand(route.tool, args);
      this.auditControlRequest({ req, path, tool: route.tool, args, result, startedAt });
      return sendJson(res, 200, controlResponse(result));
    } catch (error) {
      this.auditControlRequest({ req, path, tool: route.tool, args, error, startedAt });
      return sendJson(res, 400, controlError(error));
    }
  }

  async handleControlSessionRoute(req, res, route, startedAt = Date.now(), requestPath = null) {
    const path = requestPath ?? new URL(req.url, "http://127.0.0.1").pathname;
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
        this.auditControlRequest({
          req,
          path,
          tool: "control.sessions.create",
          args: body,
          result: session,
          startedAt,
          controlSessionId: session.controlSessionId,
          runtimeSessionId: session.runtimeSessionId,
        });
        return sendJson(res, 200, controlResponse(session));
      }

      if (req.method === "DELETE" && route.kind === "item") {
        const session = this.resolveAuditControlSession(route.controlSessionId);
        const result = this.database.endControlSession(route.controlSessionId);
        this.auditControlRequest({
          req,
          path,
          tool: "control.sessions.end",
          args: { controlSessionId: route.controlSessionId },
          result,
          startedAt,
          controlSessionId: route.controlSessionId,
          runtimeSessionId: session.runtimeSessionId,
        });
        return sendJson(res, 200, controlResponse(result));
      }

      if (req.method === "GET" && route.kind === "subresource") {
        const session = this.database.controlSession(route.controlSessionId);
        if (route.subresource === "snapshot") {
          const snapshot = session.appId ? this.runtimeSnapshot(session.appId) : this.database.snapshot();
          const result = { controlSessionId: session.controlSessionId, snapshot };
          this.auditControlRequest({
            req,
            path,
            tool: "control.sessions.snapshot",
            args: { controlSessionId: route.controlSessionId },
            result,
            startedAt,
            controlSessionId: session.controlSessionId,
            runtimeSessionId: session.runtimeSessionId,
          });
          return sendJson(res, 200, controlResponse(result));
        }
        if (route.subresource === "events") {
          const result = {
            controlSessionId: session.controlSessionId,
            runtimeSessionId: session.runtimeSessionId,
            appId: session.appId,
            bridgeCalls: this.database.queryBridgeCalls(session.appId),
            coreEvents: this.database.queryCoreEvents(session.appId),
          };
          this.auditControlRequest({
            req,
            path,
            tool: "control.sessions.events",
            args: { controlSessionId: route.controlSessionId },
            result,
            startedAt,
            controlSessionId: session.controlSessionId,
            runtimeSessionId: session.runtimeSessionId,
          });
          return sendJson(res, 200, controlResponse(result));
        }
        if (route.subresource === "capabilities") {
          const result = fakeHostCapabilities(session.appId);
          this.auditControlRequest({
            req,
            path,
            tool: "control.sessions.capabilities",
            args: { controlSessionId: route.controlSessionId },
            result,
            startedAt,
            controlSessionId: session.controlSessionId,
            runtimeSessionId: session.runtimeSessionId,
          });
          return sendJson(res, 200, controlResponse(result));
        }
      }

      const error = new PlatformError("not_found", "Control session route not found", {});
      this.auditControlRequest({ req, path, tool: `${req.method} ${path}`, args: route, error, startedAt, controlSessionId: route.controlSessionId });
      return sendJson(res, 404, controlError(error));
    } catch (error) {
      this.auditControlRequest({ req, path, tool: `${req.method} ${path}`, args: route, error, startedAt, controlSessionId: route.controlSessionId });
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
    const expectedAuthorization = `Bearer ${this.controlToken}`;
    const hasSpecHeader = req.headers["x-platform-control-token"] === this.controlToken;
    const hasLegacyAuthorization = req.headers.authorization === expectedAuthorization;
    if (!hasSpecHeader && !hasLegacyAuthorization) {
      throw new PlatformError("control_auth_required", "Missing or invalid control token", {});
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

function normalizeFaultArgs(args) {
  return {
    appId: args.appId ?? null,
    method: args.method ?? methodForFaultKind(args.kind),
    code: args.code ?? "fault_injected",
    message: args.message ?? "Injected bridge fault",
    details: args.details ?? { kind: args.kind ?? null },
    once: args.once ?? true,
  };
}

function methodForFaultKind(kind) {
  switch (kind) {
    case "storage.read":
      return "storage.get";
    case "storage.write":
      return "storage.set";
    case "network":
    case "network.request":
      return "network.request";
    case "core":
    case "core.step":
      return "core.step";
    default:
      return kind;
  }
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

function parseDirectControlRoute(pathname) {
  const normalized = pathname.startsWith("/control/")
    ? pathname.replace(/^\/control/, "")
    : pathname;
  const tools = new Map([
    ["/packages/validate", { tool: "platform.validate_package", methods: ["POST"] }],
    ["/packages/sign", { tool: "platform.sign_webapp_package", methods: ["POST"] }],
    ["/packages/policy-audit", { tool: "platform.run_policy_audit", methods: ["POST"] }],
    ["/db/snapshot", { tool: "db.snapshot", methods: ["POST"] }],
    ["/db/app-storage", { tool: "db.query_app_storage", methods: ["POST"] }],
    ["/db/app-versions", { tool: "db.query_app_versions", methods: ["POST"] }],
    ["/db/export-debug-bundle", { tool: "db.export_debug_bundle", methods: ["POST"] }],
    ["/db/bridge-calls", { tool: "db.query_bridge_calls", methods: ["POST"] }],
    ["/db/core-events", { tool: "db.query_core_events", methods: ["POST"] }],
    ["/db/test-runs", { tool: "db.query_test_runs", methods: ["POST"] }],
  ]);
  const mapped = tools.get(normalized);
  if (mapped) return { ...mapped, args: {} };

  const appRoute = normalized.match(/^\/apps\/([^/]+)\/(versions|install-report|rollback)$/);
  if (!appRoute) return null;
  const appId = decodeURIComponent(appRoute[1]);
  if (appRoute[2] === "versions") return { tool: "platform.list_webapp_versions", methods: ["GET"], args: { appId } };
  if (appRoute[2] === "install-report") return { tool: "platform.install_report", methods: ["GET"], args: { appId } };
  return { tool: "platform.rollback_webapp", methods: ["POST"], args: { appId } };
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

function matchesJsonSubset(actual, expected) {
  if (expected === undefined || expected === null) return true;
  if (Array.isArray(expected)) return canonicalJson(actual) === canonicalJson(expected);
  if (expected && typeof expected === "object") {
    if (!actual || typeof actual !== "object") return false;
    return Object.entries(expected).every(([key, value]) => matchesJsonSubset(actual[key], value));
  }
  return Object.is(actual, expected);
}

function repairStep(tool, status, result) {
  return {
    tool,
    status,
    result: summarizeRepairResult(result),
  };
}

function summarizeRepairResult(result) {
  if (!result || typeof result !== "object") return result;
  return {
    ok: result.ok ?? (result.status ? result.status === "passed" || result.status === "enabled" : undefined),
    appId: result.appId,
    installId: result.installId,
    sessionId: result.sessionId,
    snapshotId: result.snapshotId,
    status: result.status,
    microTestId: result.microTestId,
    testRunId: result.testRunId,
    failures: result.result?.failures ?? result.failures,
    warnings: result.warnings,
    errors: result.errors,
  };
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
