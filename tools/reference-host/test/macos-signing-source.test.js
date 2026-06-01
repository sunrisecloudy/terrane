import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("macOS dev control signs packages with Ed25519 instead of none-dev", () => {
  const control = fs.readFileSync(
    path.join(repoRoot, "native/macos/Sources/TerraneHostMac/DevControlPlane.swift"),
    "utf8",
  );
  const tests = fs.readFileSync(
    path.join(repoRoot, "native/macos/Tests/TerraneHostMacTests/NativeHostTests.swift"),
    "utf8",
  );

  assert.match(control, /Curve25519\.Signing\.PrivateKey/);
  assert.match(control, /SecItemCopyMatching/);
  assert.match(control, /SecItemAdd/);
  assert.match(control, /kSecAttrAccessibleWhenUnlocked/);
  assert.match(control, /"storage": "keychain"/);
  assert.match(control, /"algorithm": "ed25519"/);
  assert.match(control, /"permissionsHash": hashes\["permissionsHash"\]/);
  assert.match(control, /"policyHash": hashes\["policyHash"\]/);
  assert.match(control, /signaturePayload\(/);
  assert.match(control, /signingKey\.signature/);
  assert.match(control, /verifyActiveInstallForMount/);
  assert.match(control, /isValidSignature/);
  assert.doesNotMatch(control, /"algorithm": "none-dev"/);
  assert.match(tests, /platform\.sign_webapp_package/);
  assert.match(tests, /debugControlPlanePersistsSigningKeyInKeychain/);
  assert.match(tests, /debugControlPlaneRejectsTamperedInstalledPackageBeforeOpen/);
  assert.match(tests, /signature\["algorithm"\] as\? String == "ed25519"/);
  assert.match(tests, /pendingSignature\["algorithm"\] as\? String == "ed25519"/);
});
