import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { buildPublicContract, writePublicContract } from "../../../tools/export-public-contract.mjs";
import { verifyPublicContract } from "../../../tools/verify-public-contract.mjs";
import { repoRoot } from "../../../tools/reference-host/src/paths.js";

function createSchemaValidator(schema) {
  return function validate(value, currentSchema = schema, valuePath = "$") {
    const errors = [];
    if (currentSchema.$ref) {
      return validate(value, resolveRef(schema, currentSchema.$ref), valuePath);
    }
    if (currentSchema.const !== undefined && JSON.stringify(value) !== JSON.stringify(currentSchema.const)) {
      errors.push(`${valuePath} must equal ${JSON.stringify(currentSchema.const)}`);
    }
    if (currentSchema.type && !typeMatches(value, currentSchema.type)) {
      errors.push(`${valuePath} must be ${Array.isArray(currentSchema.type) ? currentSchema.type.join(" or ") : currentSchema.type}`);
    }
    if (typeof value === "string") {
      if (Number.isInteger(currentSchema.minLength) && value.length < currentSchema.minLength) errors.push(`${valuePath} is shorter than ${currentSchema.minLength}`);
      if (currentSchema.pattern && !new RegExp(currentSchema.pattern).test(value)) errors.push(`${valuePath} does not match ${currentSchema.pattern}`);
    }
    if (Array.isArray(value)) {
      if (Number.isInteger(currentSchema.minItems) && value.length < currentSchema.minItems) errors.push(`${valuePath} must contain at least ${currentSchema.minItems} items`);
      if (currentSchema.uniqueItems && new Set(value.map((item) => JSON.stringify(item))).size !== value.length) errors.push(`${valuePath} must contain unique items`);
      if (currentSchema.items) value.forEach((item, index) => errors.push(...validate(item, currentSchema.items, `${valuePath}[${index}]`)));
    }
    if (isPlainObject(value)) {
      const properties = currentSchema.properties ?? {};
      for (const required of currentSchema.required ?? []) {
        if (!Object.hasOwn(value, required)) errors.push(`${valuePath}.${required} is required`);
      }
      for (const [key, item] of Object.entries(value)) {
        if (properties[key]) errors.push(...validate(item, properties[key], `${valuePath}.${key}`));
        else if (currentSchema.additionalProperties === false) errors.push(`${valuePath}.${key} is not allowed`);
      }
    }
    return errors;
  };
}

function resolveRef(rootSchema, ref) {
  return ref
    .slice(2)
    .split("/")
    .reduce((value, segment) => value?.[segment.replace(/~1/g, "/").replace(/~0/g, "~")], rootSchema);
}

function typeMatches(value, type) {
  const types = Array.isArray(type) ? type : [type];
  return types.some((candidate) => {
    if (candidate === "array") return Array.isArray(value);
    if (candidate === "object") return isPlainObject(value);
    if (candidate === "integer") return Number.isInteger(value);
    if (candidate === "number") return typeof value === "number";
    if (candidate === "null") return value === null;
    return typeof value === candidate;
  });
}

function isPlainObject(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

test("public contract export is deterministic and covers downstream boundaries", () => {
  const first = buildPublicContract();
  const second = buildPublicContract();
  assert.deepEqual(second, first);

  assert.equal(first.schemaVersion, 1);
  assert.equal(first.contractId, "terrane-public-contract");
  assert.equal(first.contractVersion, "0.1.0");
  assert.equal(first.platformBaseline, "forge-v1-m0b");
  assert.equal(first.runtimeVersion, "0.1.0");
  assert.equal(first.license, "MIT");
  assert.equal(first.provenance.sourceRepository, "terrane");
  assert.match(first.provenance.sourceCommit, /^[a-f0-9]{40}$/);
  assert.equal(first.provenance.generatedFrom, "tools/export-public-contract.mjs");
  assert.equal(first.provenance.releaseManifestArtifactId, "public-contract");

  assert.equal(first.generatedAppBoundary.api.includes("CoreCommand"), true);
  assert.equal(first.generatedAppBoundary.api.includes("ctx.db"), true);
  assert.equal(first.generatedAppBoundary.forbidden.includes("direct SaaS API calls"), true);
  assert.equal(first.generatedAppBoundary.forbidden.includes("SaaS access tokens"), true);
  assert.equal(first.bridge.methods.includes("runtime.run"), true);
  assert.equal(first.bridge.methods.includes("sync.export"), true);
  assert.equal(first.bridge.methods.includes("ui.dispatch_event"), true);
  assert.deepEqual(first.bridge.events.map((event) => event.eventName), ["ui.patch", "record.changed", "sync.packet"]);
  assert.equal(first.sync.syncableRecordKinds.includes("collection_record"), true);
  assert.equal(first.sync.nonSyncableRecordKinds.includes("operator_local_token"), true);
  assert.equal(first.sync.nonSyncableRecordKinds.includes("private_signing_key"), true);
  assert.equal(first.sync.recordMappings.some((mapping) => mapping.kind === "collection_record" && mapping.localTables.includes("records")), true);
  assert.equal(first.sync.nonSyncableRecordMappings.some((mapping) => mapping.kind === "secret_material" && mapping.localTables.includes("secrets")), true);
  assert.equal(first.conformance.sourceCheckoutRequired, true);
  assert.equal(first.conformance.commands.includes("node --no-warnings tools/verify-public-contract.mjs --contract artifacts/public-contract.json --root ."), true);

  const docs = new Set(first.files.docs.map((file) => file.path));
  const contracts = new Set(first.files.contracts.map((file) => file.path));
  const fixtures = new Set(first.files.fixtures.map((file) => file.path));
  const tools = new Set(first.files.tools.map((file) => file.path));

  assert.equal(docs.has("docs/00_V1_PIVOT.md"), true);
  assert.equal(docs.has("prd-merged/00-master-prd.md"), true);
  assert.equal(docs.has("forge/spec/commands.md"), true);
  assert.equal(docs.has("docs/35_PUBLIC_CONTRACT_EXPORT.md"), true);
  assert.equal(contracts.has("forge/contracts/public-contract.schema.json"), true);
  assert.equal(contracts.has("forge/std/forge-std.d.ts"), true);
  assert.equal(contracts.has("forge/crates/ffi/include/forge_ffi.h"), true);
  assert.equal(fixtures.has("forge/fixtures/e2e/note_taker/manifest.json"), true);
  assert.equal(fixtures.has("forge/fixtures/sync/already_in_sync_noop.json"), true);
  assert.equal(fixtures.has("forge/examples/notes-lite/manifest.json"), true);
  assert.equal(tools.has("forge/crates/ffi/tests/ffi.rs"), true);
  assert.equal(tools.has("tools/export-public-contract.mjs"), true);
  assert.equal(tools.has("tools/verify-public-contract.mjs"), true);

  for (const group of Object.values(first.files)) {
    const paths = group.map((file) => file.path);
    assert.deepEqual(paths, [...paths].sort());
    for (const file of group) {
      assert.match(file.sha256, /^[a-f0-9]{64}$/);
      assert.equal(file.bytes > 0, true);
    }
  }
});

test("public contract matches its schema", () => {
  const schema = JSON.parse(fs.readFileSync(path.join(repoRoot, "forge", "contracts", "public-contract.schema.json"), "utf8"));
  const validate = createSchemaValidator(schema);
  assert.deepEqual(validate(buildPublicContract()), []);
});

test("public contract writer emits stable JSON", () => {
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-public-contract-"));
  try {
    const firstPath = path.join(outDir, "first.json");
    const secondPath = path.join(outDir, "second.json");
    writePublicContract({ outPath: firstPath });
    writePublicContract({ outPath: secondPath });

    const first = fs.readFileSync(firstPath);
    const second = fs.readFileSync(secondPath);
    assert.deepEqual(second, first);
    assert.match(crypto.createHash("sha256").update(first).digest("hex"), /^[a-f0-9]{64}$/);

    const parsed = JSON.parse(first.toString("utf8"));
    assert.equal(parsed.$schema, "https://example.local/forge/contracts/public-contract.schema.json");
  } finally {
    fs.rmSync(outDir, { recursive: true, force: true });
  }
});

test("public contract verifier checks schema, provenance, and file hashes", () => {
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-public-contract-verify-"));
  try {
    const contractPath = path.join(outDir, "public-contract.json");
    writePublicContract({ outPath: contractPath });

    const result = verifyPublicContract({ contractPath, root: repoRoot });
    assert.equal(result.ok, true);
    assert.equal(result.errors.length, 0);
    assert.equal(result.filesChecked > 0, true);
  } finally {
    fs.rmSync(outDir, { recursive: true, force: true });
  }
});

test("public contract verifier rejects changed file hashes", () => {
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-public-contract-verify-"));
  try {
    const contractPath = path.join(outDir, "public-contract.json");
    const { contract } = writePublicContract({ outPath: contractPath });
    contract.files.docs[0].sha256 = "0".repeat(64);
    fs.writeFileSync(contractPath, `${JSON.stringify(contract, null, 2)}\n`);

    const result = verifyPublicContract({ contractPath, root: repoRoot });
    assert.equal(result.ok, false);
    assert.equal(result.errors.some((error) => error.includes("sha256")), true);
  } finally {
    fs.rmSync(outDir, { recursive: true, force: true });
  }
});
