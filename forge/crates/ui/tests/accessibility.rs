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

#[test]
fn structural_catalog_components_emit_their_spec_roles_not_the_unknown_fallback() {
    // Grid, Card, Scroll, Spacer, Divider, Markdown, Tabs are all in the spec
    // table; they must emit their spec role, NOT the UI-6 "Unsupported
    // component" group, and never the raw JSON as a name.
    let grid = from_str(r#"{"type":"Grid","children":[]}"#).unwrap();
    assert_eq!(ax(&grid).role.as_str(), "group");
    assert_eq!(ax(&grid).name, None);

    // An interactive grid (declared columns) upgrades to the `grid` role.
    let interactive = from_str(r#"{"type":"Grid","columns":3,"children":[]}"#).unwrap();
    assert_eq!(ax(&interactive).role.as_str(), "grid");

    // Card/Scroll are a plain group until labelled, then a region.
    let card = from_str(r#"{"type":"Card","children":[]}"#).unwrap();
    assert_eq!(ax(&card).role.as_str(), "group");
    let labelled_card = from_str(r#"{"type":"Card","ariaLabel":"Summary","children":[]}"#).unwrap();
    assert_eq!(ax(&labelled_card).role.as_str(), "region");
    assert_eq!(ax(&labelled_card).name.as_deref(), Some("Summary"));
    assert_eq!(ax(&labelled_card).name_source, AxNameSource::AriaLabel);

    let scroll = from_str(r#"{"type":"Scroll","children":[]}"#).unwrap();
    assert_eq!(ax(&scroll).role.as_str(), "group");

    let spacer = from_str(r#"{"type":"Spacer"}"#).unwrap();
    assert_eq!(ax(&spacer).role.as_str(), "presentation");
    assert_eq!(ax(&spacer).name, None);

    let divider = from_str(r#"{"type":"Divider","ariaLabel":"Section"}"#).unwrap();
    assert_eq!(ax(&divider).role.as_str(), "separator");
    assert_eq!(ax(&divider).name.as_deref(), Some("Section"));

    let markdown = from_str(r##"{"type":"Markdown","text":"# Hi"}"##).unwrap();
    assert_eq!(ax(&markdown).role.as_str(), "document");
    assert_eq!(ax(&markdown).name, None);

    let tabs = from_str(r#"{"type":"Tabs","ariaLabel":"Sections","tabs":[]}"#).unwrap();
    assert_eq!(ax(&tabs).role.as_str(), "tablist");
    assert_eq!(ax(&tabs).name.as_deref(), Some("Sections"));
    // None of these structural roles expose raw JSON as the accessible name.
    for node in [&grid, &card, &scroll, &markdown, &tabs] {
        if let Some(name) = ax(node).name {
            assert!(!name.contains('{') && !name.contains("Unsupported"), "{name}");
        }
    }
}

#[test]
fn badge_and_stat_emit_status_named_by_label() {
    let badge = from_str(r#"{"type":"Badge","label":"New","intent":"info"}"#).unwrap();
    assert_eq!(ax(&badge).role.as_str(), "status");
    assert_eq!(ax(&badge).name.as_deref(), Some("New"));

    let stat = from_str(r#"{"type":"Stat","label":"Revenue","value":"$1.2M"}"#).unwrap();
    assert_eq!(ax(&stat).role.as_str(), "status");
    assert_eq!(ax(&stat).name.as_deref(), Some("Revenue"));
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
fn structural_containers_have_optional_names_and_pass_validation_unlabelled() {
    // Spec marks Grid/Card/Scroll/Divider/Tabs names "optional", so an
    // unlabelled instance must NOT be a validation error — and its labelled
    // descendants are still validated.
    for json in [
        r#"{"type":"Grid","children":[]}"#,
        r#"{"type":"Card","children":[]}"#,
        r#"{"type":"Scroll","children":[]}"#,
        r#"{"type":"Spacer"}"#,
        r#"{"type":"Divider"}"#,
        r#"{"type":"Markdown","text":"hi"}"#,
        r#"{"type":"Tabs","tabs":[]}"#,
    ] {
        let node = from_str(json).unwrap();
        assert!(validate_accessibility(&node).is_ok(), "{json} should pass");
    }

    // But a bad control nested inside a structural container is still caught.
    let bad = from_str(
        r#"{"type":"Card","children":[{"type":"Image","src":"a.png"}]}"#,
    )
    .unwrap();
    let err = validate_accessibility(&bad).unwrap_err();
    assert!(err.to_string().contains("Image"), "{err}");
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
