//! The declarative component-tree protocol (prd-merged/05 UI-1, UI-2, UI-6, UI-12).
//!
//! [`Node`] is the versioned wire format for the M0a UI catalog subset. It
//! mirrors the TypeScript host contract in `forge/std/forge-std.d.ts` exactly,
//! so the field names on the wire are the TS-facing camelCase ones
//! (`direction`, `onTap`, `onChange`, ...), not Rust snake_case.
//!
//! Forward-compatibility (UI-6, NORMATIVE): any object whose `"type"` is not a
//! known catalog member deserializes to [`Node::Unknown`] — a labeled fallback
//! that preserves the original payload verbatim — rather than erroring. Unknown
//! props on *known* nodes are likewise ignored (serde drops unmatched fields by
//! default), never an error.

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Serializable reference to a host action. M0a rendered trees carry these
/// strings instead of closures; renderers send the referenced action back
/// through the core event queue (prd-merged/05 UI-4, UI-12).
pub type ActionRef = String;

/// Layout direction for a [`Node::Stack`]. Matches the `"h" | "v"` literal in
/// the TS contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StackDir {
    /// Horizontal.
    #[serde(rename = "h")]
    H,
    /// Vertical.
    #[serde(rename = "v")]
    V,
}

/// The M0a declarative UI catalog subset (prd-merged/05 UI-2) plus the
/// forward-compatible [`Node::Unknown`] fallback (UI-6).
///
/// Serialization is serde-tagged on `"type"`; `serde_json` is the canonical
/// wire encoding for golden-tree tests (prd-merged/05 UI-12). Field order on
/// the wire follows declaration order below.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A directional container of child nodes.
    Stack {
        /// Layout direction.
        dir: StackDir,
        /// Ordered children.
        children: Vec<Node>,
    },
    /// A run of display text.
    Text {
        /// The displayed string (wire key `text`).
        value: String,
    },
    /// A tappable button.
    Button {
        /// Button label.
        label: String,
        /// Optional action ref fired on tap (wire key `onTap`).
        on_tap: Option<ActionRef>,
    },
    /// A single-line editable text field.
    TextField {
        /// Current field value.
        value: String,
        /// Optional action ref fired on change (wire key `onChange`).
        on_change: Option<ActionRef>,
    },
    /// A list of item nodes.
    List {
        /// Ordered items.
        items: Vec<Node>,
    },
    /// Forward-compatible fallback for any unknown `"type"` (UI-6, NORMATIVE).
    ///
    /// Preserves the original object verbatim (including the `type` field, kept
    /// in `props` so it round-trips) so a future-aware renderer loses nothing
    /// and a current renderer can show a labeled placeholder.
    Unknown {
        /// The unrecognized `"type"` value.
        type_name: String,
        /// The full original object (the `type` key included) so the node
        /// round-trips byte-for-shape.
        props: serde_json::Map<String, serde_json::Value>,
    },
}

impl Node {
    /// Convenience constructor for a [`Node::Stack`].
    pub fn stack(dir: StackDir, children: Vec<Node>) -> Self {
        Node::Stack { dir, children }
    }

    /// Convenience constructor for a [`Node::Text`].
    pub fn text(value: impl Into<String>) -> Self {
        Node::Text {
            value: value.into(),
        }
    }

    /// Convenience constructor for a [`Node::Button`].
    pub fn button(label: impl Into<String>, on_tap: Option<ActionRef>) -> Self {
        Node::Button {
            label: label.into(),
            on_tap,
        }
    }

    /// Convenience constructor for a [`Node::TextField`].
    pub fn text_field(value: impl Into<String>, on_change: Option<ActionRef>) -> Self {
        Node::TextField {
            value: value.into(),
            on_change,
        }
    }

    /// Convenience constructor for a [`Node::List`].
    pub fn list(items: Vec<Node>) -> Self {
        Node::List { items }
    }

    /// The wire `"type"` tag for this node.
    pub fn type_name(&self) -> &str {
        match self {
            Node::Stack { .. } => "Stack",
            Node::Text { .. } => "Text",
            Node::Button { .. } => "Button",
            Node::TextField { .. } => "TextField",
            Node::List { .. } => "List",
            Node::Unknown { type_name, .. } => type_name,
        }
    }

    /// Whether this node is the forward-compatible fallback (UI-6).
    pub fn is_unknown(&self) -> bool {
        matches!(self, Node::Unknown { .. })
    }

    /// Borrow the ordered child/item nodes a container exposes for diffing.
    /// Leaf nodes return an empty slice.
    pub(crate) fn children(&self) -> &[Node] {
        match self {
            Node::Stack { children, .. } => children,
            Node::List { items } => items,
            _ => &[],
        }
    }
}

// --- Manual serde --------------------------------------------------------
//
// We hand-roll (de)serialization because the catch-all `Unknown` arm cannot be
// expressed with `#[serde(tag = "type")]` (serde's internally-tagged enums
// error on unknown tags, which would violate UI-6). The known arms still emit
// exactly the TS-facing wire shapes.

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Node::Stack { dir, children } => {
                let mut m = serializer.serialize_map(Some(3))?;
                m.serialize_entry("type", "Stack")?;
                m.serialize_entry("direction", dir)?;
                m.serialize_entry("children", children)?;
                m.end()
            }
            Node::Text { value } => {
                let mut m = serializer.serialize_map(Some(2))?;
                m.serialize_entry("type", "Text")?;
                m.serialize_entry("text", value)?;
                m.end()
            }
            Node::Button { label, on_tap } => {
                let mut m = serializer.serialize_map(Some(3))?;
                m.serialize_entry("type", "Button")?;
                m.serialize_entry("label", label)?;
                if let Some(a) = on_tap {
                    m.serialize_entry("onTap", a)?;
                }
                m.end()
            }
            Node::TextField { value, on_change } => {
                let mut m = serializer.serialize_map(Some(3))?;
                m.serialize_entry("type", "TextField")?;
                m.serialize_entry("value", value)?;
                if let Some(a) = on_change {
                    m.serialize_entry("onChange", a)?;
                }
                m.end()
            }
            Node::List { items } => {
                let mut m = serializer.serialize_map(Some(2))?;
                m.serialize_entry("type", "List")?;
                m.serialize_entry("items", items)?;
                m.end()
            }
            Node::Unknown { props, .. } => {
                // Re-emit the original object verbatim (type key included).
                let mut m = serializer.serialize_map(Some(props.len()))?;
                for (k, v) in props {
                    m.serialize_entry(k, v)?;
                }
                m.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(NodeVisitor)
    }
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a forge UI node object with a \"type\" field")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Node, M::Error> {
        // Buffer the whole object as JSON so we can (a) read `type` then (b)
        // fall back to a verbatim `Unknown` if it isn't a known catalog member.
        let mut obj = serde_json::Map::new();
        while let Some((k, v)) = map.next_entry::<String, serde_json::Value>()? {
            obj.insert(k, v);
        }

        let type_name = match obj.get("type").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            // No string `type` at all → treat as unknown fallback (UI-6),
            // never an error.
            None => {
                let tn = obj
                    .get("type")
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                return Ok(Node::Unknown {
                    type_name: tn,
                    props: obj,
                });
            }
        };

        let node = match type_name.as_str() {
            "Stack" => {
                let dir = match obj.get("direction").and_then(|v| v.as_str()) {
                    Some("h") => StackDir::H,
                    // Default to vertical when absent/unrecognized — tolerant by
                    // design, consistent with the TS contract's optional field.
                    _ => StackDir::V,
                };
                let children = take_node_array(&obj, "children").map_err(de::Error::custom)?;
                Node::Stack { dir, children }
            }
            "Text" => {
                let value = obj
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                Node::Text { value }
            }
            "Button" => {
                let label = obj
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let on_tap = obj
                    .get("onTap")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                Node::Button { label, on_tap }
            }
            "TextField" => {
                let value = obj
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let on_change = obj
                    .get("onChange")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                Node::TextField { value, on_change }
            }
            "List" => {
                let items = take_node_array(&obj, "items").map_err(de::Error::custom)?;
                Node::List { items }
            }
            // Unknown catalog member → forward-compatible fallback (UI-6).
            _ => Node::Unknown {
                type_name,
                props: obj,
            },
        };
        Ok(node)
    }
}

/// Decode the named field of `obj` as a `Vec<Node>`, defaulting to empty when
/// the field is absent. Each element is recursively decoded as a `Node` so
/// nested unknowns also become fallbacks (UI-6).
fn take_node_array(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Vec<Node>, String> {
    match obj.get(key) {
        None => Ok(Vec::new()),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .map(|v| serde_json::from_value::<Node>(v.clone()).map_err(|e| e.to_string()))
            .collect(),
        Some(other) => Err(format!(
            "field `{key}` must be an array of nodes, found {other}"
        )),
    }
}
