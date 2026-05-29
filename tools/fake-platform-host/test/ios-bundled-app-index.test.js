import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const exampleIds = ["notes-lite", "task-workbench", "file-transformer", "api-dashboard", "core-replay-lab"];

test("bundled example manifests expose App Store content ratings", () => {
  for (const appId of exampleIds) {
    const manifest = JSON.parse(
      fs.readFileSync(path.join(repoRoot, "webapps", "examples", appId, "manifest.json"), "utf8"),
    );
    assert.equal(manifest.trust?.level, "bundled", appId);
    assert.deepEqual(manifest.contentRating, {
      scheme: "app-store",
      label: "4+",
      minimumAge: 4,
      descriptors: [],
    }, appId);
  }
});

test("iOS host serves a content-rating gated bundled app index", () => {
  const catalog = fs.readFileSync(
    path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "BundledAppCatalog.swift"),
    "utf8",
  );
  const webHost = fs.readFileSync(
    path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "WebHostView.swift"),
    "utf8",
  );
  const bridge = fs.readFileSync(
    path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "WebBridge.swift"),
    "utf8",
  );

  for (const appId of exampleIds) {
    assert.equal(catalog.includes(`"${appId}"`), true, appId);
  }
  assert.match(catalog, /"source": "ios-bundled"/);
  assert.match(catalog, /"contentRating": contentRating/);
  assert.match(catalog, /NATIVE_AI_IOS_MAX_CONTENT_AGE/);
  assert.match(catalog, /--native-ai-max-content-age/);
  assert.match(webHost, /runtime\/app-index\.json/);
  assert.match(webHost, /BundledAppCatalog\.appIndexData\(\)/);
  assert.match(bridge, /BundledAppCatalog\.isAllowed/);
  assert.match(bridge, /reason": "content_rating"/);
});
