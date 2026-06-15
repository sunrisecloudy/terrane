# Review 008 — forge std net/files types

- **Slice goal:** Close the follow-up from the Forge examples slice by typing the implemented `ctx.net` and `ctx.files` applet APIs in `@forge/std`.
- **Reviewed:** working diff updating `forge/std/forge-std.d.ts` and removing temporary `(ctx as any)` usage from the Forge examples.
- **Files changed:** `forge/std/forge-std.d.ts`, `forge/examples/api-dashboard/src/main.ts`, `forge/examples/file-transformer/src/main.ts`.
- **Review mode:** independent Codex/self-review. Claude Code Opus review remains waived by the user instruction on 2026-06-15 to work independently from Claude Code.
- **Commands run:** `cd forge && cargo test -p forge-cli --test forge_examples --locked` -> passed; `node --test --no-warnings tools/reference-host/test/public-contract.test.js` -> passed; `node --no-warnings tools/export-public-contract.mjs --out /private/tmp/terrane-public-contract.json` -> passed; `node --no-warnings tools/verify-public-contract.mjs --contract /private/tmp/terrane-public-contract.json --root /Users/vehasuwat/Project/terrane` -> passed, `filesChecked=457`.

## Findings

No blocking findings.

- [P3] The std declarations are still hand-maintained rather than generated from Rust request/response structs. Resolution: acceptable for this narrow slice; future public-contract work can add schema/codegen for applet APIs.

## Resolution status

- Added `Net`, `NetRequest`, `NetResponse`, `Files`, `FileReadRequest`, `FileReadResponse`, `FileWriteRequest`, and `FileWriteResponse` to `@forge/std`.
- Updated `api-dashboard` and `file-transformer` examples to call `ctx.net.fetch` and `ctx.files.write` directly.

## Follow-ups

- Add a typecheck sidecar once CR-15 / tsgo integration is resumed.
