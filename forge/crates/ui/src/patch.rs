//! Minimal index-path diff/patch over the UI component tree
//! (prd-merged/05 UI-1).
//!
//! M0a diffing is **index-path based**: a node is identified by its position
//! among its parent's children (`[]` = root, `[0]` = first child, `[0,2]` =
//! third child of the first child). There is no keyed reconciliation yet, so
//! reordering an unkeyed list is expressed as in-place updates, not moves
//! (see `diff_reordered_list_index_updates` in the golden corpus).
//!
//! [`diff`] produces the smallest reasonable patch set; identical trees yield an
//! empty `Vec`. [`apply`] replays a patch set so that
//! `apply(&mut a, &diff(Some(&a), &b))` makes `a == b` (round-trip property).

use crate::node::Node;
use forge_domain::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// An index path from the root of a tree. `[]` is the root itself.
pub type Path = Vec<usize>;

/// A single mutation against a tree, addressed by index [`Path`].
///
/// Serializes to the wire shapes shared with the golden fixtures, tagged on
/// `"op"`:
/// - `{"op":"replace","path":[..],"node":{..}}`
/// - `{"op":"update_text","path":[..],"value":".."}`
/// - `{"op":"update_prop","path":[..],"key":"..","value":".."}`
/// - `{"op":"insert","path":[..],"node":{..}}`
/// - `{"op":"remove","path":[..]}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Patch {
    /// Replace the node at `path` wholesale (used when the node type changes).
    Replace {
        /// Target node path.
        path: Path,
        /// New node.
        node: Node,
    },
    /// Update the `text` of a [`Node::Text`] at `path`.
    UpdateText {
        /// Target node path.
        path: Path,
        /// New text value.
        value: String,
    },
    /// Update a scalar prop (`label`, `value`, `onTap`, `onChange`) of the node
    /// at `path`.
    UpdateProp {
        /// Target node path.
        path: Path,
        /// Wire prop key (matches the TS-facing name).
        key: String,
        /// New string value.
        value: String,
    },
    /// Insert `node` as the child at the final index of `path` under its parent.
    Insert {
        /// Path of the inserted node (parent + new index).
        path: Path,
        /// Node to insert.
        node: Node,
    },
    /// Remove the child at the final index of `path`.
    Remove {
        /// Path of the node to remove.
        path: Path,
    },
}

impl Patch {
    /// The path this patch targets.
    pub fn path(&self) -> &Path {
        match self {
            Patch::Replace { path, .. }
            | Patch::UpdateText { path, .. }
            | Patch::UpdateProp { path, .. }
            | Patch::Insert { path, .. }
            | Patch::Remove { path } => path,
        }
    }
}

/// Diff `new` against `old`, producing the minimal index-path patch set
/// (prd-merged/05 UI-1). `None` for `old` means "no previous tree" → a single
/// [`Patch::Replace`] at the root.
///
/// Identical trees produce an empty `Vec`.
pub fn diff(old: Option<&Node>, new: &Node) -> Vec<Patch> {
    let mut patches = Vec::new();
    match old {
        None => patches.push(Patch::Replace {
            path: Vec::new(),
            node: new.clone(),
        }),
        Some(old) => diff_node(&mut Vec::new(), old, new, &mut patches),
    }
    patches
}

/// Diff a single node pair at `path`, appending patches.
fn diff_node(path: &mut Path, old: &Node, new: &Node, out: &mut Vec<Patch>) {
    // Type change (or a leaf↔container shift) → replace the whole subtree.
    if old.type_name() != new.type_name() {
        out.push(Patch::Replace {
            path: path.clone(),
            node: new.clone(),
        });
        return;
    }

    match (old, new) {
        (Node::Text { value: a }, Node::Text { value: b }) => {
            if a != b {
                out.push(Patch::UpdateText {
                    path: path.clone(),
                    value: b.clone(),
                });
            }
        }
        (
            Node::Button {
                label: la,
                on_tap: ta,
            },
            Node::Button {
                label: lb,
                on_tap: tb,
            },
        ) => {
            if la != lb {
                out.push(Patch::UpdateProp {
                    path: path.clone(),
                    key: "label".to_string(),
                    value: lb.clone(),
                });
            }
            diff_optional_prop(path, "onTap", ta.as_deref(), tb.as_deref(), new, out);
        }
        (
            Node::TextField {
                value: va,
                on_change: ca,
            },
            Node::TextField {
                value: vb,
                on_change: cb,
            },
        ) => {
            if va != vb {
                out.push(Patch::UpdateProp {
                    path: path.clone(),
                    key: "value".to_string(),
                    value: vb.clone(),
                });
            }
            diff_optional_prop(path, "onChange", ca.as_deref(), cb.as_deref(), new, out);
        }
        (Node::Stack { dir: da, .. }, Node::Stack { dir: db, .. }) => {
            if da != db {
                // Direction is not a scalar string prop with its own patch op in
                // the M0a vocabulary; a layout-axis change re-lays the whole
                // container, so replace it wholesale.
                out.push(Patch::Replace {
                    path: path.clone(),
                    node: new.clone(),
                });
                return;
            }
            diff_children(path, old.children(), new.children(), out);
        }
        (Node::List { .. }, Node::List { .. }) => {
            diff_children(path, old.children(), new.children(), out);
        }
        (Node::Unknown { props: a, .. }, Node::Unknown { props: b, .. }) => {
            // Forward-compat (UI-6): we cannot semantically diff an unknown
            // node, so replace on any change and never error.
            if a != b {
                out.push(Patch::Replace {
                    path: path.clone(),
                    node: new.clone(),
                });
            }
        }
        // Same type tag but mismatched arms can only happen for distinct
        // unknown type names, already handled by the type_name guard above.
        _ => {
            out.push(Patch::Replace {
                path: path.clone(),
                node: new.clone(),
            });
        }
    }
}

/// Emit an `update_prop` for an optional string prop when it changes. A prop
/// that gains or loses its value is treated as a property update carrying the
/// new (possibly empty) value; clearing to `None` re-uses replace to stay
/// lossless rather than inventing a "clear" op the wire vocabulary lacks.
fn diff_optional_prop(
    path: &Path,
    key: &str,
    old: Option<&str>,
    new: Option<&str>,
    new_node: &Node,
    out: &mut Vec<Patch>,
) {
    if old == new {
        return;
    }
    match new {
        Some(v) => out.push(Patch::UpdateProp {
            path: path.clone(),
            key: key.to_string(),
            value: v.to_string(),
        }),
        // Prop dropped (Some -> None): no clear op exists, so replace the node.
        None => out.push(Patch::Replace {
            path: path.clone(),
            node: new_node.clone(),
        }),
    }
}

/// Diff two ordered child lists at `path` by index (no keyed reconciliation).
fn diff_children(path: &mut Path, old: &[Node], new: &[Node], out: &mut Vec<Patch>) {
    let common = old.len().min(new.len());
    for i in 0..common {
        path.push(i);
        diff_node(path, &old[i], &new[i], out);
        path.pop();
    }
    // Appended children → insert in ascending index order.
    for (i, node) in new.iter().enumerate().skip(common) {
        let mut child_path = path.clone();
        child_path.push(i);
        out.push(Patch::Insert {
            path: child_path,
            node: node.clone(),
        });
    }
    // Removed children → remove in descending index order so earlier removals
    // don't shift the indices of still-pending ones.
    for i in (common..old.len()).rev() {
        let mut child_path = path.clone();
        child_path.push(i);
        out.push(Patch::Remove { path: child_path });
    }
}

/// Apply `patches` to `root` in order (prd-merged/05 UI-1). Returns
/// [`CoreError::ValidationError`] if a path is out of range or addresses a node
/// the op can't be applied to (e.g. `update_text` on a Button).
pub fn apply(root: &mut Node, patches: &[Patch]) -> Result<()> {
    for patch in patches {
        apply_one(root, patch)?;
    }
    Ok(())
}

fn apply_one(root: &mut Node, patch: &Patch) -> Result<()> {
    match patch {
        Patch::Replace { path, node } => {
            let target = resolve_mut(root, path)?;
            *target = node.clone();
            Ok(())
        }
        Patch::UpdateText { path, value } => {
            let target = resolve_mut(root, path)?;
            match target {
                Node::Text { value: v } => {
                    *v = value.clone();
                    Ok(())
                }
                other => Err(CoreError::ValidationError(format!(
                    "update_text at {path:?} targets a {} node",
                    other.type_name()
                ))),
            }
        }
        Patch::UpdateProp { path, key, value } => {
            let target = resolve_mut(root, path)?;
            apply_prop(target, key, value, path)
        }
        Patch::Insert { path, node } => {
            let (parent_path, index) = split_path(path)?;
            let parent = resolve_mut(root, parent_path)?;
            let children = children_mut(parent, path)?;
            if index > children.len() {
                return Err(CoreError::ValidationError(format!(
                    "insert index {index} out of range at {path:?}"
                )));
            }
            children.insert(index, node.clone());
            Ok(())
        }
        Patch::Remove { path } => {
            let (parent_path, index) = split_path(path)?;
            let parent = resolve_mut(root, parent_path)?;
            let children = children_mut(parent, path)?;
            if index >= children.len() {
                return Err(CoreError::ValidationError(format!(
                    "remove index {index} out of range at {path:?}"
                )));
            }
            children.remove(index);
            Ok(())
        }
    }
}

/// Set a scalar prop on a known node by its wire key.
fn apply_prop(target: &mut Node, key: &str, value: &str, path: &[usize]) -> Result<()> {
    match (target, key) {
        (Node::Button { label, .. }, "label") => {
            *label = value.to_string();
            Ok(())
        }
        (Node::Button { on_tap, .. }, "onTap") => {
            *on_tap = Some(value.to_string());
            Ok(())
        }
        (Node::TextField { value: v, .. }, "value") => {
            *v = value.to_string();
            Ok(())
        }
        (Node::TextField { on_change, .. }, "onChange") => {
            *on_change = Some(value.to_string());
            Ok(())
        }
        (other, key) => Err(CoreError::ValidationError(format!(
            "update_prop key `{key}` is not valid for a {} node at {path:?}",
            other.type_name()
        ))),
    }
}

/// Resolve a `&mut Node` at `path`, walking container children by index.
fn resolve_mut<'a>(root: &'a mut Node, path: &[usize]) -> Result<&'a mut Node> {
    let mut cur = root;
    for (depth, &idx) in path.iter().enumerate() {
        let here = &path[..=depth];
        let children = children_mut(cur, here)?;
        cur = children.get_mut(idx).ok_or_else(|| {
            CoreError::ValidationError(format!("path index {idx} out of range at {here:?}"))
        })?;
    }
    Ok(cur)
}

/// Borrow the mutable child vector of a container node, or error if `node`
/// is a leaf that has no addressable children.
fn children_mut<'a>(node: &'a mut Node, path: &[usize]) -> Result<&'a mut Vec<Node>> {
    match node {
        Node::Stack { children, .. } => Ok(children),
        Node::List { items } => Ok(items),
        other => Err(CoreError::ValidationError(format!(
            "node at {path:?} is a leaf {} with no children",
            other.type_name()
        ))),
    }
}

/// Split a non-empty path into (parent_path, last_index).
fn split_path(path: &[usize]) -> Result<(&[usize], usize)> {
    match path.split_last() {
        Some((&last, parent)) => Ok((parent, last)),
        None => Err(CoreError::ValidationError(
            "insert/remove requires a non-root path".to_string(),
        )),
    }
}
