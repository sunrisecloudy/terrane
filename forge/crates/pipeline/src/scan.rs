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
//! Shadowing is *scope-aware* (review 016 P1). A benign local legitimately named
//! like a global (`let process = 1`, `function f(fetch) {...}`) suppresses the
//! global-read finding only for references in that binding's own scope or an
//! enclosing one — tracked with a scope stack the visitor pushes/pops as it
//! descends. A binding that exists only inside *another* function never
//! suppresses a forbidden reference elsewhere, so
//! `fetch("x"); function f(fetch){}` is still rejected for the top-level
//! `fetch(...)`.
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

use forge_domain::{CoreError, Result};
use std::collections::{HashMap, HashSet};
use swc_core::common::{sync::Lrc, FileName, SourceMap};
use swc_core::ecma::ast::{
    AssignTarget, Callee, EsVersion, Expr, MemberExpr, MemberProp, ModuleItem, NewExpr,
    OptChainBase, Pat, SimpleAssignTarget,
};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::visit::{Visit, VisitWith};

/// Dangerous global *names* — reaching any of these (call OR read, directly,
/// via alias, or as a member/computed property of a global container) is
/// forbidden. The value is the `(construct, reason)` pair to report.
fn forbidden_global(name: &str) -> Option<(&'static str, &'static str)> {
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
fn is_global_container(name: &str) -> bool {
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
    fn new(construct: &str, reason: &str) -> Self {
        ScanFinding { construct: construct.to_string(), reason: reason.to_string() }
    }
}

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

/// Reject any static `import ... from ...` (review 010 P2). Type-only imports
/// (`import type {...}`) are erased before runtime and carry no runtime
/// specifier, so they are *not* rejected.
fn reject_static_imports(body: &[ModuleItem]) -> Result<()> {
    use swc_core::ecma::ast::ModuleDecl;
    for item in body {
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
            if import.type_only {
                continue;
            }
            return Err(CoreError::ValidationError(
                "static imports not supported in M0a; use a single entry module".to_string(),
            ));
        }
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
/// identifier: parentheses, `as`/`satisfies` casts, `!` non-null, `<T>`
/// assertions, and a comma `(0, eval)` sequence (last element is the value).
/// e.g. `(globalThis as Record<string, unknown>)` -> `globalThis`,
/// `(0, eval)` -> `eval`.
fn unwrap_expr(mut e: &Expr) -> &Expr {
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
fn base_ident(e: &Expr) -> Option<&str> {
    match unwrap_expr(e) {
        Expr::Ident(id) => Some(id.sym.as_str()),
        _ => None,
    }
}

/// The member property name (`.foo` or `["foo"]`) as a string, if statically
/// known. Constant-foldable string concatenations (`["fe" + "tch"]`) are folded
/// so `globalThis["fe" + "tch"]` resolves to `fetch`.
fn member_prop_name(prop: &MemberProp) -> Option<String> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.as_str().to_string()),
        MemberProp::Computed(c) => const_string(&c.expr),
        MemberProp::PrivateName(_) => None,
    }
}

/// The statically-known name of a `PropName` (object-pattern / object-literal
/// key): a bare ident, a string/number literal, or a constant-foldable computed
/// key. `None` for anything dynamic.
fn prop_name_key(key: &swc_core::ecma::ast::PropName) -> Option<String> {
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
fn const_string(e: &Expr) -> Option<String> {
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
fn resolve_ident<'a>(e: &'a Expr, aliases: &'a HashMap<String, String>) -> Option<String> {
    let name = base_ident(e)?;
    Some(aliases.get(name).cloned().unwrap_or_else(|| name.to_string()))
}

/// Collects simple `const`/`let`/`var X = <ident>` aliases where the RHS is a
/// forbidden global or a global container (possibly itself an alias). Multiple
/// hops are flattened during collection so a single lookup resolves the chain.
#[derive(Default)]
struct AliasCollector(HashMap<String, String>);

impl AliasCollector {
    /// Record `bind = <rhs-ident>` as an alias iff the RHS resolves (possibly via
    /// an already-known alias) to a forbidden global or a global container.
    fn record(&mut self, bind: &str, rhs_init: &Expr) {
        if let Some(rhs) = base_ident(rhs_init) {
            // Flatten one hop through an already-known alias.
            let resolved = self.0.get(rhs).cloned().unwrap_or_else(|| rhs.to_string());
            if forbidden_global(&resolved).is_some() || is_global_container(&resolved) {
                self.0.insert(bind.to_string(), resolved);
            }
        }
    }
}

impl Visit for AliasCollector {
    fn visit_var_declarator(&mut self, d: &swc_core::ecma::ast::VarDeclarator) {
        if let (Pat::Ident(bind), Some(init)) = (&d.name, &d.init) {
            self.record(bind.id.sym.as_str(), init);
        }
        d.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, n: &swc_core::ecma::ast::AssignExpr) {
        // Alias via *assignment* rather than declarator: `let e; e = eval; e(...)`
        // (review 010 follow-up gap b). Only a plain `=` to a bare identifier
        // target establishes an alias.
        use swc_core::ecma::ast::AssignOp;
        if n.op == AssignOp::Assign {
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(bind)) = &n.left {
                self.record(bind.id.sym.as_str(), &n.right);
            }
        }
        n.visit_children_with(self);
    }
}

/// Add every identifier bound by a binding pattern (`Pat`) to `out`. Covers
/// simple idents, array/object destructuring (including renames, defaults and
/// rest) so a benign local `let process = 1` or `const { process } = x` is seen.
fn add_pat_bindings(p: &Pat, out: &mut HashSet<String>) {
    match p {
        Pat::Ident(b) => {
            out.insert(b.id.sym.as_str().to_string());
        }
        Pat::Array(a) => {
            for el in a.elems.iter().flatten() {
                add_pat_bindings(el, out);
            }
        }
        Pat::Object(o) => {
            use swc_core::ecma::ast::ObjectPatProp;
            for prop in &o.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => add_pat_bindings(&kv.value, out),
                    ObjectPatProp::Assign(a) => {
                        out.insert(a.key.id.sym.as_str().to_string());
                    }
                    ObjectPatProp::Rest(r) => add_pat_bindings(&r.arg, out),
                }
            }
        }
        Pat::Assign(a) => add_pat_bindings(&a.left, out),
        Pat::Rest(r) => add_pat_bindings(&r.arg, out),
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
}

/// Collects the identifiers bound *within a single (function/module) scope* —
/// `var`/`let`/`const` declarators, nested function/class declaration *names*,
/// and `catch` bindings — WITHOUT descending into nested function bodies (which
/// open their own scope). This is what makes suppression scope-aware: a binding
/// that only exists inside one function must not suppress a forbidden reference
/// in a *different* scope (review 016 P1).
///
/// Note: a nested function/class *declaration name* and a `catch` binding are
/// hoisted into / visible from the enclosing scope, so they are collected here;
/// the nested function's *parameters and body* belong to that function's own
/// scope and are deliberately not visited.
#[derive(Default)]
struct ScopeBindingCollector(HashSet<String>);

impl Visit for ScopeBindingCollector {
    fn visit_var_declarator(&mut self, d: &swc_core::ecma::ast::VarDeclarator) {
        add_pat_bindings(&d.name, &mut self.0);
        // Recurse into the initializer so a `var` hoisted out of a nested block
        // (`{ var x = 1 }`) is still seen — but `visit_*` for nested functions is
        // overridden below to NOT descend, keeping us within this scope.
        d.visit_children_with(self);
    }
    fn visit_fn_decl(&mut self, f: &swc_core::ecma::ast::FnDecl) {
        // The function's *name* is bound in this scope; its body/params are not.
        self.0.insert(f.ident.sym.as_str().to_string());
    }
    fn visit_class_decl(&mut self, c: &swc_core::ecma::ast::ClassDecl) {
        self.0.insert(c.ident.sym.as_str().to_string());
    }
    fn visit_catch_clause(&mut self, c: &swc_core::ecma::ast::CatchClause) {
        if let Some(p) = &c.param {
            add_pat_bindings(p, &mut self.0);
        }
        // Visit the catch body (same scope) but the override on nested functions
        // stops descent there.
        c.body.visit_children_with(self);
    }
    // Do NOT descend into nested function/arrow/method bodies: they open their
    // own scope, collected separately when the visitor enters them.
    fn visit_function(&mut self, _f: &swc_core::ecma::ast::Function) {}
    fn visit_arrow_expr(&mut self, _a: &swc_core::ecma::ast::ArrowExpr) {}
}

/// Names bound at module (top-level) scope: declarators, top-level function /
/// class names, plus the *parameters* of nothing (a module has none). Used as
/// the root scope frame.
fn collect_module_scope_bindings(module: &swc_core::ecma::ast::Module) -> HashSet<String> {
    let mut c = ScopeBindingCollector::default();
    for item in &module.body {
        item.visit_with(&mut c);
    }
    c.0
}

/// Names bound by a function/method scope: its own parameters plus everything
/// its body binds (stopping at nested functions). `collect_fn_scope_bindings`
/// is used when the visitor enters a `Function`; `collect_arrow_scope_bindings`
/// for an arrow.
fn collect_fn_scope_bindings(f: &swc_core::ecma::ast::Function) -> HashSet<String> {
    let mut out = HashSet::new();
    for p in &f.params {
        add_pat_bindings(&p.pat, &mut out);
    }
    if let Some(body) = &f.body {
        let mut c = ScopeBindingCollector(out);
        for stmt in &body.stmts {
            stmt.visit_with(&mut c);
        }
        return c.0;
    }
    out
}

/// Names bound by an arrow scope: its parameters plus, for a block-body arrow,
/// everything the block binds (stopping at nested functions). An
/// expression-body arrow binds only its parameters.
fn collect_arrow_scope_bindings(a: &swc_core::ecma::ast::ArrowExpr) -> HashSet<String> {
    use swc_core::ecma::ast::BlockStmtOrExpr;
    let mut out = HashSet::new();
    for p in &a.params {
        add_pat_bindings(p, &mut out);
    }
    if let BlockStmtOrExpr::BlockStmt(body) = &*a.body {
        let mut c = ScopeBindingCollector(out);
        for stmt in &body.stmts {
            stmt.visit_with(&mut c);
        }
        return c.0;
    }
    out
}

struct ScanVisitor {
    findings: Vec<ScanFinding>,
    /// alias-name -> resolved forbidden/container name.
    aliases: HashMap<String, String>,
    /// Scope stack of locally-bound names. Index 0 is the module scope; each
    /// function/arrow/method pushes its own frame on entry and pops it on exit.
    /// A bare-identifier reference is the host global only if NO enclosing scope
    /// binds that name (review 016 P1: a binding in an unrelated scope must not
    /// suppress a real reference elsewhere).
    scopes: Vec<HashSet<String>>,
}

impl ScanVisitor {
    /// True if any scope currently on the stack (module scope + enclosing
    /// functions/arrows) binds `name` — i.e. the reference resolves to a local,
    /// not the host global. A binding inside a *sibling* or *nested* scope is not
    /// on the stack at the reference site and therefore does not suppress.
    fn in_scope(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(name))
    }

    fn push(&mut self, construct: &str, reason: &str) {
        let f = ScanFinding::new(construct, reason);
        if !self.findings.contains(&f) {
            self.findings.push(f);
        }
    }

    fn push_global(&mut self, name: &str) {
        if let Some((construct, reason)) = forbidden_global(name) {
            self.push(construct, reason);
        }
    }

    /// Flag any forbidden global reached as a *value* (not an immediate call):
    /// an object property value (`{ run: eval }`), an array element (`[eval]`),
    /// a call argument (`doThing(eval)`), or an assignment RHS (`x = eval`).
    /// Capturing a forbidden global by value lets it escape this scanner's
    /// call-site checks (`o.run('1')`, `arr.map(f => f('1'))`), so the *read* is
    /// the boundary we enforce (review 010 follow-up gap a — matches the module
    /// doc's "reads ... as forbidden, not just their call sites" intent).
    fn inspect_value_read(&mut self, e: &Expr) {
        if let Some(name) = base_ident(e) {
            self.check_forbidden_ref(name, false);
        }
    }

    /// If `e` resolves (through aliases) to a forbidden global that is NOT a
    /// locally-bound benign variable, flag it. `via_member` is true when the
    /// reference is a member/computed read of a *global container*, in which
    /// case a local binding of the same name is irrelevant (the property lives
    /// on the real global object).
    fn check_forbidden_ref(&mut self, name: &str, via_global_container: bool) {
        let resolved = self
            .aliases
            .get(name)
            .map(String::as_str)
            .unwrap_or(name)
            .to_string();
        if forbidden_global(&resolved).is_none() {
            return;
        }
        // A bare identifier that is shadowed by a real local binding *in an
        // enclosing scope* (e.g. `let process = 1` in this function or at module
        // scope) is the local, not the host global — unless we reached it as a
        // property of a global container. A binding that lives only in some other
        // (sibling/nested) scope is NOT on the stack here, so it cannot suppress
        // this reference (review 016 P1).
        if !via_global_container && self.in_scope(name) {
            return;
        }
        self.push_global(&resolved);
    }

    /// Flag a destructure of a forbidden key off a *global container*:
    /// `const { eval: e } = globalThis` / `const { Function } = window`
    /// (review 010 follow-up gap c). The init must resolve (through aliases) to a
    /// global container; each object-pattern key that names a forbidden global is
    /// a forbidden read, regardless of the local binding name it lands in.
    fn inspect_destructure_from_global(&mut self, pat: &Pat, init: &Expr) {
        use swc_core::ecma::ast::ObjectPatProp;
        let Pat::Object(obj) = pat else { return };
        // The thing being destructured must be a global container (possibly an
        // alias). Anything else (`const { eval } = someUserObject`) is benign.
        match resolve_ident(init, &self.aliases) {
            Some(name) if is_global_container(&name) => {}
            _ => return,
        }
        for prop in &obj.props {
            // The *key* is the property name read off the global, not the local
            // binding it is renamed to (`{ eval: e }` reads `eval`).
            let key = match prop {
                ObjectPatProp::KeyValue(kv) => prop_name_key(&kv.key),
                ObjectPatProp::Assign(a) => Some(a.key.id.sym.as_str().to_string()),
                ObjectPatProp::Rest(_) => None,
            };
            if let Some(k) = key {
                self.push_global(&k);
            }
        }
    }

    /// Inspect a member expression for forbidden access — both the "dangerous
    /// property off a global container" form (`globalThis.eval`,
    /// `self["process"]`) and the "dangerous global as the object" form
    /// (`process.env`, `require.resolve`), plus the legacy `__proto__` /
    /// `Object.prototype` reads.
    fn inspect_member_read(&mut self, m: &MemberExpr) {
        let prop = member_prop_name(&m.prop);

        // 1) `<globalContainer>.<forbidden>` / `<globalContainer>["forbidden"]`.
        //    The container itself may be an alias (`g.eval` where `g =
        //    globalThis`).
        if let Some(obj_name) = resolve_ident(&m.obj, &self.aliases) {
            if is_global_container(&obj_name) {
                if let Some(p) = &prop {
                    if forbidden_global(p).is_some() {
                        self.push_global(p);
                    }
                }
            }
            // 2) `<forbiddenGlobal>.<anything>` — a *read* of a dangerous global
            //    used as an object (`process.env`, `require.resolve`). This is a
            //    bare-identifier reference, so respect local shadowing.
            self.check_forbidden_ref(&obj_name, false);
        }

        // 3) Legacy prototype-tamper reads.
        if let Some(p) = &prop {
            match p.as_str() {
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
        // `globalThis.<x> = ...` (base may be wrapped in an `as` cast or alias).
        if resolve_ident(&member.obj, &self.aliases).as_deref() == Some("globalThis") {
            self.push(
                "globalThis mutation",
                "mutating the global object tampers with the realm",
            );
        }
        // `<x>.__proto__ = ...`
        if member_prop_name(&member.prop).as_deref() == Some("__proto__") {
            self.push(
                "__proto__ assignment",
                "writing __proto__ pollutes the prototype chain",
            );
        }
        // `Object.prototype.<x> = ...` — the assigned member's object is itself
        // `Object.prototype`.
        if let Expr::Member(inner) = unwrap_expr(&member.obj) {
            if member_prop_name(&inner.prop).as_deref() == Some("prototype")
                && base_ident(&inner.obj) == Some("Object")
            {
                self.push(
                    "Object.prototype mutation",
                    "prototype pollution can corrupt every object",
                );
            }
        }
    }

    /// Common handling for a call site whose callee may be direct, aliased,
    /// comma-wrapped, or a member/computed of a global container.
    fn inspect_callee_expr(&mut self, callee: &Expr) {
        let inner = unwrap_expr(callee);
        // `eval(...)`, `e(...)` (alias), `(0, eval)(...)`, `Function(...)`,
        // `fetch(...)`, `require(...)`.
        if let Some(name) = base_ident(inner) {
            self.check_forbidden_ref(name, false);
        }
        // `globalThis.eval(...)`, `globalThis["eval"](...)`, `g.eval(...)`.
        if let Expr::Member(m) = inner {
            // inspect_member_read already flags the dangerous member; calling it
            // here covers callees that aren't otherwise visited as member reads.
            self.inspect_member_read(m);
        }
    }
}

impl Visit for ScanVisitor {
    // --- scope management (review 016 P1) ---
    // Entering a function/arrow opens a new lexical scope. We push that scope's
    // bindings (params + body-local declarations, not descending into further
    // nested functions) so references *inside* the function see them, then pop on
    // exit so a sibling/outer reference does not (a binding in one function must
    // never suppress a forbidden reference in another).
    fn visit_function(&mut self, f: &swc_core::ecma::ast::Function) {
        self.scopes.push(collect_fn_scope_bindings(f));
        f.visit_children_with(self);
        self.scopes.pop();
    }

    fn visit_arrow_expr(&mut self, a: &swc_core::ecma::ast::ArrowExpr) {
        self.scopes.push(collect_arrow_scope_bindings(a));
        a.visit_children_with(self);
        self.scopes.pop();
    }

    fn visit_call_expr(&mut self, n: &swc_core::ecma::ast::CallExpr) {
        match &n.callee {
            // Dynamic `import(...)`.
            Callee::Import(_) => self.push(
                "dynamic import",
                "dynamic module loading is outside the applet profile",
            ),
            Callee::Expr(e) => self.inspect_callee_expr(e),
            Callee::Super(_) => {}
        }
        // A forbidden global passed *as an argument* (`doThing(eval)`) escapes by
        // value (gap a).
        for arg in &n.args {
            self.inspect_value_read(&arg.expr);
        }
        n.visit_children_with(self);
    }

    fn visit_opt_call(&mut self, n: &swc_core::ecma::ast::OptCall) {
        // `window.eval?.("1")` parses as an optional call; its callee carries
        // the dangerous member.
        self.inspect_callee_expr(&n.callee);
        for arg in &n.args {
            self.inspect_value_read(&arg.expr);
        }
        n.visit_children_with(self);
    }

    fn visit_new_expr(&mut self, n: &NewExpr) {
        // `new Function(...)`, `new F(...)` (alias), `new XMLHttpRequest()`,
        // `new globalThis.XMLHttpRequest()`.
        let callee = unwrap_expr(&n.callee);
        if let Some(name) = base_ident(callee) {
            let resolved = self.aliases.get(name).map(String::as_str).unwrap_or(name);
            match resolved {
                "Function" => self.push(
                    "new Function",
                    "constructing code from strings is forbidden",
                ),
                "XMLHttpRequest" if !self.in_scope(name) => self.push(
                    "XMLHttpRequest",
                    "raw network is forbidden; use the ctx.* host API",
                ),
                _ => {}
            }
        }
        if let Expr::Member(m) = callee {
            self.inspect_member_read(m);
        }
        // `new Ctor(eval)` — forbidden global passed by value as an argument.
        if let Some(args) = &n.args {
            for arg in args {
                self.inspect_value_read(&arg.expr);
            }
        }
        n.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, n: &MemberExpr) {
        self.inspect_member_read(n);
        n.visit_children_with(self);
    }

    fn visit_opt_chain_expr(&mut self, n: &swc_core::ecma::ast::OptChainExpr) {
        // `window.eval?.x` — the base of an optional chain can be a member read.
        if let OptChainBase::Member(m) = &*n.base {
            self.inspect_member_read(m);
        }
        n.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, n: &swc_core::ecma::ast::AssignExpr) {
        self.inspect_assign_target(&n.left);
        // `x = eval;` / `e = process;` — capturing a forbidden global on an
        // assignment RHS is a read of that global (gap a / alias-by-assignment).
        self.inspect_value_read(&n.right);
        n.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, d: &swc_core::ecma::ast::VarDeclarator) {
        // `const p = process;` / `const e = eval;` — aliasing a forbidden global
        // is itself a read of that global. (The alias map records the binding;
        // here we *flag* the read so the assignment is rejected.)
        if let Some(init) = &d.init {
            if let Some(name) = base_ident(init) {
                self.check_forbidden_ref(name, false);
            }
            // A destructured forbidden property off a global container
            // (`const { eval: e } = globalThis`) is a forbidden read (gap c).
            self.inspect_destructure_from_global(&d.name, init);
        }
        d.visit_children_with(self);
    }

    fn visit_object_lit(&mut self, n: &swc_core::ecma::ast::ObjectLit) {
        // `{ run: eval }` — forbidden global captured as an object property value.
        use swc_core::ecma::ast::{Prop, PropOrSpread};
        for prop in &n.props {
            match prop {
                PropOrSpread::Prop(p) => {
                    if let Prop::KeyValue(kv) = &**p {
                        self.inspect_value_read(&kv.value);
                    }
                }
                PropOrSpread::Spread(s) => self.inspect_value_read(&s.expr),
            }
        }
        n.visit_children_with(self);
    }

    fn visit_array_lit(&mut self, n: &swc_core::ecma::ast::ArrayLit) {
        // `[eval]` — forbidden global captured as an array element.
        for el in n.elems.iter().flatten() {
            self.inspect_value_read(&el.expr);
        }
        n.visit_children_with(self);
    }
}

/// Comment- and string-aware surface-text backstop for the highest-severity
/// code-eval spellings, matched at an identifier boundary so a benign name like
/// `retrieval(` or `myFunction` is never flagged. The AST pass is authoritative
/// and catches all of these already on parseable input; this is
/// belt-and-suspenders for any edge the parser could normalise away.
///
/// Review 010 P3: the backstop must NOT fire inside string literals or comments
/// — `const msg = "eval("` and `// Function(` must pass. We therefore scan a
/// *masked* copy of the source where the bytes of every line/block comment and
/// every string/template literal are blanked out, so only real code text is
/// matched. Adds a finding only if not already reported by the AST walk.
fn text_backstop(src: &str, findings: &mut Vec<ScanFinding>) {
    let code = mask_comments_and_strings(src);
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
        if contains_ident_call(&code, ident) {
            let f = ScanFinding::new(construct, reason);
            if !findings.contains(&f) {
                findings.push(f);
            }
        }
    }
}

/// Replace the bytes of every comment (line `//…`, block `/*…*/`) and every
/// string/template literal with spaces, preserving length and newlines, so a
/// text scan over the result only sees real code. Best-effort lexing that is
/// good enough for the narrow backstop (the AST pass is authoritative).
fn mask_comments_and_strings(src: &str) -> String {
    #[derive(PartialEq)]
    enum St {
        Code,
        Line,           // // comment
        Block,          // /* */ comment
        Str(char),      // '...' or "..."
        Template,       // `...`
    }
    let bytes = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut st = St::Code;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let next = bytes.get(i + 1).copied();
        match st {
            St::Code => {
                if b == b'/' && next == Some(b'/') {
                    st = St::Line;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                    continue;
                }
                if b == b'/' && next == Some(b'*') {
                    st = St::Block;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                    continue;
                }
                if b == b'\'' || b == b'"' {
                    st = St::Str(b as char);
                    out.push(b' ');
                    i += 1;
                    continue;
                }
                if b == b'`' {
                    st = St::Template;
                    out.push(b' ');
                    i += 1;
                    continue;
                }
                out.push(b);
            }
            St::Line => {
                if b == b'\n' {
                    st = St::Code;
                    out.push(b'\n');
                } else {
                    out.push(if b.is_ascii_whitespace() { b } else { b' ' });
                }
            }
            St::Block => {
                if b == b'*' && next == Some(b'/') {
                    st = St::Code;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                    continue;
                }
                out.push(if b.is_ascii_whitespace() { b } else { b' ' });
            }
            St::Str(q) => {
                if b == b'\\' {
                    // Skip the escaped char too.
                    out.push(b' ');
                    if next.is_some() {
                        out.push(b' ');
                        i += 2;
                        continue;
                    }
                } else if b as char == q {
                    st = St::Code;
                    out.push(b' ');
                } else {
                    out.push(if b == b'\n' { b'\n' } else { b' ' });
                }
            }
            St::Template => {
                // Note: template `${...}` substitutions are masked too. The AST
                // pass remains authoritative for code inside substitutions.
                if b == b'\\' {
                    out.push(b' ');
                    if next.is_some() {
                        out.push(b' ');
                        i += 2;
                        continue;
                    }
                } else if b == b'`' {
                    st = St::Code;
                    out.push(b' ');
                } else {
                    out.push(if b == b'\n' { b'\n' } else { b' ' });
                }
            }
        }
        i += 1;
    }
    // The masked buffer is byte-for-byte length-preserving over ASCII control
    // characters; any multi-byte UTF-8 in code text is preserved as-is, and in
    // masked regions each byte became a single space, which is still valid
    // UTF-8. So `from_utf8` cannot fail, but fall back defensively.
    String::from_utf8(out).unwrap_or_else(|_| src.to_string())
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
        // The match must not be a prefix of a longer identifier (e.g. `evalX`).
        let boundary_after = end >= bytes.len() || !is_ident_byte(bytes[end]);
        // After the identifier, skip whitespace, then require an opening paren.
        let mut i = end;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let next_ok = i < bytes.len() && bytes[i] == b'(';
        if prev_ok && boundary_after && next_ok {
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
