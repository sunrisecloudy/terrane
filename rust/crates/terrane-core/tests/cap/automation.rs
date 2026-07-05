use terrane_core::{Core, Request};

use crate::helpers::{grant_resource, req};

#[test]
fn automation_resource_surface_is_registered() {
    let surface = terrane_core::declared_resource_surface();
    for method in [
        "ctx.resource.automation.set",
        "ctx.resource.automation.rm",
        "ctx.resource.automation.list",
        "ctx.resource.automation.stat",
    ] {
        assert!(surface.contains(method), "missing {method}");
    }
}

#[test]
fn automation_set_fire_suppress_and_replay() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["mailbox", "Mailbox"]))
        .unwrap();

    core.dispatch(req(
        "automation.set",
        &[
            "mailbox",
            "summarize-inbox",
            r#"{"trigger":{"kind":"kv.set","filter":"payload.key == 'inbox/1'"},"action":{"verb":"summarize","argsTemplate":["{{payload.key}}"]},"cooldownMs":1}"#,
        ],
    ))
    .unwrap();
    let rule = &core.state().automation.rules["mailbox"]["summarize-inbox"];
    assert_eq!(rule.spec.cooldown_ms, 1000);
    let hash = rule.rule_hash.clone();

    let public_err = core
        .dispatch(Request::new(
            "automation.fire",
            vec![
                "mailbox".into(),
                "summarize-inbox".into(),
                hash.clone(),
                "event-1".into(),
                "1000".into(),
            ],
        ))
        .unwrap_err();
    assert!(
        public_err
            .to_string()
            .contains("requires trusted host authority"),
        "{public_err}"
    );

    core.dispatch(Request::trusted_host(
        "automation.fire",
        vec![
            "mailbox".into(),
            "summarize-inbox".into(),
            hash.clone(),
            "event-1".into(),
            "1000".into(),
        ],
    ))
    .unwrap();
    core.dispatch(Request::trusted_host(
        "automation.suppress",
        vec![
            "mailbox".into(),
            "summarize-inbox".into(),
            hash,
            "event-2".into(),
            "1001".into(),
            "fire_budget_exceeded".into(),
        ],
    ))
    .unwrap();
    let rule = &core.state().automation.rules["mailbox"]["summarize-inbox"];
    assert_eq!(rule.fire_count, 1);
    assert_eq!(rule.suppressed_count, 1);
    assert!(core.replay_matches().unwrap());
    assert_eq!(
        Core::open(&log).unwrap().state().automation,
        core.state().automation
    );
}

#[test]
fn automation_validation_rejects_unknown_event_bad_filter_and_cross_app_without_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["mailbox", "Mailbox"]))
        .unwrap();
    core.dispatch(req("app.add", &["summarizer", "Summarizer"]))
        .unwrap();

    let unknown = core
        .dispatch(req(
            "automation.set",
            &[
                "mailbox",
                "bad",
                r#"{"trigger":{"kind":"ghost.set"},"action":{"verb":"run"}}"#,
            ],
        ))
        .unwrap_err();
    assert!(unknown.to_string().contains("declared event"), "{unknown}");

    let bad_filter = core
        .dispatch(req(
            "automation.set",
            &[
                "mailbox",
                "bad-filter",
                r#"{"trigger":{"kind":"kv.set","filter":"["},"action":{"verb":"run"}}"#,
            ],
        ))
        .unwrap_err();
    assert!(bad_filter.to_string().contains("JMESPath"), "{bad_filter}");

    let cross = core
        .dispatch(req(
            "automation.set",
            &[
                "summarizer",
                "cross",
                r#"{"trigger":{"kind":"kv.set","sourceApp":"mailbox"},"action":{"verb":"run"}}"#,
            ],
        ))
        .unwrap_err();
    assert!(
        cross
            .to_string()
            .contains("requires grant kv on mailbox"),
        "{cross}"
    );

    grant_resource(&mut core, "mailbox", "kv");
    core.dispatch(req(
        "automation.set",
        &[
            "summarizer",
            "cross",
            r#"{"trigger":{"kind":"kv.set","sourceApp":"mailbox"},"action":{"verb":"run"}}"#,
        ],
    ))
    .unwrap();
}

#[test]
fn automation_matcher_honors_filter_cooldown_and_seen_refs() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["mailbox", "Mailbox"]))
        .unwrap();
    core.dispatch(req(
        "automation.set",
        &[
            "mailbox",
            "summarize",
            r#"{"trigger":{"kind":"kv.set","filter":"payload.key == 'inbox/1'"},"action":{"verb":"summarize","argsTemplate":["{{payload.key}}"]},"cooldownMs":1000}"#,
        ],
    ))
    .unwrap();
    let ignored = core
        .dispatch(req("kv.set", &["mailbox", "archive/1", "old"]))
        .unwrap();
    assert!(
        terrane_cap_automation::matching_rules(&core.state().automation, &ignored[0], 1000)
            .unwrap()
            .is_empty()
    );
    let matching = core
        .dispatch(req("kv.set", &["mailbox", "inbox/1", "new"]))
        .unwrap();
    let rules =
        terrane_cap_automation::matching_rules(&core.state().automation, &matching[0], 1000)
            .unwrap();
    assert_eq!(rules.len(), 1);
    let event = terrane_cap_automation::event_json(&matching[0])
        .unwrap()
        .unwrap();
    core.dispatch(Request::trusted_host(
        "automation.fire",
        vec![
            rules[0].app.clone(),
            rules[0].name.clone(),
            rules[0].rule_hash.clone(),
            event.event_ref,
            "1000".into(),
        ],
    ))
    .unwrap();
    assert!(
        terrane_cap_automation::matching_rules(&core.state().automation, &matching[0], 1500)
            .unwrap()
            .is_empty(),
        "seen event refs should not re-fire"
    );
}
