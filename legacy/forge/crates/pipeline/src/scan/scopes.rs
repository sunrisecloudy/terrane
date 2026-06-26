//! Pass 2: scope binding collection. Scope-aware shadowing (review 016 P1,
//! refined in review 018 P1, review 025): distinguishes function-scoped names
//! (`var`, function/class declaration names, parameters) from block-scoped
//! `let`/`const`/`catch` bindings so a benign local named like a global
//! suppresses the global-read finding only in its own scope or an enclosing one.

use std::collections::HashSet;
use swc_core::ecma::ast::Pat;
use swc_core::ecma::visit::{Visit, VisitWith};

/// Add every identifier bound by a binding pattern (`Pat`) to `out`. Covers
/// simple idents, array/object destructuring (including renames, defaults and
/// rest) so a benign local `let process = 1` or `const { process } = x` is seen.
pub(crate) fn add_pat_bindings(p: &Pat, out: &mut HashSet<String>) {
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

/// Collects only the *function-scoped* identifiers visible throughout a
/// function/module scope: `var` declarators (which hoist out of any nested
/// block) plus nested function/class declaration *names* (hoisted into the
/// enclosing scope). It descends through nested *blocks* (so a `var` inside an
/// `if {}` is seen) but NOT into nested function/arrow/method bodies (which open
/// their own scope).
///
/// Crucially, `let`/`const` declarators and `catch` bindings are *block*-scoped,
/// not function-scoped, so they are deliberately NOT collected here — they are
/// tracked per-block by [`collect_block_scope_bindings`] and pushed/popped as the
/// visitor enters/leaves each block (review 018 P1: a block-only `let fetch`
/// must not suppress a forbidden reference outside its block).
#[derive(Default)]
pub(crate) struct FnScopeBindingCollector(pub(crate) HashSet<String>);

impl Visit for FnScopeBindingCollector {
    fn visit_var_decl(&mut self, d: &swc_core::ecma::ast::VarDecl) {
        // Only `var` is function-scoped (hoisted out of blocks). `let`/`const`
        // are block-scoped and handled per-block, not here.
        if d.kind == swc_core::ecma::ast::VarDeclKind::Var {
            for decl in &d.decls {
                add_pat_bindings(&decl.name, &mut self.0);
            }
        }
        // Descend into initializers so a nested block's `var` is reached, but the
        // nested-function overrides below keep us within this function scope.
        d.visit_children_with(self);
    }
    fn visit_fn_decl(&mut self, f: &swc_core::ecma::ast::FnDecl) {
        // The function's *name* is bound in this scope; its body/params are not.
        self.0.insert(f.ident.sym.as_str().to_string());
    }
    fn visit_class_decl(&mut self, c: &swc_core::ecma::ast::ClassDecl) {
        self.0.insert(c.ident.sym.as_str().to_string());
    }
    // Do NOT descend into nested function/arrow/method bodies: they open their
    // own scope, collected separately when the visitor enters them.
    fn visit_function(&mut self, _f: &swc_core::ecma::ast::Function) {}
    fn visit_arrow_expr(&mut self, _a: &swc_core::ecma::ast::ArrowExpr) {}
}

/// Collect the `let`/`const` declarator names bound *directly* in one block's
/// statement list (not in nested blocks or nested functions). These are
/// block-scoped: visible only inside this block (and blocks it encloses), so the
/// visitor pushes them on entry to the block and pops them on exit. `var` is
/// function-scoped and intentionally excluded here (it is handled by
/// [`FnScopeBindingCollector`]).
pub(crate) fn collect_block_scope_bindings(stmts: &[swc_core::ecma::ast::Stmt]) -> HashSet<String> {
    use swc_core::ecma::ast::{Decl, Stmt, VarDeclKind};
    let mut out = HashSet::new();
    for stmt in stmts {
        if let Stmt::Decl(decl) = stmt {
            match decl {
                Decl::Var(v) if v.kind != VarDeclKind::Var => {
                    for d in &v.decls {
                        add_pat_bindings(&d.name, &mut out);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Names bound by a `for-in`/`for-of` head (`for (const x of xs)`). A plain
/// `for (x of xs)` head (existing variable, no declaration) binds nothing new.
pub(crate) fn for_head_bindings(head: &swc_core::ecma::ast::ForHead) -> HashSet<String> {
    use swc_core::ecma::ast::ForHead;
    let mut out = HashSet::new();
    if let ForHead::VarDecl(v) = head {
        for d in &v.decls {
            add_pat_bindings(&d.name, &mut out);
        }
    }
    out
}

/// Names bound at module (top-level) scope and visible throughout the module
/// body: the *function-scoped* names (`var` declarators, top-level
/// function/class declaration names) **and** the top-level `let`/`const`
/// declarators bound *directly* in the module body. The module body is the
/// outermost lexical scope, so a top-level `let`/`const fetch` legitimately
/// shadows the global for the whole module and must populate the root frame
/// (review 025: top-level `const fetch = …; fetch("a")` is valid, not a
/// forbidden global read). A `let`/`const` nested inside a top-level *block*
/// (`{ let fetch }`) is NOT collected here — it is pushed/popped per-block by
/// [`ScanVisitor::visit_block_stmt`] so it cannot leak (review 018 P1). Used as
/// the root scope frame.
///
/// [`ScanVisitor::visit_block_stmt`]: super::visitor::ScanVisitor
pub(crate) fn collect_module_scope_bindings(module: &swc_core::ecma::ast::Module) -> HashSet<String> {
    use swc_core::ecma::ast::{Decl, ModuleDecl, ModuleItem, Stmt, VarDeclKind};
    let mut c = FnScopeBindingCollector::default();
    for item in &module.body {
        item.visit_with(&mut c);
    }
    // Top-level `let`/`const` declared directly in the module body are
    // block-scoped, but their block *is* the module body (the outermost scope),
    // so they belong in the root frame. We only walk module items directly here,
    // never descending into nested blocks/functions, so a `{ let fetch }` block
    // is left to its own per-block frame. Both a bare top-level declaration and an
    // `export const fetch = …` (which wraps the same `Decl`) are covered.
    for item in &module.body {
        let decl = match item {
            ModuleItem::Stmt(Stmt::Decl(d)) => d,
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => &e.decl,
            _ => continue,
        };
        if let Decl::Var(v) = decl {
            if v.kind != VarDeclKind::Var {
                for d in &v.decls {
                    add_pat_bindings(&d.name, &mut c.0);
                }
            }
        }
    }
    c.0
}

/// Function-scoped names of a function/method scope: its own parameters plus the
/// `var` declarators and nested function/class *names* its body hoists (stopping
/// at nested functions). `let`/`const`/`catch` bindings are block-scoped and are
/// pushed/popped per-block by the visitor, not collected here.
pub(crate) fn collect_fn_scope_bindings(f: &swc_core::ecma::ast::Function) -> HashSet<String> {
    let mut out = HashSet::new();
    for p in &f.params {
        add_pat_bindings(&p.pat, &mut out);
    }
    if let Some(body) = &f.body {
        let mut c = FnScopeBindingCollector(out);
        for stmt in &body.stmts {
            stmt.visit_with(&mut c);
        }
        return c.0;
    }
    out
}

/// Function-scoped names of an arrow scope: its parameters plus, for a block-body
/// arrow, the `var`/function/class names its block hoists (stopping at nested
/// functions). An expression-body arrow binds only its parameters. `let`/`const`
/// are block-scoped and tracked per-block by the visitor.
pub(crate) fn collect_arrow_scope_bindings(a: &swc_core::ecma::ast::ArrowExpr) -> HashSet<String> {
    use swc_core::ecma::ast::BlockStmtOrExpr;
    let mut out = HashSet::new();
    for p in &a.params {
        add_pat_bindings(p, &mut out);
    }
    if let BlockStmtOrExpr::BlockStmt(body) = &*a.body {
        let mut c = FnScopeBindingCollector(out);
        for stmt in &body.stmts {
            stmt.visit_with(&mut c);
        }
        return c.0;
    }
    out
}
