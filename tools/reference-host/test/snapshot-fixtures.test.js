import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { ReferenceHost } from "../src/reference-host.js";
import { repoRoot } from "../src/paths.js";

test("checked-in runtime snapshot fixtures compare through reference-host controls", async () => {
  const snapshotsDir = path.join(repoRoot, "tests", "fixtures", "snapshots");
  const fixtureFiles = fs.readdirSync(snapshotsDir).filter((fileName) => fileName.endsWith(".json")).sort();
  assert.deepEqual(fixtureFiles, ["notes-lite-minimal-snapshot.json"]);

  const host = new ReferenceHost();
  try {
    for (const fileName of fixtureFiles) {
      const snapshot = JSON.parse(fs.readFileSync(path.join(snapshotsDir, fileName), "utf8"));
      const same = await host.runControlCommand("runtime.compare_snapshot", {
        left: snapshot,
        right: JSON.parse(JSON.stringify(snapshot)),
      });
      assert.equal(same.ok, true, fileName);
      assert.equal(same.equal, true, fileName);
      assert.match(same.leftHash, /^sha256:[a-f0-9]{64}$/);

      const changed = JSON.parse(JSON.stringify(snapshot));
      changed.resourceUsage.bridgeCalls += 1;
      const different = await host.runControlCommand("runtime.compare_snapshot", {
        left: snapshot,
        right: changed,
      });
      assert.equal(different.ok, false, fileName);
      assert.equal(different.equal, false, fileName);
    }
  } finally {
    host.close();
  }
});
