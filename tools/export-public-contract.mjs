#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const CONTRACT_VERSION = "0.1.0";
const PLATFORM_BASELINE = "forge-v1-m0b";
const RUNTIME_VERSION = "0.4.0";

const PUBLIC_DOCS = [
  "docs/00_V1_PIVOT.md",
  "docs/34_LOCAL_FIRST_OSS_SERVER_AND_SAAS_PRD.md",
  "docs/35_PUBLIC_CONTRACT_EXPORT.md",
  "prd-merged/00-master-prd.md",
  "prd-merged/01-core-runtime-prd.md",
  "prd-merged/02-data-layer-prd.md",
  "prd-merged/03-sync-server-prd.md",
  "prd-merged/05-ui-system-prd.md",
  "prd-merged/06-platform-shells-prd.md",
  "prd-merged/07-security-prd.md",
  "prd-merged/09-roadmap-quality-gates-prd.md",
  "prd-merged/DECISIONS.md",
  "forge/docs/applet-authoring-guide.md",
  "forge/docs/architecture-overview.md",
  "forge/docs/cli-reference.md",
  "forge/docs/example-applets.md",
  "forge/docs/http-bridge-reference.md",
  "forge/docs/public-api-reference.md",
  "forge/spec/commands.md",
  "forge/spec/conformance-vector-format.md",
  "forge/spec/cross-engine-conformance.md",
  "forge/spec/errors.md",
  "forge/spec/policy-gates.md",
  "forge/spec/quotas.md",
  "forge/spec/sync-protocol.md",
  "forge/spec/ui-catalog.md",
  "forge/spec/workspace-export-format.md",
];

const PUBLIC_CONTRACT_FILES = [
  "forge/contracts/public-contract.schema.json",
  "forge/crates/ffi/include/forge_ffi.h",
  "forge/std/README.md",
  "forge/std/forge-std.d.ts",
  "forge/std/ui-catalog.d.ts",
  "forge/docs/public-api/index.html",
  "forge/docs/public-api/styles.css",
  "forge/docs/public-api/app.js",
];

const PUBLIC_DATA_FILES_BASE = [
  "forge/data/README.md",
  "forge/data/bundled-apps.json",
  "forge/data/mime-types.json",
  "forge/data/env-variables.json",
  "forge/data/control-plane-config.json",
  "forge/data/runtime-config.json",
  "forge/data/engine-room-tables.json",
  "forge/data/snapshot-types.json",
  "forge/data/app-status-enums.json",
  "forge/data/trust-levels.json",
  "forge/data/package-manifest.json",
  "forge/data/control-commands.json",
  "forge/data/control-response-schema.json",
  "forge/data/tables.json",
  "forge/spec/data-catalog.md",
];

function publicDataFiles(root = repoRoot) {
  const files = [...PUBLIC_DATA_FILES_BASE];
  const commandsJson = path.join(root, "forge", "data", "commands.json");
  if (fs.existsSync(commandsJson)) {
    files.push("forge/data/commands.json");
  }
  return files;
}

const PUBLIC_FIXTURE_DIRS = [
  "webapps/examples",
  "forge/fixtures",
];

const PUBLIC_TOOL_FILES = [
  "tools/check-repo.mjs",
  "tools/agent-adapter/catalog-to-tools.mjs",
  "tools/agent-adapter/execute-tool.mjs",
  "tools/agent-adapter/test/agent-adapter.test.mjs",
  "tools/build-forge-api-docs.mjs",
  "tools/export-commands-catalog.mjs",
  "tools/export-public-contract.mjs",
  "tools/test/forge-api-docs.test.mjs",
  "tools/package-release.mjs",
  "tools/verify-public-contract.mjs",
  "forge/crates/cli/tests/e2e.rs",
  "forge/crates/core/tests/spine.rs",
  "forge/crates/core/tests/sync.rs",
  "forge/crates/ffi/tests/ffi.rs",
  "forge/crates/runtime/tests/conformance_engines.rs",
  "forge/crates/ui/tests/protocol.rs",
  "tools/reference-host/test/public-contract.test.js",
];

const CORE_COMMANDS = [
  "applet.install",
  "applet.suspend",
  "applet.uninstall",
  "applet.upgrade",
  "bridge.prepare_session",
  "bridge.record_call",
  "bridge.record_core_event",
  "bridge.record_crash_recovery",
  "bridge.validate_envelope",
  "bridge.validate_network_request",
  "audit.query",
  "permission.request_grant",
  "permission.revoke",
  "query.execute",
  "quota.set",
  "quota.status",
  "record.delete",
  "record.patch",
  "record.put",
  "runtime.replay",
  "runtime.replay_session",
  "runtime.run",
  "schema.apply_change",
  "schema.rebuild_indexes",
  "schema.validate_compatibility",
  "sync.export",
  "sync.import",
  "sync.trust_peer",
  "ui.dispatch_event",
  "workspace.export",
  "workspace.import",
  "workspace.open",
  "package.get_manifest",
  "package.get_permissions",
];

const RUNTIME_EVENTS = [
  {
    eventName: "ui.patch",
    payload: "{ request_id, tree | patch }",
    when: "A run or UI event produces a new declarative UI tree or patch",
  },
  {
    eventName: "record.changed",
    payload: "{ collection, record_id, mutation_id }",
    when: "A committed record mutation dirties a watched collection",
  },
  {
    eventName: "sync.packet",
    payload: "{ source, chunks[] }",
    when: "A trusted peer exports CRDT chunks through sync.export",
  },
];

const SYNCABLE_RECORD_KINDS = [
  "collection_record",
  "crdt_chunk",
  "run_log",
  "schema_registry",
  "workspace_export",
];

const NON_SYNCABLE_RECORD_KINDS = [
  "local_file_path",
  "operator_local_token",
  "private_signing_key",
  "secret_material",
  "unredacted_host_trace",
];

const SYNC_RECORD_MAPPINGS = [
  {
    kind: "collection_record",
    localTables: ["records", "record_history", "oplog"],
    privateAdapterResource: "collectionRecord",
  },
  {
    kind: "crdt_chunk",
    localTables: ["crdt_chunks", "oplog"],
    privateAdapterResource: "crdtChunk",
  },
  {
    kind: "run_log",
    localTables: ["run_logs", "audit_log"],
    privateAdapterResource: "deterministicRunLog",
  },
  {
    kind: "schema_registry",
    localTables: ["schema_registry", "oplog"],
    privateAdapterResource: "schemaRegistry",
  },
  {
    kind: "workspace_export",
    localTables: ["workspace_export"],
    privateAdapterResource: "workspaceExport",
  },
];

const NON_SYNC_RECORD_MAPPINGS = [
  {
    kind: "local_file_path",
    localTables: ["file_grants"],
    reason: "OS paths can reveal local filesystem identity and layout.",
  },
  {
    kind: "operator_local_token",
    localTables: ["host_sessions"],
    reason: "Operator/session tokens are per-launch local secrets.",
  },
  {
    kind: "private_signing_key",
    localTables: ["trust_store"],
    reason: "Private signing keys must never leave the local trust boundary.",
  },
  {
    kind: "secret_material",
    localTables: ["secrets"],
    reason: "Secret values sync only through explicit future key-management policy, never by default.",
  },
  {
    kind: "unredacted_host_trace",
    localTables: ["host_trace"],
    reason: "Host traces can contain app data before redaction.",
  },
];

function loadCommandCatalog(root) {
  const catalogPath = path.join(root, "forge", "data", "commands.json");
  if (!fs.existsSync(catalogPath)) {
    return null;
  }
  try {
    return JSON.parse(fs.readFileSync(catalogPath, "utf8"));
  } catch (error) {
    throw new Error(`failed to parse ${catalogPath}: ${error.message}`);
  }
}

function bridgeMethodsFromCatalog(catalog) {
  if (!catalog?.commands?.length) {
    return null;
  }
  return catalog.commands
    .filter((entry) => entry.surface !== "inner" && entry.visibility !== "debug")
    .map((entry) => entry.name)
    .sort();
}

export function buildPublicContract({ root = repoRoot } = {}) {
  const catalog = loadCommandCatalog(root);
  const bridgeMethods = bridgeMethodsFromCatalog(catalog) ?? CORE_COMMANDS;
  return {
    $schema: "https://example.local/forge/contracts/public-contract.schema.json",
    schemaVersion: 1,
    contractId: "terrane-public-contract",
    contractVersion: CONTRACT_VERSION,
    platformBaseline: PLATFORM_BASELINE,
    runtimeVersion: RUNTIME_VERSION,
    license: "MIT",
    provenance: {
      sourceRepository: "terrane",
      sourceCommit: readGitCommit(root),
      releaseTag: process.env.TERRANE_RELEASE_TAG ?? null,
      generatedFrom: "tools/export-public-contract.mjs",
      releaseManifestArtifactId: "public-contract",
    },
    generatedAppBoundary: {
      api: ["CoreCommand", "CoreResponse", "CoreEvent", "ctx.db", "ctx.net", "ctx.files", "ctx.ui"],
      forbidden: [
        "direct SaaS API calls",
        "direct native API access",
        "direct fetch/XMLHttpRequest/WebSocket/EventSource",
        "localStorage/sessionStorage/IndexedDB/cookies",
        "raw SQL or database handles",
        "SaaS access tokens",
        "sync tokens",
        "billing/admin tokens",
        "signing keys",
        "local control tokens",
      ],
    },
    bridge: {
      methods: bridgeMethods,
      notebookMethods: [],
      events: RUNTIME_EVENTS,
      ...(catalog?.catalogVersion
        ? { catalogVersion: catalog.catalogVersion, commandCatalog: catalog.commands }
        : {}),
    },
    sync: {
      syncableRecordKinds: SYNCABLE_RECORD_KINDS,
      nonSyncableRecordKinds: NON_SYNCABLE_RECORD_KINDS,
      recordMappings: SYNC_RECORD_MAPPINGS,
      nonSyncableRecordMappings: NON_SYNC_RECORD_MAPPINGS,
    },
    files: {
      docs: describeFiles(root, PUBLIC_DOCS),
      contracts: describeFiles(root, PUBLIC_CONTRACT_FILES),
      data: describeFiles(root, publicDataFiles(root)),
      fixtures: describeFiles(root, collectFixtureFiles(root)),
      tools: describeFiles(root, PUBLIC_TOOL_FILES),
    },
    conformance: {
      sourceCheckoutRequired: true,
      commands: [
        "cd forge && cargo test --workspace --locked",
        "cd forge && cargo clippy --workspace --all-targets --locked -- -D warnings",
        "cd forge && cargo run -p forge-cli -- demo",
        "node --no-warnings tools/export-commands-catalog.mjs",
        "node --no-warnings tools/export-public-contract.mjs --out artifacts/public-contract.json",
        "node --no-warnings tools/verify-public-contract.mjs --contract artifacts/public-contract.json --root .",
      ],
    },
  };
}

function readGitCommit(root) {
  try {
    return execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim();
  } catch {
    return null;
  }
}

export function writePublicContract({ outPath = path.join(repoRoot, "artifacts", "public-contract.json"), root = repoRoot } = {}) {
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  const contract = buildPublicContract({ root });
  fs.writeFileSync(outPath, `${JSON.stringify(contract, null, 2)}\n`);
  return {
    outPath,
    contract,
  };
}

function collectFixtureFiles(root) {
  return PUBLIC_FIXTURE_DIRS.flatMap((dir) =>
    walk(path.join(root, dir))
      .filter(isPublicFixtureFile)
      .map((filePath) => toPosix(path.relative(root, filePath)))
  ).sort(compareStrings);
}

function isPublicFixtureFile(filePath) {
  return [".fingerprint.txt", ".json", ".ts"].some((suffix) => filePath.endsWith(suffix));
}

function describeFiles(root, relativePaths) {
  return relativePaths
    .map((relativePath) => describeFile(path.join(root, relativePath), relativePath))
    .sort((left, right) => compareStrings(left.path, right.path));
}

function describeFile(filePath, relativePath) {
  const data = fs.readFileSync(filePath);
  return {
    path: toPosix(relativePath),
    bytes: data.length,
    sha256: crypto.createHash("sha256").update(data).digest("hex"),
  };
}

function walk(dir) {
  if (!fs.existsSync(dir)) return [];
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  const results = [];
  for (const entry of entries) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      results.push(...walk(fullPath));
    } else if (entry.isFile()) {
      results.push(fullPath);
    }
  }
  return results;
}

function toPosix(value) {
  return value.split(path.sep).join("/");
}

function compareStrings(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function cli() {
  const outIndex = process.argv.indexOf("--out");
  const outPath = outIndex >= 0 ? path.resolve(process.argv[outIndex + 1]) : path.join(repoRoot, "artifacts", "public-contract.json");
  if (outIndex >= 0 && !process.argv[outIndex + 1]) {
    throw new Error("--out requires a path");
  }
  writePublicContract({ outPath });
  console.log(outPath);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  cli();
}
