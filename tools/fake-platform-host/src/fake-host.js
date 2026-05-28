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
import { prettyJson } from "./util.js";

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
    const canActivate = smokeTest.ok && compatibility.ok;
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
      activate: canActivate,
      versionStatus: canActivate ? "enabled" : "quarantined",
      reportStatus: canActivate ? "accepted" : "failed",
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
      status: canActivate ? "enabled" : "quarantined",
      smokeTest,
      compatibility,
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

  async dispatchBridge(request, context) {
    return this.bridge.dispatch(request, context);
  }

  async runControlCommand(tool, args = {}) {
    switch (tool) {
      case "platform.health":
        return this.health();
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

function parseSemver(version) {
  const match = String(version ?? "").match(/^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/);
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
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
