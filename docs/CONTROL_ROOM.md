# Control Room

Control Room is Terrane's built-in, user-facing management catalog. It answers
two related questions without becoming a general-purpose admin console:

1. Which apps are installed, what are they for, and what do they declare?
2. Which Terrane capabilities exist, who owns them, how can they be used, and
   what permission or safety boundary applies?

The first version is deliberately read-only. It declares only the
`control-room` resource and exposes no mutation, grant, approval, install,
secret-reveal, or cross-app invocation action.

## Information model

The catalog covers:

- installed and built-in apps: identity, purpose, version, runtime, UI/backend
  presence, declared resources, browser permissions, interfaces, public verbs,
  safe per-app data counts, grants, and help pointers;
- every registered capability: purpose, owning crate, category, registry
  availability, documented status, commands, queries, events, app resource
  methods, grant requirements, command policy classification, safe aggregate
  state where one is explicitly implemented, and capability documentation;
- MCP: the compiled tool table and documentation resources, including the
  app-discovery/action tools and capability operations;
- permissions: app, namespace, verbs, selector schema, and status, while
  selector details and resource identifiers remain redacted;
- model/runtime integration: registered local model ids, backend/format,
  generation-versus-embedding kind, size, and defaults, without paths, prompts,
  responses, or weights;
- storage: counts and byte totals for KV values, documents, blobs, and recorded
  MCP results, never the values or records themselves;
- connections: name, kind, authorization state, scopes, and expiry only.

Search, tabs, category filtering, cards, and drill-down keep this broad catalog
navigable.

## Live facts versus static contract

Every catalog slice carries a `factKind`.

- `live-registry`, `live-folded-state`, and `live-safe-aggregate` come from the
  Core instance that ran the app.
- `live-host-discovery` and sanitized manifest facts come from the host scanning
  its configured built-in and installed app roots and reading only public fields
  in each current `manifest.json`. Discovery does not catalog or execute an app.
- `static-contract` comes from compiled capability/MCP documentation.
- `explicit-unavailable` means Control Room intentionally did not probe or
  infer the fact.

Control Room does not execute another app to claim runtime health. App action
discovery remains available through MCP `app_actions`; the catalog reports that
boundary instead of silently running app code.

## Privacy boundary

The `control-room.catalog()` resource excludes by construction:

- passwords, tokens, master passwords, secret bytes, and connection transport
  configuration;
- grant selector JSON and resource identifiers;
- raw KV keys/values, document titles/bodies/metadata, blob bytes, and
  relational/CRDT records;
- private chat text, local-model prompts/responses, builder prompts/files, job
  arguments/output, and MCP arguments/results;
- Terrane home paths and app source paths.

Only counts, sizes, health/status labels, and other safe metadata are returned.

## App and MCP contract

The app id is `control-room`.

App actions:

- `overview` — system counts, catalog source labels, and the privacy boundary;
- `catalog` — the complete metadata-only snapshot;
- `search <query>` — matching apps, capabilities, and MCP tools;
- `common.list` / `common.get` — standard item-interface projections.

Over MCP:

1. call `list_apps`;
2. call `app_actions` with `{"app":"control-room"}`;
3. if the read-only resource is not granted, hand the returned permission
   request to a trusted user/admin;
4. call `invoke` with `{"app":"control-room","verb":"catalog"}`.

The model/requesting client cannot self-grant.

## Sidebar contribution

The app feature-detects the pending generic host-owned lower-sidebar API. Where
available it contributes Overview, Apps, Capabilities, and MCP & operations
navigation. On hosts without that API it explicitly degrades to the same
in-page tabs. No host sidebar implementation is duplicated in this change.
