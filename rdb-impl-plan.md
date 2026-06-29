# Terrane Relational DB Capability Implementation Plan

## Goal

Add a new capability named `relational_db` in a new crate named
`terrane-cap-relational-db`. The capability gives app backends a small
relational table interface while storing all durable data in the existing
per-app `kv` capability.

The design is DynamoDB-like in the important sense: table specs declare the
primary key and secondary indexes up front, and writes maintain materialized KV
lookup records. Reads do not run arbitrary joins or table scans by default; they
read rows by primary key or scan bounded key ranges for declared indexes.

## Current repo baseline

- Shared capability ABI: `rust/crates/terrane-cap-interface/src/lib.rs`
- KV capability: `rust/crates/terrane-cap-kv/src/lib.rs`
- Core registry and state: `rust/crates/terrane-core/src/lib.rs`
- Capability re-exports: `rust/crates/terrane-core/src/cap/mod.rs`
- Host JS resource bridge: `rust/crates/terrane-core/src/cap/host.rs`
- Generated app resource docs: `docs/APP_API.md`
- Workspace members/dependencies: `rust/Cargo.toml`

The new capability should use the current `terrane-cap-interface` crate, not the
older `terrane-cap-api` name.

## Capability docs and agent exposure

The RDB capability needs rich machine-readable docs for AI agents. Do not
maintain separate hand-written descriptions for MCP, CLI help, generated skill
docs, and public contract export. Add one canonical `CapabilityDoc` model to the
capability interface, then render it in each host surface.

Current repo state to account for:

- MCP is app-centric today: `list_apps`, `app_actions`, and `invoke` are
  declared in `rust/crates/terrane-api/src/lib.rs` and routed in
  `rust/crates/terrane-host/src/mcp.rs`.
- Public contract export currently contains capability names and shallow
  `ctx.resource` method metadata through `terrane_core::resource_surface()`.
- App action discovery (`__actions__`) describes an installed app's verbs. It
  does not teach an agent how to author a new backend against platform
  resources like `relational_db`.

Add a shared documentation interface:

```rust
pub trait Capability {
    fn namespace(&self) -> &'static str;
    fn manifest(&self) -> CapManifest;

    fn doc(&self) -> CapabilityDoc {
        CapabilityDoc::from_manifest(self.namespace(), self.manifest())
    }
}
```

The default `doc()` gives every capability a minimal generated description from
its manifest. `RelationalDbCapability` overrides it with complete method docs,
schemas, examples, limits, error cases, and internal notes.

Core doc model:

```ts
type CapabilityDoc = {
  namespace: string;
  displayName: string;
  summary: string;
  detail: string;
  audiences: CapabilityAudience[];
  resources: ResourceMethodDoc[];
  schemas: SchemaDoc[];
  examples: ExampleDoc[];
  limits: Record<string, JsonValue>;
  errors: ErrorDoc[];
  security: string[];
  determinism: string[];
  compatibility: string[];
  internal?: InternalDoc;
};

type CapabilityAudience =
  | "app-author"
  | "agent"
  | "human"
  | "host-implementer";

type ResourceMethodDoc = {
  name: string;
  kind: "read" | "write";
  signature: string;
  summary: string;
  params: ParamDoc[];
  returns: ReturnDoc;
  errors: string[];
  examples: string[];
  notes: string[];
};

type ParamDoc = {
  name: string;
  type: string;
  required: boolean;
  summary: string;
  schemaRef?: string;
};

type ReturnDoc = {
  type: string;
  summary: string;
  schemaRef?: string;
};

type SchemaDoc = {
  id: string;
  contentType: "application/schema+json" | "application/json";
  summary: string;
  json: JsonValue;
};

type ExampleDoc = {
  name: string;
  language: "javascript" | "json" | "shell" | "markdown";
  summary: string;
  source: string;
};

type ErrorDoc = {
  code: string;
  message: string;
  when: string;
};

type InternalDoc = {
  warning: string;
  kvPrefixes: string[];
  storageLayout: string[];
  writeAmplification: string[];
};
```

Capability doc rendering rules:

- JSON is the canonical wire format.
- Markdown is rendered from the same JSON for humans.
- Skill artifacts are rendered from the same JSON into a directory tree.
- `includeInternal` defaults to `false`. Agents and app authors see the
  app-facing RDB contract by default. Reserved KV layout and internal write
  amplification details appear only when explicitly requested.
- The renderer must never invent methods from prose. It renders methods,
  schemas, examples, and notes present in `CapabilityDoc`.

MCP exposure:

```text
capabilities_list {}
capability_info { namespace, format?, detail?, includeSchemas?, includeInternal? }
```

MCP input schemas:

```json
{
  "type": "object",
  "properties": {},
  "additionalProperties": false
}
```

```json
{
  "type": "object",
  "properties": {
    "namespace": { "type": "string" },
    "format": { "enum": ["json", "markdown", "skill"] },
    "detail": { "enum": ["summary", "full"] },
    "includeSchemas": { "type": "boolean" },
    "includeInternal": { "type": "boolean" }
  },
  "required": ["namespace"],
  "additionalProperties": false
}
```

MCP behavior:

- `capabilities_list` returns installed capability namespaces and summaries.
- `capability_info` returns one rendered capability doc. Default:
  `format = "json"`, `detail = "full"`, `includeSchemas = true`,
  `includeInternal = false`.
- Unknown namespace returns an MCP tool result with `isError: true`, matching the
  current `invoke` failure style.
- This is separate from `app_actions`: `app_actions` discovers installed app
  verbs, while `capability_info` teaches agents how to use platform resources.

CLI exposure:

```text
terrane cap list
terrane cap info <namespace>
terrane cap info <namespace> --format json
terrane cap info <namespace> --format markdown
terrane cap info <namespace> --format skill
terrane cap info <namespace> --include-internal
terrane cap skill <namespace> --out <dir>
```

CLI behavior:

- Human default is Markdown to stdout.
- `--format json` prints canonical doc JSON.
- `--format skill` prints a single-file `SKILL.md` rendering to stdout.
- `terrane cap skill ... --out <dir>` writes a skill artifact directory:

```text
<dir>/SKILL.md
<dir>/references/table_spec.schema.json
<dir>/references/query.schema.json
<dir>/examples/users.js
```

Generated skill requirements:

- `SKILL.md` must identify itself as generated from `CapabilityDoc`, including
  capability namespace and doc schema version.
- Skill instructions must tell the agent to use `ctx.resource.relational_db`,
  pass JSON strings, parse JSON strings returned by reads, and declare
  `"relational_db"` in app manifests.
- Schema references and examples must be copied from `CapabilityDoc.schemas` and
  `CapabilityDoc.examples`; no duplicate hand-maintained versions.

Public contract export:

- Extend `terrane_api::PublicSurface` to include `capability_docs` or
  `capabilities: Vec<CapabilitySummary>` plus optional full docs.
- `terrane contract export` should include enough capability-doc metadata for
  premium/host implementations to validate exposed resources and MCP docs.
- Full large schemas can either be embedded directly in the contract surface or
  included as named artifacts with content hashes. Choose direct embedding for
  local simplicity first; split artifacts later if contract size becomes a real
  problem.

Relational DB doc payload must include:

- Full method docs for `defineTable`, `put`, `delete`, `get`, `query`,
  `tables`, and `spec`.
- Embedded `rdb.tableSpec.v1` JSON Schema from `table_spec.schema.json`.
- Embedded `rdb.query.v1` JSON Schema from `query.schema.json`.
- Examples for table definition, insert/update, get by primary key, query by
  secondary index, unique index conflict, and sparse index behavior.
- Limits: `maxRowBytes`, `defaultQueryLimit`, `maxQueryLimit`, reserved prefix,
  and maximum schema size if one is added.
- Determinism notes: no generated IDs/timestamps/random defaults; writes produce
  recorded KV events; reads are not recorded.
- Security/sandbox notes: app-scoped data, manifest resource allowlist, no
  direct reserved KV access through public `kv`.
- Internal notes, gated behind `includeInternal = true`: reserved KV prefixes,
  materialized index keys, unique-key records, delete/backfill mechanics, and
  write amplification.

## Public backend API

Apps opt in through `manifest.json`:

```json
{
  "resources": ["relational_db"]
}
```

The resource appears at `ctx.resource.relational_db`.

Initial app-facing methods:

| Method | Kind | Return shape |
| --- | --- | --- |
| `defineTable(table, specJson)` | write | no direct JS return |
| `put(table, rowJson)` | write | no direct JS return |
| `delete(table, keyJson)` | write | no direct JS return |
| `get(table, keyJson)` | read | row JSON string or `null` |
| `query(table, index, queryJson)` | read | JSON array string |
| `tables()` | read | JSON array string |
| `spec(table)` | read | normalized spec JSON string or `null` |

Example backend code:

```js
var rdb = ctx.resource.relational_db;

var userSpec = {
  specVersion: 1,
  schemaVersion: 1,
  name: "users",
  description: "User account records for this Terrane app.",
  primaryKey: { partition: ["id"] },
  fields: {
    id: { type: "string", required: true, maxLength: 80 },
    email: { type: "string", required: true, maxLength: 254 },
    status: { type: "string", required: true, enum: ["active", "disabled"] },
    orgId: { type: "string", required: true },
    createdAt: { type: "string", required: true, format: "date-time" },
    profile: { type: "json" }
  },
  indexes: {
    byEmail: {
      partition: ["email"],
      unique: true,
      projection: { type: "keys" }
    },
    byStatus: {
      partition: ["status"],
      sort: ["createdAt"],
      projection: { type: "keys" }
    },
    byOrgStatus: {
      partition: ["orgId", "status"],
      sort: ["createdAt"],
      projection: { type: "include", fields: ["id", "email", "status", "createdAt"] }
    }
  },
  options: {
    unknownFields: "preserve",
    maxRowBytes: 65536,
    defaultQueryLimit: 100,
    maxQueryLimit: 500
  }
};

function handle(input) {
  if (input[0] === "init") {
    rdb.defineTable("users", JSON.stringify(userSpec));
    return "ok";
  }

  if (input[0] === "add") {
    rdb.put("users", JSON.stringify({
      id: input[1],
      email: input[2],
      status: "active",
      createdAt: input[3]
    }));
    return "saved";
  }

  if (input[0] === "active") {
    return rdb.query("users", "byStatus", JSON.stringify({
      partition: ["active"],
      limit: 50
    }));
  }

  return "?";
}
```

## Complete table spec JSON

`defineTable(table, specJson)` accepts a complete table contract, not a loose
hint. The capability must parse, validate, normalize defaults, serialize a
canonical form, then store that canonical JSON string. The `table` argument is
the storage identifier; if `specJson.name` is present it must equal `table`.

Use `specVersion` for the Terrane RDB table-spec format version. Use
`schemaVersion` for the app's logical schema migration version.

```ts
type TableSpec = {
  specVersion: 1;
  schemaVersion: number;
  name?: string;
  description?: string;
  fields: Record<FieldName, FieldSpec>;
  primaryKey: PrimaryKeySpec;
  indexes?: Record<string, IndexSpec>;
  constraints?: Record<string, ConstraintSpec>;
  options?: TableOptions;
};

type FieldName = string;

type PrimaryKeySpec = {
  partition: KeyPart[];
  sort?: KeyPart[];
};

type FieldSpec = {
  type: FieldType;
  description?: string;
  required?: boolean;
  nullable?: boolean;
  default?: JsonValue;
  enum?: JsonScalar[];
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  format?: FieldFormat;
  minimum?: number;
  maximum?: number;
  exclusiveMinimum?: number;
  exclusiveMaximum?: number;
  multipleOf?: number;
  minItems?: number;
  maxItems?: number;
  itemType?: FieldType;
};

type FieldType =
  | "string"
  | "number"
  | "integer"
  | "boolean"
  | "json"
  | "object"
  | "array";

type FieldFormat =
  | "date-time"
  | "date"
  | "email"
  | "uri"
  | "uuid";

type KeyPart =
  | FieldName
  | {
      field: FieldName;
      order?: "asc";
      nulls?: "reject";
    };

type IndexSpec = {
  description?: string;
  partition: KeyPart[];
  sort?: KeyPart[];
  unique?: boolean;
  sparse?: boolean;
  projection?: ProjectionSpec;
  status?: "active";
};

type ProjectionSpec =
  | { type: "keys" }
  | { type: "all" }
  | { type: "include"; fields: string[] };

type ConstraintSpec =
  | {
      type: "unique";
      fields: FieldName[];
      sparse?: boolean;
    }
  | {
      type: "requiredTogether";
      fields: FieldName[];
    };

type TableOptions = {
  unknownFields?: "preserve" | "reject";
  maxRowBytes?: number;
  defaultQueryLimit?: number;
  maxQueryLimit?: number;
  canonicalJson?: true;
};

type JsonScalar = string | number | boolean | null;
type JsonValue = JsonScalar | JsonValue[] | { [key: string]: JsonValue };
```

Canonical normalized defaults:

```json
{
  "specVersion": 1,
  "schemaVersion": 1,
  "fields": {},
  "primaryKey": { "partition": [], "sort": [] },
  "indexes": {},
  "constraints": {},
  "options": {
    "unknownFields": "preserve",
    "maxRowBytes": 65536,
    "defaultQueryLimit": 100,
    "maxQueryLimit": 500,
    "canonicalJson": true
  }
}
```

Per-field defaults:

```json
{
  "required": false,
  "nullable": false
}
```

Per-index defaults:

```json
{
  "sort": [],
  "unique": false,
  "sparse": true,
  "projection": { "type": "keys" },
  "status": "active"
}
```

Validation rules:

- `specVersion` must be `1`.
- `schemaVersion` must be a positive integer.
- `table`, `name`, field names, index names, and constraint names must match
  `^[A-Za-z][A-Za-z0-9_]{0,63}$`.
- `name`, when present, must exactly match the `defineTable(table, specJson)`
  table argument.
- `description` values are metadata only and must not affect storage keys,
  query planning, or replay behavior.
- `fields` is required and must be non-empty.
- `primaryKey.partition` must contain at least one field.
- Every field referenced by `primaryKey`, `index.partition`, `index.sort`,
  `projection.fields`, and constraints must exist in `fields`.
- Primary and index key fields must be scalar typed: `string`, `number`,
  `integer`, or `boolean`. `json`, `object`, and `array` fields are allowed as
  payload fields but cannot be key fields.
- `KeyPart.order` only accepts `"asc"` in v1. Descending indexes can be added
  later with explicit reverse key encoding.
- `KeyPart.nulls` only accepts `"reject"` in v1. Key fields cannot be `null`.
- `projection.type = "include"` must list at least one field and all listed
  fields must exist.
- `unique: true` indexes may have `sort: []` in MVP. Reject unique indexes with
  sort fields until uniqueness semantics for composite ranges are explicitly
  needed.
- Missing primary key fields are invalid for all writes.
- Missing index fields omit the index entry when `sparse: true`. If
  `sparse: false`, missing index fields reject the write.
- Field `default` values, if supported in v1, must be literal JSON values and
  must pass the field's own type and constraint validation. No generated default
  values are allowed.
- `number` and `integer` key values must be finite JSON numbers. Reject
  NaN/Infinity even if a caller somehow produces them before JSON
  serialization.
- `integer` values must be safe JSON integers in the inclusive range
  `[-9007199254740991, 9007199254740991]`.
- `pattern` is an ECMA-style regular expression string. If the Rust regex
  engine cannot support a pattern safely, table definition must reject it.
- `format` validation is deterministic and local only. It must not make network
  calls or consult locale/timezone state.
- `unknownFields: "reject"` rejects row fields not declared in `fields`.
  `unknownFields: "preserve"` stores them in the row JSON but they cannot be
  referenced by keys, indexes, or projections unless later added to `fields`.
- `maxRowBytes`, `defaultQueryLimit`, and `maxQueryLimit` are table-level
  boundedness controls. They must be positive integers, and `maxQueryLimit` must
  be capped by a platform maximum, initially `500`.
- No timestamps, auto IDs, clocks, random values, or external state may be
  generated by the capability. Replay must be deterministic from recorded events
  alone.

Example normalized spec:

```json
{
  "specVersion": 1,
  "schemaVersion": 1,
  "name": "users",
  "description": "User account records for this Terrane app.",
  "primaryKey": {
    "partition": ["id"],
    "sort": []
  },
  "fields": {
    "id": {
      "type": "string",
      "required": true,
      "nullable": false,
      "maxLength": 80
    },
    "email": {
      "type": "string",
      "required": true,
      "nullable": false,
      "format": "email",
      "maxLength": 254
    },
    "status": {
      "type": "string",
      "required": true,
      "nullable": false,
      "enum": ["active", "disabled"]
    },
    "orgId": {
      "type": "string",
      "required": true,
      "nullable": false,
      "maxLength": 80
    },
    "createdAt": {
      "type": "string",
      "required": true,
      "nullable": false,
      "format": "date-time"
    },
    "age": {
      "type": "integer",
      "required": false,
      "nullable": false,
      "minimum": 0,
      "maximum": 150
    },
    "profile": {
      "type": "json",
      "required": false,
      "nullable": false
    }
  },
  "indexes": {
    "byEmail": {
      "description": "Unique email lookup.",
      "partition": ["email"],
      "sort": [],
      "unique": true,
      "sparse": true,
      "projection": { "type": "keys" },
      "status": "active"
    },
    "byStatus": {
      "partition": ["status"],
      "sort": ["createdAt"],
      "unique": false,
      "sparse": true,
      "projection": { "type": "keys" },
      "status": "active"
    },
    "byOrgStatus": {
      "partition": ["orgId", "status"],
      "sort": ["createdAt"],
      "unique": false,
      "sparse": true,
      "projection": {
        "type": "include",
        "fields": ["id", "email", "status", "createdAt"]
      },
      "status": "active"
    }
  },
  "constraints": {
    "emailRequiredWithStatus": {
      "type": "requiredTogether",
      "fields": ["email", "status"]
    }
  },
  "options": {
    "unknownFields": "preserve",
    "maxRowBytes": 65536,
    "defaultQueryLimit": 100,
    "maxQueryLimit": 500,
    "canonicalJson": true
  }
}
```

Machine-readable JSON Schema draft for `specJson` should be checked into the
new crate as `src/table_spec.schema.json` or embedded as a golden test fixture.
The implementation structs in `src/spec.rs` must be tested against this schema
so generated examples and parser behavior cannot drift.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://terrane.local/schemas/rdb-table-spec-v1.json",
  "title": "Terrane RDB Table Spec v1",
  "type": "object",
  "additionalProperties": false,
  "required": ["specVersion", "schemaVersion", "fields", "primaryKey"],
  "properties": {
    "specVersion": { "const": 1 },
    "schemaVersion": { "type": "integer", "minimum": 1 },
    "name": { "$ref": "#/$defs/name" },
    "description": { "type": "string", "maxLength": 512 },
    "fields": {
      "type": "object",
      "minProperties": 1,
      "propertyNames": { "$ref": "#/$defs/name" },
      "additionalProperties": { "$ref": "#/$defs/field" }
    },
    "primaryKey": { "$ref": "#/$defs/keySpec" },
    "indexes": {
      "type": "object",
      "propertyNames": { "$ref": "#/$defs/name" },
      "additionalProperties": { "$ref": "#/$defs/index" },
      "default": {}
    },
    "constraints": {
      "type": "object",
      "propertyNames": { "$ref": "#/$defs/name" },
      "additionalProperties": { "$ref": "#/$defs/constraint" },
      "default": {}
    },
    "options": { "$ref": "#/$defs/options" }
  },
  "$defs": {
    "name": {
      "type": "string",
      "pattern": "^[A-Za-z][A-Za-z0-9_]{0,63}$"
    },
    "fieldName": {
      "type": "string",
      "pattern": "^[A-Za-z][A-Za-z0-9_]{0,63}$"
    },
    "keyPart": {
      "oneOf": [
        { "$ref": "#/$defs/fieldName" },
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["field"],
          "properties": {
            "field": { "$ref": "#/$defs/fieldName" },
            "order": { "const": "asc", "default": "asc" },
            "nulls": { "const": "reject", "default": "reject" }
          }
        }
      ]
    },
    "keySpec": {
      "type": "object",
      "additionalProperties": false,
      "required": ["partition"],
      "properties": {
        "partition": {
          "type": "array",
          "minItems": 1,
          "items": { "$ref": "#/$defs/keyPart" }
        },
        "sort": {
          "type": "array",
          "items": { "$ref": "#/$defs/keyPart" },
          "default": []
        }
      }
    },
    "field": {
      "type": "object",
      "additionalProperties": false,
      "required": ["type"],
      "properties": {
        "type": {
          "enum": ["string", "number", "integer", "boolean", "json", "object", "array"]
        },
        "description": { "type": "string", "maxLength": 512 },
        "required": { "type": "boolean", "default": false },
        "nullable": { "type": "boolean", "default": false },
        "default": true,
        "enum": {
          "type": "array",
          "items": { "type": ["string", "number", "boolean", "null"] }
        },
        "minLength": { "type": "integer", "minimum": 0 },
        "maxLength": { "type": "integer", "minimum": 0 },
        "pattern": { "type": "string" },
        "format": { "enum": ["date-time", "date", "email", "uri", "uuid"] },
        "minimum": { "type": "number" },
        "maximum": { "type": "number" },
        "exclusiveMinimum": { "type": "number" },
        "exclusiveMaximum": { "type": "number" },
        "multipleOf": { "type": "number", "exclusiveMinimum": 0 },
        "minItems": { "type": "integer", "minimum": 0 },
        "maxItems": { "type": "integer", "minimum": 0 },
        "itemType": {
          "enum": ["string", "number", "integer", "boolean", "json", "object", "array"]
        }
      }
    },
    "projection": {
      "oneOf": [
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["type"],
          "properties": { "type": { "const": "keys" } }
        },
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["type"],
          "properties": { "type": { "const": "all" } }
        },
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["type", "fields"],
          "properties": {
            "type": { "const": "include" },
            "fields": {
              "type": "array",
              "minItems": 1,
              "items": { "$ref": "#/$defs/fieldName" }
            }
          }
        }
      ]
    },
    "index": {
      "type": "object",
      "additionalProperties": false,
      "required": ["partition"],
      "properties": {
        "description": { "type": "string", "maxLength": 512 },
        "partition": {
          "type": "array",
          "minItems": 1,
          "items": { "$ref": "#/$defs/keyPart" }
        },
        "sort": {
          "type": "array",
          "items": { "$ref": "#/$defs/keyPart" },
          "default": []
        },
        "unique": { "type": "boolean", "default": false },
        "sparse": { "type": "boolean", "default": true },
        "projection": { "$ref": "#/$defs/projection", "default": { "type": "keys" } },
        "status": { "const": "active", "default": "active" }
      }
    },
    "constraint": {
      "oneOf": [
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["type", "fields"],
          "properties": {
            "type": { "const": "unique" },
            "fields": {
              "type": "array",
              "minItems": 1,
              "items": { "$ref": "#/$defs/fieldName" }
            },
            "sparse": { "type": "boolean", "default": true }
          }
        },
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["type", "fields"],
          "properties": {
            "type": { "const": "requiredTogether" },
            "fields": {
              "type": "array",
              "minItems": 2,
              "items": { "$ref": "#/$defs/fieldName" }
            }
          }
        }
      ]
    },
    "options": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "unknownFields": { "enum": ["preserve", "reject"], "default": "preserve" },
        "maxRowBytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "default": 65536 },
        "defaultQueryLimit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 100 },
        "maxQueryLimit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 500 },
        "canonicalJson": { "const": true, "default": true }
      },
      "default": {}
    }
  }
}
```

## Query JSON

`query(table, index, queryJson)` reads through a declared index. Use
`"__primary"` as the index name to scan the primary key layout.

```ts
type QueryJson = {
  partition: JsonScalar[];
  sort?: SortPredicate;
  limit?: number;
  reverse?: boolean;
  consistent?: true;
};

type SortPredicate =
  | { eq: JsonScalar[] }
  | { prefix: JsonScalar[] }
  | { from?: JsonScalar[]; to?: JsonScalar[]; inclusiveStart?: boolean; inclusiveEnd?: boolean };

type JsonScalar = string | number | boolean | null;
```

MVP query rules:

- `partition` is required and must provide exactly the index partition field
  count.
- `limit` defaults to `100` and is capped at `500`.
- `reverse: true` can be rejected in MVP if the first implementation only scans
  forward.
- `consistent` is accepted only as `true` because local event-log state is
  already strongly consistent inside one Terrane home.
- The return value is a JSON array string. Each entry is a row object unless
  later options add `select: "keys"`.

Example:

```json
{
  "partition": ["active"],
  "sort": {
    "from": ["2026-06-01T00:00:00Z"],
    "to": ["2026-07-01T00:00:00Z"]
  },
  "limit": 50
}
```

## Stored KV layout

All relational DB keys live under a reserved app-local prefix:

```text
__terrane/rdb/v1/
```

Reserved prefix handling is mandatory. Public `ctx.resource.kv` operations must
not be able to read, write, scan, or delete keys under `__terrane/`. Internal
Rust helpers may read/write these keys.

Metadata keys:

```text
__terrane/rdb/v1/tables/<table>
__terrane/rdb/v1/table/<table>/spec
```

Data keys:

```text
__terrane/rdb/v1/row/<table>/<pkKey>
__terrane/rdb/v1/idx/<table>/<index>/<partitionKey>/<sortKey>/<pkKey>
__terrane/rdb/v1/uniq/<table>/<index>/<partitionKey>
```

Values:

- `tables/<table>`: compact JSON summary, for example
  `{"name":"users","specVersion":1,"schemaVersion":1}`.
- `table/<table>/spec`: normalized table spec JSON.
- `row/<table>/<pkKey>`: canonical row JSON.
- `idx/...`: either primary key JSON for `projection: keys`, full row JSON for
  `projection: all`, or projected row JSON for `projection: include`.
- `uniq/...`: primary key JSON string.

The `pkKey`, `partitionKey`, and `sortKey` values are encoded key tuples, not
raw JSON. See key encoding below.

## Key encoding

The implementation needs stable, delimiter-safe, lexicographically ordered key
components.

Recommended component encoding:

```text
S<percent-encoded-utf8>
B0
B1
N<sortable-f64-hex>
Z
```

Meanings:

- `S...`: string.
- `B0` / `B1`: boolean false / true.
- `N...`: finite number encoded so lexicographic order matches numeric order.
  Use the standard sortable IEEE-754 transform: take `f64::to_bits()`, invert
  all bits for negative numbers, otherwise flip the sign bit, then render as
  16 lower-case hex chars.
- `Z`: null. Null is allowed in query values but not in primary key values.

Join encoded components with `/`. Strings must percent-encode at least `%` and
`/` so component boundaries cannot be forged.

Examples:

```text
["active"] -> Sactive
["active", "2026-06-28T10:00:00Z"] -> Sactive/S2026-06-28T10%3A00%3A00Z
[true] -> B1
[42] -> Nc045000000000000
```

## KV capability changes

`terrane-cap-relational-db` should build on `terrane-cap-kv`, but not by copying
private KV event payload structs. Add public Rust helpers in
`terrane-cap-kv/src/lib.rs`.

Internal helper API:

```rust
pub const RESERVED_PREFIX: &str = "__terrane/";

pub fn set_event(app: impl Into<String>, key: impl Into<String>, value: impl Into<String>) -> Result<EventRecord>;
pub fn delete_event(app: impl Into<String>, key: impl Into<String>) -> Result<EventRecord>;

pub fn get_value(state: &dyn StateStore, app: &str, key: &str) -> Result<Option<String>>;
pub fn scan_prefix(state: &dyn StateStore, app: &str, prefix: &str, limit: usize) -> Result<Vec<(String, String)>>;
pub fn scan_range(
    state: &dyn StateStore,
    app: &str,
    start: &str,
    end_exclusive: &str,
    limit: usize,
) -> Result<Vec<(String, String)>>;
pub fn delete_prefix_events(
    state: &dyn StateStore,
    app: &str,
    prefix: &str,
    limit: usize,
) -> Result<Vec<EventRecord>>;
```

Public KV resource API changes:

| Method | Kind | Return shape |
| --- | --- | --- |
| `scan(prefix, limit)` | read | object map of key/value pairs |
| `range(start, endExclusive, limit)` | read | object map of key/value pairs |
| `keys(prefix, limit)` | read | array of keys |

Public KV rules:

- `set`, `rm`, `get`, `all`, `scan`, `range`, and `keys` must hide or reject
  keys beginning with `__terrane/`.
- `all()` should continue to exist for compatibility, but filter out reserved
  keys.
- `scan` and `range` must be bounded. Default limit `100`; max limit `500`.
- CLI `terrane kv set` and `terrane kv rm` should reject reserved keys too,
  because they route through the same `kv` capability.

## Relational DB command/event model

The new capability owns commands/resources in namespace `relational_db`, but it
does not need its own durable state slice in MVP.

Manifest commands:

```text
relational_db.defineTable
relational_db.put
relational_db.delete
```

Manifest events:

```text
(none in MVP)
```

Resource methods:

```text
defineTable(table, specJson)  write
put(table, rowJson)           write
delete(table, keyJson)        write
get(table, keyJson)           read
query(table, index, queryJson) read
tables()                      read
spec(table)                   read
```

The `decide` implementation for write commands returns `Decision::Commit` with
ordinary `kv.set` and `kv.deleted` event records produced by
`terrane_cap_kv::set_event` and `terrane_cap_kv::delete_event`. This is the
core "built on kv" property.

Because the host bridge applies emitted records to the per-run working state,
this also gives read-after-write behavior inside a single backend run:

```js
rdb.defineTable("users", spec);
rdb.put("users", row);
var row = rdb.get("users", key);
```

## Write algorithms

### defineTable

Inputs:

```text
app, table, specJson
```

Algorithm:

1. Ensure app exists with `ensure_app_exists`.
2. Validate table name.
3. Reject if `table/<table>/spec` already exists.
4. Parse, validate, and normalize `specJson`.
5. Commit:
   - `kv.set(app, "__terrane/rdb/v1/tables/<table>", tableSummaryJson)`
   - `kv.set(app, "__terrane/rdb/v1/table/<table>/spec", normalizedSpecJson)`

MVP does not allow redefining a table. Schema changes are a later migration API,
because indexes require backfill.

### put

Inputs:

```text
app, table, rowJson
```

Algorithm:

1. Ensure app exists.
2. Load normalized table spec from KV.
3. Parse `rowJson` as a JSON object.
4. Validate required fields, field types, unknown field policy, and primary key.
5. Compute `pkKey` and base row key.
6. Load old row if present.
7. Compute old secondary index entries from old row, if any.
8. Compute new secondary index entries from new row.
9. Check unique indexes:
   - For each new unique key, read `uniq/<table>/<index>/<partitionKey>`.
   - If no value exists, OK.
   - If the value equals this row's primary key JSON, OK.
   - Otherwise reject the write before producing events.
10. Emit delete events for old index entries that are no longer present.
11. Emit set event for the base row.
12. Emit set events for new index entries.
13. Emit set events for new unique entries.
14. Emit delete events for old unique entries that are no longer present.

All events are returned together from `decide`. If validation fails, return an
error and produce no events.

### delete

Inputs:

```text
app, table, keyJson
```

Algorithm:

1. Ensure app exists.
2. Load table spec.
3. Parse and validate `keyJson`.
4. Compute `pkKey` and base row key.
5. Load old row.
6. If no row exists, return `Commit([])` for idempotent delete.
7. Compute secondary index and unique keys from old row.
8. Emit delete event for the base row.
9. Emit delete events for all old secondary index entries.
10. Emit delete events for all old unique entries.

## Read algorithms

### get

Inputs:

```text
app, table, keyJson
```

Algorithm:

1. Load table spec.
2. Parse `keyJson`.
3. Compute `pkKey`.
4. Read base row key from KV.
5. Return `ReadValue::OptString(rowJson)`.

`keyJson` can be either:

```json
{ "id": "u1" }
```

or the explicit tuple form:

```json
{ "partition": ["u1"], "sort": [] }
```

### query

Inputs:

```text
app, table, index, queryJson
```

Algorithm:

1. Load table spec.
2. Resolve `index` to either `"__primary"` or a declared secondary index.
3. Parse and validate query JSON.
4. Compute scan prefix or start/end range.
5. Use `terrane_cap_kv::scan_prefix` or `scan_range`.
6. For each index entry:
   - If projection is `keys`, read the base row by primary key.
   - If projection is `all`, use the index value.
   - If projection is `include`, use projected value unless the caller later
     asks for full rows.
7. Return `ReadValue::OptString(Some(rowsJsonArray))`.

MVP can support forward scans only.

### tables

Scan:

```text
__terrane/rdb/v1/tables/
```

Return a JSON array string sorted by table name.

### spec

Read:

```text
__terrane/rdb/v1/table/<table>/spec
```

Return normalized spec JSON or `null`.

## Schema evolution plan

Do not allow arbitrary `defineTable` overwrite.

Later explicit migration verbs:

```text
relational_db.createIndex <app> <table> <index> <indexSpecJson>
relational_db.backfillIndex <app> <table> <index> <limit>
relational_db.dropIndex <app> <table> <index> <limit>
```

Migration state can be stored under:

```text
__terrane/rdb/v1/migration/<table>/<migrationId>
```

Backfill must be chunked and bounded so one command cannot create unbounded
events. Primary key changes should remain unsupported.

## File update checklist

### Workspace

- `rust/Cargo.toml`
  - Add workspace member `crates/terrane-cap-relational-db`.
  - Add workspace dependency `terrane-cap-relational-db`.
  - Add `serde_json = "1"` as a workspace dependency unless the implementation
    proves `nanoserde` can safely handle arbitrary row JSON values.

### Shared capability interface

- `rust/crates/terrane-cap-interface/src/lib.rs`
  - Add `CapabilityDoc`, `ResourceMethodDoc`, `ParamDoc`, `ReturnDoc`,
    `SchemaDoc`, `ExampleDoc`, `ErrorDoc`, and `InternalDoc` structs.
  - Add `fn doc(&self) -> CapabilityDoc` to the `Capability` trait with a
    default generated from `namespace()` and `manifest()`.
  - Keep `manifest()` as the executable source for command/event/resource
    routing. `doc()` is descriptive and must not be used for dispatch.
  - Decide serialization support. Prefer `nanoserde` for consistency if it can
    represent the doc model cleanly; otherwise add a narrowly scoped JSON
    serializer dependency at the interface/API boundary.
- `rust/crates/terrane-cap-interface/src/doc.rs` if the doc model is too large
  for `lib.rs`
  - Hold doc structs and render options such as `DocFormat`, `DocDetail`, and
    `CapabilityDocRenderOptions`.
- `rust/crates/terrane-cap-interface/src/tests.rs`
  - Verify default docs are generated from a minimal manifest.
  - Verify `includeInternal = false` removes internal docs from rendered output.

### KV capability

- `rust/crates/terrane-cap-kv/src/lib.rs`
  - Add reserved prefix constant.
  - Add public event helper functions.
  - Add internal read/scan/range/delete-prefix helpers.
  - Add bounded `scan`, `range`, and `keys` resource reads.
  - Filter reserved keys from public `get`, `all`, `scan`, `range`, and `keys`.
  - Reject reserved keys in public `set`, `rm`, and `delete` command handling.
  - Add a doc override or enriched default docs explaining `scan`, `range`,
    reserved-prefix filtering, and the fact that `kv` is lower-level than
    `relational_db`.
- `rust/crates/terrane-cap-kv/src/tests.rs`
  - Unit tests for helper events, prefix scan, range scan, limits, and reserved
    key filtering.
- `rust/crates/terrane-cap-kv/tests/capability.rs`
  - Integration tests for public KV scan/range behavior and reserved key
    rejection.

### New relational DB crate

- `rust/crates/terrane-cap-relational-db/Cargo.toml`
  - Depend on `terrane-cap-interface`, `terrane-cap-kv`, `borsh`, and JSON parser
    dependency.
- `rust/crates/terrane-cap-relational-db/src/lib.rs`
  - Capability implementation, manifest, command/resource dispatch, read/write
    algorithms.
  - Implement `doc()` with complete app-author and agent-facing docs.
- `rust/crates/terrane-cap-relational-db/src/spec.rs`
  - Table spec structs, defaults, validation, canonical serialization.
- `rust/crates/terrane-cap-relational-db/src/table_spec.schema.json`
  - Machine-readable JSON Schema for `specJson`; examples and parser structs
    must be validated against this fixture.
- `rust/crates/terrane-cap-relational-db/src/query.schema.json`
  - Machine-readable JSON Schema for `queryJson`.
- `rust/crates/terrane-cap-relational-db/src/key.rs`
  - Table/index/key name validation, component encoding, tuple encoding,
    primary/index key builders.
- `rust/crates/terrane-cap-relational-db/src/row.rs`
  - Row parsing, field validation, projection, index-entry computation.
- `rust/crates/terrane-cap-relational-db/src/query.rs`
  - Query JSON parsing, limit validation, scan range planning.
- `rust/crates/terrane-cap-relational-db/src/doc.rs`
  - Build the full `CapabilityDoc` for `relational_db`, embedding schemas with
    `include_str!`, method docs, examples, limits, errors, and internal KV
    layout notes.
- `rust/crates/terrane-cap-relational-db/examples/users.js`
  - Canonical example used by docs, skill export, and tests.
- `rust/crates/terrane-cap-relational-db/src/tests.rs`
  - Focused unit tests for spec validation, key encoding, projections, unique
    conflicts, sparse indexes, query planning, and doc rendering.
- `rust/crates/terrane-cap-relational-db/tests/capability.rs`
  - Capability-level tests using a small fake `StateStore` containing `KvState`.

### Core

- `rust/crates/terrane-core/src/cap/mod.rs`
  - Re-export `terrane_cap_relational_db as relational_db`.
- `rust/crates/terrane-core/src/lib.rs`
  - Register `cap::relational_db::RelationalDbCapability` in
    `default_registry()`.
  - No new `State` field for MVP.
  - Add `capability_docs()`, `capability_doc(namespace)`, and
    `capability_doc_rendered(namespace, options)` helpers built from the
    registry's `doc()` methods.
- `rust/crates/terrane-core/tests/cap/main.rs`
  - Add `mod relational_db;`.
- `rust/crates/terrane-core/tests/cap/relational_db.rs`
  - Engine tests for table creation, put/get/query, unique rejection, delete,
    replay identity, and app removal cleanup through KV.
- `rust/crates/terrane-core/tests/cap/interface.rs`
  - Add registry-level doc tests: every registered capability has a doc,
    namespaces match, resource docs cover manifest resources, and internal docs
    are hidden unless requested.
- `rust/crates/terrane-core/tests/cap/host.rs`
  - Add a memory backend test with `resources: ["relational_db"]` proving
    JS read-after-write within one run.
  - Add a sandbox test proving undeclared `relational_db` is absent.
  - Update declared resource surface expectations through existing generated
    surface tests.

### API and MCP

- `rust/crates/terrane-api/src/lib.rs`
  - Add MCP tool constants `TOOL_CAPABILITIES_LIST` and `TOOL_CAPABILITY_INFO`.
  - Add both tool definitions to `mcp_tools()`.
  - Add wire structs for capability summaries and info arguments if useful for
    tests and public contract export.
  - Extend `PublicSurface` with capability-doc summaries or full docs.
- `rust/crates/terrane-host/src/mcp.rs`
  - Extend `CallArgs` or add a more specific parser for `capability_info`
    arguments: `namespace`, `format`, `detail`, `includeSchemas`,
    `includeInternal`.
  - Route `capabilities_list` to `terrane_core::capability_docs()` summaries.
  - Route `capability_info` to the canonical doc renderer.
  - Return tool-level `isError: true` for unknown namespaces or invalid formats,
    matching the existing `invoke` behavior.
- `rust/crates/terrane-host/src/mcp_tests.rs`
  - Assert `tools/list` advertises both new tools.
  - Assert `capability_info` for `relational_db` includes method docs and
    schemas by default.
  - Assert internal KV layout is absent by default and present with
    `includeInternal: true`.

### Host and CLI

- `rust/crates/terrane-host/src/cli.rs`
  - Add help text for `relational_db defineTable`, `put`, and `delete`.
  - Add help text for `kv scan`, `kv range`, and `kv keys` if exposed as public
    KV commands.
  - Add `terrane cap list`.
  - Add `terrane cap info <namespace>`.
  - Add `terrane cap info <namespace> --format json|markdown|skill`.
  - Add `terrane cap info <namespace> --include-internal`.
  - Add `terrane cap skill <namespace> --out <dir>`.
  - Update `state` output to skip reserved KV keys in the `kv:` section.
  - Optionally add a `relational_db:` section summarizing tables and row counts
    from reserved KV metadata.
- `rust/crates/terrane-host/tests/cap/kv.rs`
  - Add CLI smoke tests for bounded scan/range if CLI coverage exists there.
- `rust/crates/terrane-host/tests/cap/main.rs`
  - Add relational DB cap module if host cap smoke tests mirror core cap tests.
- `rust/crates/terrane-host/tests/cap/relational_db.rs`
  - E2E smoke test: add app, define table, put row, inspect log/state/output.
- `rust/crates/terrane-host/tests/cap/docs.rs`
  - CLI smoke tests for `cap list`, `cap info relational_db --format json`,
    `--format markdown`, `--format skill`, `--include-internal`, and skill
    directory export.

### Skill rendering

- `rust/crates/terrane-host/src/cap_doc.rs` or
  `rust/crates/terrane-core/src/cap/doc_render.rs`
  - Implement renderers for JSON, Markdown, and skill output from
    `CapabilityDoc`.
  - Keep filesystem writing in the host crate; keep pure rendering in core or
    interface.
  - Skill directory writer creates `SKILL.md`, `references/*.schema.json`, and
    `examples/*` from the doc model.
- Generated skill artifact shape:

```text
SKILL.md
references/table_spec.schema.json
references/query.schema.json
examples/users.js
```

### Public contract and docs

- `docs/APP_API.md`
  - Regenerate the generated resource table after adding KV and relational DB
    resource methods.
  - Add a short explanatory section for `relational_db`, including JSON strings
    returned by `get` and `query`.
  - Point app authors and agents at `terrane cap info relational_db` for the full
    canonical schema and examples.
- `docs/SERVER_API.md`
  - Document the new MCP tools `capabilities_list` and `capability_info`.
  - Document the relationship between `app_actions` and `capability_info`.
  - Document that public contract export includes capability docs from the same
    source of truth.
- `rust/crates/terrane-host/tests/contract.rs`
  - Update expected exported surface to include capability-doc summaries/full
    docs if the test has static fixtures.
  - Prefer deriving from `terrane-core::capability_docs()` and
    `terrane-core::resource_surface()` so docs cannot drift.

## Validation commands

Run narrow tests first:

```sh
cd /Users/vehasuwat/Project/terrane/rust
cargo test -p terrane-cap-kv
cargo test -p terrane-cap-relational-db
cargo test -p terrane-cap-interface
cargo test -p terrane-core relational_db
cargo test -p terrane-core interface
cargo test -p terrane-core host
cargo test -p terrane-host mcp
```

Regenerate generated docs after the resource surface changes:

```sh
cd /Users/vehasuwat/Project/terrane/rust
UPDATE_DOCS=1 cargo test -p terrane-core
```

Then broaden:

```sh
cd /Users/vehasuwat/Project/terrane/rust
cargo fmt --all
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p terrane-host -- contract export
cargo run -p terrane-host -- cap list
cargo run -p terrane-host -- cap info relational_db --format json
cargo run -p terrane-host -- cap info relational_db --format markdown
```

If `--locked` fails only because a new dependency such as `serde_json` updates
`Cargo.lock`, run the dependency update intentionally, inspect `Cargo.lock`, and
then re-run locked tests.

Manual MCP smoke after implementation:

```json
{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
```

Then call:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "capability_info",
    "arguments": {
      "namespace": "relational_db",
      "format": "json",
      "includeSchemas": true
    }
  }
}
```

## MVP acceptance criteria

- Apps can define a table, put rows, get rows, query a declared secondary index,
  and delete rows through `ctx.resource.relational_db`.
- The capability stores all durable state as ordinary `kv.set` and `kv.deleted`
  events.
- Replay identity passes after relational DB writes.
- Unique secondary indexes reject conflicting writes before any events commit.
- Secondary indexes update correctly when indexed fields change.
- Public KV APIs cannot see or mutate `__terrane/` reserved relational DB keys.
- Resource surface docs and contract export include `relational_db`.
- `CapabilityDoc` is the single source of truth for full cap docs; MCP, CLI,
  public contract, and generated skill output render from it.
- MCP `tools/list` advertises `capabilities_list` and `capability_info`.
- `capability_info` returns full `relational_db` method docs, schemas, examples,
  limits, and errors with `includeInternal = false` by default.
- `capability_info(... includeInternal = true)` includes reserved KV layout and
  write-amplification notes.
- CLI `terrane cap info relational_db --format json|markdown|skill` works and
  matches the canonical doc model.
- CLI skill export writes `SKILL.md`, schema references, and examples from the
  canonical doc model.
- Generated app docs point agents to `capability_info` / `terrane cap info`
  rather than duplicating long RDB schema text by hand.
- No direct filesystem, network, clock, random, or non-deterministic behavior is
  introduced.

## Deferred work

- Joins.
- Arbitrary SQL or PartiQL parser.
- Multi-table transactions.
- Online schema changes beyond explicit index backfill/drop.
- Reverse scans.
- Pagination cursors that are stable across concurrent writes.
- Remote registry publishing for generated capability skills.
- Version negotiation for capability docs between older MCP clients and newer
  Terrane hosts.
- CRDT merge semantics for relational rows. MVP is local last-writer-wins via
  KV event order.
