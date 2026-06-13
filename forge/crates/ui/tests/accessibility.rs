//! UI-7 accessibility tests: role / accessible-name emission and required-name
//! validation (source of record `spec/accessibility.md`).
//!
//! Two halves:
//!   - emission: representative components emit the spec's role + accessible
//!     name + name source.
//!   - validation: each REQUIRED-name rule rejects the bad case and accepts the
//!     good case with a precise, component-named error.

use forge_ui::{
    from_str, validate_accessibility, AxNameSource, Node, StackDir,
};

fn ax(node: &Node) -> forge_ui::Accessibility {
    node.accessibility()
}

// --- Emission: role + accessible name per the spec table ------------------

#[test]
fn text_emits_text_role_named_by_content() {
    let a = ax(&Node::text("Inbox"));
    assert_eq!(a.role.as_str(), "text");
    assert_eq!(a.name.as_deref(), Some("Inbox"));
    assert_eq!(a.name_source, AxNameSource::Content);
}

#[test]
fn button_emits_button_role_named_by_label() {
    let a = ax(&Node::button("Save", Some("doc.save".into())));
    assert_eq!(a.role.as_str(), "button");
    assert_eq!(a.name.as_deref(), Some("Save"));
    assert_eq!(a.name_source, AxNameSource::Label);
}

#[test]
fn icon_only_button_is_named_by_aria_label_not_label() {
    // Empty label → icon-only; ariaLabel supplies the name (never inferred).
    let btn = Node::button("", Some("nav.close".into())).with_aria_label("Close");
    let a = ax(&btn);
    assert_eq!(a.role.as_str(), "button");
    assert_eq!(a.name.as_deref(), Some("Close"));
    assert_eq!(a.name_source, AxNameSource::AriaLabel);
}

#[test]
fn textfield_emits_textbox_role_named_by_label_not_placeholder() {
    let tf = Node::TextField {
        base: forge_ui::BaseNode::default(),
        value: String::new(),
        label: Some("Name".into()),
        aria_label: None,
        placeholder: Some("Type your name".into()),
        on_change: None,
    };
    let a = ax(&tf);
    assert_eq!(a.role.as_str(), "textbox");
    assert_eq!(a.name.as_deref(), Some("Name"));
    assert_eq!(a.name_source, AxNameSource::Label);
}

#[test]
fn stack_and_list_emit_grouping_roles_without_a_name() {
    let stack = ax(&Node::stack(StackDir::V, vec![]));
    assert_eq!(stack.role.as_str(), "group");
    assert_eq!(stack.name, None);
    assert_eq!(stack.name_source, AxNameSource::None);

    let list = ax(&Node::list(vec![]));
    assert_eq!(list.role.as_str(), "list");
    assert_eq!(list.name, None);
}

#[test]
fn image_emits_img_role_named_by_alt() {
    let img = from_str(r#"{"type":"Image","src":"a.png","alt":"A red barn"}"#).unwrap();
    let a = ax(&img);
    assert_eq!(a.role.as_str(), "img");
    assert_eq!(a.name.as_deref(), Some("A red barn"));
    assert_eq!(a.name_source, AxNameSource::Alt);
}

#[test]
fn chart_emits_img_role_named_by_summary() {
    let chart =
        from_str(r#"{"type":"Chart","kind":"line","summary":"Revenue up 12%"}"#).unwrap();
    let a = ax(&chart);
    assert_eq!(a.role.as_str(), "img");
    assert_eq!(a.name.as_deref(), Some("Revenue up 12%"));
    assert_eq!(a.name_source, AxNameSource::Summary);
}

#[test]
fn table_emits_table_role_named_by_caption() {
    let table = from_str(r#"{"type":"Table","caption":"Q3 sales"}"#).unwrap();
    let a = ax(&table);
    assert_eq!(a.role.as_str(), "table");
    assert_eq!(a.name.as_deref(), Some("Q3 sales"));
    assert_eq!(a.name_source, AxNameSource::Caption);
}

#[test]
fn informative_icon_named_by_aria_label_decorative_is_presentation() {
    let info = from_str(r#"{"type":"Icon","glyph":"warning","ariaLabel":"Warning"}"#).unwrap();
    let a = ax(&info);
    assert_eq!(a.role.as_str(), "img");
    assert_eq!(a.name.as_deref(), Some("Warning"));
    assert_eq!(a.name_source, AxNameSource::AriaLabel);

    let deco = from_str(r#"{"type":"Icon","glyph":"sparkle","decorative":true}"#).unwrap();
    let a = ax(&deco);
    assert_eq!(a.role.as_str(), "presentation");
    assert_eq!(a.name, None);
}

#[test]
fn modal_emits_dialog_role_named_by_title() {
    let modal = from_str(r#"{"type":"Modal","title":"Confirm delete"}"#).unwrap();
    let a = ax(&modal);
    assert_eq!(a.role.as_str(), "dialog");
    assert_eq!(a.name.as_deref(), Some("Confirm delete"));
    assert_eq!(a.name_source, AxNameSource::Title);
}

#[test]
fn select_and_switch_emit_their_roles_named_by_label() {
    let select = from_str(r#"{"type":"Select","label":"Country"}"#).unwrap();
    assert_eq!(ax(&select).role.as_str(), "combobox");
    assert_eq!(ax(&select).name.as_deref(), Some("Country"));

    let switch = from_str(r#"{"type":"Switch","label":"Dark mode"}"#).unwrap();
    assert_eq!(ax(&switch).role.as_str(), "switch");
    assert_eq!(ax(&switch).name.as_deref(), Some("Dark mode"));
}

// --- UI-6 fallback: labelled group, never raw JSON ------------------------

#[test]
fn unknown_component_falls_back_to_labelled_group() {
    let node = from_str(r#"{"type":"Chart3D","data":{"big":"json"}}"#).unwrap();
    let a = ax(&node);
    assert_eq!(a.role.as_str(), "group");
    assert_eq!(a.name.as_deref(), Some("Unsupported component Chart3D"));
    // Must NOT expose raw JSON as the accessible name.
    assert!(!a.name.as_deref().unwrap().contains("big"));
    assert!(!a.name.as_deref().unwrap().contains('{'));
}

// --- Validation: each required-name rule rejects bad / accepts good --------

#[test]
fn button_requires_label_or_aria_label() {
    // bad: empty label and no ariaLabel (icon-only without a name).
    let bad = Node::button("", Some("x".into()));
    let err = validate_accessibility(&bad).unwrap_err();
    assert_eq!(err.code(), "ValidationError");
    assert!(err.to_string().contains("Button"), "{err}");

    // good: a visible label.
    assert!(validate_accessibility(&Node::button("Go", None)).is_ok());
    // good: icon-only but with an ariaLabel.
    let good = Node::button("", None).with_aria_label("Menu");
    assert!(validate_accessibility(&good).is_ok());
}

#[test]
fn textfield_requires_label_placeholder_does_not_count() {
    // bad: only a placeholder, no label.
    let bad = Node::TextField {
        base: forge_ui::BaseNode::default(),
        value: String::new(),
        label: None,
        aria_label: None,
        placeholder: Some("Search...".into()),
        on_change: None,
    };
    let err = validate_accessibility(&bad).unwrap_err();
    assert_eq!(err.code(), "ValidationError");
    assert!(err.to_string().contains("TextField"), "{err}");
    assert!(err.to_string().contains("placeholder"), "{err}");

    // good: a real label (placeholder may still be present).
    let good = Node::TextField {
        base: forge_ui::BaseNode::default(),
        value: String::new(),
        label: Some("Search".into()),
        aria_label: None,
        placeholder: Some("Search...".into()),
        on_change: None,
    };
    assert!(validate_accessibility(&good).is_ok());
}

#[test]
fn icon_requires_decorative_or_informative_aria_label() {
    let bad = from_str(r#"{"type":"Icon","glyph":"bell"}"#).unwrap();
    let err = validate_accessibility(&bad).unwrap_err();
    assert!(err.to_string().contains("Icon"), "{err}");

    assert!(validate_accessibility(
        &from_str(r#"{"type":"Icon","glyph":"bell","decorative":true}"#).unwrap()
    )
    .is_ok());
    assert!(validate_accessibility(
        &from_str(r#"{"type":"Icon","glyph":"bell","ariaLabel":"Notifications"}"#).unwrap()
    )
    .is_ok());
}

#[test]
fn image_requires_alt() {
    let bad = from_str(r#"{"type":"Image","src":"a.png"}"#).unwrap();
    let err = validate_accessibility(&bad).unwrap_err();
    assert!(err.to_string().contains("Image"), "{err}");

    assert!(validate_accessibility(
        &from_str(r#"{"type":"Image","src":"a.png","alt":"Logo"}"#).unwrap()
    )
    .is_ok());
    // empty alt allowed only when decorative.
    assert!(validate_accessibility(
        &from_str(r#"{"type":"Image","src":"a.png","decorative":true}"#).unwrap()
    )
    .is_ok());
}

#[test]
fn chart_requires_summary() {
    let bad = from_str(r#"{"type":"Chart","kind":"bar"}"#).unwrap();
    let err = validate_accessibility(&bad).unwrap_err();
    assert!(err.to_string().contains("Chart"), "{err}");

    assert!(validate_accessibility(
        &from_str(r#"{"type":"Chart","kind":"bar","summary":"Up and to the right"}"#).unwrap()
    )
    .is_ok());
}

#[test]
fn standalone_table_requires_caption_or_aria_label() {
    let bad = from_str(r#"{"type":"Table","rows":[]}"#).unwrap();
    let err = validate_accessibility(&bad).unwrap_err();
    assert!(err.to_string().contains("Table"), "{err}");

    assert!(validate_accessibility(
        &from_str(r#"{"type":"Table","caption":"Totals"}"#).unwrap()
    )
    .is_ok());
    assert!(validate_accessibility(
        &from_str(r#"{"type":"Table","ariaLabel":"Totals"}"#).unwrap()
    )
    .is_ok());
}

#[test]
fn form_controls_must_have_labels() {
    // A Form (UI-6 fallback) whose control descendant lacks a label fails.
    let bad = from_str(
        r#"{"type":"Form","children":[
            {"type":"TextField","value":"","placeholder":"Email"}
        ]}"#,
    )
    .unwrap();
    let err = validate_accessibility(&bad).unwrap_err();
    assert!(err.to_string().contains("TextField"), "{err}");

    // Same Form with a labelled control passes.
    let good = from_str(
        r#"{"type":"Form","children":[
            {"type":"TextField","value":"","label":"Email"}
        ]}"#,
    )
    .unwrap();
    assert!(validate_accessibility(&good).is_ok());
}

#[test]
fn validation_recurses_into_nested_containers() {
    // A deeply nested bad Button is still caught.
    let tree = Node::stack(
        StackDir::V,
        vec![
            Node::text("Header"),
            Node::list(vec![Node::button("", None)]), // unnamed button, deep
        ],
    );
    let err = validate_accessibility(&tree).unwrap_err();
    assert!(err.to_string().contains("Button"), "{err}");

    // The same tree with the button named passes.
    let good = Node::stack(
        StackDir::V,
        vec![
            Node::text("Header"),
            Node::list(vec![Node::button("OK", None)]),
        ],
    );
    assert!(validate_accessibility(&good).is_ok());
}

#[test]
fn compliant_tree_passes_and_emits_expected_roles() {
    let tree = Node::stack(
        StackDir::V,
        vec![
            Node::text("Title"),
            Node::button("Submit", Some("form.submit".into())),
            Node::TextField {
                base: forge_ui::BaseNode::default(),
                value: String::new(),
                label: Some("Email".into()),
                aria_label: None,
                placeholder: None,
                on_change: None,
            },
        ],
    );
    assert!(validate_accessibility(&tree).is_ok());

    if let Node::Stack { children, .. } = &tree {
        assert_eq!(ax(&children[0]).role.as_str(), "text");
        assert_eq!(ax(&children[1]).role.as_str(), "button");
        assert_eq!(ax(&children[1]).name.as_deref(), Some("Submit"));
        assert_eq!(ax(&children[2]).role.as_str(), "textbox");
        assert_eq!(ax(&children[2]).name.as_deref(), Some("Email"));
    } else {
        panic!("expected stack");
    }
}

// --- Additive proof: ariaLabel round-trips on the wire (UI-12) ------------

#[test]
fn aria_label_round_trips_and_is_omitted_when_absent() {
    let btn = Node::button("", None).with_aria_label("Close");
    let json = forge_ui::to_canonical_string(&btn).unwrap();
    assert!(json.contains("\"ariaLabel\":\"Close\""), "{json}");
    assert_eq!(from_str(&json).unwrap(), btn);

    // Absent ariaLabel stays off the wire.
    let plain = forge_ui::to_canonical_string(&Node::button("Go", None)).unwrap();
    assert!(!plain.contains("ariaLabel"), "{plain}");
}
