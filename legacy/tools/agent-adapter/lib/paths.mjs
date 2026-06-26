import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const moduleDir = path.dirname(fileURLToPath(import.meta.url));

export const repoRoot = path.resolve(moduleDir, "../../..");

export const defaultCatalogPath = path.join(repoRoot, "forge", "data", "commands.json");

export function resolveFromRepo(relativePath) {
  return path.join(repoRoot, relativePath);
}

export function fileExists(filePath) {
  return fs.existsSync(filePath);
}