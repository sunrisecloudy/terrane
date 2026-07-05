# Capability: `document` — app-owned document storage

New crate `rust/crates/terrane-cap-document/`, namespace `document`, registered
in `default_registry`. Gives apps a first-class home for notes, drafts, and
generated content: today a "document" is either a JSON string wedged into `kv`
(no title, no metadata, no append) or a full Loro doc in `crdt` (merge
machinery an app with one writer never needs). The namespace already exists as
a **planned doc** (`terrane-core/src/planned_docs/document.rs`) with frozen
names — this spec keeps those exactly: commands `document.create`,
`document.patch`, `document.append`, `document.delete`; events
`document.created`, `document.patched`, `document.deleted`.

## Recommended decision

**Field-level patch, text body.** A document is
`{ id, title, body, metadata }` where `body` is a plain string (markdown
encouraged, any text allowed) and `metadata` is a small JSON object.
`document.patch` takes `{title?, body?, metadata?}`: `title` and `body`
replace wholesale; `metadata` applies **RFC 7386 JSON merge-patch** (set keys,
`null` deletes a key). Incremental body growth goes through
`document.append`, which is its own command precisely so logs and transcripts
never rewrite the whole body in the event payload. Text-splice patches
(offset/length edits) are rejected for v1 — they are the first step down the
OT/CRDT road, and that road already exists: **`document` is single-writer
simple storage; `crdt` is collaborative.** An app that needs concurrent
editors or replica merge uses `crdt`; `document` state is last-write-wins per
field and does not participate in `terrane sync` (which is crdt-delta-only).

## Capability surface

App-scoped documents keyed by `id` matching
`^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$` (the existing
`document_id.schema.json`). Fold keeps `app → id → Document`.

### Commands

| Command | Args | Decision |
| --- | --- | --- |
| `document.create` | `app, id, title, body, metadataJson?` | Validate id/limits/metadata JSON object; pure `Decision::Commit([document.created])`. Create-or-replace (matches the planned doc). |
| `document.patch` | `app, id, patchJson` | Missing document is a typed error; validate patch against `document_patch.schema.json` semantics; emit `document.patched` carrying the patch, not the merged result. |
| `document.append` | `app, id, text` | Missing document / body-size errors; emits `document.patched` with payload `{id, append: text}` — same event kind as the planned doc declares (no fourth event). |
| `document.delete` | `app, id` | Emit `document.deleted` if the id exists; deleting a missing id is a no-op success. |

### Events

| Kind | Payload (borsh) | Fold |
| --- | --- | --- |
| `document.created` | `{ app, id, title, body, metadata_json }` | upsert whole record |
| `document.patched` | `{ app, id, title?, body?, metadata_patch_json?, append? }` | apply field replaces, merge-patch metadata, append to body — pure string/JSON ops, deterministic |
| `document.deleted` | `{ app, id }` | drop record |
| (reacts) `app.removed` | — | drop all documents for the app |

### Resource methods (JS: `ctx.resource.document`)

Exactly the planned surface: `create(id, title, body, metadataJson)`,
`patch(id, patchJson)`, `append(id, text)`, `delete(id)` (writes routing to
the commands above), and reads `get(id)` → document JSON or null,
`list()` → `[{id, title, bodyBytes, updatedSeq?}]` (no bodies — keep listings
cheap), `exportMarkdown(id)` → the raw body string. Reads are pure folded-state
reads, never recorded. Grant resource: `document` namespace-v1 with
`read` + `write` verbs, described as "app-owned document storage" — flows
through the existing auth prompts unchanged.

### Host/CLI surface

`terrane document ls|get|rm <app> ...` as thin adapters over the query/command
surface. No dedicated web route in v1 — UIs read through
`window.terrane`/backend verbs like every other resource.

## Persistence & replay

- `DocumentState` is a new slice in `terrane_core::State` (one `BTreeMap`
  chain), folded entirely from the log — **no physical projection in v1**. The
  planned doc's internal note floated projecting onto reserved kv prefixes;
  rejected: the whole point of the capability is escaping kv's shape, and a
  `sync_logical_store`-style SQLite projection (the pattern auth uses) can be
  added later without any event change.
- All decisions are pure `Commit` — no effects, so replay identity is the
  ordinary fold property. Bodies live in event payloads; the 1 MiB body cap is
  what keeps that sane. Binary assets stay out entirely — that is `blob`'s job
  ([cap-blob.md](cap-blob.md)).

## Limits (planned-doc values, enforced in decide, documented in `doc.rs`)

- `maxBodyBytes` 1 048 576 (create, patch, and post-append size).
- `maxMetadataBytes` 16 384 (serialized, post-merge). Title ≤ 256 chars.
- `maxDocumentsPerApp` 10 000 — typed error.

## Implementation plan

1. **Crate:** `terrane-cap-document` — `types.rs` (Document, DocumentState),
   `lib.rs` (Capability impl: manifest, decide, fold, resource reads,
   describe), `events.rs` (payloads + constructors), `doc.rs`. Move the
   schemas/examples out of `planned_docs/document/` and reuse them.
2. **State:** add the `document` slice to `terrane_core::State` +
   `StateStore` match arms; register in `default_registry`.
3. **Retire the planned doc:** delete
   `terrane-core/src/planned_docs/document.rs` and its `mod.rs` wiring —
   `capability_doc` prefers planned docs, so leaving it would shadow the live
   capability. Drop the "feature-detect" constraint from the doc text.
4. **Surfaces:** `APP_API.md` regeneration picks up `ctx.resource.document`;
   scaffold/app-recipe resources list gains `document`; CLI subcommands.
5. **Tests:** engine tests `terrane-core/tests/cap/document.rs`
   (create/patch/append/delete round-trip, merge-patch semantics incl. `null`
   delete, limits, quota, app.removed, replay identity); e2e
   `terrane-host/tests/cap/document.rs` (JS backend via `ctx.resource.document`,
   default-run, no network).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Explicit non-goals (v1)

Text-splice/positional edits, concurrent-writer merge (use `crdt`),
document sync (crdt-only today), version history, binary bodies (use `blob`),
full-text search (the `search` cap), physical storage projection.

## Decisions to confirm

- **Patch semantics** — field replace for title/body + RFC 7386 merge-patch
  for metadata — alternative: whole-document JSON merge-patch, or text-splice
  body edits (rejected as crdt-shaped).
- **Body model** — plain string body, JSON only in `metadata` — alternative:
  first-class JSON bodies with merge-patch (pushes `document` towards kv/query
  territory).
- **Persistence** — folded state only, no physical projection in v1 —
  alternative: reserved SQLite projection via `sync_logical_store` for
  external inspection (additive later).
- **Append event shape** — `document.append` emits `document.patched` with an
  `append` field (keeps the planned three-event surface) — alternative: a new
  `document.appended` kind (cleaner but changes the frozen planned event list).
