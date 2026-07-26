#!/usr/bin/env node

import {
  createHash,
  createPrivateKey,
  createPublicKey,
  sign,
} from "node:crypto";
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

function fail(message) {
  process.stderr.write(`package-bootstrap-runtime: ${message}\n`);
  process.exit(1);
}

function argument(name) {
  const index = process.argv.indexOf(name);
  if (index < 0 || !process.argv[index + 1]) {
    fail(`missing ${name}`);
  }
  return process.argv[index + 1];
}

function run(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.status !== 0) {
    fail(
      `${command} ${args.join(" ")} failed\n${result.stderr || result.stdout || ""}`.trim(),
    );
  }
  return result.stdout.trim();
}

const application = resolve(argument("--app"));
const output = resolve(argument("--output"));
const version = argument("--version");
const baseURL = argument("--base-url").replace(/\/+$/, "");
const signingKeyPath = resolve(argument("--signing-key"));
const runtimeBundleName = "Terrane.app";
const artifactName = "TerraneRuntime-arm64.zip";

if (basename(application) !== runtimeBundleName) {
  fail(`runtime app must be named ${runtimeBundleName}`);
}
if (/[\r\n]/.test(version) || !version || version.length > 128) {
  fail("version is invalid");
}
if (!baseURL.startsWith("https://") && !baseURL.startsWith("http://127.0.0.1:")) {
  fail("base URL must use HTTPS (or loopback HTTP for local testing)");
}

run("/usr/bin/codesign", ["--verify", "--deep", "--strict", "--verbose=2", application]);
const executable = run("/usr/bin/plutil", [
  "-extract",
  "CFBundleExecutable",
  "raw",
  join(application, "Contents", "Info.plist"),
]);
const architectures = run("/usr/bin/lipo", [
  "-archs",
  join(application, "Contents", "MacOS", executable),
]);
if (architectures !== "arm64") {
  fail(`runtime must be arm64-only, found: ${architectures}`);
}

await mkdir(output, { recursive: true });
const artifact = join(output, artifactName);
await rm(artifact, { force: true });
run("/usr/bin/ditto", [
  "-c",
  "-k",
  "--sequesterRsrc",
  "--keepParent",
  application,
  artifact,
]);

const artifactBytes = await readFile(artifact);
const artifactSHA256 = createHash("sha256").update(artifactBytes).digest("hex");
const artifactSize = (await stat(artifact)).size;
const artifactURL = `${baseURL}/${artifactName}`;
const format = 1;
const signingPayload = [
  "terrane-bootstrap-manifest-v1",
  String(format),
  version,
  "arm64",
  artifactURL,
  artifactSHA256,
  String(artifactSize),
  runtimeBundleName,
  "",
].join("\n");

const privateKey = createPrivateKey(await readFile(signingKeyPath));
if (privateKey.asymmetricKeyType !== "ed25519") {
  fail("signing key must be Ed25519");
}
const signature = sign(null, Buffer.from(signingPayload, "utf8"), privateKey).toString("base64");
const publicDER = createPublicKey(privateKey).export({ type: "spki", format: "der" });
const publicKeyHex = publicDER.subarray(publicDER.length - 32).toString("hex");
const manifest = {
  format,
  version,
  architecture: "arm64",
  artifactURL,
  artifactSHA256,
  artifactSize,
  runtimeBundleName,
  signature,
};
const manifestPath = join(output, "terrane-bootstrap-manifest.json");
await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, { mode: 0o644 });
await writeFile(
  join(output, "SHA256SUMS"),
  `${artifactSHA256}  ${artifactName}\n`,
  { mode: 0o644 },
);

process.stdout.write(
  [
    `runtime=${artifact}`,
    `manifest=${manifestPath}`,
    `version=${version}`,
    `size=${artifactSize}`,
    `sha256=${artifactSHA256}`,
    `public_key=${publicKeyHex}`,
    "",
  ].join("\n"),
);
