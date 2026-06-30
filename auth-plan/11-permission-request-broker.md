# Permission Request Broker

This document captures how `host/cli`, `host/mcp`, `host/web`, and the macOS
host should ask for permission without making any one adapter the auth backend.

## Decision

`host/cli` and `host/mcp` should not call `host/web` for permission logic.

They should call a shared permission broker in `terrane-host`:

```text
host/cli
host/mcp
host/web
host/macos
  -> terrane-host permission broker
      -> auth capability / reserved KV
      -> selected trusted UI surface
```

`host/web` is the default dev UI adapter. It is not the owner of auth policy.

## Why

This keeps the existing Terrane boundary:

```text
terrane-host owns shared host behavior
host/cli is a thin command adapter
host/mcp is a thin JSON-RPC / MCP adapter
host/web is a thin HTTP and web UI adapter
host/macos is a thin native UI adapter
```

The permission broker belongs with shared host behavior because every adapter
needs the same request lifecycle, persistence, audit, and admin-surface routing.

## Broker Responsibilities

The broker handles human-in-the-loop permission requests.

Responsibilities:

- inspect requested app resources;
- create a permission request record;
- store it through auth/reserved KV;
- choose the best trusted UI surface;
- open or return that surface;
- support polling or resuming after approval;
- never grant permission by itself without an admin decision;
- never expose admin credentials or Premium tokens to generated apps.

The broker does not render the admin UI. It only routes to one.

## Request Sources

Permission requests can be created by:

- App Builder preview;
- app install/register;
- CLI invoke or run when a required resource is missing;
- MCP `invoke`, `app_register`, or `app_register_inline`;
- macOS app runtime;
- web app runtime;
- future Premium sync/import conflicts.

Each request records its source.

```text
source = cli | mcp_stdio | mcp_http | web | macos | app_builder | premium_sync
```

## Request Shape

Permission request records should be explicit enough for UI and audit.

```text
request_id
org
subject
app
app_name
source
operation
resources[]
status
created_at
expires_at
decision_by
decision_at
decision_reason
resume_token_hash optional
```

Resource rows:

```text
namespace
selector
verbs
selector_schema_id
summary
capability_doc_ref
```

For local v1, `selector` can be namespace-level while preserving the detailed
shape for later selectors.

## Request Status

```text
pending
approved
denied
expired
cancelled
superseded
```

`approved` creates or points to a grant record.

`denied`, `expired`, and `cancelled` do not create grants.

`superseded` is used when a newer request replaces an older equivalent request.

## UI Surface Selection

The broker chooses where the human reviews the request.

Configuration:

```text
TERRANE_PERMISSION_UI=web
TERRANE_PERMISSION_UI=mac
TERRANE_PERMISSION_UI=print
TERRANE_PERMISSION_UI=none
```

Default:

```text
dev/local source tree -> web
packaged mac install -> mac
headless or CI -> print
```

`web` opens or prints:

```text
http://127.0.0.1:<port>/__terrane/admin/requests/<request-id>
```

`mac` opens the installed native app to the same request, for example:

```text
terrane://admin/requests/<request-id>
```

`print` never opens a browser or native app. It returns the URL or request id.

`none` fails closed and returns permission denied / permission required.

## Dev Default

In development, default to web admin because it is easiest to inspect and test.

Flow:

```text
1. CLI/MCP sees permission is missing.
2. Adapter calls terrane-host permission broker.
3. Broker creates pending request in auth storage.
4. Broker chooses web.
5. Broker returns admin URL.
6. Interactive CLI may open the URL and wait.
7. MCP returns structured permission_required.
8. User approves or denies in /__terrane/admin.
9. CLI/MCP retries or resumes.
```

The web server may already be running. If not, the broker can either start a
local web admin server or return instructions to start it. Starting a server is a
host decision, not an auth policy decision.

## CLI Behavior

CLI can be more interactive than MCP.

Interactive mode:

```text
1. Create permission request.
2. Open selected UI when allowed.
3. Print request URL.
4. Wait for decision with timeout.
5. Continue on approval.
6. Exit clearly on deny/timeout.
```

Non-interactive mode:

```text
1. Create permission request.
2. Print request id and URL.
3. Exit with permission-required status.
```

Suggested flags:

```text
--permission-ui web|mac|print|none
--permission-wait
--permission-timeout <seconds>
--no-open
```

CLI must not silently auto-grant requested resources.

## MCP Behavior

MCP should not block forever waiting for a user.

When a tool call needs a grant, return structured pending data:

```json
{
  "status": "permission_required",
  "requestId": "req_...",
  "adminUrl": "http://127.0.0.1:8780/__terrane/admin/requests/req_...",
  "resume": {
    "tool": "permission_check",
    "arguments": {
      "requestId": "req_..."
    }
  }
}
```

Add MCP tools:

```text
permission_check
permission_cancel
permission_requests
```

`permission_check` returns:

```text
pending | approved | denied | expired | cancelled
```

On `approved`, the original app operation can be retried by the client. Later,
the broker can support a server-side resume token, but v1 can keep retry simple.

## Web Behavior

`host/web` serves the trusted admin route:

```text
/__terrane/admin
/__terrane/admin/requests/<request-id>
```

It calls the same broker/admin APIs as other host adapters.

`host/web` may expose protected HTTP endpoints for the admin UI, but those
endpoints should dispatch to shared host/auth code rather than implement policy
directly inside `host/web`.

## macOS Behavior

The macOS host is the preferred packaged UI surface.

Packaged install behavior:

```text
1. Broker chooses mac.
2. Adapter opens Terrane.app with request id.
3. Terrane.app focuses the admin request detail.
4. Approval writes through shared host/auth code.
```

The native app can use a custom URL, an IPC channel, or an FFI entry point. The
auth decision still belongs to shared host/auth behavior.

## Reserved KV Storage

Permission request storage belongs under the auth reserved prefix:

```text
__terrane/auth/v1/orgs/<org>/permission_requests/<request-id>
__terrane/auth/v1/orgs/<org>/permission_requests_by_app/<app>/<request-id>
__terrane/auth/v1/orgs/<org>/permission_requests_by_subject/<subject>/<request-id>
```

Generated apps cannot read or write these records through public KV.

## Security Rules

- Broker creates requests, not grants.
- Only an admin-capable session can approve or deny.
- Agent subjects cannot grant themselves new authority.
- MCP clients receive request ids and admin URLs, not admin tokens.
- Web admin endpoints must remain under trusted `__terrane` routes.
- Non-loopback web admin requires host auth.
- Headless mode must fail closed or return pending, not auto-approve.
- Approval must produce audit records.

## API Sketch

Shared host API shape:

```text
PermissionBroker::request_permission(input) -> PermissionRequestOutcome
PermissionBroker::check_request(request_id) -> PermissionRequestStatus
PermissionBroker::cancel_request(request_id) -> PermissionRequestStatus
PermissionBroker::admin_url(request_id) -> Option<String>
```

Input:

```text
org
subject
app
operation
resources[]
source
interactive
ui_preference
wait_policy
```

Outcome:

```text
already_granted
permission_required { request_id, admin_url, selected_ui }
approved { grant_ids }
denied { request_id, reason }
expired { request_id }
ui_unavailable { request_id, fallback }
```

## Integration Points

CLI:

```text
before app invoke/install/register commits requested runtime access
```

MCP:

```text
invoke
app_register
app_register_inline
future permission_check tool
```

Web:

```text
preview create
preview invoke
installed app invoke
admin request detail route
```

macOS:

```text
preview invoke
installed app invoke
native admin request detail
```

## Acceptance Criteria

- `host/cli` and `host/mcp` do not call `host/web` for auth decisions.
- All adapters create permission requests through shared `terrane-host` broker.
- Dev default returns or opens a web admin URL.
- Packaged mac mode opens Terrane.app to the request.
- MCP returns `permission_required` instead of blocking indefinitely.
- CLI interactive mode can wait for approval with a timeout.
- CLI non-interactive mode prints request id/URL and exits clearly.
- Generated apps cannot read request records or admin APIs.
- Approval writes normal grant records and audit events.
- Denial leaves runtime access absent.
