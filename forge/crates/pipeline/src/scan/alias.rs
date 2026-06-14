//! Pass 1: alias resolution. Collects simple `const`/`let`/`var X = <ident>`
//! aliases (and assignment-form aliases `e = eval`) whose RHS resolves to a
//! forbidden global or a global container, so a later `e(...)` / `g.eval(...)`
//! is seen for what it is (review 010 P1, alias technique).

use std::collections::HashMap;
use swc_core::ecma::ast::{AssignTarget, Expr, Pat, SimpleAssignTarget};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::models::{base_ident, forbidden_global, is_global_container};

/// Collects simple `const`/`let`/`var X = <ident>` aliases where the RHS is a
/// forbidden global or a global container (possibly itself an alias). Multiple
/// hops are flattened during collection so a single lookup resolves the chain.
#[derive(Default)]
pub(crate) struct AliasCollector(pub(crate) HashMap<String, String>);

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
