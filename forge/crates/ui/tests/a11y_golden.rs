//! UI-7 accessibility GOLDEN test (source of record `spec/accessibility.md`).
//!
//! Renders a small but representative UI tree to its accessibility annotations —
//! every node's `{path, role, name, focusable}` plus the emitted keyboard focus
//! order(s) — and compares them against a committed golden JSON
//! (`tests/golden/a11y/representative_screen.json`). This locks the a11y
//! EMISSION (roles / names / focus order / Modal containment) so any future
//! regression in [`forge_ui::Node::accessibility`] or
//! [`forge_ui::Node::focus_order`] is caught by an exact-match diff.
//!
//! The golden is built by serializing the SAME tree the crate parses, so it also
//! doubles as a wire-shape round-trip check for the catalog members it touches.
//!
//! To regenerate after an intentional spec change, run with
//! `A11Y_GOLDEN_REGEN=1` and copy the printed JSON into the golden file.

use forge_ui::{from_str, validate_accessibility, Node};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/a11y/representative_screen.json")
}

/// Flatten a node's per-node a11y annotations in deterministic pre-order, each
/// addressed by index path (same addressing as the diff/patch layer).
fn annotations(node: &Node) -> Vec<Value> {
    let mut out = Vec::new();
    walk(node, &mut Vec::new(), &mut out);
    out
}

fn walk(node: &Node, path: &mut Vec<usize>, out: &mut Vec<Value>) {
    let ax = node.accessibility();
    out.push(json!({
        "path": path.clone(),
        "type": node.type_name(),
        "role": ax.role.as_str(),
        "name": ax.name,
        "focusable": node.is_focusable(),
    }));
    for (i, child) in child_nodes(node).into_iter().enumerate() {
        path.push(i);
        walk(&child, path, out);
        path.pop();
    }
}

/// Ordered known child nodes for annotation traversal (typed children, plus an
/// `Unknown` container's verbatim `children`/`items`/`panels`). `tabs` are tab
/// *descriptors*, not renderable components, so they are surfaced only by the
/// focus order (as `tab` stops), never as standalone annotated nodes.
fn child_nodes(node: &Node) -> Vec<Node> {
    match node {
        Node::Stack { children, .. } => children.clone(),
        Node::List { items, .. } => items.clone(),
        Node::Unknown { props, .. } => {
            let mut out = Vec::new();
            for key in ["children", "items", "panels"] {
                if let Some(Value::Array(arr)) = props.get(key) {
                    for v in arr {
                        if let Ok(n) = serde_json::from_value::<Node>(v.clone()) {
                            out.push(n);
                        }
                    }
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Serialize a focus order to a stable JSON shape for the golden.
fn focus_order_json(node: &Node) -> Value {
    let order = node.focus_order();
    json!({
        "traps_focus": order.traps_focus,
        "initial_focus": order.initial_focus,
        "stops": order.stops.iter().map(|s| json!({
            "path": s.path,
            "role": s.role.as_str(),
            "name": s.name,
        })).collect::<Vec<_>>(),
    })
}

/// The representative screen: a page Stack holding a Tabs and a Grid of
/// controls, plus a separate Modal subtree (exercised rooted at the Modal so the
/// focus-trap/containment path is covered).
fn page_tree() -> Node {
    from_str(
        r#"{"type":"Stack","direction":"v","children":[
            {"type":"Text","text":"Dashboard"},
            {"type":"Tabs","activeTab":0,"ariaLabel":"Sections",
                "tabs":[{"label":"Overview"},{"label":"Settings"}],
                "panels":[
                    {"type":"Button","label":"Refresh"},
                    {"type":"Button","label":"Save settings"}
                ]},
            {"type":"Grid","columns":2,"children":[
                {"type":"TextField","value":"","label":"Search"},
                {"type":"Button","label":"","ariaLabel":"Clear"}
            ]}
        ]}"#,
    )
    .unwrap()
}

fn modal_tree() -> Node {
    from_str(
        r#"{"type":"Modal","title":"Confirm delete","children":[
            {"type":"Text","text":"This cannot be undone."},
            {"type":"Button","label":"Cancel"},
            {"type":"Button","label":"Delete","variant":"destructive"}
        ]}"#,
    )
    .unwrap()
}

fn computed() -> Value {
    let page = page_tree();
    let modal = modal_tree();
    json!({
        "page": {
            "annotations": annotations(&page),
            "focus_order": focus_order_json(&page),
        },
        "modal": {
            "annotations": annotations(&modal),
            "focus_order": focus_order_json(&modal),
        }
    })
}

#[test]
fn representative_screen_matches_committed_a11y_golden() {
    // The tree itself must satisfy the spec's required-name rules.
    assert!(validate_accessibility(&page_tree()).is_ok());
    assert!(validate_accessibility(&modal_tree()).is_ok());

    let computed = computed();

    if std::env::var("A11Y_GOLDEN_REGEN").is_ok() {
        // Regeneration aid: print the pretty JSON to copy into the golden file.
        println!(
            "REGEN GOLDEN:\n{}",
            serde_json::to_string_pretty(&computed).unwrap()
        );
        return;
    }

    let golden: Value = serde_json::from_str(
        &fs::read_to_string(golden_path()).expect("read a11y golden"),
    )
    .expect("parse a11y golden");

    assert_eq!(
        computed, golden,
        "a11y annotations drifted from golden; if intentional, regenerate with \
         A11Y_GOLDEN_REGEN=1 and update {}",
        golden_path().display()
    );
}
