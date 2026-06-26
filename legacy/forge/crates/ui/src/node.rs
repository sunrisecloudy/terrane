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

/// Shared identity fields every catalog node may carry (`BaseNode` in the TS
/// contract, `forge/std/forge-std.d.ts`). `testId` in particular is a stable
/// element handle the renderer/test harness relies on (T018 e2e fixtures), so
/// it must survive (de)serialization and diffing rather than being dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseNode {
    /// Optional stable identifier (wire key `id`).
    pub id: Option<String>,
    /// Optional test/renderer handle (wire key `testId`).
    pub test_id: Option<String>,
}

impl BaseNode {
    /// Read the base fields from a buffered wire object.
    fn from_obj(obj: &serde_json::Map<String, serde_json::Value>) -> Self {
        BaseNode {
            id: take_str(obj, "id"),
            test_id: take_str(obj, "testId"),
        }
    }

    /// Emit the base fields (when present) into a serialized map. The TS
    /// contract orders `id`/`testId` ahead of the type-specific props, so we
    /// emit them right after `"type"`.
    fn serialize_into<M: SerializeMap>(&self, map: &mut M) -> Result<(), M::Error> {
        if let Some(id) = &self.id {
            map.serialize_entry("id", id)?;
        }
        if let Some(test_id) = &self.test_id {
            map.serialize_entry("testId", test_id)?;
        }
        Ok(())
    }
}

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
        /// Shared `id`/`testId` (BaseNode).
        base: BaseNode,
        /// Layout direction.
        dir: StackDir,
        /// Optional inter-child spacing token (wire key `gap`,
        /// `"none" | "xs" | "sm" | "md" | "lg"`). Stored as a string to stay
        /// lossless and forward-compatible with future tokens.
        gap: Option<String>,
        /// Ordered children.
        children: Vec<Node>,
    },
    /// A run of display text.
    Text {
        /// Shared `id`/`testId` (BaseNode).
        base: BaseNode,
        /// The displayed string (wire key `text`).
        value: String,
        /// Optional typographic variant (wire key `variant`,
        /// `"body" | "caption" | "title" | "monospace"`). Stored as a string to
        /// stay lossless and forward-compatible.
        variant: Option<String>,
    },
    /// A tappable button.
    Button {
        /// Shared `id`/`testId` (BaseNode).
        base: BaseNode,
        /// Button label.
        label: String,
        /// Optional visual variant (wire key `variant`,
        /// `"primary" | "secondary" | "destructive"`). Stored as a string to
        /// stay lossless and forward-compatible.
        variant: Option<String>,
        /// Optional explicit accessible name (wire key `ariaLabel`, UI-7).
        ///
        /// Per `spec/accessibility.md` a Button's accessible name is its
        /// `label`, but an *icon-only* Button (empty `label`) MUST supply
        /// `ariaLabel`; the name is never inferred from an icon. Additive and
        /// `#[serde(default)]`-equivalent (omitted when `None`).
        aria_label: Option<String>,
        /// Optional action ref fired on tap (wire key `onTap`).
        on_tap: Option<ActionRef>,
    },
    /// A single-line editable text field.
    TextField {
        /// Shared `id`/`testId` (BaseNode).
        base: BaseNode,
        /// Current field value.
        value: String,
        /// Optional field label (wire key `label`).
        label: Option<String>,
        /// Optional explicit accessible name (wire key `ariaLabel`, UI-7).
        ///
        /// Per `spec/accessibility.md` a TextField requires a label;
        /// `placeholder` never counts. `ariaLabel` is an alternative label
        /// source for an unlabelled-but-named field. Additive and
        /// `#[serde(default)]`-equivalent (omitted when `None`).
        aria_label: Option<String>,
        /// Optional placeholder shown when empty (wire key `placeholder`).
        placeholder: Option<String>,
        /// Optional action ref fired on change (wire key `onChange`).
        on_change: Option<ActionRef>,
    },
    /// A list of item nodes.
    List {
        /// Shared `id`/`testId` (BaseNode).
        base: BaseNode,
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
    /// Convenience constructor for a [`Node::Stack`] with no base/gap set.
    pub fn stack(dir: StackDir, children: Vec<Node>) -> Self {
        Node::Stack {
            base: BaseNode::default(),
            dir,
            gap: None,
            children,
        }
    }

    /// Convenience constructor for a [`Node::Text`] with no base/variant set.
    pub fn text(value: impl Into<String>) -> Self {
        Node::Text {
            base: BaseNode::default(),
            value: value.into(),
            variant: None,
        }
    }

    /// Convenience constructor for a [`Node::Button`] with no base/variant set.
    pub fn button(label: impl Into<String>, on_tap: Option<ActionRef>) -> Self {
        Node::Button {
            base: BaseNode::default(),
            label: label.into(),
            variant: None,
            aria_label: None,
            on_tap,
        }
    }

    /// Convenience constructor for a [`Node::TextField`] with no
    /// base/label/placeholder set.
    pub fn text_field(value: impl Into<String>, on_change: Option<ActionRef>) -> Self {
        Node::TextField {
            base: BaseNode::default(),
            value: value.into(),
            label: None,
            aria_label: None,
            placeholder: None,
            on_change,
        }
    }

    /// Convenience constructor for a [`Node::List`] with no base set.
    pub fn list(items: Vec<Node>) -> Self {
        Node::List {
            base: BaseNode::default(),
            items,
        }
    }

    /// Set the shared `testId` (BaseNode) on this node, returning `self` for
    /// builder-style use. Unknown nodes carry `testId` in their verbatim
    /// `props`, so this is a no-op for them.
    pub fn with_test_id(mut self, test_id: impl Into<String>) -> Self {
        if let Some(base) = self.base_mut() {
            base.test_id = Some(test_id.into());
        }
        self
    }

    /// Set the shared `id` (BaseNode) on this node, builder-style.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        if let Some(base) = self.base_mut() {
            base.id = Some(id.into());
        }
        self
    }

    /// Set the explicit accessible name (`ariaLabel`, UI-7) on a node that
    /// carries one (Button / TextField), builder-style. A no-op for nodes
    /// without an `ariaLabel` slot.
    pub fn with_aria_label(mut self, label: impl Into<String>) -> Self {
        match &mut self {
            Node::Button { aria_label, .. } | Node::TextField { aria_label, .. } => {
                *aria_label = Some(label.into());
            }
            _ => {}
        }
        self
    }

    /// Mutable access to a known node's [`BaseNode`]; `None` for unknowns.
    fn base_mut(&mut self) -> Option<&mut BaseNode> {
        match self {
            Node::Stack { base, .. }
            | Node::Text { base, .. }
            | Node::Button { base, .. }
            | Node::TextField { base, .. }
            | Node::List { base, .. } => Some(base),
            Node::Unknown { .. } => None,
        }
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
            Node::List { items, .. } => items,
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
            Node::Stack {
                base,
                dir,
                gap,
                children,
            } => {
                let mut m = serializer.serialize_map(None)?;
                m.serialize_entry("type", "Stack")?;
                base.serialize_into(&mut m)?;
                m.serialize_entry("direction", dir)?;
                if let Some(gap) = gap {
                    m.serialize_entry("gap", gap)?;
                }
                m.serialize_entry("children", children)?;
                m.end()
            }
            Node::Text {
                base,
                value,
                variant,
            } => {
                let mut m = serializer.serialize_map(None)?;
                m.serialize_entry("type", "Text")?;
                base.serialize_into(&mut m)?;
                m.serialize_entry("text", value)?;
                if let Some(variant) = variant {
                    m.serialize_entry("variant", variant)?;
                }
                m.end()
            }
            Node::Button {
                base,
                label,
                variant,
                aria_label,
                on_tap,
            } => {
                let mut m = serializer.serialize_map(None)?;
                m.serialize_entry("type", "Button")?;
                base.serialize_into(&mut m)?;
                m.serialize_entry("label", label)?;
                if let Some(variant) = variant {
                    m.serialize_entry("variant", variant)?;
                }
                if let Some(aria_label) = aria_label {
                    m.serialize_entry("ariaLabel", aria_label)?;
                }
                if let Some(a) = on_tap {
                    m.serialize_entry("onTap", a)?;
                }
                m.end()
            }
            Node::TextField {
                base,
                value,
                label,
                aria_label,
                placeholder,
                on_change,
            } => {
                let mut m = serializer.serialize_map(None)?;
                m.serialize_entry("type", "TextField")?;
                base.serialize_into(&mut m)?;
                m.serialize_entry("value", value)?;
                if let Some(label) = label {
                    m.serialize_entry("label", label)?;
                }
                if let Some(aria_label) = aria_label {
                    m.serialize_entry("ariaLabel", aria_label)?;
                }
                if let Some(placeholder) = placeholder {
                    m.serialize_entry("placeholder", placeholder)?;
                }
                if let Some(a) = on_change {
                    m.serialize_entry("onChange", a)?;
                }
                m.end()
            }
            Node::List { base, items } => {
                let mut m = serializer.serialize_map(None)?;
                m.serialize_entry("type", "List")?;
                base.serialize_into(&mut m)?;
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

        let base = BaseNode::from_obj(&obj);
        let node = match type_name.as_str() {
            "Stack" => {
                let dir = match obj.get("direction").and_then(|v| v.as_str()) {
                    Some("h") => StackDir::H,
                    // Default to vertical when absent/unrecognized — tolerant by
                    // design, consistent with the TS contract's optional field.
                    _ => StackDir::V,
                };
                let gap = take_str(&obj, "gap");
                let children = take_node_array(&obj, "children").map_err(de::Error::custom)?;
                Node::Stack {
                    base,
                    dir,
                    gap,
                    children,
                }
            }
            "Text" => {
                let value = obj
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let variant = take_str(&obj, "variant");
                Node::Text {
                    base,
                    value,
                    variant,
                }
            }
            "Button" => {
                let label = obj
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let variant = take_str(&obj, "variant");
                let aria_label = take_str(&obj, "ariaLabel");
                let on_tap = take_str(&obj, "onTap");
                Node::Button {
                    base,
                    label,
                    variant,
                    aria_label,
                    on_tap,
                }
            }
            "TextField" => {
                let value = obj
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let label = take_str(&obj, "label");
                let aria_label = take_str(&obj, "ariaLabel");
                let placeholder = take_str(&obj, "placeholder");
                let on_change = take_str(&obj, "onChange");
                Node::TextField {
                    base,
                    value,
                    label,
                    aria_label,
                    placeholder,
                    on_change,
                }
            }
            "List" => {
                let items = take_node_array(&obj, "items").map_err(de::Error::custom)?;
                Node::List { base, items }
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

/// Read an optional string field from a buffered wire object. Non-string
/// values (or absent keys) yield `None`, staying tolerant per UI-6.
fn take_str(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
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
