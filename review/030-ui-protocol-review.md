# Review: commit 52a3e121 (`forge-ui`)

Claude, I reviewed `52a3e121` against `prd-merged/05` and the current `@forge/std` contract.

## Finding

### P1 - Known `@forge/std` node fields are silently dropped by `forge-ui`

`forge/crates/ui/src/node.rs:43` defines the known node variants without the shared `id` / `testId` fields and without current std props such as `Stack.gap`, `Text.variant`, `Button.variant`, and `TextField.label` / `placeholder`. Serialization for known nodes then only re-emits the reduced fields (`node.rs:161`, `node.rs:168`, `node.rs:174`, `node.rs:183`, `node.rs:192`). Those fields are not future/unknown props: they are part of the committed applet API in `forge/std/forge-std.d.ts:97` through `forge/std/forge-std.d.ts:127`.

That means a valid applet tree like `{ type: "Button", testId: "save", label: "Save", variant: "primary", onTap: "save" }` parses successfully but loses `testId` and `variant` as soon as `forge-ui` deserializes/serializes or diffs it. This breaks the UI-12 wire contract and makes renderer/test harness wiring lose stable element handles that the T018 e2e fixtures already rely on (`testId` in rendered trees). UI-6 says unknown props on known nodes should not error, but these props are known in the M0a std surface and should be preserved/diffed or the std surface must be narrowed in the same commit.

Please add the BaseNode fields and current std scalar props to `Node`, include them in known-node serde, and extend golden tests to round-trip and patch at least `testId`, `gap`, `variant`, and `placeholder`.

## Verification

- `git show --check 52a3e121`
- `cargo test --locked -p forge-ui`
- `cargo clippy --locked -p forge-ui -- -D warnings`
