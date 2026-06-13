//! Behavioural tests for the forge-ui protocol (prd-merged/05 UI-1, UI-2, UI-6,
//! UI-12). The data-driven golden corpus lives in `golden.rs`; these tests
//! exercise the same rules with hand-built trees and assert exact patch shapes.

use forge_ui::{apply, diff, from_str, to_canonical_string, Node, Patch, StackDir};

fn stack_v(children: Vec<Node>) -> Node {
    Node::stack(StackDir::V, children)
}

// --- Roundtrip (UI-12) ---------------------------------------------------

#[test]
fn each_node_type_roundtrips() {
    let nodes = vec![
        Node::text("hi"),
        Node::button("Save", Some("save.now".into())),
        Node::button("NoAction", None),
        Node::text_field("Ada", Some("name.change".into())),
        Node::text_field("", None),
        Node::list(vec![Node::text("a"), Node::text("b")]),
        stack_v(vec![Node::text("x"), Node::button("y", None)]),
        Node::stack(StackDir::H, vec![Node::text("h")]),
    ];
    for n in nodes {
        let json = to_canonical_string(&n).unwrap();
        let back = from_str(&json).unwrap();
        assert_eq!(n, back, "roundtrip mismatch for {json}");
    }
}

#[test]
fn nested_tree_roundtrips() {
    let tree = stack_v(vec![
        Node::text("Inbox"),
        Node::list(vec![
            Node::text("Review PR"),
            Node::button("Archive", Some("message.archive".into())),
        ]),
        Node::button("Compose", Some("message.compose".into())),
    ]);
    let json = to_canonical_string(&tree).unwrap();
    let back = from_str(&json).unwrap();
    assert_eq!(tree, back);
}

#[test]
fn wire_field_names_match_ts_contract() {
    // Button uses `onTap`, TextField uses `onChange`, Text uses `text`,
    // Stack uses `direction` — the TS-facing names, not Rust snake_case.
    let json = to_canonical_string(&Node::button("Save", Some("a".into()))).unwrap();
    assert!(json.contains("\"onTap\":\"a\""), "{json}");
    assert!(json.contains("\"label\":\"Save\""), "{json}");

    let json = to_canonical_string(&Node::text_field("v", Some("c".into()))).unwrap();
    assert!(json.contains("\"onChange\":\"c\""), "{json}");

    let json = to_canonical_string(&Node::text("hello")).unwrap();
    assert!(json.contains("\"text\":\"hello\""), "{json}");

    let json = to_canonical_string(&Node::stack(StackDir::H, vec![])).unwrap();
    assert!(json.contains("\"direction\":\"h\""), "{json}");
}

#[test]
fn optional_action_refs_are_omitted_when_none() {
    let json = to_canonical_string(&Node::button("X", None)).unwrap();
    assert!(!json.contains("onTap"), "None onTap should be omitted: {json}");
    let json = to_canonical_string(&Node::text_field("v", None)).unwrap();
    assert!(
        !json.contains("onChange"),
        "None onChange should be omitted: {json}"
    );
}

// --- Diff: minimal patches (UI-1) ----------------------------------------

#[test]
fn identical_trees_diff_to_empty() {
    let a = stack_v(vec![Node::text("Ready"), Node::button("Go", Some("g".into()))]);
    let b = a.clone();
    assert!(diff(Some(&a), &b).is_empty());
}

#[test]
fn text_change_yields_single_update_text() {
    let a = stack_v(vec![Node::text("Draft")]);
    let b = stack_v(vec![Node::text("Published")]);
    let patches = diff(Some(&a), &b);
    assert_eq!(
        patches,
        vec![Patch::UpdateText {
            path: vec![0],
            value: "Published".into(),
        }]
    );
}

#[test]
fn button_label_change_yields_single_update_prop() {
    let a = stack_v(vec![Node::button("Save draft", Some("doc.save".into()))]);
    let b = stack_v(vec![Node::button("Save", Some("doc.save".into()))]);
    let patches = diff(Some(&a), &b);
    assert_eq!(
        patches,
        vec![Patch::UpdateProp {
            path: vec![0],
            key: "label".into(),
            value: "Save".into(),
        }]
    );
}

#[test]
fn textfield_value_change_yields_update_prop_value() {
    let a = stack_v(vec![Node::text_field("Ada", Some("name.change".into()))]);
    let b = stack_v(vec![Node::text_field("Ada Lovelace", Some("name.change".into()))]);
    let patches = diff(Some(&a), &b);
    assert_eq!(
        patches,
        vec![Patch::UpdateProp {
            path: vec![0],
            key: "value".into(),
            value: "Ada Lovelace".into(),
        }]
    );
}

#[test]
fn nested_button_on_tap_change_yields_update_prop_at_deep_path() {
    let a = stack_v(vec![
        Node::text("Actions"),
        Node::stack(StackDir::H, vec![Node::button("Run", Some("job.run".into()))]),
    ]);
    let b = stack_v(vec![
        Node::text("Actions"),
        Node::stack(
            StackDir::H,
            vec![Node::button("Run", Some("job.run.now".into()))],
        ),
    ]);
    let patches = diff(Some(&a), &b);
    assert_eq!(
        patches,
        vec![Patch::UpdateProp {
            path: vec![1, 0],
            key: "onTap".into(),
            value: "job.run.now".into(),
        }]
    );
}

#[test]
fn child_appended_yields_insert() {
    let a = stack_v(vec![Node::text("One"), Node::text("Two")]);
    let b = stack_v(vec![
        Node::text("One"),
        Node::text("Two"),
        Node::button("Three", Some("add.three".into())),
    ]);
    let patches = diff(Some(&a), &b);
    assert_eq!(
        patches,
        vec![Patch::Insert {
            path: vec![2],
            node: Node::button("Three", Some("add.three".into())),
        }]
    );
}

#[test]
fn child_removed_yields_remove() {
    let a = stack_v(vec![Node::text("One"), Node::text("Two"), Node::text("Three")]);
    let b = stack_v(vec![Node::text("One"), Node::text("Two")]);
    let patches = diff(Some(&a), &b);
    assert_eq!(patches, vec![Patch::Remove { path: vec![2] }]);
}

#[test]
fn type_change_yields_replace() {
    let a = stack_v(vec![Node::text("Open"), Node::text("Submit")]);
    let b = stack_v(vec![
        Node::text("Open"),
        Node::button("Submit", Some("form.submit".into())),
    ]);
    let patches = diff(Some(&a), &b);
    assert_eq!(
        patches,
        vec![Patch::Replace {
            path: vec![1],
            node: Node::button("Submit", Some("form.submit".into())),
        }]
    );
}

#[test]
fn none_old_tree_yields_root_replace() {
    let b = Node::text("hello");
    let patches = diff(None, &b);
    assert_eq!(
        patches,
        vec![Patch::Replace {
            path: vec![],
            node: b,
        }]
    );
}

#[test]
fn stack_direction_change_replaces_container() {
    let a = Node::stack(StackDir::V, vec![Node::text("x")]);
    let b = Node::stack(StackDir::H, vec![Node::text("x")]);
    let patches = diff(Some(&a), &b);
    assert_eq!(
        patches,
        vec![Patch::Replace {
            path: vec![],
            node: b,
        }]
    );
}

// --- Round-trip property: apply(diff(a,b)) == b --------------------------

#[test]
fn apply_diff_roundtrips_for_many_pairs() {
    let pairs: Vec<(Node, Node)> = vec![
        // identical
        (stack_v(vec![Node::text("a")]), stack_v(vec![Node::text("a")])),
        // text change
        (stack_v(vec![Node::text("a")]), stack_v(vec![Node::text("b")])),
        // button label + action change together
        (
            stack_v(vec![Node::button("Old", Some("a".into()))]),
            stack_v(vec![Node::button("New", Some("b".into()))]),
        ),
        // append + remove mixed
        (
            stack_v(vec![Node::text("1"), Node::text("2"), Node::text("3")]),
            stack_v(vec![Node::text("1"), Node::button("2b", None)]),
        ),
        // list reorder (index updates)
        (
            Node::list(vec![Node::text("Alpha"), Node::text("Beta")]),
            Node::list(vec![Node::text("Beta"), Node::text("Alpha")]),
        ),
        // deep nesting
        (
            stack_v(vec![
                Node::text("h"),
                Node::list(vec![Node::text("x"), Node::button("y", Some("p".into()))]),
            ]),
            stack_v(vec![
                Node::text("h2"),
                Node::list(vec![Node::text("x"), Node::button("y", Some("q".into()))]),
            ]),
        ),
        // textfield clearing its action (Some -> None triggers replace path)
        (
            stack_v(vec![Node::text_field("v", Some("c".into()))]),
            stack_v(vec![Node::text_field("v2", None)]),
        ),
    ];

    for (i, (a, b)) in pairs.into_iter().enumerate() {
        let patches = diff(Some(&a), &b);
        let mut applied = a.clone();
        apply(&mut applied, &patches).unwrap();
        assert_eq!(applied, b, "pair {i} did not round-trip; patches={patches:?}");
    }
}

// --- Forward compatibility (UI-6, NORMATIVE) -----------------------------

#[test]
fn unknown_type_becomes_fallback_and_does_not_error() {
    let json = r#"{"type":"FutureWidget","title":"Heatmap","range":"30d"}"#;
    let node = from_str(json).unwrap();
    assert!(node.is_unknown());
    assert_eq!(node.type_name(), "FutureWidget");
}

#[test]
fn unknown_node_survives_roundtrip_verbatim() {
    let json = r#"{"type":"FutureWidget","points":[1,2,3],"title":"x"}"#;
    let node = from_str(json).unwrap();
    let back = to_canonical_string(&node).unwrap();
    // Re-parse and compare as JSON values (key order is preserved by serde_json
    // Map, but compare semantically to be robust).
    let a: serde_json::Value = serde_json::from_str(json).unwrap();
    let b: serde_json::Value = serde_json::from_str(&back).unwrap();
    assert_eq!(a, b);
}

#[test]
fn unknown_prop_on_known_node_is_ignored_not_errored() {
    let json = r#"{"type":"Button","label":"Sparkle","onTap":"go","sparkle":true}"#;
    let node = from_str(json).unwrap();
    assert_eq!(node, Node::button("Sparkle", Some("go".into())));
    // The unknown `sparkle` prop is dropped on re-serialization (known node).
    let back = to_canonical_string(&node).unwrap();
    assert!(!back.contains("sparkle"), "{back}");
}

#[test]
fn unknown_node_nested_in_known_container_does_not_error() {
    let json = r#"{
        "type":"List",
        "items":[
            {"type":"Text","text":"row"},
            {"type":"FutureWidget","confidence":0.82}
        ]
    }"#;
    let node = from_str(json).unwrap();
    match &node {
        Node::List { items } => {
            assert_eq!(items.len(), 2);
            assert!(!items[0].is_unknown());
            assert!(items[1].is_unknown());
        }
        other => panic!("expected List, got {}", other.type_name()),
    }
}

#[test]
fn unknown_node_diff_does_not_error() {
    let a = from_str(r#"{"type":"FutureWidget","v":1}"#).unwrap();
    let b = from_str(r#"{"type":"FutureWidget","v":2}"#).unwrap();
    // identical
    assert!(diff(Some(&a), &a).is_empty());
    // changed → a single root replace, no error/panic
    let patches = diff(Some(&a), &b);
    assert_eq!(patches.len(), 1);
    let mut applied = a.clone();
    apply(&mut applied, &patches).unwrap();
    assert_eq!(applied, b);
}

// --- Patch wire shape (matches Codex fixtures) ---------------------------

#[test]
fn patch_wire_shapes_match_fixture_vocabulary() {
    let cases = vec![
        (
            Patch::Replace {
                path: vec![1],
                node: Node::text("x"),
            },
            r#"{"op":"replace","path":[1],"node":{"type":"Text","text":"x"}}"#,
        ),
        (
            Patch::UpdateText {
                path: vec![0],
                value: "new".into(),
            },
            r#"{"op":"update_text","path":[0],"value":"new"}"#,
        ),
        (
            Patch::UpdateProp {
                path: vec![1],
                key: "label".into(),
                value: "Save".into(),
            },
            r#"{"op":"update_prop","path":[1],"key":"label","value":"Save"}"#,
        ),
        (
            Patch::Insert {
                path: vec![0, 3],
                node: Node::text("y"),
            },
            r#"{"op":"insert","path":[0,3],"node":{"type":"Text","text":"y"}}"#,
        ),
        (
            Patch::Remove { path: vec![0, 3] },
            r#"{"op":"remove","path":[0,3]}"#,
        ),
    ];
    for (patch, expected) in cases {
        let json = serde_json::to_string(&patch).unwrap();
        assert_eq!(json, expected);
        // and back
        let back: Patch = serde_json::from_str(expected).unwrap();
        assert_eq!(back, patch);
    }
}

// --- Apply error paths (no panic) ----------------------------------------

#[test]
fn apply_out_of_range_path_returns_error_not_panic() {
    let mut root = stack_v(vec![Node::text("a")]);
    let bad = vec![Patch::UpdateText {
        path: vec![5],
        value: "z".into(),
    }];
    assert!(apply(&mut root, &bad).is_err());
}

#[test]
fn apply_update_text_on_non_text_returns_error() {
    let mut root = stack_v(vec![Node::button("b", None)]);
    let bad = vec![Patch::UpdateText {
        path: vec![0],
        value: "z".into(),
    }];
    assert!(apply(&mut root, &bad).is_err());
}
