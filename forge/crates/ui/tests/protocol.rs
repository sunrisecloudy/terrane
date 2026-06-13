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
        Node::List { items, .. } => {
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

// --- Review 030: known @forge/std fields survive round-trip (UI-12) --------
//
// Every BaseNode field (`id`/`testId`) and every type-specific known scalar
// prop in `forge/std/forge-std.d.ts` (Stack.gap, Text.variant, Button.variant,
// TextField.label/placeholder) must survive serialize→deserialize→serialize.
// Before the fix these were silently dropped on the wire.

/// Build one of each known node with EVERY known optional field set.
fn fully_populated_nodes() -> Vec<Node> {
    vec![
        Node::Stack {
            base: forge_ui::BaseNode {
                id: Some("stack-id".into()),
                test_id: Some("stack-test".into()),
            },
            dir: StackDir::H,
            gap: Some("md".into()),
            children: vec![Node::text("child")],
        },
        Node::Text {
            base: forge_ui::BaseNode {
                id: Some("text-id".into()),
                test_id: Some("text-test".into()),
            },
            value: "Notes".into(),
            variant: Some("title".into()),
        },
        Node::Button {
            base: forge_ui::BaseNode {
                id: Some("btn-id".into()),
                test_id: Some("save".into()),
            },
            label: "Save".into(),
            variant: Some("primary".into()),
            on_tap: Some("save".into()),
        },
        Node::TextField {
            base: forge_ui::BaseNode {
                id: Some("tf-id".into()),
                test_id: Some("name-field".into()),
            },
            value: "Ada".into(),
            label: Some("Name".into()),
            placeholder: Some("Your name".into()),
            on_change: Some("name.change".into()),
        },
        Node::List {
            base: forge_ui::BaseNode {
                id: Some("list-id".into()),
                test_id: Some("notes-list".into()),
            },
            items: vec![Node::text("row")],
        },
    ]
}

#[test]
fn every_known_field_survives_serialize_deserialize_serialize() {
    for node in fully_populated_nodes() {
        let json1 = to_canonical_string(&node).unwrap();
        let back = from_str(&json1).unwrap();
        // The typed value must be identical (no field dropped).
        assert_eq!(node, back, "typed round-trip dropped a field for {json1}");
        // And re-serialization must be byte-identical (stable wire shape).
        let json2 = to_canonical_string(&back).unwrap();
        assert_eq!(json1, json2, "wire shape unstable across round-trip");
    }
}

#[test]
fn known_optional_fields_appear_on_the_wire() {
    // Each known field name must actually be emitted, not silently dropped.
    let json = to_canonical_string(&Node::Stack {
        base: forge_ui::BaseNode {
            id: Some("i".into()),
            test_id: Some("t".into()),
        },
        dir: StackDir::V,
        gap: Some("sm".into()),
        children: vec![],
    })
    .unwrap();
    for needle in [
        "\"id\":\"i\"",
        "\"testId\":\"t\"",
        "\"gap\":\"sm\"",
    ] {
        assert!(json.contains(needle), "missing {needle} in {json}");
    }

    let json = to_canonical_string(&Node::Text {
        base: forge_ui::BaseNode::default(),
        value: "x".into(),
        variant: Some("caption".into()),
    })
    .unwrap();
    assert!(json.contains("\"variant\":\"caption\""), "{json}");

    let json = to_canonical_string(&Node::Button {
        base: forge_ui::BaseNode::default(),
        label: "L".into(),
        variant: Some("destructive".into()),
        on_tap: None,
    })
    .unwrap();
    assert!(json.contains("\"variant\":\"destructive\""), "{json}");

    let json = to_canonical_string(&Node::TextField {
        base: forge_ui::BaseNode::default(),
        value: "v".into(),
        label: Some("Label".into()),
        placeholder: Some("hint".into()),
        on_change: None,
    })
    .unwrap();
    assert!(json.contains("\"label\":\"Label\""), "{json}");
    assert!(json.contains("\"placeholder\":\"hint\""), "{json}");
}

#[test]
fn known_optional_fields_are_omitted_when_absent() {
    // Absent optional fields stay off the wire (so default trees don't grow new
    // keys) and round-trip back to None.
    let json = to_canonical_string(&Node::text("plain")).unwrap();
    for needle in ["\"id\"", "\"testId\"", "\"variant\""] {
        assert!(!json.contains(needle), "{needle} should be omitted: {json}");
    }
    let json = to_canonical_string(&Node::text_field("v", None)).unwrap();
    for needle in ["\"label\"", "\"placeholder\"", "\"id\"", "\"testId\""] {
        assert!(!json.contains(needle), "{needle} should be omitted: {json}");
    }
}

#[test]
fn parsing_known_fields_from_ts_contract_shape_preserves_them() {
    // The exact applet-tree shape called out in review 030.
    let json = r#"{"type":"Button","testId":"save","label":"Save","variant":"primary","onTap":"save"}"#;
    let node = from_str(json).unwrap();
    match &node {
        Node::Button {
            base,
            label,
            variant,
            on_tap,
        } => {
            assert_eq!(base.test_id.as_deref(), Some("save"));
            assert_eq!(label, "Save");
            assert_eq!(variant.as_deref(), Some("primary"));
            assert_eq!(on_tap.as_deref(), Some("save"));
        }
        other => panic!("expected Button, got {}", other.type_name()),
    }
    // And nothing is lost on the way back out.
    let back: serde_json::Value = serde_json::from_str(&to_canonical_string(&node).unwrap()).unwrap();
    let want: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(back, want);
}

#[test]
fn known_field_changes_diff_and_patch_round_trip() {
    // Set then change each of testId, gap, variant, placeholder and assert the
    // diff is a granular update_prop that apply() replays back to `new`.
    let cases: Vec<(Node, Node, Patch)> = vec![
        // testId change (BaseNode, applies to any node).
        (
            Node::text("h").with_test_id("a"),
            Node::text("h").with_test_id("b"),
            Patch::UpdateProp {
                path: vec![],
                key: "testId".into(),
                value: "b".into(),
            },
        ),
        // Stack.gap change.
        (
            Node::Stack {
                base: forge_ui::BaseNode::default(),
                dir: StackDir::V,
                gap: Some("sm".into()),
                children: vec![],
            },
            Node::Stack {
                base: forge_ui::BaseNode::default(),
                dir: StackDir::V,
                gap: Some("lg".into()),
                children: vec![],
            },
            Patch::UpdateProp {
                path: vec![],
                key: "gap".into(),
                value: "lg".into(),
            },
        ),
        // Text.variant change.
        (
            Node::Text {
                base: forge_ui::BaseNode::default(),
                value: "x".into(),
                variant: Some("body".into()),
            },
            Node::Text {
                base: forge_ui::BaseNode::default(),
                value: "x".into(),
                variant: Some("title".into()),
            },
            Patch::UpdateProp {
                path: vec![],
                key: "variant".into(),
                value: "title".into(),
            },
        ),
        // TextField.placeholder change.
        (
            Node::TextField {
                base: forge_ui::BaseNode::default(),
                value: "v".into(),
                label: None,
                placeholder: Some("old".into()),
                on_change: None,
            },
            Node::TextField {
                base: forge_ui::BaseNode::default(),
                value: "v".into(),
                label: None,
                placeholder: Some("new".into()),
                on_change: None,
            },
            Patch::UpdateProp {
                path: vec![],
                key: "placeholder".into(),
                value: "new".into(),
            },
        ),
    ];

    for (i, (old, new, expected)) in cases.into_iter().enumerate() {
        let patches = diff(Some(&old), &new);
        assert_eq!(patches, vec![expected], "case {i}: unexpected diff");
        let mut applied = old.clone();
        apply(&mut applied, &patches).unwrap();
        assert_eq!(applied, new, "case {i}: apply(diff) did not reproduce new");
    }
}

#[test]
fn clearing_a_known_optional_field_replaces_the_node() {
    // Some -> None has no granular clear op, so a single whole-node replace is
    // emitted and round-trips losslessly.
    let old = Node::button("Go", Some("go".into())).with_test_id("t");
    let new = Node::button("Go", Some("go".into())); // testId cleared
    let patches = diff(Some(&old), &new);
    assert_eq!(
        patches,
        vec![Patch::Replace {
            path: vec![],
            node: new.clone(),
        }]
    );
    let mut applied = old.clone();
    apply(&mut applied, &patches).unwrap();
    assert_eq!(applied, new);
}

#[test]
fn populated_nodes_apply_diff_round_trip_against_bare_nodes() {
    // Going from a bare node to a fully-populated one (and back) must round-trip
    // through diff/apply without losing any known field.
    for populated in fully_populated_nodes() {
        let bare = match &populated {
            Node::Stack { children, .. } => Node::stack(StackDir::H, children.clone()),
            Node::Text { value, .. } => Node::text(value.clone()),
            Node::Button { label, on_tap, .. } => Node::button(label.clone(), on_tap.clone()),
            Node::TextField {
                value, on_change, ..
            } => Node::text_field(value.clone(), on_change.clone()),
            Node::List { items, .. } => Node::list(items.clone()),
            Node::Unknown { .. } => unreachable!(),
        };
        // bare -> populated
        let patches = diff(Some(&bare), &populated);
        let mut applied = bare.clone();
        apply(&mut applied, &patches).unwrap();
        assert_eq!(applied, populated, "bare->populated lost a field");
        // populated -> bare
        let patches = diff(Some(&populated), &bare);
        let mut applied = populated.clone();
        apply(&mut applied, &patches).unwrap();
        assert_eq!(applied, bare, "populated->bare lost a field");
    }
}

// UI-6 regression guard: unknown TYPE still falls back; unknown PROP on a known
// node is still dropped (NOT preserved), even now that more props are known.
#[test]
fn ui6_unknown_type_and_unknown_prop_still_hold_after_field_expansion() {
    // Unknown component type → fallback, value preserved verbatim.
    let n = from_str(r#"{"type":"Sparkline","points":[1,2],"testId":"sp"}"#).unwrap();
    assert!(n.is_unknown());
    let reser: serde_json::Value =
        serde_json::from_str(&to_canonical_string(&n).unwrap()).unwrap();
    assert_eq!(reser["testId"], "sp", "unknown node preserves all props verbatim");

    // Genuinely unknown prop on a KNOWN node is still dropped (not a std field).
    let json = r#"{"type":"Text","text":"hi","variant":"title","sparkle":true}"#;
    let back = to_canonical_string(&from_str(json).unwrap()).unwrap();
    assert!(back.contains("\"variant\":\"title\""), "known field kept: {back}");
    assert!(!back.contains("sparkle"), "unknown prop dropped: {back}");
}
