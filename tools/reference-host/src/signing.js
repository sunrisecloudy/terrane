import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { canonicalJson, sha256 } from "./util.js";
import { PlatformError } from "./errors.js";

const SIGNATURE_PREFIX = "native-ai-webapp/sig/v1";

export function createPlatformKeypair() {
  const { publicKey, privateKey } = crypto.generateKeyPairSync("ed25519");
  return keypairFromKeys({ publicKey, privateKey });
}

export function defaultPlatformKeyFile({ env = process.env, homeDir = os.homedir() } = {}) {
  return path.join(env.XDG_CACHE_HOME || path.join(homeDir, ".cache"), "native-ai-webapp", "platform.key");
}

export function loadOrCreatePlatformKeypair({ keyFile = defaultPlatformKeyFile() } = {}) {
  if (keyFile === false || keyFile === null) {
    return createPlatformKeypair();
  }

  const loaded = loadPlatformKeypair(keyFile);
  if (loaded) {
    return loaded;
  }

  const keypair = createPlatformKeypair();
  try {
    persistPlatformKeypair(keypair, keyFile);
  } catch {
    // Keep constrained dev sandboxes usable if the default cache path is not writable.
  }
  return keypair;
}

export function publicKeyDescriptor(keypair) {
  const publicDer = keypair.publicKey.export({ type: "spki", format: "der" });
  return {
    algorithm: "ed25519",
    keyId: keypair.keyId,
    format: "spki-der",
    publicKey: publicDer.toString("base64"),
  };
}

function loadPlatformKeypair(keyFile) {
  if (!fs.existsSync(keyFile)) {
    return null;
  }
  try {
    const record = JSON.parse(fs.readFileSync(keyFile, "utf8"));
    if (record.algorithm !== "ed25519" || typeof record.privateKeyPkcs8 !== "string") {
      return null;
    }
    const privateKey = crypto.createPrivateKey({
      key: Buffer.from(record.privateKeyPkcs8, "base64"),
      format: "der",
      type: "pkcs8",
    });
    const publicKey = record.publicKeySpki
      ? crypto.createPublicKey({
          key: Buffer.from(record.publicKeySpki, "base64"),
          format: "der",
          type: "spki",
        })
      : crypto.createPublicKey(privateKey);
    return keypairFromKeys({ publicKey, privateKey });
  } catch {
    return null;
  }
}

function persistPlatformKeypair(keypair, keyFile) {
  fs.mkdirSync(path.dirname(keyFile), { recursive: true, mode: 0o700 });
  const publicDer = keypair.publicKey.export({ type: "spki", format: "der" });
  const privateDer = keypair.privateKey.export({ type: "pkcs8", format: "der" });
  const record = {
    algorithm: "ed25519",
    publicKeySpki: publicDer.toString("base64"),
    privateKeyPkcs8: privateDer.toString("base64"),
    createdAt: new Date().toISOString(),
  };
  fs.writeFileSync(keyFile, `${JSON.stringify(record, null, 2)}\n`, { mode: 0o600 });
  fs.chmodSync(keyFile, 0o600);
}

function keypairFromKeys({ publicKey, privateKey }) {
  const publicDer = publicKey.export({ type: "spki", format: "der" });
  return {
    publicKey,
    privateKey,
    keyId: `platform-host:reference-host:${sha256(publicDer).slice(0, 16)}`,
  };
}

export function canonicalPackageHashes(manifest, files) {
  const normalizedManifest = canonicalJson(manifest);
  const fileHashes = {};
  const fileRecords = [...files.entries()]
    .map(([filePath, content]) => {
      const normalized = normalizeText(content);
      const hash = prefixedSha256(normalized);
      fileHashes[filePath] = hash;
      return { path: filePath, hash };
    })
    .sort((a, b) => a.path.localeCompare(b.path));

  const contentBytes = fileRecords.map((record) => `${record.path}\n${record.hash}\n`).join("");
  const permissions = [...(manifest.permissions ?? [])].sort();
  const policy = {
    capabilities: manifest.capabilities ?? {},
    networkPolicy: manifest.networkPolicy ?? {},
    resourceBudget: manifest.resourceBudget ?? {},
  };

  return {
    manifestHash: prefixedSha256(normalizedManifest),
    contentHash: prefixedSha256(contentBytes),
    permissionsHash: prefixedSha256(canonicalJson(permissions)),
    policyHash: prefixedSha256(canonicalJson(policy)),
    fileHashes,
    fileRecords,
  };
}

export function signPackage({ manifest, files, trustLevel = "developer", keypair, signedAt = new Date().toISOString() }) {
  const hashes = canonicalPackageHashes(manifest, files);
  const signatureBase = {
    appId: manifest.id,
    appVersion: manifest.version,
    dataVersion: manifest.dataVersion,
    runtimeVersion: manifest.runtimeVersion,
    trustLevel,
    algorithm: "ed25519",
    keyId: keypair.keyId,
    manifestHash: hashes.manifestHash,
    contentHash: hashes.contentHash,
    permissionsHash: hashes.permissionsHash,
    policyHash: hashes.policyHash,
    signedAt,
    signedBy: "reference-host",
  };
  const payload = signaturePayload(signatureBase);
  const signature = crypto.sign(null, Buffer.from(payload, "utf8"), keypair.privateKey).toString("base64");

  return {
    signature: {
      ...signatureBase,
      signature,
    },
    hashes,
    contentHashesDocument: {
      algorithm: "sha256",
      manifestHash: hashes.manifestHash,
      contentHash: hashes.contentHash,
      files: hashes.fileRecords,
    },
  };
}

export function verifyInstalledPackage({ manifest, files, signature, publicKey }) {
  if (!signature) {
    throw new PlatformError("signature_missing", "Installed package has no signature");
  }

  if (signature.algorithm === "none-dev") {
    throw new PlatformError("signature_untrusted", "none-dev signatures are not accepted by the verified mount path");
  }

  if (signature.algorithm !== "ed25519") {
    throw new PlatformError("signature_untrusted", `Unsupported signature algorithm: ${signature.algorithm}`);
  }

  const hashes = canonicalPackageHashes(manifest, files);
  if (hashes.manifestHash !== signature.manifestHash) {
    throw new PlatformError("manifest_tampered", "Stored manifest hash does not match the signature", {
      expected: signature.manifestHash,
      actual: hashes.manifestHash,
    });
  }
  if (hashes.contentHash !== signature.contentHash) {
    throw new PlatformError("content_tampered", "Stored app file content does not match the signature", {
      expected: signature.contentHash,
      actual: hashes.contentHash,
    });
  }
  if (hashes.permissionsHash !== signature.permissionsHash) {
    throw new PlatformError("permission_tampered", "Stored permissions hash does not match the signature", {
      expected: signature.permissionsHash,
      actual: hashes.permissionsHash,
    });
  }
  if (hashes.policyHash !== signature.policyHash) {
    throw new PlatformError("policy_tampered", "Stored policy hash does not match the signature", {
      expected: signature.policyHash,
      actual: hashes.policyHash,
    });
  }

  const ok = crypto.verify(
    null,
    Buffer.from(signaturePayload(signature), "utf8"),
    publicKey,
    Buffer.from(signature.signature, "base64"),
  );
  if (!ok) {
    throw new PlatformError("signature_invalid", "Ed25519 signature verification failed");
  }

  return { ok: true, hashes };
}

export function signaturePayload(signature) {
  return [
    SIGNATURE_PREFIX,
    signature.appId,
    signature.appVersion,
    String(signature.dataVersion),
    signature.runtimeVersion,
    signature.trustLevel,
    signature.keyId,
    signature.manifestHash,
    signature.contentHash,
    signature.permissionsHash,
    signature.policyHash,
    signature.signedAt,
  ].join("\n");
}

function prefixedSha256(value) {
  return `sha256:${sha256(value)}`;
}

function normalizeText(value) {
  const text = String(value);
  return text.replace(/^\uFEFF/, "").replace(/\r\n?/g, "\n");
}
