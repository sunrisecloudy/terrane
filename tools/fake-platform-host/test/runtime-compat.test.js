import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { readPackage } from "../src/package-validator.js";
import { examplesDir } from "../src/paths.js";

test("runtimeVersion mismatch quarantines the new install and leaves previous active", async () => {
  const host = new FakePlatformHost({ runtimeVersion: "0.1.0" });
  const incompatiblePackage = fs.mkdtempSync(path.join(os.tmpdir(), "bad-runtime-package-"));
  try {
    const first = host.installPackage(path.join(examplesDir, "notes-lite"));
    fs.cpSync(path.join(examplesDir, "notes-lite"), incompatiblePackage, { recursive: true });
    const manifestPath = path.join(incompatiblePackage, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    manifest.runtimeVersion = "0.2.0";
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

    const failed = host.installPackage(incompatiblePackage);
    assert.equal(failed.status, "quarantined");
    assert.equal(failed.compatibility.ok, false);

    const report = await host.runControlCommand("platform.install_report", { appId: "notes-lite", installId: failed.installId });
    assert.equal(report.status, "failed");
    assert.equal(report.compatibility.ok, false);
    assert.equal(report.compatibility.runtimeVersion, "0.1.0");
    assert.equal(report.compatibility.appRuntimeVersion, "0.2.0");

    assert.equal(host.database.activeInstallId("notes-lite"), first.installId);
    const opened = await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });
    assert.equal(opened.appId, "notes-lite");
  } finally {
    host.close();
  }
});

test("mount gate rejects an active incompatible install unless dev override is set", async () => {
  const strictHost = new FakePlatformHost({ runtimeVersion: "0.1.0" });
  const devHost = new FakePlatformHost({ runtimeVersion: "0.1.0", allowRuntimeMismatch: true });
  const incompatiblePackage = fs.mkdtempSync(path.join(os.tmpdir(), "active-runtime-package-"));
  try {
    fs.cpSync(path.join(examplesDir, "notes-lite"), incompatiblePackage, { recursive: true });
    const manifestPath = path.join(incompatiblePackage, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    manifest.id = "runtime-mismatch-app";
    manifest.name = "Runtime Mismatch App";
    manifest.storagePrefix = "runtime-mismatch-app:";
    manifest.runtimeVersion = "0.2.0";
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

    const pkg = readPackage(incompatiblePackage);
    const strictInstall = strictHost.signPackage(incompatiblePackage);
    strictHost.database.insertInstalledPackage({
      manifest: pkg.manifest,
      files: pkg.files,
      hashes: strictInstall.hashes,
      validation: pkg.validation,
      signature: strictInstall.signature,
      contentHashesDocument: strictInstall.contentHashesDocument,
      activate: true,
    });
    await assert.rejects(
      () => strictHost.runControlCommand("platform.open_webapp", { appId: "runtime-mismatch-app" }),
      /runtimeVersion is not compatible/,
    );

    const devInstall = devHost.signPackage(incompatiblePackage);
    devHost.database.insertInstalledPackage({
      manifest: pkg.manifest,
      files: pkg.files,
      hashes: devInstall.hashes,
      validation: pkg.validation,
      signature: devInstall.signature,
      contentHashesDocument: devInstall.contentHashesDocument,
      activate: true,
    });
    const opened = await devHost.runControlCommand("platform.open_webapp", { appId: "runtime-mismatch-app" });
    assert.equal(opened.appId, "runtime-mismatch-app");
  } finally {
    strictHost.close();
    devHost.close();
  }
});

test("mount gate rejects apps with missing required capabilities", async () => {
  const host = new FakePlatformHost();
  const packageDir = fs.mkdtempSync(path.join(os.tmpdir(), "missing-capability-package-"));
  try {
    fs.cpSync(path.join(examplesDir, "notes-lite"), packageDir, { recursive: true });
    const manifestPath = path.join(packageDir, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    manifest.id = "missing-capability-app";
    manifest.name = "Missing Capability App";
    manifest.storagePrefix = "missing-capability-app:";
    manifest.capabilities.required = [...manifest.capabilities.required, "runtime.darkMode"];
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

    const pkg = readPackage(packageDir);
    const signed = host.signPackage(packageDir);
    host.database.insertInstalledPackage({
      manifest: pkg.manifest,
      files: pkg.files,
      hashes: signed.hashes,
      validation: pkg.validation,
      signature: signed.signature,
      contentHashesDocument: signed.contentHashesDocument,
      activate: true,
    });

    await assert.rejects(
      () => host.runControlCommand("platform.open_webapp", { appId: "missing-capability-app" }),
      /Required runtime capability is unavailable/,
    );
  } finally {
    host.close();
  }
});
