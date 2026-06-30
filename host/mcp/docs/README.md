# Terrane MCP Guide

Terrane MCP is the agent-facing control surface for one local `TERRANE_HOME`.
It is exposed by the stdio host in this crate and by the web host at `POST /mcp`.
Both transports use the same shared implementation in `terrane-host`.

This directory owns the overall MCP manual:

- how clients connect
- which MCP concepts Terrane uses
- the app-building workflow
- guarded capability operation
- security and permission guidance
- weak-model and no-source operation

Capability semantics do not live here. Each capability owns its own document in
its `terrane-cap-*` crate through `Capability::doc(include_internal)`. MCP serves
those capability-owned docs through `terrane://capabilities/{namespace}` and the
`capability_info` tool.

## MCP Model

Terrane uses three MCP surfaces:

- Tools are model-called actions. Use them for app registration, app invocation,
  capability queries, and guarded capability commands.
- Resources are read-only documentation and operational context. Use them for
  the host MCP manual, capability docs, workflow recipes, and app action docs.
- Prompts are user-invoked workflows. Use them to guide app creation, app
  registration, action inspection, and safe capability commands.

## Start Here

For app building:

1. Read `terrane://docs/app-building`.
2. Call `workflows_list`.
3. Call `workflow_info` for `make_js_kv_app_no_filesystem`.
4. Use `app_scaffold`; pass `withUi: true` for visible apps such as calendars,
   dashboards, forms, and natural-language input pages.
5. Use `app_register_inline` with `dryRun: true`.
6. Commit with `app_register_inline`.
7. Inspect with `app_actions`.
8. Act with `invoke`.

Pass `app_register_inline.files` as the actual `structuredContent.files` array,
not as a JSON string. On registration retry, include the complete bundle file
array again, including `manifest.json`, backend, UI, and assets.

For UI apps, the browser bridge is
`window.terrane.invoke("verb", "arg1", "arg2")`. Do not pass an array for
multiple backend arguments. Keep complex page behavior in `ui.js`, and verify
the page itself when the requested outcome is an interactive app.

For generated JS apps that maintain optional KV indexes, use a helper such as
`kvGetOrNull(kv, "event_ids")` and default missing index keys to an empty
array before parsing.

For capability operation:

1. Read `terrane://docs/capability-operations`.
2. Read `terrane://capabilities/{namespace}` or call `capability_info`.
3. Prefer `capability_query` for reads.
4. Use `capability_command` only after `help: true` and, when supported,
   `dryRun: true`.

## Resource Index

- `terrane://docs/index`
- `terrane://docs/clients`
- `terrane://docs/app-building`
- `terrane://docs/capability-operations`
- `terrane://docs/security`
- `terrane://docs/weak-models`
- `terrane://capabilities/{namespace}`
- `terrane://workflows/{name}`
- `terrane://apps/{id}/actions`

## Boundary Rule

`host/mcp` explains how to operate Terrane over MCP. Capability crates explain
what a capability means, which commands and queries it owns, which events it
records, which app resources it grants, and what constraints apply.
