//! Shared scan model: the [`ScanFinding`] record, the forbidden-name/container
//! predicates, and the small AST helpers (`unwrap_expr`, `base_ident`,
//! constant-folding, alias resolution) used across the scan passes.

use std::collections::HashMap;
use swc_core::ecma::ast::{Expr, MemberProp};

/// Dangerous global *names* — reaching any of these (call OR read, directly,
/// via alias, or as a member/computed property of a global container) is
/// forbidden. The value is the `(construct, reason)` pair to report.
pub(crate) fn forbidden_global(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "eval" => ("eval", "arbitrary code evaluation is forbidden"),
        "Function" => (
            "Function constructor",
            "constructing code from strings is forbidden",
        ),
        "fetch" => ("fetch", "raw network is forbidden; use the ctx.* host API"),
        "XMLHttpRequest" => (
            "XMLHttpRequest",
            "raw network is forbidden; use the ctx.* host API",
        ),
        "require" => ("require", "module require is not available to applets"),
        "process" => ("process", "Node host object is not available to applets"),
        _ => return None,
    })
}

/// Names of the global container objects an applet could reach a forbidden
/// global *through*: `globalThis.eval`, `window["eval"]`, `self["process"]`,
/// `global.fetch`. Reading these by themselves is benign; reaching a forbidden
/// member off them is not.
pub(crate) fn is_global_container(name: &str) -> bool {
    matches!(name, "globalThis" | "window" | "self" | "global")
}

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
    pub(crate) fn new(construct: &str, reason: &str) -> Self {
        ScanFinding { construct: construct.to_string(), reason: reason.to_string() }
    }
}

/// Strip the wrappers SWC keeps around an expression so we can see the real base
/// identifier: parentheses, `as`/`satisfies` casts, `!` non-null, `<T>`
/// assertions, and a comma `(0, eval)` sequence (last element is the value).
/// e.g. `(globalThis as Record<string, unknown>)` -> `globalThis`,
/// `(0, eval)` -> `eval`.
pub(crate) fn unwrap_expr(mut e: &Expr) -> &Expr {
    loop {
        e = match e {
            Expr::Paren(p) => &p.expr,
            Expr::TsAs(a) => &a.expr,
            Expr::TsSatisfies(s) => &s.expr,
            Expr::TsNonNull(n) => &n.expr,
            Expr::TsConstAssertion(c) => &c.expr,
            Expr::TsTypeAssertion(t) => &t.expr,
            // Indirect/comma eval: `(0, eval)` evaluates to its LAST operand.
            Expr::Seq(seq) => match seq.exprs.last() {
                Some(last) => last,
                None => return e,
            },
            other => return other,
        };
    }
}

/// The bare identifier name of an expression, after unwrapping, if any.
pub(crate) fn base_ident(e: &Expr) -> Option<&str> {
    match unwrap_expr(e) {
        Expr::Ident(id) => Some(id.sym.as_str()),
        _ => None,
    }
}

/// The member property name (`.foo` or `["foo"]`) as a string, if statically
/// known. Constant-foldable string concatenations (`["fe" + "tch"]`) are folded
/// so `globalThis["fe" + "tch"]` resolves to `fetch`.
pub(crate) fn member_prop_name(prop: &MemberProp) -> Option<String> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.as_str().to_string()),
        MemberProp::Computed(c) => const_string(&c.expr),
        MemberProp::PrivateName(_) => None,
    }
}

/// The statically-known name of a `PropName` (object-pattern / object-literal
/// key): a bare ident, a string/number literal, or a constant-foldable computed
/// key. `None` for anything dynamic.
pub(crate) fn prop_name_key(key: &swc_core::ecma::ast::PropName) -> Option<String> {
    use swc_core::ecma::ast::PropName;
    match key {
        PropName::Ident(id) => Some(id.sym.as_str().to_string()),
        PropName::Str(s) => s.value.as_str().map(|v| v.to_string()),
        PropName::Computed(c) => const_string(&c.expr),
        PropName::Num(_) | PropName::BigInt(_) => None,
    }
}

/// Best-effort constant-fold of a string-valued expression: a string literal, a
/// substitution-free template literal (`` `eval` ``), or a `"a" + "b"` chain of
/// such literals. Returns `None` for anything dynamic.
pub(crate) fn const_string(e: &Expr) -> Option<String> {
    use swc_core::ecma::ast::{BinaryOp, Lit};
    match unwrap_expr(e) {
        // `Str.value` is WTF-8; a non-UTF-8 literal can't spell a forbidden
        // identifier, so fold it to `None` rather than forcing a lossy string.
        Expr::Lit(Lit::Str(s)) => s.value.as_str().map(|v| v.to_string()),
        // A template literal with no `${...}` substitutions is just a constant
        // string (`globalThis[`eval`]` resolves to `eval`). One with any
        // substitution is dynamic and folds to `None`.
        Expr::Tpl(t) if t.exprs.is_empty() && t.quasis.len() == 1 => t.quasis[0]
            .cooked
            .as_ref()
            .and_then(|c| c.as_str())
            .map(|v| v.to_string()),
        Expr::Bin(b) if b.op == BinaryOp::Add => {
            let l = const_string(&b.left)?;
            let r = const_string(&b.right)?;
            Some(format!("{l}{r}"))
        }
        _ => None,
    }
}

/// Resolve an expression to the *effective* identifier name it denotes, chasing
/// known aliases. `e` (declared `const e = eval`) resolves to `eval`; `g`
/// (declared `const g = globalThis`) resolves to `globalThis`.
pub(crate) fn resolve_ident<'a>(e: &'a Expr, aliases: &'a HashMap<String, String>) -> Option<String> {
    let name = base_ident(e)?;
    Some(aliases.get(name).cloned().unwrap_or_else(|| name.to_string()))
}
