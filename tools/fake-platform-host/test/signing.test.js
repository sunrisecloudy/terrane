import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { PlatformError } from "../src/errors.js";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";
import { readPackage } from "../src/package-validator.js";
import {
  createPlatformKeypair,
  loadOrCreatePlatformKeypair,
  publicKeyDescriptor,
  signPackage,
  verifyInstalledPackage,
} from "../src/signing.js";

test("signPackage emits spec-shaped Ed25519 signature and verifies", () => {
  const keypair = createPlatformKeypair();
  const pkg = readPackage(path.join(examplesDir, "notes-lite"));
  const signed = signPackage({ manifest: pkg.manifest, files: pkg.files, trustLevel: "developer", keypair });

  assert.equal(signed.signature.algorithm, "ed25519");
  assert.equal(signed.signature.appId, "notes-lite");
  assert.equal(signed.signature.appVersion, "0.1.0");
  assert.match(signed.signature.keyId, /^platform-host:fake-host:[a-f0-9]{16}$/);
  assert.match(signed.signature.manifestHash, /^sha256:[a-f0-9]{64}$/);
  assert.match(signed.signature.contentHash, /^sha256:[a-f0-9]{64}$/);
  assert.match(signed.signature.permissionsHash, /^sha256:[a-f0-9]{64}$/);
  assert.match(signed.signature.policyHash, /^sha256:[a-f0-9]{64}$/);

  const verified = verifyInstalledPackage({
    manifest: pkg.manifest,
    files: pkg.files,
    signature: signed.signature,
    publicKey: keypair.publicKey,
  });
  assert.equal(verified.ok, true);
});

test("fake-host platform signing key persists when a key file is configured", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-keypair-"));
  const keyFile = path.join(tempDir, "platform.key");
  try {
    const first = loadOrCreatePlatformKeypair({ keyFile });
    const second = loadOrCreatePlatformKeypair({ keyFile });

    assert.equal(second.keyId, first.keyId);
    assert.match(first.keyId, /^platform-host:fake-host:[a-f0-9]{16}$/);
    assert.equal(fs.statSync(keyFile).mode & 0o777, 0o600);

    const descriptor = publicKeyDescriptor(second);
    assert.equal(descriptor.keyId, second.keyId);
    assert.equal(descriptor.algorithm, "ed25519");
    assert.equal(descriptor.format, "spki-der");
    assert.match(descriptor.publicKey, /^[A-Za-z0-9+/]+=*$/);
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("verified mount path rejects tampered installed files", async () => {
  const host = new FakePlatformHost();
  try {
    const install = host.installPackage(path.join(examplesDir, "notes-lite"));
    const opened = await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });
    assert.equal(opened.appId, "notes-lite");

    host.database.run(
      "UPDATE app_files SET content_text = content_text || ? WHERE install_id = ? AND path = 'app.js'",
      "\n// tampered after signing",
      install.installId,
    );

    await assert.rejects(
      () => host.runControlCommand("platform.open_webapp", { appId: "notes-lite" }),
      (error) => error instanceof PlatformError && error.code === "content_tampered",
    );
  } finally {
    host.close();
  }
});

test("control tools expose signing and policy audit", async () => {
  const host = new FakePlatformHost();
  try {
    const health = host.health();
    assert.equal(health.signingPublicKey.keyId, host.keypair.keyId);
    assert.equal(health.signingPublicKey.algorithm, "ed25519");

    const packagePath = path.join(examplesDir, "notes-lite");
    const signed = await host.runControlCommand("platform.sign_webapp_package", { packagePath });
    assert.equal(signed.signature.appId, "notes-lite");
    assert.equal(signed.signature.algorithm, "ed25519");

    const audit = await host.runControlCommand("platform.run_policy_audit", { packagePath });
    assert.equal(audit.ok, true);
    assert.equal(audit.manifest.id, "notes-lite");
  } finally {
    host.close();
  }
});
