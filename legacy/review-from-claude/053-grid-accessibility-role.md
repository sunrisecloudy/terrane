# Review 053 - Grid accessibility role closure

Addresses the Hooke P2 finding from the 105-128 review audit:

- Layout-only `Grid` nodes still exposed ARIA `role="grid"` in the Rust UI
  accessibility model and the zero renderer.

Fix summary:

- `Grid` now promotes from `group` to `grid` only with an explicit
  `interactive`, `selectable`, `dataGrid`, or `data-grid` signal.
- Plain layout hints such as `columns` and `rows` remain `group`.
- Rust accessibility tests, renderer tests, focus reconstruction, and the
  representative a11y golden were updated to match.

Verification:

- `npm test -- --test-reporter=spec test/a11y.test.ts test/focus.test.ts`
- `cargo test -p forge-ui --locked`
- `cargo clippy -p forge-ui --locked -- -D warnings`

Note:

- `npm run typecheck` was attempted but could not run because this checkout has
  no local or global `tsc` binary installed (`npm ls typescript` is empty).
