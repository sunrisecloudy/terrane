//! Static safety lint (prd-merged/04 LM-9) + forbidden-construct enforcement
//! (prd-merged/01 CR-13, layer 1 of 2).
//!
//! [`policy_scan`] reports every escape-hatch construct it can find;
//! [`enforce_policy`] turns a non-empty report into a hard error so a dangerous
//! applet never reaches transpile/execution. The scan walks SWC's parsed AST for
//! precision and — crucially — does not stop at *direct* spellings. Review 010
//! P1 found the original scanner only caught `eval(...)`/`Function(...)` written
//! literally; an attacker could reach the same capability through an alias
//! (`const e = eval; e(...)`), an indirect/comma eval (`(0, eval)(...)`), a
//! member or computed read off a global container
//! (`globalThis.eval`, `globalThis["eval"]`, `self["process"]`, `window.eval`),
//! or a plain *read* of a dangerous global (`process.env`, `require.resolve`).
//! The hardened scanner resolves simple `const`/`let` aliases of the forbidden
//! identifiers — established by a declarator (`const e = eval`) *or* by a later
//! assignment (`let e; e = eval`) — unwraps comma/paren/optional-chain wrappers,
//! folds constant string and substitution-free template computed keys
//! (`globalThis[`eval`]`), treats a destructured key off a global container
//! (`const { eval: e } = globalThis`) as a read, and flags a forbidden global
//! captured in *value position* (object value, array element, call argument,
//! assignment RHS) where it would otherwise dodge the call-site check
//! (`const o = { run: eval }; o.run(...)`). In short, reaching a dangerous
//! global as a bare identifier, member, computed member, destructured key, or
//! captured value — not just at a call site — is forbidden (review 010 +
//! follow-up).
//!
//! Shadowing is *scope-aware* (review 016 P1, refined in review 018 P1). A benign
//! local legitimately named like a global (`let process = 1`,
//! `function f(fetch) {...}`) suppresses the global-read finding only for
//! references in that binding's *own* scope or an enclosing one — tracked with a
//! scope stack the visitor pushes/pops as it descends. The stack carries a frame
//! per lexical scope: function/arrow/method bodies *and* every block (`{ }`,
//! `if`/`for`/`while` bodies, `try`/`catch`/`finally`) and `catch`/for-header
//! binding. Function-scoped names (`var`, function/class declaration names,
//! parameters) live in the wider function frame; block-scoped `let`/`const`/catch
//! bindings live only in their block's frame and are popped on exit. So a binding
//! that exists only inside *another* function — or only inside a sibling/inner
//! block — never suppresses a forbidden reference elsewhere:
//! `fetch("x"); function f(fetch){}`, `{ let eval = 1; } eval("x")`, and
//! `try{}catch(process){} process.env` are all still rejected for the outer
//! reference.
//!
//! The forbidden set (each names a *construct* and a *reason*):
//!   - `eval` (call / alias / member / computed) — arbitrary code evaluation.
//!   - `Function` (call / `new` / alias / member / computed) — code from strings.
//!   - dynamic `import(...)`      — module loading is outside the applet profile.
//!   - `fetch` (call / member / computed) — raw network; must go via `ctx.*`.
//!   - `XMLHttpRequest` (`new` / read / member) — raw network.
//!   - `require` (call / read / member) — Node module escape.
//!   - `process` (read / member / computed) — Node host object.
//!   - `globalThis.<x> = ...`     — global mutation / realm tamper.
//!   - `__proto__ = ...` / read   — prototype-chain tamper.
//!   - `Object.prototype.<x> = …` — prototype pollution.
//!   - static `import ... from ...` — REJECTED for M0a (single global script).
//!
//! ## wasm32 / CR-15
//!
//! The scan is pure AST work plus a comment/string-aware text backstop; no I/O,
//! no tty emitter. It stays `wasm32-unknown-unknown`-clean.

mod alias;
mod models;
mod parse;
mod scopes;
mod visitor;

use forge_domain::{CoreError, Result};
use swc_core::ecma::visit::VisitWith;

use alias::AliasCollector;
use parse::{parse, reject_static_imports, text_backstop};
use scopes::collect_module_scope_bindings;
use visitor::ScanVisitor;

pub use models::ScanFinding;

#[cfg(test)]
use parse::{contains_ident_call, mask_comments_and_strings};

/// Report every forbidden construct in `ts_or_js` without failing.
///
/// Accepts TypeScript or JavaScript (parsed with the TS superset grammar so it
/// handles both). A clean source returns an empty vector. prd-merged/04 LM-9.
///
/// Note: a *static* `import ... from ...` is rejected here with a
/// [`CoreError::ValidationError`] rather than a finding, because the M0a runtime
/// evaluates a single global script and cannot resolve module specifiers
/// (review 010 P2). An aliased dynamic `import` (`const i = import`) is not even
/// valid syntax and surfaces as a parse `ValidationError` upstream.
pub fn policy_scan(ts_or_js: &str) -> Result<Vec<ScanFinding>> {
    let module = parse(ts_or_js)?;

    // M0a contract: no static module imports (review 010 P2). The runtime runs a
    // single global script with no module loader, so a bare `import x from "y"`
    // can never resolve — reject it clearly instead of letting it through.
    reject_static_imports(&module.body)?;

    // Pass 1: resolve simple const/let aliases of the forbidden identifiers and
    // of the global containers, so a later `e(...)` / `g.eval(...)` is seen for
    // what it is (review 010 P1, alias technique).
    let mut aliases = AliasCollector::default();
    module.visit_with(&mut aliases);

    // Pass 2: collect the names bound at *module (top-level) scope* so a benign
    // top-level variable legitimately named like a global (`let process = 1` at
    // module scope) is not flagged as the host object — while a binding that only
    // exists inside *some other* function scope does NOT suppress a real
    // top-level reference (review 016 P1: cross-scope shadowing must not be a
    // bypass). Per-function bindings are pushed/popped on a scope stack by the
    // visitor itself as it descends, so suppression is scope-aware.
    let module_scope = collect_module_scope_bindings(&module);

    let mut visitor = ScanVisitor {
        findings: Vec::new(),
        aliases: aliases.0,
        // Scope stack: index 0 is the module scope; each function/arrow/method
        // pushes its own frame. A bare-identifier reference is suppressed only if
        // a frame currently on the stack binds the name.
        scopes: vec![module_scope],
    };
    module.visit_with(&mut visitor);

    // Backstop: catch the highest-severity code-eval spellings that a parser
    // could normalise away on edge input. Comment/string-aware so a benign
    // `const msg = "eval("` or `// Function(` is never flagged (review 010 P3).
    text_backstop(ts_or_js, &mut visitor.findings);

    Ok(visitor.findings)
}

/// Strict gate: error if any forbidden construct is present. prd-merged/01 CR-13.
///
/// Network/host-escape globals and global/prototype tamper map to
/// [`CoreError::PermissionDenied`] (a capability/safety boundary was crossed);
/// they are reported with the most security-relevant finding first. A static
/// `import` is surfaced (by [`policy_scan`]) as a [`CoreError::ValidationError`]
/// — it is a contract violation for M0a, not a capability breach.
pub fn enforce_policy(ts_or_js: &str) -> Result<()> {
    let findings = policy_scan(ts_or_js)?;
    if let Some(first) = findings.first() {
        let detail = format!("{} forbidden — {}", first.construct, first.reason);
        return Err(CoreError::PermissionDenied(detail));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constructs(src: &str) -> Vec<String> {
        policy_scan(src).unwrap().into_iter().map(|f| f.construct).collect()
    }

    fn assert_rejected(src: &str, expect_construct: &str) {
        let err = enforce_policy(src).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied", "src: {src}");
        assert!(
            constructs(src).iter().any(|c| c == expect_construct),
            "expected construct {expect_construct:?} for {src:?}, got {:?}",
            constructs(src)
        );
    }

    #[test]
    fn rejects_eval() {
        assert_rejected("export const f = () => eval('1+1');", "eval");
    }

    #[test]
    fn rejects_new_function() {
        assert_rejected("const g = new Function('return 1');", "new Function");
    }

    #[test]
    fn rejects_function_call_form() {
        // `Function("...")` without `new` is equally dangerous.
        assert_rejected("const g = Function('return 1');", "Function constructor");
    }

    #[test]
    fn rejects_dynamic_import() {
        assert_rejected("export const f = () => import('./x.ts');", "dynamic import");
    }

    #[test]
    fn rejects_fetch() {
        assert_rejected("export const f = () => fetch('https://x');", "fetch");
    }

    #[test]
    fn rejects_xml_http_request() {
        assert_rejected("const r = new XMLHttpRequest();", "XMLHttpRequest");
    }

    #[test]
    fn rejects_require_call_and_read() {
        assert_rejected("const fs = require('fs');", "require");
        assert_rejected(
            "const r = (globalThis as Record<string, unknown>).require;",
            "require",
        );
    }

    #[test]
    fn rejects_process_access() {
        assert_rejected(
            "const p = (globalThis as Record<string, unknown>).process;",
            "process",
        );
    }

    #[test]
    fn rejects_globalthis_mutation() {
        assert_rejected(
            "(globalThis as Record<string, unknown>).x = 1;",
            "globalThis mutation",
        );
    }

    #[test]
    fn rejects_proto_assignment() {
        assert_rejected("const o: Record<string, unknown> = {}; o.__proto__ = {};", "__proto__ assignment");
    }

    #[test]
    fn rejects_object_prototype_pollution() {
        assert_rejected(
            "(Object.prototype as Record<string, unknown>).polluted = true;",
            "Object.prototype mutation",
        );
    }

    #[test]
    fn rejects_proto_read_access() {
        // Even reading `__proto__` is an escape vector and is flagged.
        let findings = constructs("const x = obj.__proto__;");
        assert!(findings.iter().any(|c| c == "__proto__ access"), "{findings:?}");
    }

    // ---- Review 010 P1: NON-direct forbidden spellings ----

    #[test]
    fn rejects_aliased_eval() {
        assert_rejected(
            "function f() { const e = eval; return e('1'); }",
            "eval",
        );
    }

    #[test]
    fn rejects_aliased_function_new() {
        assert_rejected(
            "function f() { const F = Function; return new F('return 1'); }",
            "new Function",
        );
    }

    #[test]
    fn rejects_aliased_global_eval() {
        assert_rejected(
            "function f() { const g = globalThis as Record<string, any>; return g.eval('1'); }",
            "eval",
        );
    }

    #[test]
    fn rejects_comma_indirect_eval() {
        assert_rejected("export const v = (0, eval)('1');", "eval");
        assert_rejected("export const v = (0,eval)('1');", "eval");
    }

    #[test]
    fn rejects_member_global_eval() {
        assert_rejected("export const v = globalThis.eval('1');", "eval");
    }

    #[test]
    fn rejects_optional_member_window_eval() {
        assert_rejected("export const v = window.eval?.('1');", "eval");
    }

    #[test]
    fn rejects_member_global_function() {
        assert_rejected("export const v = globalThis.Function('return 1');", "Function constructor");
    }

    #[test]
    fn rejects_computed_global_eval() {
        assert_rejected("export const v = globalThis[\"eval\"]('1');", "eval");
    }

    #[test]
    fn rejects_computed_global_fetch_concat() {
        // Constant-foldable computed key `"fe" + "tch"` resolves to `fetch`.
        assert_rejected("export const v = globalThis[\"fe\" + \"tch\"]('https://x');", "fetch");
    }

    #[test]
    fn rejects_computed_self_process() {
        assert_rejected("export const v = self[\"process\"];", "process");
    }

    #[test]
    fn rejects_global_read_process_env() {
        assert_rejected("export const v = process.env;", "process");
    }

    #[test]
    fn rejects_global_read_process_alias() {
        assert_rejected("function f() { const p = process; return p; }", "process");
    }

    #[test]
    fn rejects_global_read_require_resolve() {
        assert_rejected("export const v = require.resolve;", "require");
    }

    #[test]
    fn rejects_global_read_xmlhttprequest_member() {
        assert_rejected("export const v = globalThis.XMLHttpRequest;", "XMLHttpRequest");
    }

    #[test]
    fn rejects_network_global_fetch_member() {
        assert_rejected("export const v = globalThis.fetch('https://x');", "fetch");
    }

    #[test]
    fn rejects_new_global_xmlhttprequest_member() {
        assert_rejected("export const v = new globalThis.XMLHttpRequest();", "XMLHttpRequest");
    }

    #[test]
    fn rejects_multi_hop_alias_chain() {
        // `const a = eval; const b = a; b('1')` — the alias map flattens hops, so
        // `b` resolves to `eval`.
        assert_rejected(
            "function f() { const a = eval; const b = a; return b('1'); }",
            "eval",
        );
    }

    #[test]
    fn rejects_aliased_container_computed_member() {
        // `g['eval'](...)` where `g = globalThis` — computed member off an
        // *aliased* global container.
        assert_rejected(
            "function f() { const g = globalThis as any; return g['eval']('1'); }",
            "eval",
        );
    }

    #[test]
    fn rejects_aliased_container_chain_member() {
        // Two-hop container alias: `g = globalThis; h = g; h.eval(...)`.
        assert_rejected(
            "function f() { let g = globalThis as any; let h = g; return h.eval('1'); }",
            "eval",
        );
    }

    #[test]
    fn rejects_self_and_global_container_reads() {
        // The full set of global containers (`globalThis|window|self|global`) all
        // gate a forbidden member/computed read.
        assert_rejected("export const v = self.eval('1');", "eval");
        assert_rejected("export const v = global.process;", "process");
        assert_rejected("export const v = (window as Record<string, any>).fetch('x');", "fetch");
    }

    // ---- Review 010 follow-up: value-position reads, assignment aliases,
    //      destructured global keys, template-literal computed keys ----

    #[test]
    fn rejects_destructured_forbidden_off_global_container() {
        // gap c: `const { eval: e } = globalThis` reads `eval` off the global.
        assert_rejected(
            "const { eval: e } = globalThis as Record<string, any>; export const v = e('1');",
            "eval",
        );
        // The destructured *key* is the read we flag (a `Function` finding); the
        // `new F(...)` call site need not also resolve to know it's forbidden.
        assert_rejected(
            "const { Function: F } = globalThis as Record<string, any>; export const v = new F('return 1');",
            "Function constructor",
        );
        // Shorthand destructure (`const { process } = self`) reads the key too.
        assert_rejected(
            "const { process } = self as Record<string, any>; export const v = process;",
            "process",
        );
    }

    #[test]
    fn destructure_off_user_object_is_allowed() {
        // The container must be a *global*; a plain user object is benign even if
        // a key happens to be named `eval`.
        let src = "const h = { eval: (x: number) => x + 1 }; const { eval: run } = h; export const v = run(41);";
        let findings = policy_scan(src).unwrap();
        assert!(findings.is_empty(), "false positive: {findings:?}");
        assert!(enforce_policy(src).is_ok());
    }

    #[test]
    fn rejects_forbidden_global_in_value_position() {
        // gap a: a forbidden global captured by value (object value, array
        // element, call argument, assignment RHS) escapes the call-site check.
        assert_rejected("const o = { run: eval }; export const v = o.run('1');", "eval");
        assert_rejected("const fns = [eval]; export const v = fns.map(f => f('1'));", "eval");
        assert_rejected(
            "function doThing(f: (s: string) => unknown){ return f('1'); } export const v = doThing(eval);",
            "eval",
        );
        assert_rejected(
            "function f(){ let e: any; e = eval; return e('1'); }",
            "eval",
        );
    }

    #[test]
    fn value_position_capture_of_benign_local_is_allowed() {
        // A benign local in value position must not trip the new check.
        let src = "const evaluate = (x: number) => x + 1; const o = { run: evaluate }; const fns = [evaluate]; export const v = o.run(1) + fns[0](2);";
        let findings = policy_scan(src).unwrap();
        assert!(findings.is_empty(), "false positive: {findings:?}");
        assert!(enforce_policy(src).is_ok());
    }

    #[test]
    fn rejects_alias_via_assignment() {
        // gap b: `let e; e = eval; e('1')` — alias established by AssignExpr.
        assert_rejected(
            "function f(){ let e: any; e = eval; return e('1'); }",
            "eval",
        );
        // The alias also resolves a container assigned by `=`.
        assert_rejected(
            "function f(){ let g: any; g = globalThis; return g.eval('1'); }",
            "eval",
        );
    }

    #[test]
    fn rejects_template_literal_computed_key() {
        // gap d: a substitution-free template key folds to a constant string.
        assert_rejected(
            "export const v = (globalThis as Record<string, any>)[`eval`]('1');",
            "eval",
        );
        assert_rejected(
            "export const v = (globalThis as Record<string, any>)[`fe` + `tch`]('https://x');",
            "fetch",
        );
    }

    #[test]
    fn template_key_with_substitution_is_not_folded() {
        // A template with a `${...}` substitution is dynamic; the key cannot be
        // statically resolved to `eval`, so the computed read is not flagged.
        // (Defence-in-depth still applies at the runtime realm.)
        let src = "function f(x: string){ const o: Record<string, unknown> = {}; return o[`pre${x}`]; } export const v = f;";
        let findings = policy_scan(src).unwrap();
        assert!(findings.is_empty(), "unexpected: {findings:?}");
    }

    // ---- Review 010 P2: static imports rejected for M0a ----

    #[test]
    fn rejects_static_import_default() {
        let err = enforce_policy("import x from './y';").unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert!(
            err.to_string().contains("static imports not supported in M0a"),
            "{err}"
        );
    }

    #[test]
    fn rejects_static_import_named() {
        let err = enforce_policy("import { a } from './y'; export const z = a;").unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn rejects_static_import_namespace() {
        let err = enforce_policy("import * as ns from './y';").unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn type_only_import_is_allowed() {
        // `import type { T }` is erased before runtime — no runtime specifier.
        assert!(
            enforce_policy("import type { T } from './y'; export const z: number = 1;").is_ok()
        );
    }

    // ---- Review 010 P3: backstop is comment/string aware ----

    #[test]
    fn backstop_does_not_fire_inside_string_literal() {
        // `eval(` inside a string must PASS.
        let findings = policy_scan("export const msg = \"eval(\"; export const m2 = msg;").unwrap();
        assert!(findings.is_empty(), "false positive: {findings:?}");
        assert!(enforce_policy("export const msg = \"eval(\";").is_ok());
    }

    #[test]
    fn backstop_does_not_fire_inside_comment() {
        // `// Function(` and `/* eval( */` must PASS.
        for src in [
            "// Function(\nexport const x = 1;",
            "/* eval( XMLHttpRequest( */ export const x = 1;",
            "export const x = 1; // eval(",
        ] {
            let findings = policy_scan(src).unwrap();
            assert!(findings.is_empty(), "false positive on {src:?}: {findings:?}");
        }
    }

    #[test]
    fn backstop_does_not_fire_inside_template_literal() {
        let findings =
            policy_scan("export const t = `prefix eval( suffix`; export const u = t;").unwrap();
        assert!(findings.is_empty(), "false positive: {findings:?}");
    }

    // ---- false-positive controls ----

    #[test]
    fn allowed_code_passes_clean() {
        let ok = [
            "export const y: number = 1;",
            "export async function main(ctx: unknown, input: unknown) { return input; }",
            // `process` as a *local* variable name is not the host object.
            "function run() { let process = 1; return process + 1; }",
            // member access to an unrelated `.get`/`.prototype`-free chain.
            "export const r = ctx.storage.get('k');",
            // a method literally named `evaluate` must not trip the `eval(` rule.
            "export const v = mathlib.compute(2, 3);",
            // an ordinary property named `evaluate` (corpus benign control).
            "const mathlib = { evaluate: (v: number) => v + 1 }; export const r = mathlib.evaluate(41);",
            // a local legitimately named `process_id`.
            "export const process_id = 'job-123';",
        ];
        for src in ok {
            let findings = policy_scan(src).unwrap();
            assert!(findings.is_empty(), "false positive on {src:?}: {findings:?}");
            assert!(enforce_policy(src).is_ok(), "false rejection on {src:?}");
        }
    }

    #[test]
    fn local_identifier_named_like_a_global_is_not_flagged_by_ast() {
        // A user-defined function named `fetchData` must not match `fetch`.
        let findings = constructs("function fetchData() { return 1; } export const x = fetchData();");
        assert!(findings.is_empty(), "{findings:?}");
    }

    // ---- Review 016 P1: cross-scope shadowing must NOT suppress a real
    //      forbidden reference in another scope (scope-aware binding) ----

    #[test]
    fn cross_scope_shadow_does_not_suppress_forbidden_ref() {
        // The reviewer's exact repro: a parameter named like a global inside ONE
        // function used to land in a module-wide binding set and suppress a real
        // top-level forbidden reference. Each of these must now be REJECTED.
        assert_rejected(
            r#"export const leak = fetch("https://x"); function shadow(fetch) { return fetch; }"#,
            "fetch",
        );
        assert_rejected(
            "export const leak = process.env; function f(process){ return process; }",
            "process",
        );
        assert_rejected(
            r#"export const leak = require("fs"); function f(require){ return require; }"#,
            "require",
        );
        // Also a local *declaration* (not a param) in a sibling function must not
        // suppress a top-level reference.
        assert_rejected(
            "export const leak = fetch('https://x'); function other(){ const fetch = 1; return fetch; }",
            "fetch",
        );
        // A `new XMLHttpRequest()` at module scope is not suppressed by a sibling
        // function binding `XMLHttpRequest`.
        assert_rejected(
            "export const leak = new XMLHttpRequest(); function g(XMLHttpRequest){ return XMLHttpRequest; }",
            "XMLHttpRequest",
        );
    }

    #[test]
    fn nested_scope_shadow_does_not_suppress_outer_forbidden_ref() {
        // A binding in a NESTED function must not suppress a forbidden reference
        // in the ENCLOSING scope (the nested scope is not on the stack there).
        assert_rejected(
            "export function outer(){ const leak = process.env; function inner(process){ return process; } return inner; }",
            "process",
        );
    }

    #[test]
    fn same_scope_shadow_still_suppresses_benign_local() {
        // The legitimate case must keep passing: when the binding and the use are
        // in the SAME (or an enclosing) scope, the name is the local, not the
        // host global.
        for src in [
            // param shadows within its own function body
            "export function f(process){ return process.id; }",
            // local declaration shadows within its own block
            "function run(){ let fetch = (x: string) => x; return fetch('a'); } export const v = run();",
            // shadow at the use site reaches the enclosing-scope binding
            "function outer(){ const process = { id: 1 }; function inner(){ return process.id; } return inner(); } export const v = outer();",
            // arrow-param shadow
            "export const g = (fetch: (s: string) => string) => fetch('a');",
            // var hoisted out of a nested block is function-scoped and shadows
            // within the whole function body.
            "function run(){ if (true) { var process = { id: 1 }; } return process.id; } export const v = run();",
            // a method parameter shadows within the method scope.
            "class C { m(fetch: (s: string) => string){ return fetch('a'); } } export const v = new C();",
        ] {
            let findings = policy_scan(src).unwrap();
            assert!(findings.is_empty(), "false positive on {src:?}: {findings:?}");
            assert!(enforce_policy(src).is_ok(), "false rejection on {src:?}");
        }
    }

    #[test]
    fn method_scope_shadow_does_not_suppress_outer_forbidden_ref() {
        // A class method parameter named like a global must not suppress a real
        // top-level forbidden reference (the method opens its own scope).
        assert_rejected(
            "export const leak = fetch('https://x'); class C { m(fetch: unknown){ return fetch; } }",
            "fetch",
        );
    }

    // ---- Review 018 P1: BLOCK-scoped and CATCH-scoped shadows must NOT
    //      suppress a forbidden reference OUTSIDE their own block. ----

    #[test]
    fn block_scoped_shadow_does_not_suppress_outer_forbidden_ref() {
        // A `let`/`const` bound only inside a sibling/inner block must not leak
        // out and suppress a real forbidden reference after/outside that block.
        assert_rejected(
            // sibling bare block before the use
            r#"{ let eval = (x: string) => x; } export const v = eval("1");"#,
            "eval",
        );
        assert_rejected(
            // block local inside an `if`, forbidden ref after the `if` (the
            // reviewer's exact repro shape)
            r#"export function leak() { if (true) { let fetch = (x: string) => x; } return fetch("https://x"); }"#,
            "fetch",
        );
        assert_rejected(
            // top-level block `let process` before a `process.env` read
            "{ let process = 1; } export const v = process.env;",
            "process",
        );
        assert_rejected(
            // `const` in a nested block does not suppress the outer require()
            r#"function f(){ { const require = (s: string) => s; } return require("fs"); } export const v = f;"#,
            "require",
        );
    }

    #[test]
    fn catch_scoped_shadow_does_not_suppress_outer_forbidden_ref() {
        // A `catch (binding)` param is scoped to the catch body only; a forbidden
        // reference outside that body must still be flagged.
        assert_rejected(
            "try {} catch (process) {} export const v = process.env;",
            "process",
        );
        assert_rejected(
            r#"try {} catch (fetch) {} export const v = fetch("https://x");"#,
            "fetch",
        );
        assert_rejected(
            r#"try {} catch (require) {} export const v = require("fs");"#,
            "require",
        );
    }

    // ---- Review 025: TOP-LEVEL `let`/`const` shadows ARE in scope for the
    //      whole module body and must NOT be treated as forbidden globals
    //      (the review-018 fix must not over-correct module-level shadows). ----

    #[test]
    fn module_level_lexical_shadow_is_not_a_forbidden_global() {
        // A top-level `let`/`const` declaration named like a global is the local
        // binding for the entire module body — `const fetch = …; fetch("a")` is
        // valid code, not a raw-network read (review 025).
        for src in [
            // top-level `const fetch`
            r#"const fetch = (x: string) => x; export const v = fetch("a");"#,
            // top-level `let process`
            "let process = { id: 1 }; export const v = process.id;",
            // top-level `const require`
            r#"const require = (s: string) => s; export const v = require("fs");"#,
            // exported top-level `const` shadow (wraps the same Decl)
            r#"export const fetch = (x: string) => x; export const v = fetch("a");"#,
            // top-level destructured shadow
            "const { process } = { process: { id: 1 } }; export const v = process.id;",
        ] {
            let findings = policy_scan(src).unwrap();
            assert!(findings.is_empty(), "false positive on {src:?}: {findings:?}");
            assert!(enforce_policy(src).is_ok(), "false rejection on {src:?}");
        }
    }

    #[test]
    fn module_level_block_shadow_still_leaks_nothing() {
        // The review-018 guarantee must survive the review-025 fix: a `let`/`const`
        // nested in a top-level *block* still does NOT suppress a forbidden
        // reference outside that block.
        assert_rejected(
            r#"{ let fetch = (x: string) => x; } export const v = fetch("https://x");"#,
            "fetch",
        );
        assert_rejected(
            "{ const process = 1; } export const v = process.env;",
            "process",
        );
    }

    #[test]
    fn block_and_catch_shadows_still_suppress_within_their_own_scope() {
        // The legitimate same-scope cases must keep passing: a block/catch binding
        // shadows the global for references *inside* that block (review 018 P1
        // control — the fix must not regress in-scope suppression).
        for src in [
            // bare block: declaration and use in the SAME block
            r#"export function f(){ { let fetch = (x: string) => x; return fetch("a"); } }"#,
            // catch param used within the catch body
            "export function f(){ try { throw 1; } catch (process) { return process; } }",
            // `let` shadow used in an enclosed inner block
            r#"export function f(){ let fetch = (x: string) => x; { return fetch("a"); } }"#,
            // for-of header variable named like a global, used in the loop body
            r#"export function f(xs: string[]){ for (const fetch of xs) { void fetch; } }"#,
            // classic for header variable named like a global
            r#"export function f(){ for (let process = 0; process < 1; process++) { void process; } }"#,
        ] {
            let findings = policy_scan(src).unwrap();
            assert!(findings.is_empty(), "false positive on {src:?}: {findings:?}");
            assert!(enforce_policy(src).is_ok(), "false rejection on {src:?}");
        }
    }

    #[test]
    fn text_backstop_does_not_false_positive_on_substrings() {
        // `retrieval(` contains the substring `eval(` but is a different ident;
        // `myFunction(` contains `Function`. Neither should be flagged.
        for src in [
            "function retrieval() { return 1; } export const x = retrieval();",
            "function myFunction() { return 1; } export const x = myFunction();",
            "const x = obj.evaluate(1);",
        ] {
            let findings = policy_scan(src).unwrap();
            assert!(findings.is_empty(), "false positive on {src:?}: {findings:?}");
        }
    }

    #[test]
    fn contains_ident_call_respects_boundaries() {
        assert!(contains_ident_call("a = eval('x')", "eval"));
        assert!(contains_ident_call("a = eval ('x')", "eval")); // whitespace before paren
        assert!(!contains_ident_call("a = retrieval('x')", "eval"));
        assert!(!contains_ident_call("a = evaluate('x')", "eval"));
        assert!(!contains_ident_call("a = evalX('x')", "eval")); // longer ident
        assert!(!contains_ident_call("a = eval", "eval")); // no call
    }

    #[test]
    fn mask_blanks_strings_and_comments_only() {
        let src = "const a = \"eval(\"; // Function(\nconst b = 1;";
        let masked = mask_comments_and_strings(src);
        assert!(!masked.contains("eval("), "string not masked: {masked:?}");
        assert!(!masked.contains("Function("), "comment not masked: {masked:?}");
        // Real code survives.
        assert!(masked.contains("const a ="));
        assert!(masked.contains("const b = 1;"));
        // Length preserved.
        assert_eq!(masked.len(), src.len());
    }
}
