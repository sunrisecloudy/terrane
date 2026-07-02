// Browser verification for one installed Terrane app.
// Usage: node grade-ui.mjs BASE_URL APP_ID OUT_DIR [UI_INPUT_TEXT]
//
// Asserts, app-generically (no calendar-specific text checks):
//   1. GET /apps/<id> returns 200 (shell route).
//   2. The shim-injected frame page executes: window.terrane.invoke is a
//      function (the app's HTML/JS did not break script execution).
//   3. Zero uncaught page errors and zero console.error during load.
//   4. One interaction round-trip: type into the first text input, submit,
//      and require BOTH a 200 error-free POST /apps/<id>/invoke AND a changed
//      DOM within 10s — the live version of the positional-args contract.
//
// Optional dependency: puppeteer-core + a system Chrome. When either is
// missing this exits 2 and the grader records ui_check=skipped.

import { createRequire } from "node:module";
import { mkdirSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";

const [baseUrl, appId, outDir, uiInputText = "hello from the eval"] =
  process.argv.slice(2);
if (!baseUrl || !appId || !outDir) {
  console.error("usage: grade-ui.mjs BASE_URL APP_ID OUT_DIR [UI_INPUT_TEXT]");
  process.exit(2);
}
mkdirSync(outDir, { recursive: true });

let browser = null;
const consoleLines = [];
const networkLines = [];
const result = {
  page200: false,
  shim_ok: false,
  console_errors: 0,
  invoke_requests: 0,
  last_invoke_status: null,
  invoke_roundtrip: false,
  dom_changed: false,
  needs_grant: null,
  verdict: "fail",
  reason: "",
};

// Writes artifacts, kills the browser, prints the result, exits — safe to
// call from any point (process.exit would otherwise skip `finally` work).
function finish(verdict, reason) {
  result.verdict = verdict;
  result.reason = reason;
  try {
    writeFileSync(join(outDir, "result.json"), JSON.stringify(result, null, 2));
    writeFileSync(join(outDir, "console.log"), consoleLines.join("\n"));
    writeFileSync(join(outDir, "network.log"), networkLines.join("\n"));
  } catch {
    /* artifact write is best-effort */
  }
  try {
    browser?.process()?.kill();
  } catch {
    /* already gone */
  }
  console.log(JSON.stringify(result));
  process.exit(verdict === "pass" ? 0 : verdict === "skipped" ? 2 : 1);
}

const require = createRequire(import.meta.url);
let puppeteer;
try {
  puppeteer = require("puppeteer-core");
} catch {
  finish(
    "skipped",
    "puppeteer-core not installed (cd host/mcp/evals/harness && npm install)"
  );
}

const chromeCandidates = [
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
  "/Applications/Chromium.app/Contents/MacOS/Chromium",
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
];
const executablePath = chromeCandidates.find((p) => existsSync(p));
if (!executablePath) {
  finish("skipped", "no Chrome-family browser found");
}

try {
  const shell = await fetch(`${baseUrl}/apps/${appId}`);
  result.page200 = shell.status === 200;
  if (!result.page200) {
    finish("fail", `GET /apps/${appId} returned ${shell.status}`);
  }

  browser = await puppeteer.launch({ executablePath, headless: true });
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 900 });
  page.on("console", (msg) => {
    consoleLines.push(`[${msg.type()}] ${msg.text()}`);
    // Resource-load failures are counted from the network listener instead,
    // where the URL is known (a missing favicon is browser noise, not an
    // app bug).
    if (msg.type() === "error" && !msg.text().includes("Failed to load resource")) {
      result.console_errors += 1;
    }
  });
  page.on("pageerror", (err) => {
    consoleLines.push(`[pageerror] ${err.message}`);
    result.console_errors += 1;
  });
  page.on("response", async (res) => {
    if (res.url().includes("/invoke")) {
      result.invoke_requests += 1;
      result.last_invoke_status = res.status();
      let body = "";
      try {
        body = await res.text();
      } catch {
        /* stream gone */
      }
      networkLines.push(`${res.status()} ${res.url()} ${body}`);
    } else if (res.status() >= 400) {
      networkLines.push(`${res.status()} ${res.url()}`);
      if (!res.url().endsWith("/favicon.ico")) {
        result.console_errors += 1;
      }
    }
  });

  // Trailing slash matters: relative asset refs (style.css, ui.js) resolve
  // against the frame directory only when the URL ends with `/`.
  const frameUrl = `${baseUrl}/apps/${appId}/__terrane/frame/`;
  await page.goto(frameUrl, { waitUntil: "networkidle0", timeout: 20000 });
  result.shim_ok = await page.evaluate(
    () => typeof window.terrane?.invoke === "function"
  );
  await page.screenshot({ path: join(outDir, "page-load.png") });
  if (!result.shim_ok) {
    finish("fail", "window.terrane.invoke is not a function on the frame page");
  }
  if (result.console_errors > 0) {
    finish("fail", `page errors during load: ${consoleLines.join(" | ")}`);
  }

  // Interaction round-trip.
  const before = await page.evaluate(() => ({
    text: document.body.innerText,
    nodes: document.querySelectorAll("*").length,
  }));
  const input = await page.$('input[type="text"], input:not([type]), textarea');
  if (!input) {
    finish("fail", "no text input found for the interaction check");
  }
  await input.click();
  await input.type(uiInputText);

  const invokesBeforeSubmit = result.invoke_requests;
  const invokeResponse = page
    .waitForResponse((res) => res.url().includes(`/apps/${appId}/invoke`), {
      timeout: 10000,
    })
    .catch(() => null);

  // Submit: form submit button, else the nearest button walking up from the
  // input's container (many generated UIs pair a textarea with a sibling
  // button and no <form> — Enter in a textarea only adds a newline).
  const submitted = await page.evaluate(() => {
    const active = document.activeElement;
    const form = active?.closest("form");
    let btn = form?.querySelector('button[type="submit"], button') ?? null;
    if (!btn) {
      let node = active;
      while (node && node !== document.body && !btn) {
        node = node.parentElement;
        btn = node?.querySelector("button") ?? null;
      }
    }
    btn = btn ?? document.querySelector('button[type="submit"]');
    if (btn) {
      btn.click();
      return "click";
    }
    return "none";
  });
  if (submitted === "none") {
    await input.press("Enter");
  }

  const res = await invokeResponse;
  if (res) {
    const body = await res.text().catch(() => "");
    result.invoke_roundtrip = res.status() === 200 && !/"error"\s*:/.test(body);
    if (/"type"\s*:\s*"permission_required"/.test(body)) {
      const ns = body.match(/"missingResources"\s*:\s*\[([^\]]*)\]/);
      result.needs_grant = ns ? ns[1].replaceAll('"', "").trim() : "unknown";
    }
  }

  // DOM change within 10s of submitting.
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline && !result.dom_changed) {
    const now = await page.evaluate(() => ({
      text: document.body.innerText,
      nodes: document.querySelectorAll("*").length,
    }));
    result.dom_changed = now.text !== before.text || now.nodes !== before.nodes;
    if (!result.dom_changed) await new Promise((r) => setTimeout(r, 500));
  }
  await page.screenshot({ path: join(outDir, "after-interaction.png") });

  if (result.needs_grant) {
    finish(
      "needs_grant",
      `invoke returned permission_required: ${result.needs_grant}`
    );
  }
  if (result.invoke_roundtrip && result.dom_changed) {
    finish("pass", "interaction round-trip succeeded and the DOM updated");
  }
  // Explain the failure shape: local-only UI updates vs failed backend calls.
  const invokesAfterSubmit = result.invoke_requests - invokesBeforeSubmit;
  const invokeDiag =
    invokesAfterSubmit === 0
      ? "no /invoke request was made after submit (UI updates locally only or the submit wiring is broken)"
      : `last /invoke status ${result.last_invoke_status} across ${invokesAfterSubmit} post-submit request(s)`;
  finish(
    "fail",
    `invoke_roundtrip=${result.invoke_roundtrip} dom_changed=${result.dom_changed}; ${invokeDiag}`
  );
} catch (err) {
  finish("fail", `exception: ${err?.message ?? err}`);
}
