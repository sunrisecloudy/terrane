# Geo Capability Implementation Notes

## Files changed

- New crate: `rust/crates/terrane-cap-geo/`
  - `src/lib.rs`: capability manifest, decide/fold/query/resource surfaces, `observed_event()`.
  - `src/types.rs`: `GeoState`, `GeoFix`, precision parsing, integer e7 coarse rounding, JSON output helpers.
  - `src/doc.rs`: explicit capability docs.
  - `tests/capability.rs`: public capability-surface integration tests.
- Shared wiring:
  - `Cargo.toml`, `Cargo.lock`
  - `rust/crates/terrane-cap-interface/src/abi.rs`
  - `rust/crates/terrane-core/Cargo.toml`
  - `rust/crates/terrane-core/src/lib.rs`
  - `rust/crates/terrane-host/Cargo.toml`
  - `rust/crates/terrane-host/src/edge.rs`
  - `rust/crates/terrane-host/src/public_authz.rs`
- Tests/docs:
  - `rust/crates/terrane-core/tests/cap/geo.rs`
  - `rust/crates/terrane-core/tests/cap/main.rs`
  - `rust/crates/terrane-core/tests/cap/interface.rs`
  - `rust/crates/terrane-host/tests/cap/geo.rs`
  - `rust/crates/terrane-host/tests/cap/main.rs`
  - `rust/crates/terrane-host/tests/public_authz.rs`
  - `docs/APP_API.md`

## Key design choices

- `geo.locate` and `ctx.resource.geo.current(precision)` return `Decision::Effect(Effect::GeoLocate { app, precision })`; the edge returns `geo.observed`.
- `ctx.resource.geo.peek(precision)` uses `Decision::TransientEffect`, returns the same fix JSON shape, and records nothing.
- Recorded fixes use integer e7 degrees: `lat_e7`, `lon_e7`, `accuracy_m`, `precision`, `observed_at`.
- Coarse precision rounds to 0.01 degrees (`100_000` e7 units) and raises `accuracy_m` to at least `1000`.
- Fold keeps the newest 20 fixes per app and reacts to `app.removed`.
- `describe()` redacts coordinates and prints only app, precision, accuracy, and observed time.
- CLI edge returns a typed unsupported error for `GeoLocate`; host e2e proves no `geo.observed` event is recorded on unsupported acquisition.

## Deviations from the plan

- The current auth grant runtime checks namespace grants only (`auth.grant ... geo`), not per-grant selector payloads. This implementation exposes `precision` as an explicit command/resource argument and validates it as `exact | coarse`. The rounding helper and event constructor still ensure only the requested precision enters `geo.observed`.
- The rate limit is enforced deterministically while folding/committing a `geo.observed` event by comparing the incoming recorded `observed_at` to the newest folded fix. Doing it in `decide` would require a live clock read inside deterministic core logic.
- Native CoreLocation and web-shell `navigator.geolocation` edge providers are not implemented in this slice; the CLI unsupported path is wired and tested. The ABI and capability crate are ready for host-specific edges to construct `observed_event()` after applying precision.

## Test evidence

- Rounding and typed validation:
  - `terrane-cap-geo::coarse_rounding_happens_as_integer_e7_math`
  - `terrane-cap-geo::invalid_precision_and_missing_app_are_typed_errors`
- Recorded and transient surfaces:
  - `terrane-cap-geo::locate_and_peek_return_recorded_and_transient_effects`
  - `terrane-core::geo_current_records_rounded_observation_and_replays`
  - `terrane-core::geo_peek_returns_value_but_records_nothing`
- Replay identity, truncation, rate limit, app removal:
  - `terrane-cap-geo::fold_keeps_last_twenty_and_last_resource_returns_json`
  - `terrane-cap-geo::fold_rejects_recorded_fixes_inside_rate_window`
  - `terrane-cap-geo::app_removed_drops_fixes_and_supports_defaults_false`
  - `terrane-core::geo_fold_replay_identity_truncation_rate_limit_and_app_removed`
- Security/log redaction and unsupported edge:
  - `terrane-cap-geo::describe_redacts_coordinates`
  - `terrane-host::geo_cli_reports_unsupported_and_records_no_observation`
- Registry/docs/authz integration:
  - `terrane-core::default_registry_manifest_is_valid`
  - `terrane-core::default_registry_exposes_registered_grant_resource_namespaces`
  - `terrane-core::every_declared_resource_method_is_documented`
  - `terrane-host::public_command_inventory_covers_every_registered_command`
  - `terrane-host::public_query_inventory_covers_every_registered_query`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
