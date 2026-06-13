---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/files.md, forge/fixtures/files/*.json, forge/fixtures/files/manifest.json
---

# T028 — ctx.files capability validation vectors (CR-3)

The next applet-facing capability after net/secrets is `ctx.files` (CR-3 in
prd-merged/05-runtime-prd.md / the capability surface in prd-merged/07): a
sandboxed, capability-scoped file read/write surface. Like net and db, every
file op is checked against a declared grant in the manifest BEFORE it touches the
host filesystem, and paths are confined to a per-applet root (no traversal,
no symlink escape, no absolute-path escape).

I want a spec + vectors so a follow-on workflow can wire `ctx.files` into the
runtime HostBridge + core the same way net/secrets were wired.

## Deliverables

1. `forge/spec/files.md` — derive from prd-merged/05 (runtime capability model),
   prd-merged/07 (SC capability/grant model), and the existing net wiring
   (`forge/crates/runtime/src/` net host call + `forge/crates/core/src/workspace.rs`
   net grant gate) as the structural precedent. Define: the manifest grant shape
   (`files: { read: [globs], write: [globs] }` or similar — propose the shape and
   justify it), the per-applet sandbox root, the confinement rules (reject `..`
   traversal, absolute paths outside root, symlink targets outside root), the
   host-call request/response envelope, and the deterministic record/replay
   contract (a recorded file read replays its recorded bytes — same as net, so
   replay stays byte-identical offline).

2. `forge/fixtures/files/<case>.json` + manifest — each: a manifest grant, an
   incoming file op, expected outcome. Example:
   ```json
   { "case": "read_outside_grant_rejected",
     "grant": { "read": ["data/*.json"], "write": [] },
     "op": { "kind": "read", "path": "data/../secrets.txt" },
     "expect": "rejected", "reason": "path escapes grant root" }
   ```

## Coverage (~12)

granted read -> allowed; read outside grant glob -> rejected; write without write
grant -> rejected; `..` traversal -> rejected; absolute path -> rejected; symlink
escaping root -> rejected; nested allowed path -> allowed; write then read-back
within grant -> allowed + bytes match; read of a missing file -> a clean
not_found error (not a panic); a path with special chars within grant -> allowed;
a deterministic replay vector (recorded read replays identical bytes); an op whose
declared grant is absent from the manifest entirely -> rejected.

## Result

(codex fills this in)
