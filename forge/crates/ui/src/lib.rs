//! forge-ui: the declarative component-tree protocol + minimal diff/patch.
//!
//! prd-merged/05 (UI spec):
//! - **UI-1** — diff successive trees into minimal index-path patches; apply
//!   replays them so `apply(&mut a, diff(Some(&a), &b))` yields `a == b`.
//! - **UI-2** — the M0a catalog subset: Stack / Text / Button / TextField /
//!   List, mirroring the TS host contract in `forge/std/forge-std.d.ts`.
//! - **UI-6** (NORMATIVE forward-compat) — an unknown component `"type"`
//!   deserializes to [`Node::Unknown`] (a labeled fallback that round-trips),
//!   and unknown props on known nodes are ignored — never an error.
//! - **UI-7** — every rendered node carries an accessibility annotation
//!   ([`Node::accessibility`]) emitting its ARIA role + accessible name per
//!   `spec/accessibility.md`, and [`validate_accessibility`] enforces the
//!   spec's REQUIRED accessible-name rules as `ValidationError`s.
//! - **UI-12** — versioned wire format with golden trees. `serde_json` is the
//!   canonical encoding (see [`to_canonical_string`]).
//!
//! This crate depends only on `forge-domain` (for [`forge_domain::CoreError`])
//! and is pure logic / `wasm32-unknown-unknown`-clean: no I/O, no panics on
//! real paths.

mod accessibility;
mod node;
mod patch;

pub use accessibility::{
    validate_accessibility, Accessibility, AxNameSource, AxRole,
};
pub use node::{ActionRef, BaseNode, Node, StackDir};
pub use patch::{apply, diff, Patch, Path};

use forge_domain::{CoreError, Result};

/// Wire-format version of the UI protocol (prd-merged/05 UI-12). Bumped when
/// the catalog or patch vocabulary changes incompatibly.
pub const WIRE_VERSION: u32 = 1;

/// Canonical serialization of a node tree (prd-merged/05 UI-12).
///
/// `serde_json` with field order fixed by the manual serializer in
/// `node` is the canonical encoding for golden-tree tests: known nodes emit
/// their TS-facing keys in a stable order, and [`Node::Unknown`] re-emits its
/// captured object verbatim.
pub fn to_canonical_string(node: &Node) -> Result<String> {
    serde_json::to_string(node)
        .map_err(|e| CoreError::ValidationError(format!("ui serialize failed: {e}")))
}

/// Parse a node tree from its canonical JSON encoding.
///
/// Unknown `"type"` values never error here — they become [`Node::Unknown`]
/// (UI-6).
pub fn from_str(json: &str) -> Result<Node> {
    serde_json::from_str(json)
        .map_err(|e| CoreError::ValidationError(format!("ui parse failed: {e}")))
}

/// Serialize a patch list to canonical JSON (the wire shape shared with the
/// renderer and the golden fixtures).
pub fn patches_to_string(patches: &[Patch]) -> Result<String> {
    serde_json::to_string(patches)
        .map_err(|e| CoreError::ValidationError(format!("patch serialize failed: {e}")))
}
