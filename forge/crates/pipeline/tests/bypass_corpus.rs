//! Data-driven bypass corpus (review 010 P1/P3).
//!
//! The fixtures in `tests/bypass/` encode adversarial spellings that reach a
//! forbidden capability *without* writing it directly — aliasing/data-flow
//! (`const e = eval; e(...)`), alias-by-assignment (`let e; e = eval; e(...)`),
//! comma/indirect eval (`(0, eval)(...)`), member and computed reads off a
//! global container (`globalThis["eval"]`, `globalThis[`eval`]`,
//! `self["process"]`), destructured keys off a global container
//! (`const { eval: e } = globalThis`), value-position captures that dodge the
//! call-site check (`{ run: eval }`, `[eval].map(...)`, `doThing(eval)`),
//! dangerous-global *reads* (`process.env`, `require.resolve`), dynamic
//! `import(...)`, and prototype tamper — plus benign controls that look
//! dangerous to a naive text grep but are clean to an AST (`"eval("` in a
//! string, `// Function(` in a comment, an `evaluate` property, a `process_id`
//! local, an `eval` key on a plain user object).
//!
//! Contract enforced here:
//!   * every `expect: "rejected"` case MUST be stopped by
//!     [`forge_pipeline::enforce_policy`] / [`forge_pipeline::compile`] *before*
//!     execution — either as a [`CoreError::PermissionDenied`] (capability /
//!     safety boundary crossed) or, for a spelling that is not even valid syntax
//!     (`const i = import`), as a [`CoreError::ValidationError`] (review 010 P2
//!     surfaces aliased dynamic import as a parse/validation failure).
//!   * every `expect: "allowed"` case MUST pass the scan with no findings and
//!     transpile cleanly through `compile()`.

use std::fs;
use std::path::{Path, PathBuf};

use forge_pipeline::{compile, enforce_policy, policy_scan};

fn bypass_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("bypass")
}

#[derive(serde::Deserialize)]
struct Manifest {
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    file: String,
    /// `"rejected"` or `"allowed"`.
    expect: String,
    /// Operator-facing note on what the spelling does (used in failure messages).
    #[serde(default)]
    reason: String,
}

fn load_manifest() -> Manifest {
    let path = bypass_dir().join("manifest.json");
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read bypass manifest {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("parse bypass manifest")
}

fn read_case(file: &str) -> String {
    let path = bypass_dir().join(file);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read bypass case {file}: {e}"))
}

#[test]
fn every_rejected_bypass_is_stopped_before_execution() {
    let manifest = load_manifest();
    let mut checked = 0usize;

    for case in &manifest.cases {
        if case.expect != "rejected" {
            continue;
        }
        checked += 1;
        let src = read_case(&case.file);

        // The strict gate must error. A non-direct forbidden spelling is either a
        // capability breach (PermissionDenied) or — for `const i = import`, which
        // is not valid syntax — a parse/validation failure (ValidationError).
        let err = enforce_policy(&src).expect_err(&format!(
            "{} ({}) should be rejected by static policy",
            case.file, case.reason
        ));
        let code = err.code();
        assert!(
            code == "PermissionDenied" || code == "ValidationError",
            "{} rejected with unexpected error kind {code:?}: {err:?} ({})",
            case.file,
            case.reason
        );

        // compile() must agree (policy runs before transpile).
        assert!(
            compile(&src).is_err(),
            "{} ({}) slipped through compile()",
            case.file,
            case.reason
        );

        // If the source parses, the report must name at least one construct +
        // reason. (A syntax-level rejection like `const i = import` produces no
        // findings because the parser fails first — that is expected.)
        if let Ok(findings) = policy_scan(&src) {
            assert!(
                !findings.is_empty(),
                "{} ({}) parsed but produced no findings",
                case.file,
                case.reason
            );
            for f in &findings {
                assert!(!f.construct.is_empty(), "{}: empty construct", case.file);
                assert!(!f.reason.is_empty(), "{}: empty reason", case.file);
            }
        }
    }

    // Guard against the corpus silently shrinking out from under this test.
    assert!(
        checked >= 30,
        "expected at least 30 rejected bypass cases, saw {checked}"
    );
}

#[test]
fn every_allowed_bypass_control_passes_clean() {
    let manifest = load_manifest();
    let mut checked = 0usize;

    for case in &manifest.cases {
        if case.expect != "allowed" {
            continue;
        }
        checked += 1;
        let src = read_case(&case.file);

        // No false positives: a dangerous-looking substring inside a string /
        // comment / unrelated property must NOT be flagged (review 010 P3).
        let findings = policy_scan(&src)
            .unwrap_or_else(|e| panic!("{} ({}) failed to parse: {e:?}", case.file, case.reason));
        assert!(
            findings.is_empty(),
            "{} ({}) FALSE POSITIVE, got {findings:?}",
            case.file,
            case.reason
        );

        // And it survives the full front-of-spine.
        compile(&src).unwrap_or_else(|e| {
            panic!("{} ({}) should compile, got {e:?}", case.file, case.reason)
        });
    }

    assert!(
        checked >= 6,
        "expected at least 6 benign bypass controls, saw {checked}"
    );
}
