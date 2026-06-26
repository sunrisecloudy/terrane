/**
 * Locate and load the REAL committed forge fixtures (not copies) so the
 * conformance suite is driven by the ground-truth corpus. The golden trees live
 * in the sibling forge crate inside the same git worktree.
 */

import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
// renderer-zero/test -> renderer-zero -> <worktree root>
const WORKTREE_ROOT = join(here, "..", "..");

export const GOLDEN_DIR = join(WORKTREE_ROOT, "forge", "crates", "ui", "tests", "golden");
export const A11Y_DIR = join(GOLDEN_DIR, "a11y");
export const UI_EVENTS_DIR = join(WORKTREE_ROOT, "forge", "fixtures", "ui-events");

/** Read + parse a JSON fixture by absolute path. */
export function readJson<T = unknown>(path: string): T {
  return JSON.parse(readFileSync(path, "utf8")) as T;
}

/** List fixture file names in a directory matching `predicate`. */
export function listFixtures(dir: string, predicate: (name: string) => boolean): string[] {
  return readdirSync(dir)
    .filter((n) => n.endsWith(".json") && predicate(n))
    .sort();
}

export { join };
