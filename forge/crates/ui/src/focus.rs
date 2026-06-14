//! UI-7 focus-order emission for the declarative UI catalog (prd-merged/05
//! UI-7, source of record `spec/accessibility.md`, "Focus Order" + "Unknown
//! Component Fallback").
//!
//! Phase 1 ([`crate::accessibility`]) derived each node's ARIA role + accessible
//! name and enforced the REQUIRED-name rules. This module is phase 2: given a
//! *rendered* tree it emits the deterministic **focus order** — the sequence in
//! which a keyboard user reaches the tree's focusable elements — exactly per the
//! spec's container rules:
//!
//! - **Stack** — focusable descendants in child source order.
//! - **Grid** — row-major child order; for the catalog's flat child vector this
//!   *is* source order, which the spec also mandates be preserved for assistive
//!   tech ("must preserve logical source order").
//! - **Scroll / Card** — the container is entered first (when it is itself
//!   focusable, e.g. an independently-focusable Scroll region), then its
//!   children in source order.
//! - **Tabs** — the tablist is reached first, then the *active* panel's
//!   focusables; inactive panels are NOT in the tab order.
//! - **Modal** — focus is **contained**: the order holds only the dialog's own
//!   focusable descendants, focus moves to the first focusable child (or the
//!   dialog itself when it has none), and [`FocusOrder::traps_focus`] is set so
//!   a renderer wraps Tab/Shift-Tab inside the dialog.
//! - **UI-6 unknown fallback** — an unrecognized component is itself NOT a tab
//!   stop, but its focusable *known* descendants stay in the order (in source
//!   order), so accessibility is never lost.
//!
//! The order is reported as [`FocusStop`]s keyed by index [`Path`](crate::Path)
//! — the same deterministic addressing the diff/patch layer uses — plus each
//! stop's role and accessible name, so goldens are both stable and readable.

use crate::accessibility::AxRole;
use crate::node::Node;

/// One stop in a tree's focus order: a focusable element addressed by its index
/// path from the focus root, with the role + accessible name it exposes.
///
/// The `path` is the same `[]`/`[0]`/`[0,2]` index addressing used by the
/// diff/patch layer ([`crate::Path`]), making the order deterministic and
/// directly comparable against a committed golden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusStop {
    /// Index path of this stop from the root the order was computed for.
    pub path: Vec<usize>,
    /// The ARIA role this stop exposes (mirrors [`Node::accessibility`]).
    pub role: AxRole,
    /// The accessible name this stop exposes, if any.
    pub name: Option<String>,
}

/// A tree's emitted focus order (UI-7): the ordered focusable stops plus the
/// dialog-containment metadata the spec requires for Modal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FocusOrder {
    /// Focusable stops in keyboard-traversal order.
    pub stops: Vec<FocusStop>,
    /// Whether focus is TRAPPED within this order (Modal containment). When
    /// `true` a renderer must wrap Tab/Shift-Tab at the ends of `stops` rather
    /// than letting focus escape to the page behind the dialog.
    pub traps_focus: bool,
    /// Where focus should move when this order becomes active — the first
    /// focusable child, or the container itself when it has no focusable child
    /// (the spec's "first focusable child or the dialog title" rule for Modal).
    /// `None` when the order is empty and the container is not itself focusable.
    pub initial_focus: Option<Vec<usize>>,
}

impl Node {
    /// Whether this node is itself a keyboard tab stop (`spec/accessibility.md`,
    /// "Keyboard/focus behavior" column).
    ///
    /// The interactive controls are tab stops; presentational/text/container
    /// nodes are not (their focusable descendants are reached *through* them).
    /// A [`Node::Unknown`](crate::Node::Unknown) is itself never focusable — per
    /// the UI-6 fallback rule it is "not focusable unless it contains focusable
    /// known children", and those children are surfaced by the traversal, not by
    /// the container itself. The one exception is a Scroll declared
    /// independently focusable, which the spec lets become a focus stop.
    pub fn is_focusable(&self) -> bool {
        match self {
            Node::Button { .. } | Node::TextField { .. } => true,
            Node::Stack { .. } | Node::Text { .. } | Node::List { .. } => false,
            Node::Unknown { type_name, props } => unknown_is_focusable(type_name, props),
        }
    }

    /// Emit this tree's deterministic focus order (UI-7).
    ///
    /// The returned [`FocusOrder`] lists every focusable descendant (and `self`
    /// when it is itself focusable) in keyboard-traversal order per the spec's
    /// per-container rules, addressed by index [`Path`](crate::Path) from `self`.
    /// For a Modal the order is focus-trapped and reports its initial focus
    /// target (containment).
    pub fn focus_order(&self) -> FocusOrder {
        // A Modal/dialog is a focus container: its order is contained and traps.
        if is_modal(self) {
            let mut stops = Vec::new();
            // The dialog itself is not a tab stop; collect its focusable
            // descendants in source order.
            collect_descendants(self, &mut Vec::new(), &mut stops);
            // Focus moves to the first focusable child, else the dialog itself
            // (named by its title) — the spec's Modal entry rule.
            let initial_focus = stops
                .first()
                .map(|s| s.path.clone())
                .or(Some(Vec::new()));
            return FocusOrder {
                stops,
                traps_focus: true,
                initial_focus,
            };
        }

        let mut stops = Vec::new();
        collect(self, &mut Vec::new(), &mut stops);
        let initial_focus = stops.first().map(|s| s.path.clone());
        FocusOrder {
            stops,
            traps_focus: false,
            initial_focus,
        }
    }
}

/// Collect focusable stops from `node` (inclusive of `node` when it is itself a
/// tab stop), at `path`, appending in keyboard order.
fn collect(node: &Node, path: &mut Vec<usize>, out: &mut Vec<FocusStop>) {
    if node.is_focusable() {
        out.push(stop_for(node, path));
    }
    descend(node, path, out, collect);
}

/// Collect only the focusable *descendants* of `node` (NOT `node` itself), used
/// for a Modal whose own dialog box is not a tab stop but whose contents are.
fn collect_descendants(node: &Node, path: &mut Vec<usize>, out: &mut Vec<FocusStop>) {
    descend(node, path, out, collect);
}

/// Walk `node`'s ordered children with `visit`, honoring the spec's per-container
/// traversal rules (Tabs descends only the active panel; everything else is
/// source order).
fn descend(
    node: &Node,
    path: &mut Vec<usize>,
    out: &mut Vec<FocusStop>,
    visit: fn(&Node, &mut Vec<usize>, &mut Vec<FocusStop>),
) {
    // Tabs: the tablist comes first (its tabs are focusable stops within the
    // tablist), then ONLY the active panel's focusables; inactive panels are
    // excluded from the tab order entirely.
    if is_tabs(node) {
        descend_tabs(node, path, out, visit);
        return;
    }

    for (i, child) in ordered_children(node).into_iter().enumerate() {
        path.push(i);
        visit(&child, path, out);
        path.pop();
    }
}

/// The ordered child nodes a container exposes for focus traversal, in source
/// order. Typed containers expose their typed children; an
/// [`Node::Unknown`](crate::Node::Unknown) container re-parses its verbatim
/// `children`/`items` node arrays (so a Grid/Card/Scroll/Form arriving as a UI-6
/// fallback still traverses its known descendants). Leaves return `[]`.
fn ordered_children(node: &Node) -> Vec<Node> {
    match node {
        Node::Stack { children, .. } => children.clone(),
        Node::List { items, .. } => items.clone(),
        Node::Unknown { props, .. } => unknown_child_nodes(props, &["children", "items"]),
        _ => Vec::new(),
    }
}

/// Tabs focus traversal: emit each tab's focusable stop in declaration order
/// (the tablist), then descend ONLY the active panel. `tabs`/`panels` are
/// verbatim arrays on the UI-6 fallback; the active index is `activeTab`/`active`
/// (default 0). Inactive panels are intentionally skipped (`spec`: "inactive
/// panels are not in the tab order").
fn descend_tabs(
    node: &Node,
    path: &mut Vec<usize>,
    out: &mut Vec<FocusStop>,
    visit: fn(&Node, &mut Vec<usize>, &mut Vec<FocusStop>),
) {
    let Node::Unknown { props, .. } = node else {
        return;
    };
    // Each declared tab is a focusable stop in the tablist, addressed under the
    // `tabs` array so the path stays deterministic.
    let tabs = unknown_child_nodes(props, &["tabs"]);
    for (i, tab) in tabs.iter().enumerate() {
        path.push(i);
        out.push(tab_stop(tab, path));
        path.pop();
    }
    // Then only the active panel's focusables. Panels are addressed after the
    // tabs (offset by tab count) so paths never collide.
    let panels = unknown_child_nodes(props, &["panels", "children"]);
    if panels.is_empty() {
        return;
    }
    let active = active_tab_index(props).min(panels.len().saturating_sub(1));
    path.push(tabs.len() + active);
    visit(&panels[active], path, out);
    path.pop();
}

/// A focus stop for a Tabs `tab` descriptor: tabs always expose the `tab` role,
/// named by their `label`/`title`/`ariaLabel` (required per spec).
fn tab_stop(tab: &Node, path: &[usize]) -> FocusStop {
    let name = match tab {
        Node::Unknown { props, .. } => ["label", "title", "ariaLabel"]
            .iter()
            .find_map(|k| props.get(*k).and_then(|v| v.as_str()))
            .map(str::to_string),
        _ => tab.accessibility().name,
    };
    FocusStop {
        path: path.to_vec(),
        role: AxRole::from("tab"),
        name,
    }
}

/// Build a [`FocusStop`] from a focusable node at `path`, taking its role + name
/// from the single accessibility source of truth ([`Node::accessibility`]).
fn stop_for(node: &Node, path: &[usize]) -> FocusStop {
    let ax = node.accessibility();
    FocusStop {
        path: path.to_vec(),
        role: ax.role,
        name: ax.name,
    }
}

/// Re-parse the nested node arrays an [`Node::Unknown`](crate::Node::Unknown)
/// carries verbatim under any of `keys`, in array order. Non-array/non-node
/// entries are skipped (tolerant per UI-6).
fn unknown_child_nodes(
    props: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Vec<Node> {
    let mut out = Vec::new();
    for key in keys {
        if let Some(serde_json::Value::Array(arr)) = props.get(*key) {
            for v in arr {
                if let Ok(node) = serde_json::from_value::<Node>(v.clone()) {
                    out.push(node);
                }
            }
        }
    }
    out
}

/// Whether a UI-6 fallback node is itself a tab stop. Only an independently
/// focusable Scroll region qualifies (`focusable: true`); every other unknown is
/// reached only through its focusable known children, never as a stop itself.
fn unknown_is_focusable(
    type_name: &str,
    props: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    type_name == "Scroll"
        && props
            .get("focusable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

/// Whether `node` is a Modal/dialog (focus container that traps focus).
fn is_modal(node: &Node) -> bool {
    matches!(node, Node::Unknown { type_name, .. } if type_name == "Modal")
}

/// Whether `node` is a Tabs container.
fn is_tabs(node: &Node) -> bool {
    matches!(node, Node::Unknown { type_name, .. } if type_name == "Tabs")
}

/// The active tab index for a Tabs node (`activeTab`/`active`, default 0).
fn active_tab_index(props: &serde_json::Map<String, serde_json::Value>) -> usize {
    ["activeTab", "active"]
        .iter()
        .find_map(|k| props.get(*k).and_then(|v| v.as_u64()))
        .unwrap_or(0) as usize
}
