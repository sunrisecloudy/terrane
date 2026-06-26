#!/usr/bin/env node
/**
 * Generate per-command request/response JSON schemas under schemas/commands/.
 * Run once to bootstrap; stable-command schemas are authored inline below.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const outDir = path.join(repoRoot, "schemas", "commands");

const COMMANDS = [
  ["workspace.create", "Create a new workspace at a path."],
  ["workspace.open", "Report workspace metadata and the current logical clock."],
  ["applet.install", "Compile, validate, and store an applet from manifest + sources."],
  ["applet.enable", "Transition an installed applet to enabled state."],
  ["applet.suspend", "Suspend an active applet before user code runs."],
  ["applet.upgrade", "Atomically upgrade an active applet to a new version."],
  ["applet.uninstall", "Remove the active applet record (retention policy optional)."],
  ["runtime.run", "Record one deterministic run of an installed applet."],
  ["legacy.core_step", "Bridge compatibility shim for legacy webapp core.step."],
  ["bridge.validate_network_request", "Phase C bridge: validate a network request envelope."],
  ["bridge.validate_envelope", "Phase C bridge: validate a bridge message envelope."],
  ["bridge.prepare_session", "Phase C bridge: prepare a bridge session."],
  ["bridge.record_call", "Phase C bridge: record a bridge host call."],
  ["bridge.record_core_event", "Phase C bridge: record a core event from the bridge."],
  ["bridge.record_crash_recovery", "Phase C bridge: record crash recovery metadata."],
  ["package.get_manifest", "Legacy webapp: read the trusted package manifest."],
  ["package.get_permissions", "Legacy webapp: read package permissions."],
  ["package.provision_registry", "Legacy webapp: provision the package registry."],
  ["package.list_versions", "Legacy webapp: list installed package versions."],
  ["package.activate_version", "Legacy webapp: activate a package version."],
  ["package.rollback_version", "Legacy webapp: roll back to a prior version."],
  ["package.set_status", "Legacy webapp: set package status (quarantine, etc.)."],
  ["runtime.replay", "Replay a single recorded run and assert byte-identical trace."],
  ["runtime.replay_session", "Replay an ordered interactive session (run + UI events)."],
  ["ui.dispatch_event", "Dispatch a UI event handler and produce the next UI patch."],
  ["query.execute", "List records in a collection (collection-scoped db.read gate)."],
  ["audit.query", "Privileged read of the durable SC-12 audit log."],
  ["db.watch", "Register a reactive live query over a row query."],
  ["db.unwatch", "Cancel a live query registration (idempotent)."],
  ["db.history", "Read a record's change feed (time travel)."],
  ["db.restore", "Non-destructive restore: append a new record version."],
  ["schema.apply_change", "Apply a schema registry change and optional migration."],
  ["schema.validate_compatibility", "Validate proposed schema compatibility."],
  ["schema.rebuild_indexes", "Rebuild collection indexes from the schema registry."],
  ["sync.trust_peer", "Trust a sync peer for CRDT import authorization."],
  ["sync.export", "Export CRDT sync chunks for a workspace."],
  ["sync.import", "Import authorized CRDT sync chunks."],
  ["quota.status", "Report workspace quota usage vs trusted limits."],
  ["quota.set", "Configure trusted quota policy override (Owner-only)."],
  ["quota.auto_quarantine", "Auto-quarantine applets exceeding quota policy."],
  ["workspace.export", "Export a workspace backup archive."],
  ["workspace.import", "Import a workspace backup archive."],
];

const STABLE = new Set([
  "workspace.open",
  "applet.install",
  "runtime.run",
  "ui.dispatch_event",
  "query.execute",
]);

function previewSchema(name, kind, description) {
  const title = `${name} ${kind}`;
  const id = `https://example.local/schemas/commands/${name}.${kind}.schema.json`;
  return {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: id,
    title,
    description: `Preview schema for ${name}. ${description} Full schema pending.`,
    type: "object",
  };
}

const stableSchemas = {
  "workspace.open.request.schema.json": {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: "https://example.local/schemas/commands/workspace.open.request.schema.json",
    title: "workspace.open request",
    description:
      "Open an already-attached workspace file and report metadata. The handler ignores payload fields; the workspace is bound when the core is constructed.",
    type: "object",
    additionalProperties: true,
    properties: {
      workspace_id: {
        type: "string",
        description:
          "Optional; informational only — the active workspace is already bound to this core instance.",
      },
    },
  },
  "workspace.open.response.schema.json": {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: "https://example.local/schemas/commands/workspace.open.response.schema.json",
    title: "workspace.open response",
    description: "Workspace metadata and the current logical clock (event sink length).",
    type: "object",
    additionalProperties: false,
    required: ["workspace_id", "logical_clock"],
    properties: {
      workspace_id: { type: "string" },
      logical_clock: { type: "integer", minimum: 0 },
    },
  },
  "query.execute.request.schema.json": {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: "https://example.local/schemas/commands/query.execute.request.schema.json",
    title: "query.execute request",
    description:
      "List every record in a collection from the projection. Collection-scoped db.read is enforced from trusted workspace grants keyed by actor, not from payload.grants.",
    type: "object",
    additionalProperties: true,
    required: ["collection"],
    properties: {
      collection: { type: "string", minLength: 1 },
      grants: {
        type: "object",
        description:
          "Documented in spec for grant shape; enforcement reads trusted grants, not this field.",
        additionalProperties: true,
      },
    },
  },
  "query.execute.response.schema.json": {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: "https://example.local/schemas/commands/query.execute.response.schema.json",
    title: "query.execute response",
    description: "Projection rows for the queried collection.",
    type: "object",
    additionalProperties: false,
    required: ["collection", "rows"],
    properties: {
      collection: { type: "string" },
      rows: {
        type: "array",
        items: {
          type: "object",
          additionalProperties: false,
          required: ["id", "fields"],
          properties: {
            id: { type: "string" },
            fields: { type: "object", additionalProperties: true },
          },
        },
      },
    },
  },
  "runtime.run.request.schema.json": {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: "https://example.local/schemas/commands/runtime.run.request.schema.json",
    title: "runtime.run request",
    description:
      "Run an installed applet's entrypoint in record mode. Optional random_seed/time_start must be set together to pin deterministic seams.",
    type: "object",
    additionalProperties: true,
    required: ["applet_id"],
    properties: {
      applet_id: { type: "string", minLength: 1 },
      input: { description: "Entrypoint input; defaults to null when omitted." },
      random_seed: {
        type: "integer",
        minimum: 0,
        description: "Optional deterministic RNG seed override (must pair with time_start).",
      },
      time_start: {
        type: "integer",
        minimum: 0,
        maximum: 9223372036854775807,
        description:
          "Optional deterministic clock start (must fit i64; must pair with random_seed).",
      },
    },
    allOf: [
      {
        if: { required: ["random_seed"] },
        then: { required: ["time_start"] },
      },
      {
        if: { required: ["time_start"] },
        then: { required: ["random_seed"] },
      },
    ],
  },
  "runtime.run.response.schema.json": {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: "https://example.local/schemas/commands/runtime.run.response.schema.json",
    title: "runtime.run response",
    description: "Run summary, app result, host-call trace surface, UI renders, and quota warnings.",
    type: "object",
    additionalProperties: false,
    required: [
      "run_id",
      "code_hash",
      "ok",
      "result",
      "summary",
      "host_call_methods",
      "ui_renders",
      "quota_warnings",
    ],
    properties: {
      run_id: { type: "string" },
      code_hash: { type: "string" },
      ok: { type: "boolean" },
      result: {
        description:
          "AppResult on success ({ ok, value }) or failure envelope ({ error }).",
        type: "object",
        additionalProperties: true,
      },
      summary: {
        type: "object",
        additionalProperties: false,
        required: ["run_id", "applet_id", "code_hash", "calls", "logs", "completed"],
        properties: {
          run_id: { type: "string" },
          applet_id: { type: "string" },
          code_hash: { type: "string" },
          calls: { type: "integer", minimum: 0 },
          logs: { type: "integer", minimum: 0 },
          completed: { type: "boolean" },
        },
      },
      host_call_methods: {
        type: "array",
        items: { type: "string" },
        description: "Ordered host-call method names from the recorded trace.",
      },
      ui_renders: {
        type: "array",
        items: { type: "object", additionalProperties: true },
        description: "Final UI tree from each ui.render host call.",
      },
      quota_warnings: {
        type: "array",
        items: {
          type: "object",
          additionalProperties: false,
          required: ["collection", "scope", "projected", "limit", "suggestion"],
          properties: {
            collection: { type: "string" },
            scope: { type: "string" },
            projected: { type: "integer", minimum: 0 },
            limit: { type: "integer", minimum: 0 },
            suggestion: { type: "string" },
          },
        },
      },
    },
  },
  "applet.install.request.schema.json": {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: "https://example.local/schemas/commands/applet.install.request.schema.json",
    title: "applet.install request",
    description:
      "Compile each source, validate the manifest, optionally verify an Ed25519 signed package, and store the applet.",
    type: "object",
    additionalProperties: true,
    required: ["applet_id", "manifest", "sources"],
    properties: {
      applet_id: { type: "string", minLength: 1 },
      manifest: { $ref: "../applet-manifest.schema.json" },
      sources: {
        type: "object",
        minProperties: 1,
        additionalProperties: { type: "string", minLength: 1 },
        description: "Map of source path to TypeScript source text.",
      },
      signature: {
        type: "object",
        description:
          "Optional MP-4 signed package (T012 shape). When present, verified before install.",
        additionalProperties: true,
        properties: {
          package: { $ref: "../app-package.schema.json" },
          signature: { type: "string" },
          public_key: { type: "string" },
          publisher_trust: { type: "object", additionalProperties: true },
        },
      },
    },
  },
  "applet.install.response.schema.json": {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: "https://example.local/schemas/commands/applet.install.response.schema.json",
    title: "applet.install response",
    description: "Install outcome with lifecycle, version identity, and trust provenance.",
    type: "object",
    additionalProperties: false,
    required: [
      "applet_id",
      "install_generation",
      "version",
      "code_hash",
      "lifecycle",
      "warnings",
      "trust",
    ],
    properties: {
      applet_id: { type: "string" },
      install_generation: { type: "integer", minimum: 1 },
      version: { type: "integer", minimum: 1 },
      code_hash: { type: "string" },
      lifecycle: { type: "string", const: "enabled" },
      idempotent: {
        type: "boolean",
        description: "Present on idempotent reinstall (same code_hash + manifest).",
      },
      warnings: { type: "array", items: { type: "string" } },
      trust: {
        oneOf: [
          {
            type: "object",
            additionalProperties: false,
            required: ["status"],
            properties: { status: { const: "unsigned" } },
          },
          {
            type: "object",
            additionalProperties: false,
            required: ["status", "publisher", "key_id", "publisher_trust_enforced"],
            properties: {
              status: { const: "signed" },
              publisher: { type: ["string", "null"] },
              key_id: { type: ["string", "null"] },
              publisher_trust_enforced: { type: "boolean" },
            },
          },
        ],
      },
    },
  },
  "ui.dispatch_event.request.schema.json": {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: "https://example.local/schemas/commands/ui.dispatch_event.request.schema.json",
    title: "ui.dispatch_event request",
    description:
      "Re-enter an applet handler for a UI ActionRef. Null/absent action_ref is a safe ignored no-op.",
    type: "object",
    additionalProperties: true,
    required: ["applet_id"],
    properties: {
      applet_id: { type: "string", minLength: 1 },
      action_ref: {
        type: ["string", "null"],
        description: "Exported handler name from the rendered control; null/absent → ignored no-op.",
      },
      event_payload: {
        type: "object",
        additionalProperties: true,
        description: "Event-specific payload passed to the handler; defaults to {}.",
      },
    },
  },
  "ui.dispatch_event.response.schema.json": {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: "https://example.local/schemas/commands/ui.dispatch_event.response.schema.json",
    title: "ui.dispatch_event response",
    description:
      "Handler dispatch result: new tree + patches, ignored no-op, or error (typed CoreError) before return.",
    oneOf: [
      {
        type: "object",
        additionalProperties: false,
        required: ["applet_id", "ignored", "reason", "patches"],
        properties: {
          applet_id: { type: "string" },
          ignored: { const: true },
          reason: { type: "string" },
          patches: { type: "array", maxItems: 0 },
        },
      },
      {
        type: "object",
        additionalProperties: false,
        required: ["applet_id", "action_ref", "run_id", "tree", "patches"],
        properties: {
          applet_id: { type: "string" },
          action_ref: { type: "string" },
          run_id: { type: "string" },
          tree: { description: "New UI tree JSON; null when no prior tree and handler rendered nothing." },
          patches: {
            type: "array",
            items: { type: "object", additionalProperties: true },
            description: "UI diff patches against the prior last-known tree.",
          },
        },
      },
    ],
  },
};

fs.mkdirSync(outDir, { recursive: true });

let written = 0;
for (const [name, summary] of COMMANDS) {
  for (const kind of ["request", "response"]) {
    const filename = `${name}.${kind}.schema.json`;
    const dest = path.join(outDir, filename);
    let schema;
    if (STABLE.has(name) && stableSchemas[filename]) {
      schema = stableSchemas[filename];
    } else {
      schema = previewSchema(name, kind, summary);
    }
    fs.writeFileSync(dest, `${JSON.stringify(schema, null, 2)}\n`);
    written++;
  }
}

console.log(`Wrote ${written} schema files to ${outDir}`);