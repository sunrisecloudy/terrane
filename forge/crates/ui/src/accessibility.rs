//! UI-7 accessibility role / accessible-name emission and required-name
//! validation for the declarative UI catalog (prd-merged/05 UI-7, source of
//! record `spec/accessibility.md`).
//!
//! Two responsibilities live here, both additive over the existing
//! [`Node`](crate::Node) protocol (no wire-shape change to known nodes):
//!
//! 1. **Emission** — [`Node::accessibility`] derives the ARIA role and the
//!    accessible name (plus its *source*) every rendered node exposes, exactly
//!    per the component table in `spec/accessibility.md`. Renderers/tests read
//!    this instead of re-deriving the mapping.
//! 2. **Validation** — [`validate_accessibility`] enforces the spec's
//!    REQUIRED accessible-name rules as [`CoreError::ValidationError`]s,
//!    mirroring the existing forge-ui validation idiom (precise,
//!    component-named messages). A compliant tree passes; a violating tree
//!    fails with a message naming the offending component.
//!
//! The typed catalog (`Stack`/`Text`/`Button`/`TextField`/`List`) is emitted
//! and validated from its Rust variants. The remaining `@forge/std` components
//! in the spec table (`Icon`, `Image`, `Chart`, `Table`, `Form`, the other form
//! controls, ...) reach this crate as forward-compatible
//! [`Node::Unknown`](crate::Node::Unknown) fallbacks (UI-6); their role/name and
//! required-name rules are derived from their verbatim props by component name,
//! so the spec contract still holds for them without breaking UI-6.

use crate::node::Node;
use forge_domain::{CoreError, Result};

/// The ARIA role a rendered node exposes to assistive technology
/// (`spec/accessibility.md`, "Role" column).
///
/// Stored as an open string under [`AxRole`] rather than a closed enum so the
/// full `@forge/std` catalog — including components that only reach this crate
/// as [`Node::Unknown`](crate::Node::Unknown) — maps cleanly and stays
/// forward-compatible with future roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxRole(pub String);

impl AxRole {
    /// The ARIA role token (e.g. `"button"`, `"textbox"`, `"group"`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<S: Into<String>> From<S> for AxRole {
    fn from(s: S) -> Self {
        AxRole(s.into())
    }
}

/// Where a node's accessible name comes from (`spec/accessibility.md`,
/// "Accessible name rule" column).
///
/// Recording the *source* (not just the resolved string) lets renderers and
/// conformance tests distinguish, say, a Button named by its visible `label`
/// from one named by an explicit `ariaLabel`, and lets decorative elements
/// declare they intentionally expose no name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxNameSource {
    /// Name taken from the node's visible text content (Text, Markdown).
    Content,
    /// Name taken from the node's visible `label` field (Button, ...).
    Label,
    /// Name taken from an explicit `ariaLabel` (icon-only Button, Icon, ...).
    AriaLabel,
    /// Name taken from `alt` text (Image).
    Alt,
    /// Name taken from a `caption` (Table).
    Caption,
    /// Name taken from a `summary` (Chart).
    Summary,
    /// Name taken from a `title` (Modal).
    Title,
    /// The element is presentational/decorative and intentionally exposes no
    /// accessible name.
    None,
}

/// The accessibility annotation a rendered node carries (UI-7): its ARIA role,
/// its accessible name (when it exposes one) and the source that name was
/// derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accessibility {
    /// ARIA role exposed to assistive tech.
    pub role: AxRole,
    /// Resolved accessible name, or `None` for presentational/decorative nodes.
    pub name: Option<String>,
    /// How `name` was derived (or [`AxNameSource::None`]).
    pub name_source: AxNameSource,
}

impl Accessibility {
    fn new(role: impl Into<AxRole>, name: Option<String>, name_source: AxNameSource) -> Self {
        Accessibility {
            role: role.into(),
            name,
            name_source,
        }
    }
}

/// Treat an optional string as "present" only when it has non-whitespace
/// content — an empty/blank `ariaLabel`/`alt`/`caption` is not a name.
fn non_blank(s: Option<&str>) -> Option<&str> {
    match s {
        Some(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

/// Read a non-blank string prop from an [`Node::Unknown`] verbatim-props object.
fn prop_str<'a>(
    props: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    non_blank(props.get(key).and_then(|v| v.as_str()))
}

impl Node {
    /// Emit this node's accessibility role + accessible name + name source per
    /// `spec/accessibility.md` (UI-7).
    ///
    /// This is the single source of truth a renderer reads to label a node; it
    /// does **not** validate (see [`validate_accessibility`]). For an
    /// [`Node::Unknown`](crate::Node::Unknown) it follows the spec's "Unknown
    /// Component Fallback": a labelled `group` named `"Unsupported component
    /// <Type>"`, never the raw JSON.
    pub fn accessibility(&self) -> Accessibility {
        match self {
            Node::Stack { .. } => Accessibility::new("group", None, AxNameSource::None),
            Node::List { .. } => Accessibility::new("list", None, AxNameSource::None),
            Node::Text { value, .. } => {
                // Text content is the accessible name.
                Accessibility::new("text", Some(value.clone()), AxNameSource::Content)
            }
            Node::Button {
                label, aria_label, ..
            } => {
                // Visible label is the name; an icon-only (blank-label) button
                // is named by its ariaLabel. Never inferred from an icon.
                if let Some(l) = non_blank(Some(label.as_str())) {
                    Accessibility::new("button", Some(l.to_string()), AxNameSource::Label)
                } else if let Some(al) = non_blank(aria_label.as_deref()) {
                    Accessibility::new("button", Some(al.to_string()), AxNameSource::AriaLabel)
                } else {
                    // Unnamed (validation rejects this); emit no name.
                    Accessibility::new("button", None, AxNameSource::None)
                }
            }
            Node::TextField {
                label, aria_label, ..
            } => {
                // Label (or ariaLabel) is the name; placeholder never counts.
                let name = non_blank(label.as_deref())
                    .map(|s| (s.to_string(), AxNameSource::Label))
                    .or_else(|| {
                        non_blank(aria_label.as_deref())
                            .map(|s| (s.to_string(), AxNameSource::AriaLabel))
                    });
                match name {
                    Some((n, src)) => Accessibility::new("textbox", Some(n), src),
                    None => Accessibility::new("textbox", None, AxNameSource::None),
                }
            }
            Node::Unknown { type_name, props } => unknown_accessibility(type_name, props),
        }
    }
}

/// Accessibility for catalog components that reach this crate only as
/// [`Node::Unknown`](crate::Node::Unknown) — the structural containers
/// (Grid/Card/Scroll/Spacer/Divider/Markdown/Tabs), the media/region components
/// (Icon/Image/Chart/Table/Modal/Form) and the form controls — keyed off the
/// verbatim `type` per `spec/accessibility.md`. Every component named in the
/// spec table emits its spec role here; only a `type` genuinely OUTSIDE the
/// catalog falls through to the UI-6 "Unsupported component <Type>" group.
fn unknown_accessibility(
    type_name: &str,
    props: &serde_json::Map<String, serde_json::Value>,
) -> Accessibility {
    let aria = prop_str(props, "ariaLabel");
    match type_name {
        // Structural containers (`spec/accessibility.md` table). These are real
        // catalog members, NOT the UI-6 unknown fallback: they expose a grouping
        // role and an OPTIONAL `ariaLabel` (no name when absent), and never the
        // raw JSON. (`Stack` is a typed node handled in `Node::accessibility`;
        // `Grid` arrives here as a UI-6 fallback.) Grid is `grid` when
        // interactive (has cells/columns), else a plain `group`; Card/Scroll
        // become `region` once labelled.
        "Grid" => {
            let role = if is_interactive_grid(props) {
                "grid"
            } else {
                "group"
            };
            optional_aria_named(role, aria)
        }
        "Card" => optional_aria_named(if aria.is_some() { "region" } else { "group" }, aria),
        "Scroll" => optional_aria_named(if aria.is_some() { "region" } else { "group" }, aria),
        // Spacer / Divider are presentational; Divider is a `separator` and may
        // carry an optional meaningful label.
        "Spacer" => Accessibility::new("presentation", None, AxNameSource::None),
        "Divider" => optional_aria_named("separator", aria),
        // Markdown is a `document`; its visible content supplies names, so it
        // exposes no container-level accessible name of its own.
        "Markdown" => Accessibility::new("document", None, AxNameSource::None),
        // Tabs exposes a `tablist`; individual tab labels are required but live
        // on child tab descriptors, so the tablist itself is named (optionally)
        // by an `ariaLabel`.
        "Tabs" => optional_aria_named("tablist", aria),
        "Icon" => {
            // Decorative icons expose nothing; informative icons need ariaLabel.
            if is_decorative(props) {
                Accessibility::new("presentation", None, AxNameSource::None)
            } else {
                Accessibility::new(
                    "img",
                    aria.map(str::to_string),
                    if aria.is_some() {
                        AxNameSource::AriaLabel
                    } else {
                        AxNameSource::None
                    },
                )
            }
        }
        "Image" => {
            let alt = prop_str(props, "alt");
            Accessibility::new(
                "img",
                alt.map(str::to_string),
                if alt.is_some() {
                    AxNameSource::Alt
                } else {
                    AxNameSource::None
                },
            )
        }
        "Chart" => {
            let summary = prop_str(props, "summary");
            Accessibility::new(
                "img",
                summary.map(str::to_string),
                if summary.is_some() {
                    AxNameSource::Summary
                } else {
                    AxNameSource::None
                },
            )
        }
        "Table" => {
            let name = prop_str(props, "caption")
                .map(|s| (s.to_string(), AxNameSource::Caption))
                .or_else(|| aria.map(|s| (s.to_string(), AxNameSource::AriaLabel)));
            match name {
                Some((n, src)) => Accessibility::new("table", Some(n), src),
                None => Accessibility::new("table", None, AxNameSource::None),
            }
        }
        "Modal" => {
            let title = prop_str(props, "title");
            Accessibility::new(
                "dialog",
                title.map(str::to_string),
                if title.is_some() {
                    AxNameSource::Title
                } else {
                    AxNameSource::None
                },
            )
        }
        "Form" => Accessibility::new("form", aria.map(str::to_string), AxNameSource::AriaLabel),
        // Form controls whose name comes from `label` (or `ariaLabel`).
        "TextArea" | "Select" | "MultiSelect" | "Checkbox" | "Switch" | "Slider"
        | "DatePicker" | "Badge" | "Stat" => {
            let role = control_role(type_name);
            let name = prop_str(props, "label")
                .map(|s| (s.to_string(), AxNameSource::Label))
                .or_else(|| aria.map(|s| (s.to_string(), AxNameSource::AriaLabel)));
            match name {
                Some((n, src)) => Accessibility::new(role, Some(n), src),
                None => Accessibility::new(role, None, AxNameSource::None),
            }
        }
        // UI-6 fallback: a labelled group, NOT the raw JSON, never focusable
        // (focusability is the renderer's concern). `spec/accessibility.md`
        // "Unknown Component Fallback".
        other => Accessibility::new(
            "group",
            Some(format!("Unsupported component {other}")),
            AxNameSource::Label,
        ),
    }
}

/// A grouping/structural node whose accessible name is an OPTIONAL `ariaLabel`:
/// it carries the name when present and exposes none otherwise (the spec marks
/// these names "optional", so absence is not a violation).
fn optional_aria_named(role: &'static str, aria: Option<&str>) -> Accessibility {
    match aria {
        Some(a) => Accessibility::new(role, Some(a.to_string()), AxNameSource::AriaLabel),
        None => Accessibility::new(role, None, AxNameSource::None),
    }
}

/// Whether a `Grid` is interactive enough to expose the `grid` role rather than
/// a plain `group` (`spec/accessibility.md`: "group/grid when interactive").
/// Heuristic over the verbatim props: an explicit `interactive`/`selectable`
/// flag, or a declared column/row structure.
fn is_interactive_grid(props: &serde_json::Map<String, serde_json::Value>) -> bool {
    props
        .get("interactive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || props
            .get("selectable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || props.contains_key("columns")
        || props.contains_key("rows")
}

/// The ARIA role for a known-by-name form control (`spec/accessibility.md`).
fn control_role(type_name: &str) -> &'static str {
    match type_name {
        "TextArea" => "textbox",
        "Select" => "combobox",
        "MultiSelect" => "listbox",
        "Checkbox" => "checkbox",
        "Switch" => "switch",
        "Slider" => "slider",
        "DatePicker" => "combobox",
        "Badge" | "Stat" => "status",
        _ => "group",
    }
}

/// Whether an Icon declared itself decorative (`decorative: true`).
fn is_decorative(props: &serde_json::Map<String, serde_json::Value>) -> bool {
    props
        .get("decorative")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Validate the REQUIRED accessible-name rules from `spec/accessibility.md`
/// across a whole tree (UI-7). Returns the first violation as a
/// [`CoreError::ValidationError`] naming the offending component; `Ok(())` when
/// every component in the tree satisfies its rule.
///
/// Enforced rules (per the spec table + "Form Label-Presence Rule" +
/// "Ambiguous Name Sources To Decide"):
/// - Button: `label` or `ariaLabel` required; an icon-only (blank-label) Button
///   MUST provide `ariaLabel`.
/// - TextField / TextArea / Select / MultiSelect / Slider / DatePicker:
///   `label` required (`placeholder` never counts).
/// - Checkbox / Switch: `label` or `ariaLabel` required.
/// - Icon: must declare `decorative: true` or supply an informative
///   `ariaLabel`.
/// - Image: `alt` required (empty `alt` allowed only when `decorative: true`).
/// - Chart: `summary` required.
/// - Table: a standalone Table needs `caption` or `ariaLabel`.
/// - Form: every interactive descendant control must satisfy its label rule.
pub fn validate_accessibility(node: &Node) -> Result<()> {
    validate_node(node)?;
    match node {
        Node::Stack { children, .. } => {
            for child in children {
                validate_accessibility(child)?;
            }
        }
        Node::List { items, .. } => {
            for item in items {
                validate_accessibility(item)?;
            }
        }
        Node::Unknown { props, .. } => {
            // Unknown containers (e.g. a `Form`/`Modal` arriving as a UI-6
            // fallback) hold descendants as raw JSON, not typed `Node`s. Re-parse
            // any nested node arrays so interactive descendants are still
            // validated — this is what keeps Form's label-presence rule working
            // even when Form is not a typed catalog member.
            for child in unknown_children(props) {
                validate_accessibility(&child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Re-parse the nested node arrays an [`Node::Unknown`](crate::Node::Unknown)
/// carries verbatim (`children`/`items`) into owned [`Node`]s for accessibility
/// traversal. Non-array / non-node entries are skipped (tolerant per UI-6).
fn unknown_children(props: &serde_json::Map<String, serde_json::Value>) -> Vec<Node> {
    let mut out = Vec::new();
    for key in ["children", "items"] {
        if let Some(serde_json::Value::Array(arr)) = props.get(key) {
            for v in arr {
                if let Ok(node) = serde_json::from_value::<Node>(v.clone()) {
                    out.push(node);
                }
            }
        }
    }
    out
}

/// Validate a single node's own required-name rule (not its descendants).
fn validate_node(node: &Node) -> Result<()> {
    match node {
        Node::Button {
            label, aria_label, ..
        } => {
            let has_label = non_blank(Some(label.as_str())).is_some();
            let has_aria = non_blank(aria_label.as_deref()).is_some();
            if !has_label && !has_aria {
                return Err(name_err(
                    "Button",
                    "an icon-only Button (empty label) must provide `ariaLabel`; \
                     a Button requires `label` or `ariaLabel`",
                ));
            }
            Ok(())
        }
        Node::TextField {
            label,
            aria_label,
            placeholder,
            ..
        } => {
            if non_blank(label.as_deref()).is_some() || non_blank(aria_label.as_deref()).is_some() {
                return Ok(());
            }
            // Placeholder explicitly never counts as a label.
            if non_blank(placeholder.as_deref()).is_some() {
                return Err(name_err(
                    "TextField",
                    "requires a `label` (or `ariaLabel`); `placeholder` is not a label",
                ));
            }
            Err(name_err("TextField", "requires a `label` (or `ariaLabel`)"))
        }
        Node::Unknown { type_name, props } => validate_unknown(type_name, props),
        // Stack / Text / List have no required accessible name.
        _ => Ok(()),
    }
}

/// Required-name rules for catalog components carried as
/// [`Node::Unknown`](crate::Node::Unknown) (UI-6) — keyed off the verbatim
/// `type`.
fn validate_unknown(
    type_name: &str,
    props: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let has = |k: &str| prop_str(props, k).is_some();
    match type_name {
        "Icon" => {
            if is_decorative(props) || has("ariaLabel") {
                Ok(())
            } else {
                Err(name_err(
                    "Icon",
                    "must declare `decorative: true` or provide an informative `ariaLabel`",
                ))
            }
        }
        "Image" => {
            // alt required; empty alt only when explicitly decorative.
            if has("alt") || is_decorative(props) {
                Ok(())
            } else {
                Err(name_err(
                    "Image",
                    "requires `alt` text (empty `alt` allowed only when `decorative: true`)",
                ))
            }
        }
        "Chart" => {
            if has("summary") {
                Ok(())
            } else {
                Err(name_err("Chart", "requires a `summary`"))
            }
        }
        "Table" => {
            if has("caption") || has("ariaLabel") {
                Ok(())
            } else {
                Err(name_err(
                    "Table",
                    "a standalone Table requires `caption` or `ariaLabel`",
                ))
            }
        }
        // Form controls requiring `label` (placeholder/value never counts).
        "TextArea" | "Select" | "MultiSelect" | "Slider" | "DatePicker" | "Badge" | "Stat" => {
            if has("label") || has("ariaLabel") {
                Ok(())
            } else {
                Err(name_err(type_name, "requires a `label`"))
            }
        }
        // Controls requiring `label` OR `ariaLabel`.
        "Checkbox" | "Switch" => {
            if has("label") || has("ariaLabel") {
                Ok(())
            } else {
                Err(name_err(type_name, "requires a `label` or `ariaLabel`"))
            }
        }
        // Other unknown types use the UI-6 fallback group and impose no
        // required-name rule.
        _ => Ok(()),
    }
}

/// Build a precise, component-named accessible-name validation error mirroring
/// the existing forge-ui [`CoreError::ValidationError`] idiom.
fn name_err(component: &str, detail: &str) -> CoreError {
    CoreError::ValidationError(format!(
        "accessibility: {component} accessible name rule violated: {detail}"
    ))
}
