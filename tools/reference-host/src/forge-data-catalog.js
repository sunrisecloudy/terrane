import fs from "node:fs";
import path from "node:path";
import { repoRoot } from "./paths.js";

const DATA_DIR = path.join(repoRoot, "forge", "data");

function readJson(filename) {
  return JSON.parse(fs.readFileSync(path.join(DATA_DIR, filename), "utf8"));
}

let cached = null;

export function forgeDataCatalog() {
  if (cached) return cached;
  cached = {
    bundledApps: readJson("bundled-apps.json"),
    mimeTypes: readJson("mime-types.json"),
    envVariables: readJson("env-variables.json"),
    controlPlaneConfig: readJson("control-plane-config.json"),
    runtimeConfig: readJson("runtime-config.json"),
    engineRoomTables: readJson("engine-room-tables.json"),
    snapshotTypes: readJson("snapshot-types.json"),
    appStatusEnums: readJson("app-status-enums.json"),
    trustLevels: readJson("trust-levels.json"),
    packageManifest: readJson("package-manifest.json"),
    controlCommands: readJson("control-commands.json"),
    controlResponseSchema: readJson("control-response-schema.json"),
  };
  cached.macosControlTools = new Set(
    cached.controlCommands.filter((entry) => entry.platforms.includes("macos") || entry.platforms.includes("reference-host")).map((entry) => entry.name),
  );
  cached.referenceHostControlTools = new Set(
    cached.controlCommands.filter((entry) => entry.platforms.includes("reference-host")).map((entry) => entry.name),
  );
  return cached;
}

export function isKnownReferenceHostTool(tool) {
  return forgeDataCatalog().referenceHostControlTools.has(tool);
}

export function mimeTypeForExtension(extension) {
  const { mimeTypes } = forgeDataCatalog();
  const key = extension.startsWith(".") ? extension.toLowerCase() : `.${extension.toLowerCase()}`;
  return mimeTypes.extensions[key] ?? mimeTypes.default;
}