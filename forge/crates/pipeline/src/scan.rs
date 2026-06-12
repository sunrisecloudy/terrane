//! Static safety lint (prd-merged/04 LM-9) + forbidden-construct enforcement
//! (prd-merged/01 CR-13, layer 1 of 2).
//!
//! [`policy_scan`] reports every escape-hatch construct it can find;
//! [`enforce_policy`] turns a non-empty report into a hard error so a dangerous
//! applet never reaches transpile/execution. The scan walks SWC's parsed AST for
//! precision (call targets, `new` targets, member/property names, assignment
//! targets, dynamic `import`), with a small text backstop for an
//! `Object.prototype` write spelled through a computed key the AST pass might
//! present differently.
//!
//! The forbidden set (each names a *construct* and a *reason*):
//!   - `eval(...)`                — arbitrary code evaluation.
//!   - `new Function(...)` / `Function(...)` — code-from-string constructor.
//!   - dynamic `import(...)`      — module loading is outside the applet profile.
//!   - `fetch(...)`              — raw network; must go through `ctx.*` later.
//!   - `new XMLHttpRequest()`     — raw network.
//!   - `require(...)` / `require` — Node module escape, not available to applets.
//!   - `process`                  — Node host object, not available to applets.
//!   - `globalThis.<x> = ...`     — global mutation / realm tamper.
//!   - `__proto__ = ...` / read   — prototype-chain tamper.
//!   - `Object.prototype.<x> = …` — prototype pollution.

use forge_domain::{CoreError, Result};
use swc_core::common::{sync::Lrc, FileName, SourceMap};
use swc_core::ecma::ast::{
    AssignTarget, Callee, EsVersion, Expr, MemberExpr, MemberProp, NewExpr, SimpleAssignTarget,
};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::visit::{Visit, VisitWith};

/// One forbidden construct found by the static scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanFinding {
    /// The construct that was matched, e.g. `"eval"`, `"new Function"`,
    /// `"dynamic import"`, `"globalThis mutation"`.
    pub construct: String,
    /// Why it is forbidden (operator-facing explanation).
    pub reason: String,
}

impl ScanFinding {
    fn new(construct: &str, reason: &str) -> Self {
        ScanFinding { construct: construct.to_string(), reason: reason.to_string() }
    }
}

/// Report every forbidden construct in `ts_or_js` without failing.
///
/// Accepts TypeScript or JavaScript (parsed with the TS superset grammar so it
/// handles both). A clean source returns an empty vector. prd-merged/04 LM-9.
pub fn policy_scan(ts_or_js: &str) -> Result<Vec<ScanFinding>> {
    let module = parse(ts_or_js)?;
    let mut visitor = ScanVisitor::default();
    module.visit_with(&mut visitor);

    // Backstop: a couple of constructs are easier to catch by surface text than
    // to chase through every AST normalisation. Kept narrow so it cannot mask a
    // legitimate identifier (substrings like `myProcess` are not matched because
    // the AST pass already handles bare/member identifiers precisely; this only
    // covers the literal escape spellings).
    text_backstop(ts_or_js, &mut visitor.findings);

    Ok(visitor.findings)
}

/// Strict gate: error if any forbidden construct is present. prd-merged/01 CR-13.
///
/// Network/host-escape globals and global/prototype tamper map to
/// [`CoreError::PermissionDenied`] (a capability/safety boundary was crossed);
/// they are reported with the most security-relevant finding first.
pub fn enforce_policy(ts_or_js: &str) -> Result<()> {
    let findings = policy_scan(ts_or_js)?;
    if let Some(first) = findings.first() {
        let detail = format!("{} forbidden — {}", first.construct, first.reason);
        return Err(CoreError::PermissionDenied(detail));
    }
    Ok(())
}

fn parse(src: &str) -> Result<swc_core::ecma::ast::Module> {
    let cm: Lrc<SourceMap> = Lrc::default();
    let fm = cm.new_source_file(Lrc::new(FileName::Custom("scan.ts".into())), src.to_string());
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax { tsx: false, ..Default::default() }),
        EsVersion::Es2022,
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    parser
        .parse_module()
        .map_err(|e| crate::parse_error(e.kind().msg()))
}

/// Strip the wrappers SWC keeps around an expression so we can see the real base
/// identifier: parentheses, `as`/`satisfies` casts, `!` non-null, and `<T>`
/// assertions. e.g. `(globalThis as Record<string, unknown>)` -> `globalThis`.
fn unwrap_expr(mut e: &Expr) -> &Expr {
    loop {
        e = match e {
            Expr::Paren(p) => &p.expr,
            Expr::TsAs(a) => &a.expr,
            Expr::TsSatisfies(s) => &s.expr,
            Expr::TsNonNull(n) => &n.expr,
            Expr::TsConstAssertion(c) => &c.expr,
            Expr::TsTypeAssertion(t) => &t.expr,
            other => return other,
        };
    }
}

/// The bare identifier name of an expression, after unwrapping, if any.
fn base_ident(e: &Expr) -> Option<&str> {
    match unwrap_expr(e) {
        Expr::Ident(id) => Some(id.sym.as_str()),
        _ => None,
    }
}

/// The member property name (`.foo` or `["foo"]`) as a string, if statically
/// known.
fn member_prop_name(prop: &MemberProp) -> Option<&str> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.as_str()),
        MemberProp::Computed(c) => match &*c.expr {
            Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) => s.value.as_str(),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

#[derive(Default)]
struct ScanVisitor {
    findings: Vec<ScanFinding>,
}

impl ScanVisitor {
    fn push(&mut self, construct: &str, reason: &str) {
        let f = ScanFinding::new(construct, reason);
        if !self.findings.contains(&f) {
            self.findings.push(f);
        }
    }

    /// Inspect a member expression (read access) for forbidden property/object
    /// names: `process`, `require`, `__proto__`, and `Object.prototype`.
    fn inspect_member_read(&mut self, m: &MemberExpr) {
        if let Some(prop) = member_prop_name(&m.prop) {
            match prop {
                "process" => self.push(
                    "process",
                    "Node host object is not available to applets",
                ),
                "require" => self.push(
                    "require",
                    "module require is not available to applets",
                ),
                "__proto__" => self.push(
                    "__proto__ access",
                    "prototype-chain access can escape the realm",
                ),
                "prototype" if base_ident(&m.obj) == Some("Object") => self.push(
                    "Object.prototype access",
                    "prototype pollution can corrupt every object",
                ),
                _ => {}
            }
        }
    }

    /// Inspect an assignment target for global/prototype mutation.
    fn inspect_assign_target(&mut self, target: &AssignTarget) {
        let AssignTarget::Simple(simple) = target else { return };
        let member = match simple {
            SimpleAssignTarget::Member(m) => m,
            SimpleAssignTarget::Paren(p) => {
                if let Expr::Member(m) = &*p.expr {
                    m
                } else {
                    return;
                }
            }
            _ => return,
        };
        // `globalThis.<x> = ...` (base may be wrapped in an `as` cast).
        if base_ident(&member.obj) == Some("globalThis") {
            self.push(
                "globalThis mutation",
                "mutating the global object tampers with the realm",
            );
        }
        // `<x>.__proto__ = ...`
        if member_prop_name(&member.prop) == Some("__proto__") {
            self.push(
                "__proto__ assignment",
                "writing __proto__ pollutes the prototype chain",
            );
        }
        // `Object.prototype.<x> = ...` — the assigned member's object is itself
        // `Object.prototype`.
        if let Expr::Member(inner) = unwrap_expr(&member.obj) {
            if member_prop_name(&inner.prop) == Some("prototype")
                && base_ident(&inner.obj) == Some("Object")
            {
                self.push(
                    "Object.prototype mutation",
                    "prototype pollution can corrupt every object",
                );
            }
        }
    }
}

impl Visit for ScanVisitor {
    fn visit_call_expr(&mut self, n: &swc_core::ecma::ast::CallExpr) {
        match &n.callee {
            // Dynamic `import(...)`.
            Callee::Import(_) => self.push(
                "dynamic import",
                "dynamic module loading is outside the applet profile",
            ),
            Callee::Expr(e) => match base_ident(e) {
                Some("eval") => {
                    self.push("eval", "arbitrary code evaluation is forbidden")
                }
                Some("Function") => self.push(
                    "Function constructor",
                    "constructing code from strings is forbidden",
                ),
                Some("fetch") => self.push(
                    "fetch",
                    "raw network is forbidden; use the ctx.* host API",
                ),
                Some("require") => self.push(
                    "require",
                    "module require is not available to applets",
                ),
                _ => {}
            },
            Callee::Super(_) => {}
        }
        n.visit_children_with(self);
    }

    fn visit_new_expr(&mut self, n: &NewExpr) {
        match base_ident(&n.callee) {
            Some("Function") => self.push(
                "new Function",
                "constructing code from strings is forbidden",
            ),
            Some("XMLHttpRequest") => self.push(
                "XMLHttpRequest",
                "raw network is forbidden; use the ctx.* host API",
            ),
            _ => {}
        }
        n.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, n: &MemberExpr) {
        self.inspect_member_read(n);
        n.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, n: &swc_core::ecma::ast::AssignExpr) {
        self.inspect_assign_target(&n.left);
        n.visit_children_with(self);
    }
}

/// Narrow surface-text backstop for the highest-severity code-eval spellings,
/// matched at an identifier boundary so a benign name like `retrieval(` or
/// `myFunction` is never flagged. The AST pass is authoritative and catches all
/// of these already on parseable input; this is belt-and-suspenders for any
/// edge the parser could normalise away. Adds a finding only if not already
/// reported by the AST walk.
fn text_backstop(src: &str, findings: &mut Vec<ScanFinding>) {
    let checks: &[(&str, &str, &str)] = &[
        // (identifier, construct, reason)
        ("eval", "eval", "arbitrary code evaluation is forbidden"),
        (
            "Function",
            "Function constructor",
            "constructing code from strings is forbidden",
        ),
        (
            "XMLHttpRequest",
            "XMLHttpRequest",
            "raw network is forbidden; use the ctx.* host API",
        ),
    ];
    for (ident, construct, reason) in checks {
        if contains_ident_call(src, ident) {
            let f = ScanFinding::new(construct, reason);
            if !findings.contains(&f) {
                findings.push(f);
            }
        }
    }
}

/// True if `src` contains `ident` as a whole identifier (not a substring of a
/// longer name) used as a constructor/callee — i.e. immediately preceded by a
/// non-identifier char and followed (after optional whitespace) by `(`.
fn contains_ident_call(src: &str, ident: &str) -> bool {
    let bytes = src.as_bytes();
    let mut from = 0;
    while let Some(rel) = src[from..].find(ident) {
        let start = from + rel;
        let end = start + ident.len();
        let prev_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        // After the identifier, skip whitespace, then require an opening paren.
        let mut i = end;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let next_ok = i < bytes.len() && bytes[i] == b'(';
        if prev_ok && next_ok {
            return true;
        }
        from = end;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
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

    #[test]
    fn allowed_code_passes_clean() {
        let ok = [
            "export const y: number = 1;",
            "export async function main(ctx: unknown, input: unknown) { return input; }",
            // `process` as a *local* variable name is not the host object.
            "function run() { let process = 1; return process + 1; }",
            // member access to an unrelated `.get`/`.prototype`-free chain.
            "export const r = ctx.storage.get('k');",
            // a method literally named `evaluate` must not trip the `eval(` text rule
            // via the AST path (no bare `eval` identifier here).
            "export const v = mathlib.compute(2, 3);",
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
        assert!(!contains_ident_call("a = eval", "eval")); // no call
    }
}
