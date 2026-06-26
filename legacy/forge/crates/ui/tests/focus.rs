//! UI-7 focus-order tests (source of record `spec/accessibility.md`, "Focus
//! Order" + "Unknown Component Fallback").
//!
//! Each container's traversal rule is asserted against the spec:
//!   - Stack / Grid: focusable descendants in child source order.
//!   - Scroll: an independently-focusable region is entered, then its children.
//!   - Tabs: tablist first, then ONLY the active panel; inactive panels excluded.
//!   - Modal: contained order, traps focus, reports its initial focus target.
//!   - UI-6 unknown: the container is not itself focusable, but its focusable
//!     KNOWN descendants stay in the order, in source order.

use forge_ui::{from_str, FocusStop, FocusStopKind, Node, StackDir};

/// Convenience: the `(path, role, name)` triples of an order, for compact asserts.
fn order_triples(node: &Node) -> Vec<(Vec<usize>, String, Option<String>)> {
    node.focus_order()
        .stops
        .into_iter()
        .map(|s| (s.path, s.role.as_str().to_string(), s.name))
        .collect()
}

#[test]
fn stack_focus_order_follows_child_source_order() {
    // Stack of: Text (not focusable), Button, TextField, Text.
    let tree = Node::stack(
        StackDir::V,
        vec![
            Node::text("Header"),
            Node::button("Save", Some("a".into())),
            Node::TextField {
                base: forge_ui::BaseNode::default(),
                value: String::new(),
                label: Some("Email".into()),
                aria_label: None,
                placeholder: None,
                on_change: None,
            },
            Node::text("Footer"),
        ],
    );
    let order = node_paths(&tree);
    // Only the Button [1] and TextField [2] are focusable, in source order.
    assert_eq!(order, vec![vec![1], vec![2]]);

    let triples = order_triples(&tree);
    assert_eq!(triples[0].1, "button");
    assert_eq!(triples[0].2.as_deref(), Some("Save"));
    assert_eq!(triples[1].1, "textbox");
    assert_eq!(triples[1].2.as_deref(), Some("Email"));

    // initial_focus is the full first stop (path + kind), not a bare path.
    let initial = tree.focus_order().initial_focus.unwrap();
    assert_eq!(initial.path, vec![1]);
    assert_eq!(initial.kind, FocusStopKind::Element);
    assert_eq!(initial, tree.focus_order().stops[0]);
    assert!(!tree.focus_order().traps_focus);
}

#[test]
fn nested_stack_focus_order_is_preorder_by_path() {
    // Stack[ Button, Stack[ Button, Button ], Button ] → preorder paths.
    let tree = Node::stack(
        StackDir::V,
        vec![
            Node::button("A", None),
            Node::stack(
                StackDir::H,
                vec![Node::button("B", None), Node::button("C", None)],
            ),
            Node::button("D", None),
        ],
    );
    assert_eq!(
        node_paths(&tree),
        vec![vec![0], vec![1, 0], vec![1, 1], vec![2]]
    );
}

#[test]
fn grid_focus_order_is_row_major_child_source_order() {
    // Grid (UI-6 fallback) with focusable children → source/row-major order.
    let grid = from_str(
        r#"{"type":"Grid","columns":2,"children":[
            {"type":"Button","label":"One"},
            {"type":"Text","text":"x"},
            {"type":"Button","label":"Two"}
        ]}"#,
    )
    .unwrap();
    let triples = order_triples(&grid);
    assert_eq!(
        triples,
        vec![
            (vec![0], "button".to_string(), Some("One".to_string())),
            (vec![2], "button".to_string(), Some("Two".to_string())),
        ]
    );
}

#[test]
fn scroll_is_focusable_only_when_declared_so_then_children_follow() {
    // Plain Scroll: not a stop; its focusable child still appears.
    let plain = from_str(
        r#"{"type":"Scroll","children":[{"type":"Button","label":"Go"}]}"#,
    )
    .unwrap();
    assert!(!plain.is_focusable());
    assert_eq!(node_paths(&plain), vec![vec![0]]);

    // Independently focusable Scroll region: the region is entered first, then
    // its children (spec: "focus enters region then children").
    let region = from_str(
        r#"{"type":"Scroll","focusable":true,"ariaLabel":"Log","children":[{"type":"Button","label":"Go"}]}"#,
    )
    .unwrap();
    assert!(region.is_focusable());
    let triples = order_triples(&region);
    assert_eq!(triples[0].0, Vec::<usize>::new()); // region itself, at root path
    assert_eq!(triples[0].1, "region");
    assert_eq!(triples[0].2.as_deref(), Some("Log"));
    assert_eq!(triples[1].0, vec![0]); // then its child button
    assert_eq!(triples[1].1, "button");
}

#[test]
fn tabs_focus_order_is_tablist_then_active_panel_only() {
    // Two tabs; active panel is index 1. Inactive panel (0) must be excluded.
    let tabs = from_str(
        r#"{"type":"Tabs","activeTab":1,
            "tabs":[{"label":"First"},{"label":"Second"}],
            "panels":[
                {"type":"Button","label":"InFirstPanel"},
                {"type":"Button","label":"InSecondPanel"}
            ]}"#,
    )
    .unwrap();
    let triples = order_triples(&tabs);
    // tablist first: both tabs, in declaration order, role "tab".
    assert_eq!(triples[0].1, "tab");
    assert_eq!(triples[0].2.as_deref(), Some("First"));
    assert_eq!(triples[1].1, "tab");
    assert_eq!(triples[1].2.as_deref(), Some("Second"));
    // then ONLY the active (second) panel's focusable; the first panel's button
    // must NOT appear.
    let names: Vec<_> = triples.iter().filter_map(|t| t.2.clone()).collect();
    assert!(names.contains(&"InSecondPanel".to_string()), "{names:?}");
    assert!(!names.contains(&"InFirstPanel".to_string()), "{names:?}");
    // The active panel's stop is addressed at its REAL render index `[active]`,
    // matching the accessibility annotation path for that panel — not offset
    // past the tab count.
    let stops = tabs.focus_order().stops;
    let panel_stop = stops.last().unwrap();
    assert_eq!(panel_stop.role.as_str(), "button");
    assert_eq!(panel_stop.path, vec![1]); // active panel index 1, render-consistent
    assert_eq!(panel_stop.kind, FocusStopKind::Element);
}

#[test]
fn tabs_tab_and_panel_paths_are_disambiguated_by_kind_not_collision() {
    // Regression: tab descriptors are NOT render nodes. A tab and the first
    // rendered panel child can share numeric path `[0]`; the `kind` tag (Tab vs
    // Element) is what disambiguates them, so a renderer resolves each against
    // the right array. Active panel index 0 here makes the collision concrete.
    let tabs = from_str(
        r#"{"type":"Tabs","activeTab":0,
            "tabs":[{"label":"Alpha"},{"label":"Beta"}],
            "panels":[
                {"type":"Button","label":"InAlpha"},
                {"type":"Button","label":"InBeta"}
            ]}"#,
    )
    .unwrap();
    let stops = tabs.focus_order().stops;
    // First tab and the active panel's button both sit at numeric path [0]...
    assert_eq!(stops[0].path, vec![0]);
    assert_eq!(stops[0].kind, FocusStopKind::Tab);
    assert_eq!(stops[0].name.as_deref(), Some("Alpha"));
    let panel = stops.last().unwrap();
    assert_eq!(panel.path, vec![0]);
    // ...but they are different KINDS, so they never actually collide.
    assert_eq!(panel.kind, FocusStopKind::Element);
    assert_eq!(panel.name.as_deref(), Some("InAlpha"));
    assert_ne!(stops[0].kind, panel.kind);
}

#[test]
fn tabs_default_active_is_first_panel() {
    let tabs = from_str(
        r#"{"type":"Tabs",
            "tabs":[{"label":"A"},{"label":"B"}],
            "panels":[
                {"type":"Button","label":"PanelA"},
                {"type":"Button","label":"PanelB"}
            ]}"#,
    )
    .unwrap();
    let names: Vec<_> = order_triples(&tabs)
        .iter()
        .filter_map(|t| t.2.clone())
        .collect();
    assert!(names.contains(&"PanelA".to_string()), "{names:?}");
    assert!(!names.contains(&"PanelB".to_string()), "{names:?}");
}

#[test]
fn modal_traps_focus_and_reports_initial_focus_on_first_focusable_child() {
    let modal = from_str(
        r#"{"type":"Modal","title":"Confirm",
            "children":[
                {"type":"Text","text":"Are you sure?"},
                {"type":"Button","label":"Cancel"},
                {"type":"Button","label":"Delete"}
            ]}"#,
    )
    .unwrap();
    let order = modal.focus_order();
    assert!(order.traps_focus, "Modal must trap focus");
    // Order holds only the dialog's focusable descendants (the two buttons).
    let names: Vec<_> = order
        .stops
        .iter()
        .filter_map(|s| s.name.clone())
        .collect();
    assert_eq!(names, vec!["Cancel".to_string(), "Delete".to_string()]);
    // Initial focus is the first focusable child (the full stop, kind-tagged).
    let initial = order.initial_focus.clone().unwrap();
    assert_eq!(initial.path, vec![1]);
    assert_eq!(initial.kind, FocusStopKind::Element);
    assert_eq!(initial.name.as_deref(), Some("Cancel"));
    assert_eq!(initial, order.stops[0]);
    // The dialog box itself is NOT a tab stop in its own order.
    assert!(order.stops.iter().all(|s| s.path != Vec::<usize>::new()));
}

#[test]
fn modal_with_no_focusable_child_moves_initial_focus_to_the_dialog_itself() {
    // Spec: "first focusable child OR the dialog title". With no focusable
    // child, focus moves to the dialog (root path), and it still traps.
    let modal = from_str(
        r#"{"type":"Modal","title":"Notice",
            "children":[{"type":"Text","text":"Saved."}]}"#,
    )
    .unwrap();
    let order = modal.focus_order();
    assert!(order.traps_focus);
    assert!(order.stops.is_empty());
    // Focus moves to the dialog itself: a stop at the Modal's root path, carrying
    // the dialog role + title (so a renderer focuses the dialog box, not nothing).
    let initial = order.initial_focus.unwrap();
    assert_eq!(initial.path, Vec::<usize>::new());
    assert_eq!(initial.kind, FocusStopKind::Element);
    assert_eq!(initial.role.as_str(), "dialog");
    assert_eq!(initial.name.as_deref(), Some("Notice"));
}

#[test]
fn nested_open_modal_contains_focus_and_excludes_elements_behind_it() {
    // Regression: a Modal nested in a page (not the focus root) must STILL trap
    // focus and contain the order to itself — focusables behind the scrim (the
    // "Open" button) must NOT be reachable. The dialog's stops keep the Modal's
    // render-tree path prefix `[1, _]`, matching the annotation paths.
    let page = from_str(
        r#"{"type":"Stack","direction":"v","children":[
            {"type":"Button","label":"Open"},
            {"type":"Modal","title":"Confirm","children":[
                {"type":"Text","text":"Sure?"},
                {"type":"Button","label":"Cancel"},
                {"type":"Button","label":"Delete"}
            ]}
        ]}"#,
    )
    .unwrap();
    let order = page.focus_order();
    assert!(order.traps_focus, "an open nested Modal must trap focus");
    let triples = order_triples(&page);
    // Only the dialog's two buttons, path-prefixed by the Modal at index [1].
    assert_eq!(
        triples,
        vec![
            (vec![1, 1], "button".to_string(), Some("Cancel".to_string())),
            (vec![1, 2], "button".to_string(), Some("Delete".to_string())),
        ]
    );
    // The "Open" button BEHIND the modal is excluded entirely.
    let names: Vec<_> = triples.iter().filter_map(|t| t.2.clone()).collect();
    assert!(!names.contains(&"Open".to_string()), "{names:?}");
    // Initial focus is the dialog's first focusable child, at its real path,
    // carrying its kind so the [1,1] target is unambiguous.
    let initial = order.initial_focus.clone().unwrap();
    assert_eq!(initial.path, vec![1, 1]);
    assert_eq!(initial.kind, FocusStopKind::Element);
    assert_eq!(initial, order.stops[0]);
}

#[test]
fn closed_modal_traps_nothing_and_hides_its_descendants() {
    // A Modal with `open: false` is off-screen: it does not trap, and its
    // children are NOT in the page focus order — only the page button remains.
    let page = from_str(
        r#"{"type":"Stack","direction":"v","children":[
            {"type":"Button","label":"Open"},
            {"type":"Modal","title":"Confirm","open":false,"children":[
                {"type":"Button","label":"Cancel"},
                {"type":"Button","label":"Delete"}
            ]}
        ]}"#,
    )
    .unwrap();
    let order = page.focus_order();
    assert!(!order.traps_focus, "a closed Modal must not trap");
    let names: Vec<_> = order_triples(&page)
        .iter()
        .filter_map(|t| t.2.clone())
        .collect();
    assert_eq!(names, vec!["Open".to_string()]);
}

#[test]
fn open_modal_inside_active_tabs_panel_is_found_at_its_real_render_path() {
    // A Modal opened inside the ACTIVE tab panel still traps, addressed at the
    // panel's real render index (not offset), so its stops match annotations.
    let tabs = from_str(
        r#"{"type":"Tabs","activeTab":1,
            "tabs":[{"label":"A"},{"label":"B"}],
            "panels":[
                {"type":"Button","label":"InA"},
                {"type":"Modal","title":"Pick","children":[
                    {"type":"Button","label":"Choose"}
                ]}
            ]}"#,
    )
    .unwrap();
    let order = tabs.focus_order();
    assert!(order.traps_focus, "modal in active panel traps");
    let triples = order_triples(&tabs);
    // Only the modal's button, at panel index [1] then child [0].
    assert_eq!(
        triples,
        vec![(vec![1, 0], "button".to_string(), Some("Choose".to_string()))]
    );
    assert_eq!(triples[0].0, vec![1, 0]);
}

// --- UI-6 unknown-component fallback focus behavior ------------------------

#[test]
fn unknown_component_is_not_focusable_but_keeps_focusable_known_children() {
    // An unrecognized component is itself NOT a tab stop, but a focusable known
    // child inside it stays in the order, in source order (no lost a11y).
    let tree = from_str(
        r#"{"type":"Chart3D","data":{"big":"json"},"children":[
            {"type":"Text","text":"caption"},
            {"type":"Button","label":"Export"}
        ]}"#,
    )
    .unwrap();
    assert!(!tree.is_focusable(), "unknown component is not itself focusable");
    let triples = order_triples(&tree);
    assert_eq!(triples.len(), 1);
    assert_eq!(triples[0].0, vec![1]); // the Button child, in source order
    assert_eq!(triples[0].1, "button");
    assert_eq!(triples[0].2.as_deref(), Some("Export"));
}

#[test]
fn unknown_component_with_no_focusable_children_yields_empty_order() {
    let tree = from_str(r#"{"type":"Widget3000","children":[{"type":"Text","text":"x"}]}"#).unwrap();
    let order = tree.focus_order();
    assert!(order.stops.is_empty());
    assert_eq!(order.initial_focus, None);
    assert!(!order.traps_focus);
}

#[test]
fn focus_order_never_panics_on_malformed_unknown_children() {
    // Non-node entries in a verbatim children array are tolerated (UI-6).
    let tree = from_str(
        r#"{"type":"Grid","children":[42,"oops",{"type":"Button","label":"Ok"}]}"#,
    )
    .unwrap();
    // Should not panic; the one real Button is found (its index among the
    // successfully-parsed nodes).
    let triples = order_triples(&tree);
    assert_eq!(triples.len(), 1);
    assert_eq!(triples[0].2.as_deref(), Some("Ok"));
}

/// Helper: the index paths of a tree's focus stops, in order.
fn node_paths(node: &Node) -> Vec<Vec<usize>> {
    node.focus_order()
        .stops
        .into_iter()
        .map(|s: FocusStop| s.path)
        .collect()
}
