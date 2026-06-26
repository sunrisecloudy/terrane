import { fileURLToPath } from "node:url";
import path from "node:path";

export const hostRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const repoRoot = path.resolve(hostRoot, "../..");
export const sqliteMigrationsDir = path.join(repoRoot, "db", "sqlite");
export const examplesDir = path.join(repoRoot, "webapps", "examples");
export const runtimeWebDir = path.join(repoRoot, "runtime-web");
export const hostFixturesDir = path.join(hostRoot, "fixtures");

export function resolveInside(root, candidate) {
  const resolved = path.resolve(root, candidate);
  const rel = path.relative(root, resolved);
  if (rel.startsWith("..") || path.isAbsolute(rel)) {
    throw new Error(`Path escapes root: ${candidate}`);
  }
  return resolved;
}
