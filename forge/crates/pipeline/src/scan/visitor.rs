//! Pass 3: the AST walk. [`ScanVisitor`] descends the module, pushing/popping
//! per-scope binding frames and recording a [`ScanFinding`] for every forbidden
//! construct it reaches (direct, aliased, member/computed of a global container,
//! captured-by-value, destructured, or mutated).

use std::collections::{HashMap, HashSet};
use swc_core::ecma::ast::{
    AssignTarget, Callee, Expr, MemberExpr, NewExpr, OptChainBase, Pat, SimpleAssignTarget,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::models::{
    base_ident, forbidden_global, is_global_container, member_prop_name, prop_name_key,
    resolve_ident, unwrap_expr, ScanFinding,
};
use super::scopes::{
    add_pat_bindings, collect_arrow_scope_bindings, collect_block_scope_bindings,
    collect_fn_scope_bindings, for_head_bindings,
};

pub(crate) struct ScanVisitor {
    pub(crate) findings: Vec<ScanFinding>,
    /// alias-name -> resolved forbidden/container name.
    pub(crate) aliases: HashMap<String, String>,
    /// Scope stack of locally-bound names. Index 0 is the module scope; each
    /// function/arrow/method pushes its own frame on entry and pops it on exit.
    /// A bare-identifier reference is the host global only if NO enclosing scope
    /// binds that name (review 016 P1: a binding in an unrelated scope must not
    /// suppress a real reference elsewhere).
    pub(crate) scopes: Vec<HashSet<String>>,
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

    // Entering a lexical *block* opens a new block scope for its `let`/`const`
    // declarations. We push only the names bound *directly* in this block so a
    // reference *inside* it sees them, then pop on exit so a sibling/outer
    // reference does NOT (review 018 P1: `{ let fetch = 1; } fetch("x")` must
    // still flag the outer `fetch`). A function/arrow body is itself a block, so
    // its body-local `let`/`const` are covered here while its params/`var` live
    // in the function frame.
    fn visit_block_stmt(&mut self, b: &swc_core::ecma::ast::BlockStmt) {
        self.scopes.push(collect_block_scope_bindings(&b.stmts));
        b.visit_children_with(self);
        self.scopes.pop();
    }

    // A `catch (binding)` clause binds `binding` in a scope covering the catch
    // body only. Push it as its own frame so `try{}catch(process){} process.env`
    // still flags the outer `process` read (review 018 P1). The catch body block
    // gets its own block frame via `visit_block_stmt`.
    fn visit_catch_clause(&mut self, c: &swc_core::ecma::ast::CatchClause) {
        let mut frame = HashSet::new();
        if let Some(p) = &c.param {
            add_pat_bindings(p, &mut frame);
        }
        self.scopes.push(frame);
        c.visit_children_with(self);
        self.scopes.pop();
    }

    // A `for (let/const/var x ...)` header binds `x` in a scope covering the
    // header and the loop body only. Push it as a frame so a benign loop variable
    // named like a global (`for (const fetch of xs) fetch(...)`) is not a false
    // positive, while it does not leak to code after the loop.
    fn visit_for_stmt(&mut self, n: &swc_core::ecma::ast::ForStmt) {
        use swc_core::ecma::ast::VarDeclOrExpr;
        let mut frame = HashSet::new();
        if let Some(VarDeclOrExpr::VarDecl(v)) = &n.init {
            for d in &v.decls {
                add_pat_bindings(&d.name, &mut frame);
            }
        }
        self.scopes.push(frame);
        n.visit_children_with(self);
        self.scopes.pop();
    }

    fn visit_for_in_stmt(&mut self, n: &swc_core::ecma::ast::ForInStmt) {
        self.scopes.push(for_head_bindings(&n.left));
        n.visit_children_with(self);
        self.scopes.pop();
    }

    fn visit_for_of_stmt(&mut self, n: &swc_core::ecma::ast::ForOfStmt) {
        self.scopes.push(for_head_bindings(&n.left));
        n.visit_children_with(self);
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
