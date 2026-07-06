# Geo Capability Implementation Notes

## Files changed

- `rust/crates/terrane-host/src/geo_edge.rs`: new host edge module. Non-macOS
  keeps the typed unsupported path; macOS uses CoreLocation through
  `objc2-core-location`.
- `rust/crates/terrane-host/src/edge.rs`: routes `Effect::GeoLocate` to the
  host geo edge.
- `rust/crates/terrane-host/src/lib.rs`: registers the module and makes the
  host `geo.supports` query reflect the host edge.
- `rust/crates/terrane-host/src/cli.rs`: adds `terrane geo supports`.
- `rust/crates/terrane-host/tests/cap/geo.rs`: keeps non-macOS unsupported
  coverage, adds macOS supports coverage, and adds an ignored real CoreLocation
  e2e.
- `rust/crates/terrane-host/Cargo.toml`, `Cargo.lock`: macOS-only ObjC /
  CoreLocation binding dependencies.

## Key design choices

- Kept the deterministic capability unchanged: `geo.locate` still decides
  `Effect::GeoLocate`, and replay still folds only recorded `geo.observed`
  facts. The edge only supplies the coordinate fact.
- The macOS edge obtains a one-shot CoreLocation fix with `CLLocationManager`
  and a Rust-defined `CLLocationManagerDelegate`.
- Precision is parsed and applied at the edge with
  `terrane_cap_geo::round_for_precision` before `observed_event()` is built.
- `observed_at` is sampled at the edge using the existing time capability helper.
- `geo.supports` is host-aware in `terrane-host::query_on_core`; it reports
  true on macOS because this host binary has a CoreLocation edge, and false on
  non-macOS. Disabled Location Services, denial, timeout, or TCC failure remain
  typed runtime errors for the actual locate effect and record no event.

## Deviations from the original completed-plan slice

- The original merged slice left `GeoState.supports` defaulting false and did
  not include a recorded platform-observation event. For this follow-up, I kept
  platform detection in the host adapter instead of adding a new core event or
  putting OS probes into deterministic core state.
- The macOS edge uses the `objc2` binding crates rather than handwritten raw
  `objc_msgSend` signatures. This keeps the unsafe surface narrow and compiling
  under the normal Rust gate.

## Shared files touched

- `Cargo.lock`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/tests/cap/geo.rs`

## Test evidence

- Existing deterministic capability/core coverage remains:
  - `terrane-cap-geo::coarse_rounding_happens_as_integer_e7_math`
  - `terrane-cap-geo::invalid_precision_and_missing_app_are_typed_errors`
  - `terrane-cap-geo::locate_and_peek_return_recorded_and_transient_effects`
  - `terrane-cap-geo::fold_keeps_last_twenty_and_last_resource_returns_json`
  - `terrane-cap-geo::fold_rejects_recorded_fixes_inside_rate_window`
  - `terrane-cap-geo::app_removed_drops_fixes_and_supports_defaults_false`
  - `terrane-core::geo_current_records_rounded_observation_and_replays`
  - `terrane-core::geo_peek_returns_value_but_records_nothing`
  - `terrane-core::geo_fold_replay_identity_truncation_rate_limit_and_app_removed`
- Host edge coverage:
  - `terrane-host::geo_supports_reports_true_on_macos`
  - `terrane-host::geo_macos_corelocation_records_observation` is
    `#[ignore = "requires macOS GUI location services and TCC consent"]`.
  - `terrane-host::geo_cli_reports_unsupported_and_records_no_observation`
    remains compiled on non-macOS.

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
