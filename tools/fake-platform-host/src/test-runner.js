import fs from "node:fs";
import path from "node:path";
import { BrowserSmokeRunner } from "./browser-smoke-runner.js";
import { PlatformError } from "./errors.js";
import { repoRoot, resolveInside } from "./paths.js";

export class TestRunner {
  constructor({ database, runControlCommand = null, browserSmokeRunner = null, smokeRunner = process.env.NATIVE_AI_SMOKE_RUNNER ?? "static" }) {
    this.database = database;
    this.runControlCommand = runControlCommand;
    this.browserSmokeRunner = browserSmokeRunner;
    this.smokeRunner = smokeRunner;
  }

  async runSmokeTests(appId, { runner = this.smokeRunner } = {}) {
    const installed = this.database.activeInstallPackage(appId);
    if (!installed) {
      throw new PlatformError("app_not_installed", `App is not installed: ${appId}`, { appId });
    }
    const smokeText = installed.files.get("smoke-tests.json");
    if (!smokeText) {
      throw new PlatformError("smoke_tests_missing", `App has no smoke-tests.json: ${appId}`, { appId });
    }

    const tests = JSON.parse(smokeText);
    const normalizedRunner = normalizeSmokeRunner(runner);
    let result;
    if (normalizedRunner === "browser" || normalizedRunner === "auto") {
      try {
        const browserRunner = this.browserSmokeRunner ?? new BrowserSmokeRunner({ database: this.database });
        result = await browserRunner.run({
          appId,
          installId: installed.installId,
          tests,
          files: installed.files,
        });
      } catch (error) {
        if (normalizedRunner === "browser") {
          throw error;
        }
        const fallback = this.evaluateSmokeTests({
          appId,
          tests,
          html: installed.files.get("index.html") ?? "",
          appJs: installed.files.get("app.js") ?? "",
        });
        result = {
          ...fallback,
          runner: "static",
          fallback: {
            from: "browser",
            code: error.code ?? "browser_smoke_unavailable",
            message: error.message,
          },
        };
      }
    } else {
      result = {
        ...this.evaluateSmokeTests({
          appId,
          tests,
          html: installed.files.get("index.html") ?? "",
          appJs: installed.files.get("app.js") ?? "",
        }),
        runner: "static",
      };
    }

    return this.database.recordTestRun({
      microTestId: `smoke:${appId}`,
      name: `${appId} bundled smoke tests`,
      appId,
      spec: tests,
      status: result.ok ? "passed" : "failed",
      result,
    });
  }

  async runMicroTest({ spec, microtestPath } = {}) {
    const microtest = spec ?? readMicrotest(microtestPath);
    const appId = microtest.targetApps?.[0];
    if (!appId) {
      throw new PlatformError("invalid_microtest", "Micro-test must target at least one app");
    }

    const setup = await this.executeControlPhase("setup", microtest.setup ?? [], appId);
    const installed = this.database.activeInstallPackage(appId);
    if (!installed) {
      throw new PlatformError("app_not_installed", `App is not installed: ${appId}`, { appId });
    }

    const result = this.evaluateMicroTest({
      microtest,
      html: installed.files.get("index.html") ?? "",
      appJs: installed.files.get("app.js") ?? "",
      commandResults: setup.commands,
    });
    const teardown = await this.executeControlPhase("teardown", microtest.teardown ?? [], appId);

    return this.database.recordTestRun({
      microTestId: microtest.id,
      name: microtest.id,
      appId,
      spec: microtest,
      status: setup.ok && result.ok && teardown.ok ? "passed" : "failed",
      result: {
        ...result,
        setup,
        teardown,
      },
    });
  }

  async runPlatformSmokeTest({ spec, smokePath, platform = "fake-host" } = {}) {
    const smoke = spec ?? readPlatformSmoke(smokePath);
    const appResults = [];
    const failures = [];

    for (const appId of smoke.apps ?? []) {
      const appCommands = [];
      const install = await this.executeControlStep({
        phase: "setup",
        index: 0,
        step: { tool: "platform.install_webapp_package", args: { path: path.join("webapps", "examples", appId) } },
        appId,
      });
      appCommands.push(install);
      if (install.status === "failed") failures.push({ appId, ...install });

      for (const [index, step] of (smoke.stepsPerApp ?? []).entries()) {
        const expanded = expandControlStep(step, { appId, platform });
        const execution = await this.executeControlStep({ phase: "steps", index, step: expanded, appId });
        appCommands.push(execution);
        if (execution.status === "failed") failures.push({ appId, ...execution });
      }

      appResults.push({
        appId,
        ok: appCommands.every((command) => command.status !== "failed"),
        commands: appCommands,
      });
    }

    const result = {
      ok: failures.length === 0,
      id: smoke.id,
      platform,
      totalApps: smoke.apps?.length ?? 0,
      failures,
      apps: appResults,
    };

    return this.database.recordTestRun({
      microTestId: `platform-smoke:${smoke.id}:${platform}`,
      name: `${smoke.id} (${platform})`,
      appId: null,
      spec: smoke,
      status: result.ok ? "passed" : "failed",
      result,
    });
  }

  evaluateSmokeTests({ appId, tests, html, appJs }) {
    const failures = [];
    const dynamicText = new Set();
    for (const test of tests) {
      for (const step of test.steps ?? []) {
        if (step.selector && !selectorExists(html, step.selector)) {
          failures.push({ test: test.name, code: "selector.not_found", selector: step.selector });
        }
        if ((step.type === "fill" || step.type === "select") && typeof step.value === "string") {
          dynamicText.add(step.value);
        }
      }
      for (const method of test.expected?.bridgeCallsInclude ?? []) {
        if (!bridgeMethodReferenced(appJs, method)) {
          failures.push({ test: test.name, code: "bridge.call_missing", method });
        }
      }
      if (test.expected?.textIncludes && !textCanAppear(html, dynamicText, test.expected.textIncludes)) {
        failures.push({ test: test.name, code: "text.not_found", text: test.expected.textIncludes });
      }
    }
    return {
      ok: failures.length === 0,
      appId,
      total: tests.length,
      assertions: tests.reduce((count, test) => count + (test.steps?.length ?? 0) + Object.keys(test.expected ?? {}).length, 0),
      failures,
    };
  }

  evaluateMicroTest({ microtest, html, appJs, commandResults = [] }) {
    const failures = [];
    const dynamicText = new Set(dynamicTextFromCommands(commandResults));
    for (const step of [...(microtest.setup ?? []), ...(microtest.steps ?? []), ...(microtest.teardown ?? [])]) {
      const args = step.args ?? {};
      if (["runtime.click", "runtime.type", "runtime.set_value", "runtime.assert_visible"].includes(step.tool) && args.testId && !testIdExists(html, args.testId)) {
        failures.push({ tool: step.tool, code: "selector.not_found", testId: args.testId });
      }
      if ((step.tool === "runtime.type" && args.text) || (step.tool === "runtime.set_value" && args.value)) {
        dynamicText.add(String(args.text ?? args.value));
      }
      if (["runtime.assert_text", "runtime.assert_visible"].includes(step.tool) && args.text) {
        if (!textCanAppear(html, dynamicText, args.text)) {
          failures.push({ tool: step.tool, code: "text.not_found", text: args.text });
        }
      }
      if (step.tool === "runtime.assert_bridge_call" && args.method && !bridgeMethodReferenced(appJs, args.method)) {
        failures.push({ tool: step.tool, code: "bridge.call_missing", method: args.method });
      }
      if (step.tool === "runtime.replay_events" && !bridgeMethodReferenced(appJs, "core.step")) {
        failures.push({ tool: step.tool, code: "core.action_missing", method: "core.step" });
      }
    }
    return {
      ok: failures.length === 0,
      id: microtest.id,
      totalSteps: (microtest.setup?.length ?? 0) + (microtest.steps?.length ?? 0) + (microtest.teardown?.length ?? 0),
      failures,
    };
  }

  async executeControlPhase(phase, steps, appId) {
    const commands = [];
    const failures = [];
    for (const [index, step] of steps.entries()) {
      const execution = await this.executeControlStep({ phase, index, step, appId });
      commands.push(execution);
      if (execution.status === "failed") {
        failures.push(execution);
      }
    }
    return {
      ok: failures.length === 0,
      commands,
      failures,
    };
  }

  async executeControlStep({ phase, index, step, appId }) {
    const normalized = normalizeControlStep(step, appId);
    if (normalized.mode === "noop" || !this.runControlCommand) {
      return {
        phase,
        index,
        tool: step.tool,
        status: "skipped",
        reason: normalized.reason ?? "not executable by static fake-host runner",
      };
    }
    try {
      const result = await this.runControlCommand(normalized.tool, normalized.args);
      return {
        phase,
        index,
        tool: step.tool,
        status: "passed",
        args: summarizeControlArgs(normalized.args),
        result: summarizeCommandResult(result),
      };
    } catch (error) {
      return {
        phase,
        index,
        tool: step.tool,
        status: "failed",
        error: {
          code: error.code ?? "platform.unavailable",
          message: error.message,
          details: error.details ?? {},
        },
      };
    }
  }
}

function normalizeSmokeRunner(runner) {
  if (runner === "browser" || runner === "auto" || runner === "static") {
    return runner;
  }
  return "static";
}

function readMicrotest(microtestPath) {
  if (!microtestPath) {
    throw new PlatformError("invalid_request", "runtime.run_microtest requires spec or microtestPath");
  }
  const resolved = resolveInside(repoRoot, microtestPath);
  return JSON.parse(fs.readFileSync(resolved, "utf8"));
}

function readPlatformSmoke(smokePath) {
  if (!smokePath) {
    throw new PlatformError("invalid_request", "platform.run_platform_smoke requires spec or smokePath");
  }
  const resolved = resolveInside(repoRoot, smokePath);
  return JSON.parse(fs.readFileSync(resolved, "utf8"));
}

function selectorExists(html, selector) {
  if (selector.startsWith("#")) {
    return new RegExp(`\\bid=["']${escapeRegExp(selector.slice(1))}["']`).test(html);
  }
  const testId = selector.match(/\[data-testid=["']([^"']+)["']\]/)?.[1];
  if (testId) {
    return testIdExists(html, testId);
  }
  return html.includes(selector);
}

function testIdExists(html, testId) {
  return new RegExp(`\\bdata-testid=["']${escapeRegExp(testId)}["']`).test(html);
}

function bridgeMethodReferenced(appJs, method) {
  return appJs.includes(`'${method}'`) || appJs.includes(`"${method}"`);
}

function textCanAppear(html, dynamicText, text) {
  if (html.includes(text)) return true;
  for (const value of dynamicText) {
    if (String(value).includes(text)) {
      return true;
    }
  }
  return false;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function normalizeControlStep(step, appId) {
  const args = { ...(step.args ?? {}) };
  if (args.path && !args.packagePath) {
    args.packagePath = args.path;
  }

  if (step.tool === "runtime.network_mock_set") {
    args.appId ??= appId;
    args.urlPattern ??= args.match?.url ?? args.match?.urlPattern;
    args.method ??= args.match?.method ?? "GET";
  } else if (step.tool === "runtime.dialog_mock_set") {
    args.appId ??= appId;
    args.dialogType ??= args.method?.replace(/^dialog\./, "");
  } else if (step.tool === "platform.open_webapp" || step.tool === "platform.create_snapshot") {
    args.appId ??= appId;
  } else if (
    [
      "runtime.capabilities",
      "runtime.run_smoke_tests",
      "runtime.resource_usage",
      "runtime.run_accessibility_audit",
      "runtime.accessibility_snapshot",
      "runtime.assert_accessibility",
      "runtime.assert_no_console_errors",
      "runtime.screenshot",
    ].includes(step.tool)
  ) {
    args.appId ??= appId;
  }

  const executable = new Set([
    "platform.validate_package",
    "platform.sign_webapp_package",
    "platform.install_webapp_package",
    "platform.open_webapp",
    "platform.create_snapshot",
    "runtime.capabilities",
    "runtime.run_smoke_tests",
    "runtime.resource_usage",
    "runtime.run_accessibility_audit",
    "runtime.accessibility_snapshot",
    "runtime.assert_accessibility",
    "runtime.assert_no_console_errors",
    "runtime.screenshot",
    "runtime.network_mock_set",
    "runtime.dialog_mock_set",
  ]);
  if (executable.has(step.tool)) {
    return { mode: "execute", tool: step.tool, args };
  }

  const noops = new Set([
    "runtime.wait_for",
    "platform.reset_webapp",
  ]);
  if (noops.has(step.tool)) {
    return { mode: "noop", reason: "not needed for static validation" };
  }

  return { mode: "noop", reason: "UI step validated statically" };
}

function summarizeCommandResult(result) {
  if (!result || typeof result !== "object") {
    return result;
  }
  return {
    ok: result.ok ?? true,
    appId: result.appId,
    installId: result.installId,
    sessionId: result.sessionId,
    keyId: result.keyId,
    status: result.status,
  };
}

function summarizeControlArgs(args) {
  const summary = { ...args };
  if (summary.packagePath) {
    summary.packagePath = path.relative(repoRoot, path.resolve(repoRoot, summary.packagePath));
  }
  if (summary.path) {
    summary.path = path.relative(repoRoot, path.resolve(repoRoot, summary.path));
  }
  return summary;
}

function dynamicTextFromCommands(commands) {
  const values = [];
  for (const command of commands) {
    collectText(command.args, values);
    collectText(command.result, values);
  }
  return values;
}

function expandControlStep(step, values) {
  return {
    ...step,
    args: expandPlaceholders(step.args ?? {}, values),
  };
}

function expandPlaceholders(value, values) {
  if (typeof value === "string") {
    return value.replace(/\$\{([a-zA-Z0-9_]+)\}/g, (match, name) => (name in values ? values[name] : match));
  }
  if (Array.isArray(value)) {
    return value.map((item) => expandPlaceholders(item, values));
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, expandPlaceholders(item, values)]));
  }
  return value;
}

function collectText(value, values) {
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    values.push(String(value));
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) collectText(item, values);
    return;
  }
  if (value && typeof value === "object") {
    for (const item of Object.values(value)) collectText(item, values);
  }
}
