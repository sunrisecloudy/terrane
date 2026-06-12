//! forge-pipeline: the front of the M0a spine.
//!
//! TypeScript source enters here and leaves as policy-checked, hashed ES-module
//! JavaScript ready for the QuickJS stage. Two responsibilities:
//!
//!   1. **Transpile** (prd-merged/01-core-runtime-prd.md **CR-14**): strip
//!      TypeScript types to ES-module JS *in-core and offline* using SWC. No
//!      bundling, no network, no `tsc` subprocess. The output is deterministic —
//!      identical input yields byte-identical `js_code` and `code_hash` — which
//!      is what makes [`Program::code_hash`] a stable `RunRecord.code_hash`
//!      provenance/replay key (prd-merged/01 CR-9).
//!
//!   2. **Static policy scan** (prd-merged/04 **LM-9** safety lint;
//!      prd-merged/01 **CR-13** layer 1 of 2): reject applet source that reaches
//!      for an escape hatch — `eval` / `Function` constructor / dynamic
//!      `import()` / raw network globals (`fetch`, `XMLHttpRequest`) / host
//!      escapes (`require`, `process`, `globalThis` mutation,
//!      `__proto__` / `Object.prototype` writes) — *before* a single line runs.
//!      This is the first of two layers; the QuickJS realm (no such globals
//!      bound) is the second, defence-in-depth.
//!
//! The scan walks SWC's parsed AST (precise call/identifier/member matching)
//! rather than grepping text, with a tiny regex backstop only for constructs the
//! parser can normalise away. Each finding names the construct and a reason.
//!
//! ## CR-15 / wasm32
//!
//! This crate is `wasm32-unknown-unknown`-clean — verified with
//! `cargo build -p forge-pipeline --target wasm32-unknown-unknown`. The enabled
//! `swc_core` features are pure Rust and we never enable the `tty-emitter`
//! diagnostic path (which would pull a tty/termcolor dependency). Parse errors
//! are surfaced from the parser's own diagnostic messages instead of an emitting
//! `Handler`, so no std-tty dependency reaches the web core.

use forge_domain::{code_hash, CoreError, Result};

mod scan;
mod transpile;

pub use scan::{enforce_policy, policy_scan, ScanFinding};

/// A transpiled, hashed program: the TS that came in, the JS that goes to the
/// engine, an optional source map, and the content hash used as the
/// `RunRecord.code_hash` (prd-merged/01 CR-9, CR-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// The original TypeScript source (kept for provenance/debugging).
    pub ts_source: String,
    /// The transpiled ES-module JavaScript that the engine executes.
    pub js_code: String,
    /// Optional source map (`None` in M0a; the seam exists for later).
    pub source_map: Option<String>,
    /// `"sha256:" + hex(sha256(js_code))` — stable across runs and platforms.
    pub code_hash: String,
}

/// Transpile TypeScript to ES-module JavaScript, in-core and offline.
///
/// prd-merged/01 CR-14. Returns [`CoreError::ValidationError`] on a parse error
/// (malformed source). The result is deterministic: same `ts` in, same
/// `js_code` + `code_hash` out.
///
/// The `code_hash` fingerprints the *transpiled JS* (not the TS) via the single
/// canonical [`forge_domain::code_hash`] (`"sha256:" + lowercase-hex`), so the
/// hash the pipeline produces is byte-identical to the one the runtime records
/// (review 010 P1 — one hash, no divergence).
pub fn transpile_ts(ts: &str) -> Result<Program> {
    let (js_code, source_map) = transpile::strip_types(ts)?;
    let code_hash = code_hash(&js_code);
    Ok(Program { ts_source: ts.to_string(), js_code, source_map, code_hash })
}

/// The full front-of-spine: enforce static policy (**CR-13** / **LM-9**), then
/// transpile (**CR-14**).
///
/// Policy is checked on the *TypeScript* source so a rejection names the
/// construct the author wrote. A clean source is then type-stripped and hashed.
pub fn compile(ts: &str) -> Result<Program> {
    enforce_policy(ts)?;
    transpile_ts(ts)
}

/// Map an SWC parse failure to the crate's error type without leaking SWC types
/// into the public API.
pub(crate) fn parse_error(msg: impl Into<String>) -> CoreError {
    CoreError::ValidationError(format!("transpile parse error: {}", msg.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_sha256_prefixed_lowercase_hex() {
        // Known-answer cross-check that the pipeline uses the single canonical
        // forge_domain::code_hash: sha256("") well-known digest.
        let h = code_hash("");
        assert_eq!(
            h,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn transpile_strips_a_type_annotation() {
        let p = transpile_ts("const x: number = 1; export const y = x;").unwrap();
        assert!(!p.js_code.contains(": number"), "annotation survived: {}", p.js_code);
        assert!(p.js_code.contains("const x = 1"), "value lost: {}", p.js_code);
        assert!(p.js_code.contains("export const y = x"), "export lost: {}", p.js_code);
        assert_eq!(p.ts_source, "const x: number = 1; export const y = x;");
        assert!(p.code_hash.starts_with("sha256:"));
    }

    #[test]
    fn transpile_is_deterministic() {
        let src = "const x: number = 1; export const y = x;";
        let a = transpile_ts(src).unwrap();
        let b = transpile_ts(src).unwrap();
        assert_eq!(a.js_code, b.js_code, "js differs across runs");
        assert_eq!(a.code_hash, b.code_hash, "hash differs across runs");
    }

    #[test]
    fn code_hash_differs_for_different_code() {
        let a = transpile_ts("export const y = 1;").unwrap();
        let b = transpile_ts("export const y = 2;").unwrap();
        assert_ne!(a.code_hash, b.code_hash);
    }

    #[test]
    fn applet_ish_source_with_interface_type_enum_transpiles() {
        let src = r#"
            interface Item { id: string; done: boolean; }
            type Id = string;
            enum Status { Open, Done }
            export async function main(ctx: unknown, input: { id: Id }): Promise<Item> {
                const status: Status = Status.Open;
                const item: Item = { id: input.id, done: status === Status.Done };
                return item;
            }
        "#;
        let p = transpile_ts(src).unwrap();
        // Type-only constructs are gone.
        assert!(!p.js_code.contains("interface Item"), "interface survived: {}", p.js_code);
        assert!(!p.js_code.contains("type Id"), "type alias survived: {}", p.js_code);
        // The enum is a value-level construct and remains.
        assert!(p.js_code.contains("Status"), "enum lost: {}", p.js_code);
        assert!(p.js_code.contains("async function main"), "fn lost: {}", p.js_code);
        // Annotation on the parameter is stripped.
        assert!(!p.js_code.contains(": Promise<Item>"), "ret annotation survived: {}", p.js_code);
    }

    #[test]
    fn malformed_source_is_validation_error() {
        let err = transpile_ts("const x: = ;").unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn compile_rejects_forbidden_then_never_transpiles() {
        let err = compile("export const f = () => eval('1');").unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
    }

    #[test]
    fn compile_passes_clean_source() {
        let p = compile("export const y: number = 1;").unwrap();
        assert!(p.js_code.contains("export const y = 1"));
    }
}
