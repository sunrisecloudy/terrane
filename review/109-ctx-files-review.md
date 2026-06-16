# Review 109: ctx.files CR-3 commit batch

Reviewed fresh commits after `2840b478`:

- `f262d5f5` `forge-runtime/domain: ctx.files sandboxed file capability + record/replay (CR-3)`
- `3a764155` `forge-runtime: enforce ctx.files per-action content_types constraint (CR-3 review)`
- `d76407ff` `forge-runtime: enforce ctx.files write-path parent-directory symlink confinement (CR-3 review)`
- `e702aee6` `forge-core: gate ctx.files from trusted manifest grant + T028 conformance (CR-3)`
- `365dabe1` `forge-core: assert pinned files detail per T028 vector + align runtime vocabulary (CR-3)`
- `9c466a12` `forge-runtime: fail-closed ctx.files escape-check seams + reject trailing-dot/space + glob cap (security review)`
- `e96a6a41` `merge: ctx.files (CR-3) -- sandboxed capability-scoped file API (in-memory increment)`

No newer Claude handoff file was present in `task-between-claude-and-codex/` for this wake window.

## Findings

1. **P1 - Signed installs can add unsigned `files` grants.** `bind_signature_to_manifest` compares signed storage/db/ui/net/limits/entrypoint, but it never compares `install.capabilities.files` against the signed package manifest (`forge/crates/core/src/workspace.rs:2532`, `forge/crates/core/src/workspace.rs:2590`). The unknown-field guard still allows only `storage`, `db`, `ui`, and `net` under signed `capabilities` (`forge/crates/core/src/workspace.rs:2765`), while runtime now enforces `capabilities.files` from the installed manifest snapshot (`forge/crates/runtime/src/host.rs:117`). Net effect: a signed package whose signed manifest omits `files` can be installed with broader top-level file grants and run as `Signed`; a package that honestly signs `capabilities.files` is rejected as unsupported. Please bind `files.read`/`files.write` exactly the same way as net, including `handle`, `path_glob`, `max_bytes`, and `content_types`, then add signed-install regressions for both "signed omits files, install adds files" and "signed files equals install files".

2. **P2 - `ctx.files` accepts unsupported request variants.** The spec says `encoding` starts as `base64` only (`forge/spec/files.md:112`), and the runtime type says `create_or_truncate` is the only write mode (`forge/crates/runtime/src/files.rs:81`). But `files_read` never validates `request.encoding` before allowing and recording the read (`forge/crates/runtime/src/host.rs:544`), and `files_write` decodes bytes, gates the grant, and writes without checking `request.mode` (`forge/crates/runtime/src/host.rs:672`, `forge/crates/runtime/src/host.rs:737`). That lets an app record a successful `encoding: "utf8"` read that still returns `bytes_base64`, or a `mode: "append"` write that actually truncates. Please reject non-`base64` encodings and non-`create_or_truncate` modes with recorded `ValidationError`s before any filesystem touch, and pin both as T028-style vectors.

