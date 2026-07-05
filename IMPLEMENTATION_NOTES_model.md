# Model v2 Implementation Notes

## Files changed

- `rust/crates/terrane-cap-interface/src/abi.rs`
- `rust/crates/terrane-cap-interface/src/lib.rs`
- `rust/crates/terrane-cap-model/Cargo.toml`
- `rust/crates/terrane-cap-model/src/lib.rs`
- `rust/crates/terrane-cap-model/src/doc.rs`
- `rust/crates/terrane-cap-model/src/tests.rs`
- `rust/crates/terrane-cap-model/tests/capability.rs`
- `rust/crates/terrane-cap-local-model/Cargo.toml`
- `rust/crates/terrane-cap-local-model/src/lib.rs`
- `rust/crates/terrane-cap-local-model/src/commands.rs`
- `rust/crates/terrane-cap-local-model/src/doc.rs`
- `rust/crates/terrane-cap-local-model/src/tests.rs`
- `rust/crates/terrane-cap-local-model/tests/capability.rs`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/tests/cap/interface.rs`
- `rust/crates/terrane-core/tests/cap/model.rs`
- `rust/crates/terrane-core/tests/cap/local_model.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/local_llm.rs`
- `rust/crates/terrane-host/tests/cap/model.rs`
- `rust/crates/terrane-host/tests/permission.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`
- `docs/APP_API.md`
- `Cargo.lock`

## Design choices

- Extended the existing `terrane-cap-model` and `terrane-cap-local-model` capabilities additively. No new namespace or central command/event enum was added.
- Added `ModelImagePart { name, hash, size, mime }` to the ABI and threaded `image_parts` through `Effect::ModelCall` and `Effect::LocalModelCall`.
- `model.ask` accepts plain text prompts as before, plus JSON prompts with `parts`. Image parts may point at a folded blob name or a content-addressed blob ref. The model capability normalizes these to `{hash,size,mime}` refs before the effect crosses the edge.
- Inline image bytes are rejected deterministically before an effect is emitted. The recorded model response event remains unchanged, so replay identity still comes from the recorded response and does not re-run inference.
- The model prompt validator is public and reused by `local-model`; local-model currently rejects image parts with a typed error until model specs can declare a vision-capable backend.
- Added decide-time caps in the same spirit as `common.send`: per-app recorded model/local-model turns are capped at 64, model prompt bytes at 256 KiB, image parts at 16 per call, image part size at 16 MiB, and recorded backend-run resource calls at 4.
- Added `ctx.resource.model.ask(agent, promptJsonOrText)` to the model manifest/docs and regenerated `docs/APP_API.md`.
- Direct provider and connection-secret handling remains deferred per the plan's non-goal boundary. No API key material is accepted by these paths or recorded in events.

## Shared files touched

- `Cargo.lock`
- `docs/APP_API.md`
- `rust/crates/terrane-cap-interface/src/abi.rs`
- `rust/crates/terrane-cap-interface/src/lib.rs`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/local_llm.rs`
- Host public authorization and permission inventory tests.

## Deviations and follow-ups

- The host edge now carries image refs in the effect ABI. The current CLI agent adapter does not translate refs into provider-specific attachment flags; that remains an edge-adapter follow-up.
- Sessions, streaming, tool use, and direct HTTP providers were left untouched because this slice was scoped to blob image parts, replay-preserving effects, resource exposure, and spend/size limits.

## Tests proving the properties

- Blob ref normalization and replay identity: `model_prompt_json_blob_names_normalize_to_content_refs_and_replay`.
- Inline image rejection and image count validation: `model_prompt_json_rejects_inline_bytes_and_image_limit_before_effect`.
- Model per-app spend cap: `model_per_app_spend_limit_blocks_after_recorded_turns`.
- Local-model typed image refusal: `local_model_rejects_image_parts_until_model_is_vision_capable`.
- Local-model per-app spend cap: `local_model_per_app_spend_limit_blocks_after_recorded_turns`.
- Host fake-agent e2e and replay: `model_e2e_fake_agent_records_and_replays`.
- Public resource/docs inventory: `model_doc_includes_resource_surface_and_image_limits`, `permission_required_reports_only_grantable_missing_resources`, and `grantable_command_inventory_requires_explicit_extractors_or_refusal`.

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
