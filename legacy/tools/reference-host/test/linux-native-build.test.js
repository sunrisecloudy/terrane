import assert from "node:assert/strict";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { packageReleaseArtifacts } from "../../../tools/package-release.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const linuxDir = path.join(repoRoot, "native", "linux");
const forgeDir = path.join(repoRoot, "forge");

function commandWorks(command, args = ["--version"]) {
  try {
    execFileSync(command, args, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function commandExists(command) {
  try {
    execFileSync("sh", ["-c", "command -v \"$1\" >/dev/null", "sh", command], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasLinuxNativeDependencies() {
  return commandWorks("pkg-config", [
    "--exists",
    "gtk4",
    "webkitgtk-6.0",
    "json-glib-1.0",
    "sqlite3",
    "libsoup-3.0",
  ]);
}

function linuxNativeSkipReason({ requireCargo = false, requireSqliteCli = false } = {}) {
  if (process.platform !== "linux") return "Linux native smoke only runs on Linux hosts";
  if (!commandWorks("meson")) return "meson is not available";
  if (!commandWorks("ninja")) return "ninja is not available";
  if (requireCargo && !commandWorks("cargo", ["--version"])) return "cargo is not available";
  if (requireSqliteCli && !commandWorks("sqlite3", ["-version"])) return "sqlite3 CLI is not available";
  if (!hasLinuxNativeDependencies()) return "GTK/WebKitGTK development dependencies are not available";
  return false;
}

function linuxPackagedNativeSmokeSkipReason() {
  const baseReason = linuxNativeSkipReason({ requireCargo: true });
  if (baseReason) return baseReason;
  if (process.env.TERRANE_LINUX_SMOKE_LAUNCH !== "1") {
    return "set TERRANE_LINUX_SMOKE_LAUNCH=1 to run packaged Linux native launch smoke";
  }
  return "packaged Linux release smoke waits for the Phase 2.6 Forge packaging cutover";
}

function buildLinuxForgeFfi(scratch) {
  const targetDir = path.join(scratch, "forge-target");
  execFileSync("cargo", ["build", "-p", "forge-ffi", "--locked"], {
    cwd: forgeDir,
    env: {
      ...process.env,
      CARGO_TARGET_DIR: targetDir,
    },
    stdio: "ignore",
  });
  const forgeFfiSo = path.join(targetDir, "debug", "libforge_ffi.so");
  assert.equal(fs.existsSync(forgeFfiSo), true);
  return forgeFfiSo;
}

test(
  "Linux GTK/WebKitGTK host builds and optionally runs native smoke",
  {
    skip: linuxNativeSkipReason({ requireCargo: true }),
    timeout: 180_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-linux-smoke-"));
    try {
      const forgeFfiSo = buildLinuxForgeFfi(scratch);

      const buildDir = path.join(scratch, "build");
      execFileSync("meson", ["setup", buildDir, linuxDir], { stdio: "ignore" });
      execFileSync("meson", ["compile", "-C", buildDir], { stdio: "ignore" });
      const binaryPath = path.join(buildDir, "terrane-host");
      assert.equal(fs.existsSync(binaryPath), true);

      runOptionalSmoke({ binaryPath, scratch, forgeFfiSo });
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Linux release host rejects dev-only startup flags and audits the rejection",
  {
    skip: linuxNativeSkipReason({ requireSqliteCli: true }),
    timeout: 120_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-linux-production-guard-"));
    try {
      const buildDir = path.join(scratch, "release-build");
      execFileSync("meson", ["setup", "--buildtype=release", buildDir, linuxDir], { stdio: "ignore" });
      execFileSync("meson", ["compile", "-C", buildDir], { stdio: "ignore" });

      const binaryPath = path.join(buildDir, "terrane-host");
      const xdgDataHome = path.join(scratch, "xdg-data");
      const result = spawnSync(binaryPath, ["--allow-unsigned-dev"], {
        cwd: repoRoot,
        env: { ...process.env, XDG_DATA_HOME: xdgDataHome },
        encoding: "utf8",
        timeout: 30_000,
      });
      const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
      assert.equal(result.status, 1, output);
      assert.match(output, /production build rejects dev-only startup flag --allow-unsigned-dev/);

      const dbPath = path.join(xdgDataHome, "Terrane", "platform.sqlite");
      assert.equal(fs.existsSync(dbPath), true, "production guard should create the platform audit database");
      const count = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'native.production_guard' AND decision = 'rejected' AND error_code = 'dev_only_flag' AND args_json LIKE '%--allow-unsigned-dev%';",
        ],
        { encoding: "utf8" },
      ).trim();
      assert.equal(count, "1");
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Linux debug dev control health is token-gated and audited",
  {
    skip: linuxNativeSkipReason({ requireSqliteCli: true, requireCargo: true }),
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-linux-dev-control-"));
    let child = null;
    try {
      const forgeFfiSo = buildLinuxForgeFfi(scratch);

      const buildDir = path.join(scratch, "debug-build");
      execFileSync("meson", ["setup", buildDir, linuxDir], { stdio: "ignore" });
      execFileSync("meson", ["compile", "-C", buildDir], { stdio: "ignore" });

      const binaryPath = path.join(buildDir, "terrane-host");
      const xdgDataHome = path.join(scratch, "xdg-data");
      const xdgRuntimeDir = path.join(scratch, "xdg-runtime");
      fs.mkdirSync(xdgRuntimeDir, { recursive: true, mode: 0o700 });

      child = launchHost(binaryPath, ["--terrane-dev-control", "--control-plane-port=0"], {
        ...process.env,
        XDG_DATA_HOME: xdgDataHome,
        XDG_RUNTIME_DIR: xdgRuntimeDir,
        TERRANE_FORGE_FFI_SO: forgeFfiSo,
      });
      const ready = await waitForControlReady(child);
      assert.equal(ready.tokenPath, path.join(xdgRuntimeDir, "terrane", "control.token"));

      const tokenStat = fs.statSync(ready.tokenPath);
      assert.equal(tokenStat.mode & 0o777, 0o600);
      const token = fs.readFileSync(ready.tokenPath, "utf8").trim();
      assert.match(token, /^[A-Za-z0-9_-]{43}$/);

      const unauthorized = await requestControlHealth(ready.port);
      assert.equal(unauthorized.statusCode, 401);
      assert.equal(JSON.parse(unauthorized.body).error.code, "control_auth_required");

      const authorized = await requestControlHealth(ready.port, token);
      assert.equal(authorized.statusCode, 200);
      const body = JSON.parse(authorized.body);
      assert.equal(body.ok, true);
      assert.equal(body.target, "linux");
      assert.equal(body.controlPlane.port, ready.port);

      const unauthorizedSession = await requestControl(ready.port, "/sessions", {
        method: "POST",
        body: { appId: "notes-lite" },
      });
      assert.equal(unauthorizedSession.statusCode, 401);
      assert.equal(JSON.parse(unauthorizedSession.body).error.code, "control_auth_required");

      const session = await requestControl(ready.port, "/control/sessions", {
        method: "POST",
        token,
        body: { appId: "task-workbench", metadata: { smoke: "linux-dev-control" } },
      });
      assert.equal(session.statusCode, 200, session.body);
      const sessionBody = JSON.parse(session.body);
      assert.equal(sessionBody.ok, true);
      assert.match(sessionBody.result.controlSessionId, /^control-/);
      assert.match(sessionBody.result.runtimeSessionId, /^session-/);
      assert.equal(sessionBody.result.appId, "task-workbench");
      assert.equal(sessionBody.result.status, "running");

      const sessionId = sessionBody.result.controlSessionId;
      const runtimeSessionId = sessionBody.result.runtimeSessionId;
      const snapshot = await requestControl(ready.port, `/control/sessions/${encodeURIComponent(sessionId)}/snapshot`, { token });
      assert.equal(snapshot.statusCode, 200, snapshot.body);
      const snapshotBody = JSON.parse(snapshot.body);
      assert.equal(snapshotBody.ok, true);
      assert.equal(snapshotBody.result.controlSessionId, sessionId);
      assert.equal(snapshotBody.result.snapshot.appId, "task-workbench");
      assert.equal(snapshotBody.result.snapshot.target, "linux");

      const events = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/events`, { token });
      assert.equal(events.statusCode, 200, events.body);
      const eventsBody = JSON.parse(events.body);
      assert.equal(eventsBody.ok, true);
      assert.equal(Array.isArray(eventsBody.result.bridgeCalls), true);
      assert.equal(Array.isArray(eventsBody.result.coreEvents), true);

      const capabilities = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/capabilities`, { token });
      assert.equal(capabilities.statusCode, 200, capabilities.body);
      const capabilitiesBody = JSON.parse(capabilities.body);
      assert.equal(capabilitiesBody.ok, true);
      assert.equal(capabilitiesBody.result.platform, "linux");
      assert.equal(capabilitiesBody.result.features["runtime.capabilities"], true);

      const command = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.health", args: {} },
      });
      assert.equal(command.statusCode, 200, command.body);
      const commandBody = JSON.parse(command.body);
      assert.equal(commandBody.ok, true);
      assert.equal(commandBody.result.ok, true);
      assert.equal(commandBody.result.target, "linux");

      const listTargets = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.list_targets", args: {} },
      });
      assert.equal(listTargets.statusCode, 200, listTargets.body);
      assert.equal(
        JSON.parse(listTargets.body).result.targets.some((target) => target.id === "linux-native" && target.status === "available"),
        true,
      );

      const listWebapps = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.list_webapps", args: {} },
      });
      assert.equal(listWebapps.statusCode, 200, listWebapps.body);
      const listedApps = JSON.parse(listWebapps.body).result.apps;
      assert.equal(listedApps.some((app) => app.appId === "notes-lite" && app.bundled === true && app.installed === false), true);
      assert.equal(listedApps.some((app) => app.appId === "task-workbench" && app.bundled === true && app.installed === false), true);

      const packagePath = "webapps/examples/notes-lite";
      const validatePackage = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.validate_package", args: { packagePath } },
      });
      assert.equal(validatePackage.statusCode, 200, validatePackage.body);
      const validatePackageBody = JSON.parse(validatePackage.body);
      assert.equal(validatePackageBody.result.ok, true);
      assert.equal(validatePackageBody.result.status, "passed");
      assert.equal(validatePackageBody.result.appId, "notes-lite");
      assert.equal(validatePackageBody.result.files.includes("manifest.json"), true);
      assert.equal(validatePackageBody.result.files.includes("app.js"), true);

      const policyAudit = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.run_policy_audit", args: { path: packagePath } },
      });
      assert.equal(policyAudit.statusCode, 200, policyAudit.body);
      const policyAuditBody = JSON.parse(policyAudit.body);
      assert.equal(policyAuditBody.result.ok, true);
      assert.equal(policyAuditBody.result.errors.length, 0);

      const signedPackage = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.sign_webapp_package", args: { packagePath, trustLevel: "developer" } },
      });
      assert.equal(signedPackage.statusCode, 200, signedPackage.body);
      const signedPackageBody = JSON.parse(signedPackage.body);
      assert.equal(signedPackageBody.result.ok, true);
      assert.equal(signedPackageBody.result.status, "signed");
      assert.equal(signedPackageBody.result.signature.algorithm, "ed25519");
      assert.equal(signedPackageBody.result.signature.signer, "linux-dev-control");
      assert.match(signedPackageBody.result.hashes.contentHash, /^sha256:[a-f0-9]{64}$/);

      const installPackage = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.install_webapp_package", args: { packagePath, trustLevel: "developer" } },
      });
      assert.equal(installPackage.statusCode, 200, installPackage.body);
      const installPackageBody = JSON.parse(installPackage.body);
      assert.equal(installPackageBody.result.ok, true);
      assert.equal(installPackageBody.result.status, "enabled");
      assert.equal(installPackageBody.result.appId, "notes-lite");
      assert.match(installPackageBody.result.installId, /^install-linux-/);
      assert.match(installPackageBody.result.reportId, /^report-linux-/);
      assert.equal(installPackageBody.result.smokeTest.status, "passed");
      assert.equal(installPackageBody.result.accessibility.status, "pass");
      assert.equal(installPackageBody.result.compatibility.ok, true);
      assert.equal(installPackageBody.result.files.some((file) => file.path === "manifest.json" && file.mime === "application/json"), true);

      const packageSession = await requestControl(ready.port, "/control/sessions", {
        method: "POST",
        token,
        body: { appId: "notes-lite", metadata: { smoke: "linux-package-lifecycle" } },
      });
      assert.equal(packageSession.statusCode, 200, packageSession.body);
      const packageSessionBody = JSON.parse(packageSession.body);
      const packageSessionId = packageSessionBody.result.controlSessionId;
      assert.equal(packageSessionBody.result.appId, "notes-lite");

      const openInstalledPackage = await requestControl(ready.port, `/sessions/${encodeURIComponent(packageSessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.open_webapp", args: { appId: "notes-lite" } },
      });
      assert.equal(openInstalledPackage.statusCode, 200, openInstalledPackage.body);
      const openInstalledPackageBody = JSON.parse(openInstalledPackage.body);
      assert.equal(openInstalledPackageBody.result.ok, true);
      assert.equal(openInstalledPackageBody.result.appId, "notes-lite");
      assert.equal(openInstalledPackageBody.result.installId, installPackageBody.result.installId);
      assert.equal(openInstalledPackageBody.result.bundled, false);
      assert.match(openInstalledPackageBody.result.sessionId, /^session-/);

      const installedVersions = await requestControl(ready.port, `/sessions/${encodeURIComponent(packageSessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_app_versions", args: { appId: "notes-lite" } },
      });
      assert.equal(installedVersions.statusCode, 200, installedVersions.body);
      const installedVersionRows = JSON.parse(installedVersions.body).result.rows;
      assert.equal(
        installedVersionRows.some((row) => row.install_id === installPackageBody.result.installId && row.status === "enabled" && row.signature_json.includes("\"algorithm\":\"ed25519\"")),
        true,
      );

      const postInstallSnapshot = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.snapshot", args: {} },
      });
      assert.equal(postInstallSnapshot.statusCode, 200, postInstallSnapshot.body);
      const postInstallSnapshotBody = JSON.parse(postInstallSnapshot.body);
      assert.equal(postInstallSnapshotBody.result.apps.some((row) => row.id === "notes-lite" && row.active_install_id === installPackageBody.result.installId), true);
      assert.equal(postInstallSnapshotBody.result.app_files.some((row) => row.install_id === installPackageBody.result.installId && row.path === "index.html"), true);
      assert.equal(postInstallSnapshotBody.result.app_permissions.some((row) => row.install_id === installPackageBody.result.installId && row.permission === "storage.read" && Number(row.approved) === 1), true);
      assert.equal(postInstallSnapshotBody.result.app_install_reports.some((row) => row.report_id === installPackageBody.result.reportId && row.status === "accepted"), true);
      assert.equal(postInstallSnapshotBody.result.runtime_sessions.some((row) => row.session_id === openInstalledPackageBody.result.sessionId && row.active_install_id === installPackageBody.result.installId), true);

      const packageLifecycleDbPath = path.join(xdgDataHome, "Terrane", "platform.sqlite");
      const packageLifecycleInstallationCount = execFileSync(
        "sqlite3",
        [
          packageLifecycleDbPath,
          `SELECT COUNT(*) FROM app_installations WHERE app_id = 'notes-lite' AND install_id = '${installPackageBody.result.installId}' AND action IN ('install', 'activate');`,
        ],
        { encoding: "utf8" },
      ).trim();
      assert.equal(Number(packageLifecycleInstallationCount), 2);

      const listWebappsAfterInstall = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.list_webapps", args: {} },
      });
      assert.equal(listWebappsAfterInstall.statusCode, 200, listWebappsAfterInstall.body);
      const appsAfterInstall = JSON.parse(listWebappsAfterInstall.body).result.apps;
      assert.equal(
        appsAfterInstall.some((app) => app.appId === "notes-lite" && app.installed === true && app.bundled === false && app.activeInstallId === installPackageBody.result.installId),
        true,
      );

      const staticScreenshot = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.screenshot", args: { appId: "task-workbench", label: "linux-ui-smoke" } },
      });
      assert.equal(staticScreenshot.statusCode, 200, staticScreenshot.body);
      assert.equal(JSON.parse(staticScreenshot.body).result.title, "Task Workbench");
      assert.equal(JSON.parse(staticScreenshot.body).result.testIds.includes("add-task-button"), true);

      const staticQuery = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.query", args: { appId: "task-workbench", testId: "add-task-button" } },
      });
      assert.equal(staticQuery.statusCode, 200, staticQuery.body);
      assert.equal(JSON.parse(staticQuery.body).result.matches[0].tag, "button");

      const staticClick = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.click", args: { appId: "task-workbench", testId: "add-task-button" } },
      });
      assert.equal(staticClick.statusCode, 200, staticClick.body);
      assert.equal(JSON.parse(staticClick.body).result.target.value, "add-task-button");

      const staticDrag = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.drag", args: { appId: "task-workbench", testId: "add-task-button" } },
      });
      assert.equal(staticDrag.statusCode, 200, staticDrag.body);
      assert.equal(JSON.parse(staticDrag.body).result.target.value, "add-task-button");

      const staticType = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.type", args: { appId: "task-workbench", testId: "task-title-input", text: "Linux static UI task" } },
      });
      assert.equal(staticType.statusCode, 200, staticType.body);
      assert.equal(JSON.parse(staticType.body).result.value, "Linux static UI task");

      const staticSetValue = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.set_value", args: { appId: "task-workbench", testId: "task-title-input", value: "Linux set value" } },
      });
      assert.equal(staticSetValue.statusCode, 200, staticSetValue.body);
      assert.equal(JSON.parse(staticSetValue.body).result.value, "Linux set value");

      const staticKey = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.press_key", args: { key: "Enter" } },
      });
      assert.equal(staticKey.statusCode, 200, staticKey.body);
      assert.equal(JSON.parse(staticKey.body).result.key, "Enter");

      const staticWait = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.wait_for", args: { appId: "task-workbench", kind: "text", text: "Task Workbench" } },
      });
      assert.equal(staticWait.statusCode, 200, staticWait.body);
      assert.equal(JSON.parse(staticWait.body).result.kind, "text");

      const staticAssertVisible = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.assert_visible", args: { appId: "task-workbench", testId: "task-workbench-title" } },
      });
      assert.equal(staticAssertVisible.statusCode, 200, staticAssertVisible.body);
      assert.equal(JSON.parse(staticAssertVisible.body).result.ok, true);

      const staticAssertText = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.assert_text", args: { appId: "task-workbench", text: "Task Workbench" } },
      });
      assert.equal(staticAssertText.statusCode, 200, staticAssertText.body);
      assert.equal(JSON.parse(staticAssertText.body).result.ok, true);

      const staticTimer = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.timer_advance", args: { ms: 25 } },
      });
      assert.equal(staticTimer.statusCode, 200, staticTimer.body);
      assert.equal(JSON.parse(staticTimer.body).result.advancedMs, 25);

      const accessibilitySnapshot = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.accessibility_snapshot", args: { appId: "task-workbench" } },
      });
      assert.equal(accessibilitySnapshot.statusCode, 200, accessibilitySnapshot.body);
      assert.equal(JSON.parse(accessibilitySnapshot.body).result.title, "Task Workbench");
      assert.equal(
        JSON.parse(accessibilitySnapshot.body).result.controls.some((control) => control.testId === "add-task-button"),
        true,
      );

      const accessibilityAudit = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.run_accessibility_audit", args: { appId: "task-workbench" } },
      });
      assert.equal(accessibilityAudit.statusCode, 200, accessibilityAudit.body);
      assert.equal(JSON.parse(accessibilityAudit.body).result.status, "pass");
      assert.equal(
        JSON.parse(accessibilityAudit.body).result.checks.some((check) => check.id === "no_unlabeled_controls" && check.status === "pass"),
        true,
      );

      const accessibilityAssert = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.assert_accessibility", args: { appId: "task-workbench", rule: "no_unlabeled_controls" } },
      });
      assert.equal(accessibilityAssert.statusCode, 200, accessibilityAssert.body);
      assert.equal(JSON.parse(accessibilityAssert.body).result.rule, "no_unlabeled_controls");

      const smokeRun = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.run_smoke_tests", args: { appId: "task-workbench" } },
      });
      assert.equal(smokeRun.statusCode, 200, smokeRun.body);
      const smokeRunBody = JSON.parse(smokeRun.body);
      assert.equal(smokeRunBody.result.ok, true);
      assert.equal(smokeRunBody.result.status, "passed");
      assert.equal(smokeRunBody.result.runner, "static");
      assert.equal(smokeRunBody.result.appId, "task-workbench");
      assert.equal(smokeRunBody.result.failures.length, 0);

      const smokeRuns = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_test_runs", args: { appId: "task-workbench" } },
      });
      assert.equal(smokeRuns.statusCode, 200, smokeRuns.body);
      assert.equal(
        JSON.parse(smokeRuns.body).result.rows.some((row) => row.micro_test_id === "smoke:task-workbench" && row.status === "passed"),
        true,
      );

      const microSession = await requestControl(ready.port, "/control/sessions", {
        method: "POST",
        token,
        body: { appId: "notes-lite", metadata: { smoke: "linux-microtest" } },
      });
      assert.equal(microSession.statusCode, 200, microSession.body);
      const microSessionId = JSON.parse(microSession.body).result.controlSessionId;

      const microRun = await requestControl(ready.port, `/sessions/${encodeURIComponent(microSessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.run_microtest",
          args: { microtestPath: "tests/micro/notes-lite-create-note.microtest.json" },
        },
      });
      assert.equal(microRun.statusCode, 200, microRun.body);
      const microRunBody = JSON.parse(microRun.body);
      assert.equal(microRunBody.result.ok, true);
      assert.equal(microRunBody.result.status, "passed");
      assert.equal(microRunBody.result.runner, "linux-static-microtest");
      assert.equal(microRunBody.result.id, "notes-lite-create-note");
      assert.equal(microRunBody.result.failures.length, 0);

      const microRuns = await requestControl(ready.port, `/sessions/${encodeURIComponent(microSessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_test_runs", args: { appId: "notes-lite" } },
      });
      assert.equal(microRuns.statusCode, 200, microRuns.body);
      assert.equal(
        JSON.parse(microRuns.body).result.rows.some((row) => row.micro_test_id === "notes-lite-create-note" && row.status === "passed"),
        true,
      );

      const platformSession = await requestControl(ready.port, "/control/sessions", {
        method: "POST",
        token,
        body: { metadata: { smoke: "linux-platform-smoke" } },
      });
      assert.equal(platformSession.statusCode, 200, platformSession.body);
      const platformSessionId = JSON.parse(platformSession.body).result.controlSessionId;

      const platformSmoke = await requestControl(ready.port, `/sessions/${encodeURIComponent(platformSessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "platform.run_platform_smoke",
          args: {
            smokePath: "tests/platform-smoke/all-example-apps.platform-smoke.json",
            platform: "linux",
          },
        },
      });
      assert.equal(platformSmoke.statusCode, 200, platformSmoke.body);
      const platformSmokeBody = JSON.parse(platformSmoke.body);
      assert.equal(platformSmokeBody.result.ok, true);
      assert.equal(platformSmokeBody.result.id, "all-example-apps-cross-platform-smoke");
      assert.equal(platformSmokeBody.result.platform, "linux");
      assert.equal(platformSmokeBody.result.totalApps, 5);
      assert.equal(platformSmokeBody.result.apps.every((app) => app.ok), true);
      assert.equal(
        platformSmokeBody.result.apps.every((app) => app.commands.some((command) => command.tool === "runtime.run_smoke_tests" && command.status === "passed")),
        true,
      );

      const platformRuns = await requestControl(ready.port, `/sessions/${encodeURIComponent(platformSessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_test_runs", args: {} },
      });
      assert.equal(platformRuns.statusCode, 200, platformRuns.body);
      assert.equal(
        JSON.parse(platformRuns.body).result.rows.some((row) => row.micro_test_id === "platform-smoke:all-example-apps-cross-platform-smoke:linux" && row.status === "passed"),
        true,
      );

      const callBridge = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "task-workbench",
            method: "storage.set",
            params: {
              key: "task-workbench:linux-dev-control-key",
              value: { source: "linux-dev-control" },
            },
          },
        },
      });
      assert.equal(callBridge.statusCode, 200, callBridge.body);
      const callBridgeBody = JSON.parse(callBridge.body);
      assert.equal(callBridgeBody.ok, true);
      assert.equal(callBridgeBody.result.id, "control_call_bridge");
      assert.equal(callBridgeBody.result.ok, true);
      assert.equal(callBridgeBody.result.result.ok, true);
      assert.equal(Number(callBridgeBody.result.result.bytesWritten) > 0, true);

      const controlStorageGet = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.storage_get",
          args: {
            appId: "task-workbench",
            key: "task-workbench:linux-dev-control-key",
          },
        },
      });
      assert.equal(controlStorageGet.statusCode, 200, controlStorageGet.body);
      assert.equal(JSON.parse(controlStorageGet.body).result.result.value.source, "linux-dev-control");

      const storageFault = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.fault_inject",
          args: {
            appId: "task-workbench",
            method: "storage.get",
            code: "linux_injected_storage",
            message: "Injected Linux storage fault",
            details: { source: "linux-fault-smoke" },
            once: true,
          },
        },
      });
      assert.equal(storageFault.statusCode, 200, storageFault.body);
      const storageFaultBody = JSON.parse(storageFault.body);
      assert.equal(storageFaultBody.result.ok, true);
      assert.match(storageFaultBody.result.faultId, /^fault_/);
      assert.equal(storageFaultBody.result.method, "storage.get");
      assert.equal(storageFaultBody.result.code, "linux_injected_storage");
      assert.equal(storageFaultBody.result.message, "Injected Linux storage fault");
      assert.equal(storageFaultBody.result.details.source, "linux-fault-smoke");
      assert.equal(storageFaultBody.result.once, true);
      const storageFaultId = storageFaultBody.result.faultId;

      const faultedStorageGet = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "task-workbench",
            method: "storage.get",
            params: {
              key: "task-workbench:linux-dev-control-key",
              defaultValue: null,
            },
          },
        },
      });
      assert.equal(faultedStorageGet.statusCode, 200, faultedStorageGet.body);
      const faultedStorageGetBody = JSON.parse(faultedStorageGet.body);
      assert.equal(faultedStorageGetBody.ok, true);
      assert.equal(faultedStorageGetBody.result.ok, false);
      assert.equal(faultedStorageGetBody.result.error.code, "linux_injected_storage");
      assert.equal(faultedStorageGetBody.result.error.message, "Injected Linux storage fault");
      assert.equal(faultedStorageGetBody.result.error.details.source, "linux-fault-smoke");
      assert.equal(faultedStorageGetBody.result.error.details.faultId, storageFaultId);
      assert.equal(faultedStorageGetBody.result.error.details.appId, "task-workbench");
      assert.equal(faultedStorageGetBody.result.error.details.method, "storage.get");

      const recoveredStorageGet = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "task-workbench",
            method: "storage.get",
            params: {
              key: "task-workbench:linux-dev-control-key",
              defaultValue: null,
            },
          },
        },
      });
      assert.equal(recoveredStorageGet.statusCode, 200, recoveredStorageGet.body);
      const recoveredStorageGetBody = JSON.parse(recoveredStorageGet.body);
      assert.equal(recoveredStorageGetBody.result.ok, true);
      assert.equal(recoveredStorageGetBody.result.result.value.source, "linux-dev-control");

      const controlStorageSet = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.storage_set",
          args: {
            appId: "task-workbench",
            key: "task-workbench:linux-direct-storage",
            value: { source: "runtime.storage_set" },
          },
        },
      });
      assert.equal(controlStorageSet.statusCode, 200, controlStorageSet.body);
      assert.equal(JSON.parse(controlStorageSet.body).result.result.ok, true);

      const controlStorageAssert = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.assert_storage",
          args: {
            appId: "task-workbench",
            key: "task-workbench:linux-direct-storage",
            value: { source: "runtime.storage_set" },
          },
        },
      });
      assert.equal(controlStorageAssert.statusCode, 200, controlStorageAssert.body);
      assert.equal(JSON.parse(controlStorageAssert.body).result.ok, true);

      const createdSnapshot = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "platform.create_snapshot",
          args: { appId: "task-workbench", type: "manual", sessionId: runtimeSessionId },
        },
      });
      assert.equal(createdSnapshot.statusCode, 200, createdSnapshot.body);
      const createdSnapshotBody = JSON.parse(createdSnapshot.body);
      assert.match(createdSnapshotBody.result.snapshotId, /^snapshot_/);
      assert.match(createdSnapshotBody.result.contentHash, /^sha256:[a-f0-9]{64}$/);
      assert.equal(createdSnapshotBody.result.appId, "task-workbench");
      assert.equal(createdSnapshotBody.result.storage.some((row) => row.key === "task-workbench:linux-direct-storage"), true);

      const mutatedStorage = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.storage_set",
          args: {
            appId: "task-workbench",
            key: "task-workbench:linux-direct-storage",
            value: { source: "mutated-after-snapshot" },
          },
        },
      });
      assert.equal(mutatedStorage.statusCode, 200, mutatedStorage.body);
      assert.equal(JSON.parse(mutatedStorage.body).result.result.ok, true);

      const restoreWithoutConfirm = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "platform.restore_snapshot",
          args: { snapshotId: createdSnapshotBody.result.snapshotId },
        },
      });
      assert.equal(restoreWithoutConfirm.statusCode, 400, restoreWithoutConfirm.body);
      assert.equal(JSON.parse(restoreWithoutConfirm.body).error.code, "confirmation_required");

      const restoredSnapshot = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "platform.restore_snapshot",
          args: { snapshotId: createdSnapshotBody.result.snapshotId, confirm: true },
        },
      });
      assert.equal(restoredSnapshot.statusCode, 200, restoredSnapshot.body);
      assert.equal(JSON.parse(restoredSnapshot.body).result.ok, true);
      assert.equal(JSON.parse(restoredSnapshot.body).result.restoredStorageKeys >= 2, true);

      const restoredStorageAssert = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.assert_storage",
          args: {
            appId: "task-workbench",
            key: "task-workbench:linux-direct-storage",
            value: { source: "runtime.storage_set" },
          },
        },
      });
      assert.equal(restoredStorageAssert.statusCode, 200, restoredStorageAssert.body);
      assert.equal(JSON.parse(restoredStorageAssert.body).result.ok, true);

      const restoredSnapshotBaseline = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "platform.create_snapshot",
          args: { appId: "task-workbench", type: "manual", sessionId: runtimeSessionId },
        },
      });
      assert.equal(restoredSnapshotBaseline.statusCode, 200, restoredSnapshotBaseline.body);
      const restoredSnapshotBaselineBody = JSON.parse(restoredSnapshotBaseline.body);

      const snapshotCompare = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.compare_snapshot",
          args: {
            leftSnapshotId: createdSnapshotBody.result.snapshotId,
            rightSnapshotId: restoredSnapshotBaselineBody.result.snapshotId,
          },
        },
      });
      assert.equal(snapshotCompare.statusCode, 200, snapshotCompare.body);
      const snapshotCompareBody = JSON.parse(snapshotCompare.body);
      assert.equal(snapshotCompareBody.result.ok, true);
      assert.equal(snapshotCompareBody.result.equal, true);
      assert.equal(snapshotCompareBody.result.leftHash, snapshotCompareBody.result.rightHash);

      const changedAfterCompare = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.storage_set",
          args: {
            appId: "task-workbench",
            key: "task-workbench:linux-direct-storage",
            value: { source: "changed-after-compare" },
          },
        },
      });
      assert.equal(changedAfterCompare.statusCode, 200, changedAfterCompare.body);
      assert.equal(JSON.parse(changedAfterCompare.body).result.result.ok, true);

      const changedSnapshot = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "platform.create_snapshot",
          args: { appId: "task-workbench", type: "manual", sessionId: runtimeSessionId },
        },
      });
      assert.equal(changedSnapshot.statusCode, 200, changedSnapshot.body);
      const changedSnapshotBody = JSON.parse(changedSnapshot.body);

      const snapshotCompareUnequal = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.compare_snapshot",
          args: {
            leftSnapshotId: createdSnapshotBody.result.snapshotId,
            rightSnapshotId: changedSnapshotBody.result.snapshotId,
          },
        },
      });
      assert.equal(snapshotCompareUnequal.statusCode, 200, snapshotCompareUnequal.body);
      const snapshotCompareUnequalBody = JSON.parse(snapshotCompareUnequal.body);
      assert.equal(snapshotCompareUnequalBody.result.ok, false);
      assert.equal(snapshotCompareUnequalBody.result.equal, false);
      assert.notEqual(snapshotCompareUnequalBody.result.leftHash, snapshotCompareUnequalBody.result.rightHash);

      const deniedStoragePrefix = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.storage_set",
          args: {
            appId: "task-workbench",
            key: "notes-lite:wrong-prefix",
            value: { source: "bad-prefix" },
          },
        },
      });
      assert.equal(deniedStoragePrefix.statusCode, 200, deniedStoragePrefix.body);
      assert.equal(JSON.parse(deniedStoragePrefix.body).result.ok, false);
      assert.equal(JSON.parse(deniedStoragePrefix.body).result.error.code, "permission_denied");

      const coreStep = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.core_step",
          args: {
            appId: "task-workbench",
            event: { type: "CreateTask", payload: { title: "Linux control task" } },
          },
        },
      });
      assert.equal(coreStep.statusCode, 200, coreStep.body);
      const coreStepBody = JSON.parse(coreStep.body);
      assert.equal(coreStepBody.ok, true);
      assert.equal(coreStepBody.result.id, "control_core_step");
      assert.equal(coreStepBody.result.ok, true);
      assert.equal(coreStepBody.result.result.ok, true);
      assert.equal(typeof coreStepBody.result.result.stateVersion, "number");
      assert.equal(coreStepBody.result.result.actions.some((action) => action.type === "Toast"), true);
      assert.equal(coreStepBody.result.result.actions.some((action) => action.type === "Log"), true);

      const replayEvents = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.replay_events",
          args: {
            appId: "task-workbench",
            events: [{ type: "CreateTask", payload: { title: "Linux replay task" } }],
          },
        },
      });
      assert.equal(replayEvents.statusCode, 200, replayEvents.body);
      const replayEventsBody = JSON.parse(replayEvents.body);
      assert.equal(replayEventsBody.result.ok, true);
      assert.equal(replayEventsBody.result.appId, "task-workbench");
      assert.equal(replayEventsBody.result.replay[0].index, 0);
      assert.equal(replayEventsBody.result.replay[0].event.payload.title, "Linux replay task");
      assert.equal(replayEventsBody.result.replay[0].result.ok, true);
      assert.equal(replayEventsBody.result.replay[0].result.actions.some((action) => action.type === "Toast"), true);

      const coreSnapshot = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.core_snapshot", args: { appId: "task-workbench" } },
      });
      assert.equal(coreSnapshot.statusCode, 200, coreSnapshot.body);
      const coreSnapshotBody = JSON.parse(coreSnapshot.body);
      assert.equal(coreSnapshotBody.result.appId, "task-workbench");
      assert.equal(Array.isArray(coreSnapshotBody.result.coreEvents), true);
      assert.equal(Array.isArray(coreSnapshotBody.result.coreActions), true);
      assert.equal(coreSnapshotBody.result.coreActions.some((row) => row.action?.type === "Toast"), true);

      const coreActionAssert = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.assert_core_action",
          args: { appId: "task-workbench", type: "Toast", match: { level: "success" } },
        },
      });
      assert.equal(coreActionAssert.statusCode, 200, coreActionAssert.body);
      const coreActionAssertBody = JSON.parse(coreActionAssert.body);
      assert.equal(coreActionAssertBody.result.ok, true);
      assert.equal(coreActionAssertBody.result.actions.some((action) => action.type === "Toast"), true);

      const appLog = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "task-workbench",
            method: "app.log",
            params: {
              level: "info",
              message: "Linux control log probe",
            },
          },
        },
      });
      assert.equal(appLog.statusCode, 200, appLog.body);
      assert.equal(JSON.parse(appLog.body).result.ok, true);

      const toast = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "task-workbench",
            method: "notification.toast",
            params: {
              level: "success",
              message: "Linux toast captured",
            },
          },
        },
      });
      assert.equal(toast.statusCode, 200, toast.body);
      assert.equal(JSON.parse(toast.body).result.ok, true);

      const bridgeCallAssert = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.assert_bridge_call", args: { appId: "task-workbench", method: "storage.set" } },
      });
      assert.equal(bridgeCallAssert.statusCode, 200, bridgeCallAssert.body);
      assert.equal(JSON.parse(bridgeCallAssert.body).result.latest.method, "storage.set");

      const bridgeCalls = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.bridge_calls", args: { appId: "task-workbench" } },
      });
      assert.equal(bridgeCalls.statusCode, 200, bridgeCalls.body);
      const bridgeCallsBody = JSON.parse(bridgeCalls.body);
      assert.equal(bridgeCallsBody.result.bridgeCalls.some((row) => row.method === "storage.set"), true);
      assert.equal(bridgeCallsBody.result.bridgeCalls.some((row) => row.method === "notification.toast"), true);

      const noConsoleErrors = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.assert_no_console_errors", args: { appId: "task-workbench" } },
      });
      assert.equal(noConsoleErrors.statusCode, 200, noConsoleErrors.body);
      assert.equal(JSON.parse(noConsoleErrors.body).result.errors, 0);

      const notificationCapture = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.notification_capture", args: { appId: "task-workbench" } },
      });
      assert.equal(notificationCapture.statusCode, 200, notificationCapture.body);
      assert.equal(
        JSON.parse(notificationCapture.body).result.notifications.some((row) => row.message === "Linux toast captured" && row.level === "success"),
        true,
      );

      const resourceUsage = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.resource_usage", args: { appId: "task-workbench" } },
      });
      assert.equal(resourceUsage.statusCode, 200, resourceUsage.body);
      const resourceUsageBody = JSON.parse(resourceUsage.body);
      assert.equal(resourceUsageBody.result.appId, "task-workbench");
      assert.equal(Number(resourceUsageBody.result.bridgeCalls) >= 3, true);
      assert.equal(Number(resourceUsageBody.result.coreEvents) >= 1, true);
      assert.equal(Number(resourceUsageBody.result.logLinesLastMinute) >= 1, true);

      const eventLog = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.event_log", args: { appId: "task-workbench" } },
      });
      assert.equal(eventLog.statusCode, 200, eventLog.body);
      const eventLogBody = JSON.parse(eventLog.body);
      assert.equal(eventLogBody.result.bridgeCalls.some((row) => row.method === "storage.set"), true);
      assert.equal(eventLogBody.result.bridgeCalls.some((row) => row.method === "app.log"), true);
      assert.equal(eventLogBody.result.coreEvents.length >= 1, true);

      const consoleLogs = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.console_logs", args: { appId: "task-workbench" } },
      });
      assert.equal(consoleLogs.statusCode, 200, consoleLogs.body);
      assert.equal(
        JSON.parse(consoleLogs.body).result.logs.some((row) => row.params?.message === "Linux control log probe"),
        true,
      );

      const apiSession = await requestControl(ready.port, "/control/sessions", {
        method: "POST",
        token,
        body: { appId: "api-dashboard", metadata: { smoke: "linux-network-mock" } },
      });
      assert.equal(apiSession.statusCode, 200, apiSession.body);
      const apiSessionId = JSON.parse(apiSession.body).result.controlSessionId;

      const networkMock = await requestControl(ready.port, `/sessions/${encodeURIComponent(apiSessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.network_mock_set",
          args: {
            appId: "api-dashboard",
            method: "GET",
            urlPattern: "https://api.example.com/status",
            response: {
              status: 200,
              headers: { "content-type": "application/json" },
              bodyText: "{\"ok\":true,\"source\":\"linux-network-mock\"}",
              delayMs: 1,
            },
          },
        },
      });
      assert.equal(networkMock.statusCode, 200, networkMock.body);
      assert.match(JSON.parse(networkMock.body).result.mockId, /^netmock-/);

      const mockedNetwork = await requestControl(ready.port, `/sessions/${encodeURIComponent(apiSessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "api-dashboard",
            method: "network.request",
            params: { url: "https://api.example.com/status", method: "GET", headers: {} },
          },
        },
      });
      assert.equal(mockedNetwork.statusCode, 200, mockedNetwork.body);
      assert.equal(JSON.parse(mockedNetwork.body).result.result.bodyText.includes("linux-network-mock"), true);

      const resetNetworkMocks = await requestControl(ready.port, `/sessions/${encodeURIComponent(apiSessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.network_mock_reset", args: { appId: "api-dashboard" } },
      });
      assert.equal(resetNetworkMocks.statusCode, 200, resetNetworkMocks.body);
      assert.equal(Number(JSON.parse(resetNetworkMocks.body).result.cleared) >= 1, true);

      const fileSession = await requestControl(ready.port, "/control/sessions", {
        method: "POST",
        token,
        body: { appId: "file-transformer", metadata: { smoke: "linux-dialog-mock" } },
      });
      assert.equal(fileSession.statusCode, 200, fileSession.body);
      const fileSessionId = JSON.parse(fileSession.body).result.controlSessionId;

      const dialogMock = await requestControl(ready.port, `/sessions/${encodeURIComponent(fileSessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.dialog_mock_set",
          args: {
            appId: "file-transformer",
            method: "dialog.openFile",
            response: {
              files: [{ name: "linux-mock.txt", mime: "text/plain", size: 5, text: "hello" }],
              cancelled: false,
            },
          },
        },
      });
      assert.equal(dialogMock.statusCode, 200, dialogMock.body);
      assert.match(JSON.parse(dialogMock.body).result.mockId, /^dialogmock-/);

      const mockedDialog = await requestControl(ready.port, `/sessions/${encodeURIComponent(fileSessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "file-transformer",
            method: "dialog.openFile",
            params: { accept: ["text/plain"] },
          },
        },
      });
      assert.equal(mockedDialog.statusCode, 200, mockedDialog.body);
      assert.equal(JSON.parse(mockedDialog.body).result.result.files[0].name, "linux-mock.txt");

      const missingResourceAppId = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.resource_usage", args: {} },
      });
      assert.equal(missingResourceAppId.statusCode, 400);
      assert.equal(JSON.parse(missingResourceAppId.body).error.code, "invalid_request");

      const dbSnapshot = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.snapshot", args: {} },
      });
      assert.equal(dbSnapshot.statusCode, 200, dbSnapshot.body);
      const dbSnapshotBody = JSON.parse(dbSnapshot.body);
      assert.equal(Array.isArray(dbSnapshotBody.result.apps), true);
      assert.equal(Array.isArray(dbSnapshotBody.result.app_storage), true);
      assert.equal(Array.isArray(dbSnapshotBody.result.bridge_calls), true);

      const dbStorage = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_app_storage", args: { appId: "task-workbench" } },
      });
      assert.equal(dbStorage.statusCode, 200, dbStorage.body);
      assert.equal(
        JSON.parse(dbStorage.body).result.rows.some((row) => row.key === "task-workbench:linux-dev-control-key"),
        true,
      );

      const missingDbAppId = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_app_storage", args: {} },
      });
      assert.equal(missingDbAppId.statusCode, 400);
      assert.equal(JSON.parse(missingDbAppId.body).error.code, "invalid_request");

      const unsafeDbTool = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_sql", args: { sql: "SELECT * FROM apps" } },
      });
      assert.equal(unsafeDbTool.statusCode, 400);
      assert.equal(JSON.parse(unsafeDbTool.body).error.code, "unsupported_tool");

      for (const tool of ["db.query_app_versions", "db.query_bridge_calls", "db.query_core_events", "db.query_test_runs"]) {
        const response = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
          method: "POST",
          token,
          body: { tool, args: { appId: "task-workbench" } },
        });
        assert.equal(response.statusCode, 200, response.body);
        assert.equal(Array.isArray(JSON.parse(response.body).result.rows), true);
      }

      const debugBundle = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.export_debug_bundle", args: {} },
      });
      assert.equal(debugBundle.statusCode, 200, debugBundle.body);
      const debugBundleBody = JSON.parse(debugBundle.body);
      assert.equal(debugBundleBody.result.type, "debug-bundle");
      assert.equal(debugBundleBody.result.source.platform, "linux");
      assert.equal(debugBundleBody.result.source.target, "linux-native");
      assert.match(debugBundleBody.result.contentHash, /^sha256:[a-f0-9]{64}$/);
      assert.equal(Array.isArray(debugBundleBody.result.apps), true);
      assert.equal(Array.isArray(debugBundleBody.result.appVersions), true);
      assert.equal(Array.isArray(debugBundleBody.result.appFiles), true);
      assert.equal(Array.isArray(debugBundleBody.result.appPermissions), true);
      assert.equal(Array.isArray(debugBundleBody.result.appStorage), true);
      assert.equal(debugBundleBody.result.runtimeCapabilities.platform, "linux");
      assert.equal(debugBundleBody.result.debug.bridgeCalls.some((row) => row.method === "storage.set"), true);
      assert.equal(debugBundleBody.result.debug.coreEvents.length >= 1, true);
      assert.equal(debugBundleBody.result.debug.controlCommands.some((row) => row.tool === "db.snapshot"), true);

      const backup = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.export_backup", args: {} },
      });
      assert.equal(backup.statusCode, 200, backup.body);
      const backupBody = JSON.parse(backup.body);
      assert.equal(backupBody.result.type, "backup");
      assert.equal(backupBody.result.source.platform, "linux");
      assert.equal(backupBody.result.source.target, "linux-native");
      assert.match(backupBody.result.contentHash, /^sha256:[a-f0-9]{64}$/);
      assert.equal(Array.isArray(backupBody.result.apps), true);
      assert.equal(Array.isArray(backupBody.result.appVersions), true);
      assert.equal(Array.isArray(backupBody.result.appFiles), true);
      assert.equal(Array.isArray(backupBody.result.appPermissions), true);
      assert.equal(Array.isArray(backupBody.result.appStorage), true);
      assert.equal(Object.keys(backupBody.result.debug).length, 0);
      assert.equal(
        backupBody.result.appStorage.some((row) => row.key === "task-workbench:linux-dev-control-key"),
        true,
      );

      const importBackup = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.import_backup", args: { backup: backupBody.result } },
      });
      assert.equal(importBackup.statusCode, 200, importBackup.body);
      const importBackupBody = JSON.parse(importBackup.body);
      assert.equal(importBackupBody.result.ok, true);
      assert.equal(Number(importBackupBody.result.appStorage) >= 1, true);

      const missingAppId = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.call_bridge", args: { method: "storage.set", params: {} } },
      });
      assert.equal(missingAppId.statusCode, 400);
      assert.equal(JSON.parse(missingAppId.body).error.code, "invalid_request");

      const missingEvent = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.core_step", args: { appId: "task-workbench" } },
      });
      assert.equal(missingEvent.statusCode, 400);
      assert.equal(JSON.parse(missingEvent.body).error.code, "invalid_request");

      for (const completedSessionId of [apiSessionId, fileSessionId]) {
        const completed = await requestControl(ready.port, `/control/sessions/${encodeURIComponent(completedSessionId)}`, {
          method: "DELETE",
          token,
        });
        assert.equal(completed.statusCode, 200, completed.body);
      }

      const dbPath = path.join(xdgDataHome, "Terrane", "platform.sqlite");
      assert.equal(fs.existsSync(dbPath), true, "dev control should create the platform audit database");
      const rejectedCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE http_method = 'GET' AND path = '/health' AND decision = 'rejected' AND error_code = 'control_auth_required';",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.health' AND http_method = 'GET' AND path = '/health' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      assert.equal(rejectedCount, "1");
      assert.equal(acceptedCount, "1");
      const sessionAuditCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE path LIKE '%/sessions%' AND decision = 'accepted';",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedCallBridgeCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.call_bridge' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedCoreStepCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.core_step' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedReplayEventsCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.replay_events' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedCoreSnapshotCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.core_snapshot' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedCoreActionAssertCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.assert_core_action' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedDbSnapshotCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'db.snapshot' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedDbStorageCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'db.query_app_storage' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedDebugBundleCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'db.export_debug_bundle' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedBackupCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'db.export_backup' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedImportBackupCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'db.import_backup' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const debugBundleExportCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          `SELECT COUNT(*) FROM backup_exports WHERE type = 'debug-bundle' AND source_platform = 'linux' AND content_hash = '${debugBundleBody.result.contentHash}';`,
        ],
        { encoding: "utf8" },
      ).trim();
      const backupExportCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          `SELECT COUNT(*) FROM backup_exports WHERE type = 'backup' AND source_platform = 'linux' AND content_hash = '${backupBody.result.contentHash}';`,
        ],
        { encoding: "utf8" },
      ).trim();
      const backupImportCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          `SELECT COUNT(*) FROM backup_exports WHERE type = 'import' AND content_hash = '${backupBody.result.contentHash}' AND imported_at IS NOT NULL;`,
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedCreateSnapshotCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.create_snapshot' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedRestoreSnapshotCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.restore_snapshot' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const rejectedRestoreSnapshotCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.restore_snapshot' AND decision = 'rejected' AND error_code = 'confirmation_required';",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedCompareSnapshotCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.compare_snapshot' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const manualSnapshotCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM runtime_snapshots WHERE app_id = 'task-workbench' AND type = 'manual' AND content_hash LIKE 'sha256:%';",
        ],
        { encoding: "utf8" },
      ).trim();
      const explicitSessionSnapshotCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          `SELECT COUNT(*) FROM runtime_snapshots WHERE snapshot_id = '${createdSnapshotBody.result.snapshotId}' AND session_id = '${runtimeSessionId}';`,
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedResourceUsageCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.resource_usage' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedEventLogCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.event_log' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedConsoleLogsCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.console_logs' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedBridgeCallsCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.bridge_calls' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedAssertBridgeCallCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.assert_bridge_call' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedNoConsoleErrorsCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.assert_no_console_errors' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedNotificationCaptureCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.notification_capture' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedStaticScreenshotCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.screenshot' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedStaticQueryCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.query' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedStaticAssertVisibleCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.assert_visible' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedStaticAssertTextCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.assert_text' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedAccessibilitySnapshotCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.accessibility_snapshot' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedAccessibilityAuditCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.run_accessibility_audit' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedAccessibilityAssertCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.assert_accessibility' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedSmokeRunCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.run_smoke_tests' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedMicroRunCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.run_microtest' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedPlatformSmokeCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.run_platform_smoke' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const persistedSmokeTestRunCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM test_runs WHERE app_id = 'task-workbench' AND micro_test_id = 'smoke:task-workbench' AND status = 'passed';",
        ],
        { encoding: "utf8" },
      ).trim();
      const persistedMicroTestRunCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM test_runs WHERE app_id = 'notes-lite' AND micro_test_id = 'notes-lite-create-note' AND status = 'passed';",
        ],
        { encoding: "utf8" },
      ).trim();
      const persistedPlatformSmokeRunCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM test_runs WHERE app_id IS NULL AND micro_test_id = 'platform-smoke:all-example-apps-cross-platform-smoke:linux' AND status = 'passed';",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedListTargetsCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.list_targets' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedListWebappsCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.list_webapps' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedValidatePackageCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.validate_package' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedPolicyAuditCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.run_policy_audit' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedSignPackageCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.sign_webapp_package' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedInstallPackageCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.install_webapp_package' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedOpenWebappCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'platform.open_webapp' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const installedAppCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          `SELECT COUNT(*) FROM apps WHERE id = 'notes-lite' AND active_install_id = '${installPackageBody.result.installId}' AND status = 'enabled';`,
        ],
        { encoding: "utf8" },
      ).trim();
      const installedAppFileCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          `SELECT COUNT(*) FROM app_files WHERE install_id = '${installPackageBody.result.installId}' AND path IN ('manifest.json', 'index.html', 'styles.css', 'app.js', 'smoke-tests.json');`,
        ],
        { encoding: "utf8" },
      ).trim();
      const installedAppPermissionCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          `SELECT COUNT(*) FROM app_permissions WHERE install_id = '${installPackageBody.result.installId}' AND app_id = 'notes-lite' AND approved = 1;`,
        ],
        { encoding: "utf8" },
      ).trim();
      const installedAppReportCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          `SELECT COUNT(*) FROM app_install_reports WHERE report_id = '${installPackageBody.result.reportId}' AND app_id = 'notes-lite' AND install_id = '${installPackageBody.result.installId}' AND status = 'accepted';`,
        ],
        { encoding: "utf8" },
      ).trim();
      const openedPackageRuntimeSessionCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          `SELECT COUNT(*) FROM runtime_sessions WHERE session_id = '${openInstalledPackageBody.result.sessionId}' AND active_app_id = 'notes-lite' AND active_install_id = '${installPackageBody.result.installId}' AND status = 'running';`,
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedNetworkMockCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.network_mock_set' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedNetworkMockResetCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.network_mock_reset' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedDialogMockCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.dialog_mock_set' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const acceptedFaultInjectCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.fault_inject' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const consumedFaultCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          `SELECT COUNT(*) FROM fault_injections WHERE fault_id = '${storageFaultId}' AND method = 'storage.get' AND code = 'linux_injected_storage' AND enabled = 0;`,
        ],
        { encoding: "utf8" },
      ).trim();
      const faultedBridgeCallCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          `SELECT COUNT(*) FROM bridge_calls WHERE app_id = 'task-workbench' AND method = 'storage.get' AND error_json LIKE '%linux_injected_storage%' AND error_json LIKE '%${storageFaultId}%';`,
        ],
        { encoding: "utf8" },
      ).trim();
      const bridgeCallCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM bridge_calls WHERE app_id = 'task-workbench' AND method = 'storage.set' AND error_json IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const coreBridgeCallCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM bridge_calls WHERE app_id = 'task-workbench' AND method = 'core.step' AND error_json IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      const coreEventCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM core_events WHERE app_id = 'task-workbench' AND event_json LIKE '%CreateTask%';",
        ],
        { encoding: "utf8" },
      ).trim();
      const coreActionCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM core_actions WHERE app_id = 'task-workbench';",
        ],
        { encoding: "utf8" },
      ).trim();
      const mockedNetworkBridgeCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM bridge_calls WHERE app_id = 'api-dashboard' AND method = 'network.request' AND result_json LIKE '%linux-network-mock%';",
        ],
        { encoding: "utf8" },
      ).trim();
      const mockedDialogBridgeCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM bridge_calls WHERE app_id = 'file-transformer' AND method = 'dialog.openFile' AND result_json LIKE '%linux-mock.txt%';",
        ],
        { encoding: "utf8" },
      ).trim();
      assert.equal(Number(sessionAuditCount) >= 8, true);
      assert.equal(Number(acceptedCallBridgeCount) >= 4, true);
      assert.equal(Number(acceptedCoreStepCount) >= 1, true);
      assert.equal(Number(acceptedReplayEventsCount) >= 1, true);
      assert.equal(Number(acceptedCoreSnapshotCount) >= 1, true);
      assert.equal(Number(acceptedCoreActionAssertCount) >= 1, true);
      assert.equal(Number(acceptedDbSnapshotCount) >= 1, true);
      assert.equal(Number(acceptedDbStorageCount) >= 1, true);
      assert.equal(Number(acceptedDebugBundleCount) >= 1, true);
      assert.equal(Number(acceptedBackupCount) >= 1, true);
      assert.equal(Number(acceptedImportBackupCount) >= 1, true);
      assert.equal(Number(debugBundleExportCount), 1);
      assert.equal(Number(backupExportCount), 1);
      assert.equal(Number(backupImportCount), 1);
      assert.equal(Number(acceptedCreateSnapshotCount) >= 3, true);
      assert.equal(Number(acceptedRestoreSnapshotCount) >= 1, true);
      assert.equal(Number(rejectedRestoreSnapshotCount) >= 1, true);
      assert.equal(Number(acceptedCompareSnapshotCount) >= 2, true);
      assert.equal(Number(manualSnapshotCount) >= 3, true);
      assert.equal(Number(explicitSessionSnapshotCount), 1);
      assert.equal(Number(acceptedResourceUsageCount) >= 1, true);
      assert.equal(Number(acceptedEventLogCount) >= 1, true);
      assert.equal(Number(acceptedConsoleLogsCount) >= 1, true);
      assert.equal(Number(acceptedBridgeCallsCount) >= 1, true);
      assert.equal(Number(acceptedAssertBridgeCallCount) >= 1, true);
      assert.equal(Number(acceptedNoConsoleErrorsCount) >= 1, true);
      assert.equal(Number(acceptedNotificationCaptureCount) >= 1, true);
      assert.equal(Number(acceptedStaticScreenshotCount) >= 1, true);
      assert.equal(Number(acceptedStaticQueryCount) >= 1, true);
      assert.equal(Number(acceptedStaticAssertVisibleCount) >= 1, true);
      assert.equal(Number(acceptedStaticAssertTextCount) >= 1, true);
      assert.equal(Number(acceptedAccessibilitySnapshotCount) >= 1, true);
      assert.equal(Number(acceptedAccessibilityAuditCount) >= 1, true);
      assert.equal(Number(acceptedAccessibilityAssertCount) >= 1, true);
      assert.equal(Number(acceptedSmokeRunCount) >= 1, true);
      assert.equal(Number(acceptedMicroRunCount) >= 1, true);
      assert.equal(Number(acceptedPlatformSmokeCount) >= 1, true);
      assert.equal(Number(persistedSmokeTestRunCount) >= 1, true);
      assert.equal(Number(persistedMicroTestRunCount) >= 1, true);
      assert.equal(Number(persistedPlatformSmokeRunCount) >= 1, true);
      assert.equal(Number(acceptedListTargetsCount) >= 1, true);
      assert.equal(Number(acceptedListWebappsCount) >= 2, true);
      assert.equal(Number(acceptedValidatePackageCount) >= 1, true);
      assert.equal(Number(acceptedPolicyAuditCount) >= 1, true);
      assert.equal(Number(acceptedSignPackageCount) >= 1, true);
      assert.equal(Number(acceptedInstallPackageCount) >= 1, true);
      assert.equal(Number(acceptedOpenWebappCount) >= 1, true);
      assert.equal(Number(installedAppCount), 1);
      assert.equal(Number(installedAppFileCount) >= 5, true);
      assert.equal(Number(installedAppPermissionCount) >= 4, true);
      assert.equal(Number(installedAppReportCount), 1);
      assert.equal(Number(openedPackageRuntimeSessionCount), 1);
      assert.equal(Number(acceptedNetworkMockCount) >= 1, true);
      assert.equal(Number(acceptedNetworkMockResetCount) >= 1, true);
      assert.equal(Number(acceptedDialogMockCount) >= 1, true);
      assert.equal(Number(acceptedFaultInjectCount) >= 1, true);
      assert.equal(Number(consumedFaultCount), 1);
      assert.equal(Number(faultedBridgeCallCount), 1);
      assert.equal(Number(bridgeCallCount) >= 1, true);
      assert.equal(Number(coreBridgeCallCount) >= 1, true);
      assert.equal(Number(coreEventCount) >= 1, true);
      assert.equal(Number(coreActionCount) >= 2, true);
      assert.equal(Number(mockedNetworkBridgeCount) >= 1, true);
      assert.equal(Number(mockedDialogBridgeCount) >= 1, true);

      const packageSessionEnded = await requestControl(ready.port, `/control/sessions/${encodeURIComponent(packageSessionId)}`, {
        method: "DELETE",
        token,
      });
      assert.equal(packageSessionEnded.statusCode, 200, packageSessionEnded.body);

      const clearLogs = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.clear_logs", args: { appId: "task-workbench" } },
      });
      assert.equal(clearLogs.statusCode, 200, clearLogs.body);
      assert.equal(Number(JSON.parse(clearLogs.body).result.bridgeCallsCleared) >= 1, true);
      assert.equal(Number(JSON.parse(clearLogs.body).result.coreEventsCleared) >= 1, true);
      assert.equal(Number(JSON.parse(clearLogs.body).result.coreActionsCleared) >= 1, true);

      const bridgeCallsAfterClear = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.bridge_calls", args: { appId: "task-workbench" } },
      });
      assert.equal(bridgeCallsAfterClear.statusCode, 200, bridgeCallsAfterClear.body);
      assert.equal(JSON.parse(bridgeCallsAfterClear.body).result.bridgeCalls.length, 0);

      const notificationsAfterClear = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.notification_capture", args: { appId: "task-workbench" } },
      });
      assert.equal(notificationsAfterClear.statusCode, 200, notificationsAfterClear.body);
      assert.equal(JSON.parse(notificationsAfterClear.body).result.notifications.length, 0);

      const acceptedClearLogsCount = execFileSync(
        "sqlite3",
        [
          dbPath,
          "SELECT COUNT(*) FROM control_commands WHERE tool = 'runtime.clear_logs' AND decision = 'accepted' AND error_code IS NULL;",
        ],
        { encoding: "utf8" },
      ).trim();
      assert.equal(Number(acceptedClearLogsCount) >= 1, true);

      const ended = await requestControl(ready.port, `/control/sessions/${encodeURIComponent(sessionId)}`, {
        method: "DELETE",
        token,
      });
      assert.equal(ended.statusCode, 200, ended.body);
      const endedBody = JSON.parse(ended.body);
      assert.equal(endedBody.ok, true);
      assert.equal(endedBody.result.controlSessionId, sessionId);
      assert.equal(endedBody.result.status, "ended");

      const resetSession = await requestControl(ready.port, "/control/sessions", {
        method: "POST",
        token,
        body: { appId: "task-workbench", metadata: { smoke: "linux-storage-reset" } },
      });
      assert.equal(resetSession.statusCode, 200, resetSession.body);
      const resetSessionId = JSON.parse(resetSession.body).result.controlSessionId;

      const storageResetWithoutConfirm = await requestControl(ready.port, `/sessions/${encodeURIComponent(resetSessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.storage_reset", args: { appId: "task-workbench" } },
      });
      assert.equal(storageResetWithoutConfirm.statusCode, 400);
      assert.equal(JSON.parse(storageResetWithoutConfirm.body).error.code, "confirmation_required");

      const storageReset = await requestControl(ready.port, `/sessions/${encodeURIComponent(resetSessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.storage_reset", args: { appId: "task-workbench", confirm: true } },
      });
      assert.equal(storageReset.statusCode, 200, storageReset.body);
      assert.equal(JSON.parse(storageReset.body).result.ok, true);
      assert.equal(Number(JSON.parse(storageReset.body).result.clearedStorageKeys) >= 2, true);
      assert.equal(Number(JSON.parse(storageReset.body).result.storageRowsDeleted) >= 2, true);

      const storageSetForPlatformReset = await requestControl(ready.port, `/sessions/${encodeURIComponent(resetSessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.storage_set",
          args: {
            appId: "task-workbench",
            key: "task-workbench:linux-platform-reset",
            value: { source: "platform.reset_webapp" },
          },
        },
      });
      assert.equal(storageSetForPlatformReset.statusCode, 200, storageSetForPlatformReset.body);

      const platformResetWithoutConfirm = await requestControl(ready.port, `/sessions/${encodeURIComponent(resetSessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.reset_webapp", args: { appId: "task-workbench" } },
      });
      assert.equal(platformResetWithoutConfirm.statusCode, 400);
      assert.equal(JSON.parse(platformResetWithoutConfirm.body).error.code, "confirmation_required");

      const platformReset = await requestControl(ready.port, `/sessions/${encodeURIComponent(resetSessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.reset_webapp", args: { appId: "task-workbench", confirm: true } },
      });
      assert.equal(platformReset.statusCode, 200, platformReset.body);
      assert.equal(JSON.parse(platformReset.body).result.ok, true);
      assert.equal(Number(JSON.parse(platformReset.body).result.clearedStorageKeys) >= 1, true);
      assert.equal(Number(JSON.parse(platformReset.body).result.clearedBridgeCalls) >= 1, true);

      const storageAfterReset = await requestControl(ready.port, `/sessions/${encodeURIComponent(resetSessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_app_storage", args: { appId: "task-workbench" } },
      });
      assert.equal(storageAfterReset.statusCode, 200, storageAfterReset.body);
      assert.equal(JSON.parse(storageAfterReset.body).result.rows.length, 0);

      const resetSessionEnded = await requestControl(ready.port, `/control/sessions/${encodeURIComponent(resetSessionId)}`, {
        method: "DELETE",
        token,
      });
      assert.equal(resetSessionEnded.statusCode, 200, resetSessionEnded.body);
    } finally {
      if (child) await stopChild(child);
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Linux packaged native artifact launches from executable-relative resources",
  {
    skip: linuxPackagedNativeSmokeSkipReason(),
    timeout: 240_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-linux-packaged-smoke-"));
    try {
      const outDir = path.join(scratch, "artifacts");
      const result = packageReleaseArtifacts({ outDir, buildNativeLinux: true });
      const nativeArtifact = result.artifacts.find((artifact) => artifact.id === "native-linux-linux-x86_64");
      assert.notEqual(nativeArtifact, undefined, "release manifest should include the Linux native host artifact");

      const appDir = path.join(outDir, "native-apps", "linux", "linux-x86_64", "TerraneHost");
      const binaryPath = path.join(appDir, "terrane-host");
      const packagedCorePath = path.join(appDir, "libforge_ffi.so");
      for (const relativePath of [
        "terrane-host",
        "libforge_ffi.so",
        "resources/runtime/index.html",
        "resources/runtime/runtime.js",
        "resources/webapps/examples/notes-lite/manifest.json",
        "resources/webapps/examples/task-workbench/app.js",
        "resources/db/sqlite/001_initial.sql",
      ]) {
        assert.equal(fs.existsSync(path.join(appDir, relativePath)), true, `${relativePath} should be packaged`);
      }
      assert.notEqual(fs.statSync(binaryPath).mode & 0o111, 0);
      assert.notEqual(fs.statSync(packagedCorePath).mode & 0o111, 0);

      runPackagedArtifactSmoke({ binaryPath, scratch, appDir });
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

function runOptionalSmoke({ binaryPath, scratch, forgeFfiSo }) {
  if (process.env.TERRANE_LINUX_SMOKE_LAUNCH !== "1") return;
  const storageKey = `notes-lite:linux-smoke-${process.pid}-${Date.now()}`;
  const storageValue = `linux-smoke-${process.pid}-${Date.now()}`;
  const baseEnv = {
    ...process.env,
    TERRANE_FORGE_FFI_SO: forgeFfiSo,
    TERRANE_LINUX_SMOKE_EXIT_AFTER: "1",
    XDG_DATA_HOME: path.join(scratch, "xdg-data"),
  };

  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_RUNTIME_LOADED", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "runtime-load",
  });
  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_STORAGE_SET_OK", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "storage-set",
    TERRANE_LINUX_SMOKE_STORAGE_KEY: storageKey,
    TERRANE_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_STORAGE_GET_OK", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "storage-get",
    TERRANE_LINUX_SMOKE_STORAGE_KEY: storageKey,
    TERRANE_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_CORE_STEP_OK", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "core-step",
  });
  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_FIXED_BRIDGE_SURFACE_OK", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "fixed-bridge-surface",
    TERRANE_LINUX_SMOKE_STORAGE_KEY: storageKey,
    TERRANE_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_BRIDGE_STORAGE_SET_OK", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "bridge-storage-set",
    TERRANE_LINUX_SMOKE_STORAGE_KEY: storageKey,
    TERRANE_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_BRIDGE_STORAGE_GET_OK", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "bridge-storage-get",
    TERRANE_LINUX_SMOKE_STORAGE_KEY: storageKey,
    TERRANE_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_BRIDGE_CORE_STEP_OK", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "bridge-core-step",
  });
  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_RUNTIME_APP_STORAGE_GET_OK", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "runtime-app-storage-get",
  });
}

function runPackagedArtifactSmoke({ binaryPath, scratch, appDir }) {
  const storageKey = `notes-lite:linux-packaged-smoke-${process.pid}-${Date.now()}`;
  const storageValue = `linux-packaged-smoke-${process.pid}-${Date.now()}`;
  const outsideRepoCwd = path.join(scratch, "outside-repo-cwd");
  fs.mkdirSync(outsideRepoCwd, { recursive: true });
  const { TERRANE_FORGE_FFI_SO: _ignoredForgeFfiSo, ...smokeEnv } = process.env;
  const baseEnv = {
    ...smokeEnv,
    TERRANE_LINUX_SMOKE_EXIT_AFTER: "1",
    XDG_DATA_HOME: path.join(scratch, "packaged-xdg-data"),
  };

  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_RUNTIME_LOADED", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "runtime-load",
  }, { cwd: outsideRepoCwd });
  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_BRIDGE_STORAGE_SET_OK", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "bridge-storage-set",
    TERRANE_LINUX_SMOKE_STORAGE_KEY: storageKey,
    TERRANE_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  }, { cwd: outsideRepoCwd });
  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_BRIDGE_STORAGE_GET_OK", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "bridge-storage-get",
    TERRANE_LINUX_SMOKE_STORAGE_KEY: storageKey,
    TERRANE_LINUX_SMOKE_STORAGE_VALUE: storageValue,
  }, { cwd: outsideRepoCwd });
  runSmoke(binaryPath, "TERRANE_LINUX_SMOKE_BRIDGE_CORE_STEP_OK", {
    ...baseEnv,
    TERRANE_LINUX_SMOKE: "bridge-core-step",
  }, { cwd: outsideRepoCwd });

  const dbPath = path.join(baseEnv.XDG_DATA_HOME, "Terrane", "platform.sqlite");
  assert.equal(fs.existsSync(dbPath), true, "packaged smoke should persist the platform database");
  assert.equal(appDir.includes(repoRoot), false, "packaged artifact should live outside the repo root");
}

function launchHost(binaryPath, hostArgs, env) {
  let args = [...hostArgs];
  let command = binaryPath;
  if (!process.env.DISPLAY && !process.env.WAYLAND_DISPLAY) {
    assert.equal(commandExists("xvfb-run"), true, "xvfb-run is required for headless Linux smoke");
    command = "xvfb-run";
    args = ["-a", binaryPath, ...hostArgs];
  }
  if (commandWorks("dbus-run-session", ["--version"])) {
    args = ["--", command, ...args];
    command = "dbus-run-session";
  }
  return spawn(command, args, {
    cwd: repoRoot,
    detached: true,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function waitForControlReady(child) {
  return new Promise((resolve, reject) => {
    let settled = false;
    let output = "";
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        reject(new Error(`Timed out waiting for Linux dev control readiness\n${output}`));
      }
    }, 30_000);

    function collect(chunk) {
      output += chunk.toString("utf8");
      const match = output.match(/TERRANE_LINUX_CONTROL_READY port=(\d+) token_path=([^\s]+)/);
      if (!match || settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({ port: Number(match[1]), tokenPath: match[2], output });
    }

    child.stdout.on("data", collect);
    child.stderr.on("data", collect);
    child.once("error", (error) => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        reject(error);
      }
    });
    child.once("exit", (code, signal) => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        reject(new Error(`Linux host exited before dev control was ready code=${code} signal=${signal}\n${output}`));
      }
    });
  });
}

function requestControlHealth(port, token = null) {
  return requestControl(port, "/health", { token });
}

function requestControl(port, pathName, { method = "GET", token = null, body = null } = {}) {
  return new Promise((resolve, reject) => {
    const headers = token ? { "X-Platform-Control-Token": token } : {};
    let bodyText = null;
    if (body !== null) {
      bodyText = JSON.stringify(body);
      headers["content-type"] = "application/json";
      headers["content-length"] = Buffer.byteLength(bodyText);
    }
    const req = http.request(
      {
        hostname: "127.0.0.1",
        port,
        path: pathName,
        method,
        headers,
        timeout: 10_000,
      },
      (res) => {
        let body = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => {
          body += chunk;
        });
        res.on("end", () => resolve({ statusCode: res.statusCode, body }));
      },
    );
    req.on("error", reject);
    req.on("timeout", () => {
      req.destroy(new Error(`Timed out waiting for Linux dev control ${method} ${pathName}`));
    });
    req.end(bodyText);
  });
}

async function stopChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  killChildProcessGroup(child, "SIGTERM");
  await new Promise((resolve) => {
    const timer = setTimeout(() => {
      if (child.exitCode === null && child.signalCode === null) {
        killChildProcessGroup(child, "SIGKILL");
      }
      resolve();
    }, 5_000);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

function killChildProcessGroup(child, signal) {
  try {
    process.kill(-child.pid, signal);
  } catch {
    child.kill(signal);
  }
}

function runSmoke(binaryPath, marker, env, { cwd = repoRoot } = {}) {
  let args = [];
  let command = binaryPath;
  if (!process.env.DISPLAY && !process.env.WAYLAND_DISPLAY) {
    assert.equal(commandExists("xvfb-run"), true, "xvfb-run is required for headless Linux smoke");
    command = "xvfb-run";
    args.push("-a", binaryPath);
  }
  if (commandWorks("dbus-run-session", ["--version"])) {
    args = ["--", command, ...args];
    command = "dbus-run-session";
  }

  const result = spawnSync(command, args, { env, cwd, encoding: "utf8", timeout: 30_000 });
  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
  assert.equal(output.includes("TERRANE_LINUX_SMOKE_FAILED"), false, output);
  if (output.includes(marker)) return;
  assert.fail(`Timed out waiting for ${marker}\n${output}`);
}
