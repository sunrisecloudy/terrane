# Files Capability Scenarios

Source of record: `prd-merged/01-core-runtime-prd.md` CR-1/CR-3/CR-4/CR-8/CR-9,
`prd-merged/07-security-prd.md` SC-1/SC-8/SC-10/SC-12, and the planned
`files` row in `forge/spec/capabilities.md`. Current structural precedent:
`ctx.net.fetch` is capability-gated in the runtime host layer, recorded in
`RunRecord`, and replayed from the recorded response without touching the live
bridge.

This document pins the initial `ctx.files` contract before runtime/core wiring
lands. It is intentionally narrower than the final file command surface in
CR-A2: this is the applet-facing sandbox namespace, not shell/admin
`file.write/history/restore_version` commands.

## Model

`ctx.files` exposes file reads and writes only through user-granted handles. A
handle is a stable logical id that the shell/workspace policy maps to a
per-applet sandbox root. The applet manifest may request paths beneath a handle,
but it never names a native absolute filesystem root.

Proposed manifest grant shape:

```json
{
  "capabilities": {
    "files": {
      "read": [
        {
          "handle": "workspace_data",
          "path_glob": "data/**/*.json",
          "max_bytes": 65536,
          "content_types": ["application/json"]
        }
      ],
      "write": [
        {
          "handle": "workspace_data",
          "path_glob": "drafts/*.txt",
          "max_bytes": 65536,
          "content_types": ["text/plain"]
        }
      ]
    }
  }
}
```

Rationale:

- `read` and `write` are separate arrays so review UI can show exactly which
  operations are being requested.
- `handle` models SC-8's user-granted handle requirement without letting the
  manifest choose a host root.
- `path_glob` is matched against the normalized relative path inside the handle.
  `*` matches one path segment fragment; `**` may cross segment boundaries.
- `max_bytes` and `content_types` are per-action constraints, not comments. They
  must be enforced before a read response or write payload is accepted.

## Host Calls

Initial namespace:

```ts
ctx.files.read(request)
ctx.files.write(request)
```

Read request:

```json
{
  "handle": "workspace_data",
  "path": "data/settings.json",
  "encoding": "base64"
}
```

Read response:

```json
{
  "path": "data/settings.json",
  "bytes_base64": "eyJvayI6dHJ1ZX0=",
  "size": 11,
  "content_type": "application/json"
}
```

Write request:

```json
{
  "handle": "workspace_data",
  "path": "drafts/note.txt",
  "bytes_base64": "ZHJhZnQgdjE=",
  "content_type": "text/plain",
  "mode": "create_or_truncate"
}
```

Write response:

```json
{
  "path": "drafts/note.txt",
  "written_bytes": 8,
  "version": "file_version_1"
}
```

`encoding` starts as `base64` only. A later text helper may wrap this, but the
recorded host response must be byte-exact and engine-independent.

## Gates

Every file call must pass all gates before touching the host filesystem:

- The actor role may run the applet.
- The manifest requests `capabilities.files.<read|write>` for the handle.
- The trusted workspace/user grant maps that handle to a per-applet root.
- The supplied path is a relative POSIX path, not empty, not absolute, not a URI,
  not a Windows drive path, and contains no NUL.
- Any `.` segments are removed and any `..` segment is rejected before join.
- The normalized path matches the action's `path_glob`.
- The resolved target stays under the handle root after symlink resolution.
- For writes, the canonical parent directory stays under the root and the final
  target must not be a symlink escape.
- Byte and content-type limits are enforced before returning or committing data.
- Denials are recorded as denial-shaped host calls, matching the net/db replay
  model.

## Trace And Replay

In record mode, allowed calls append `RecordedCall` entries:

```json
{
  "method": "files.read",
  "args": [
    {
      "handle": "workspace_data",
      "path": "data/settings.json",
      "encoding": "base64"
    }
  ],
  "response": {
    "path": "data/settings.json",
    "bytes_base64": "eyJvayI6dHJ1ZX0=",
    "size": 11,
    "content_type": "application/json"
  }
}
```

Replay serves the recorded response and must not consult the live filesystem.
This keeps deterministic runs byte-identical offline even when the file has
changed, gone missing, or is no longer granted. Recorded write responses are
served during replay as well; replay must not create, truncate, or modify live
files.

## Error Vocabulary

Use stable `CoreError` variants, with machine-testable detail strings:

| Case | Error |
|---|---|
| missing `files` capability or action grant | `CapabilityRequired` |
| path traversal, absolute path, URI, NUL, symlink escape | `PermissionDenied` |
| path outside granted glob | `CapabilityRequired` |
| missing file under an otherwise valid read grant | `StorageError` with `not_found` detail |
| max bytes or content type exceeded | `ResourceLimitExceeded` or `PermissionDenied` |

## Fixture Suite

The fixtures in `forge/fixtures/files/` are data-only validation vectors. They
model manifest grants, trusted handle resolution, an incoming file op, and the
expected outcome so follow-on runtime/core work can wire `ctx.files` without
inventing policy behavior.
