use std::collections::BTreeSet;

use tempfile::tempdir;
use terrane_host::public_authz::{
    authorize_public_command, authorize_public_query, classify_public_command,
    classify_public_query_name, PublicCommandAuthz, PublicCommandDisposition, PublicQueryAuthz,
    PublicQueryDisposition,
};

fn app(core: &mut terrane_host::HostCore, id: &str) {
    terrane_host::dispatch_on_core(core, "app.add", &[id.into(), id.into()]).unwrap();
}

fn grant(core: &mut terrane_host::HostCore, app: &str, namespace: &str) {
    terrane_host::dispatch_on_core(
        core,
        "auth.grant",
        &[
            terrane_host::LOCAL_OWNER_SUBJECT.into(),
            app.into(),
            namespace.into(),
        ],
    )
    .unwrap();
}

#[test]
fn public_command_inventory_covers_every_registered_command() {
    let commands = terrane_core::command_names();
    assert_eq!(
        commands.len(),
        36,
        "registered commands changed: {commands:?}"
    );

    let mut allowed = Vec::new();
    let mut refused = Vec::new();
    let mut grant_gated = Vec::new();
    let mut unclassified = Vec::new();
    for command in commands {
        match classify_public_command(command) {
            PublicCommandDisposition::Allow => allowed.push(command),
            PublicCommandDisposition::Refuse { .. } => refused.push(command),
            PublicCommandDisposition::GrantGated { .. } => grant_gated.push(command),
            PublicCommandDisposition::Unclassified => unclassified.push(command),
        }
    }

    assert!(
        unclassified.is_empty(),
        "unclassified commands: {unclassified:?}"
    );
    assert_eq!(
        grant_gated.len(),
        14,
        "grant-gated commands: {grant_gated:?}"
    );
    assert_eq!(refused.len(), 19, "refused commands: {refused:?}");
    assert_eq!(allowed, vec!["app.add", "app.import", "replica.init"]);
}

#[test]
fn grantable_command_inventory_requires_explicit_extractors_or_refusal() {
    let grantable: BTreeSet<_> = terrane_core::grant_resource_namespaces()
        .into_iter()
        .collect();
    assert_eq!(
        grantable,
        BTreeSet::from(["build", "crdt", "kv", "relational_db"])
    );

    let mut bad = Vec::new();
    for command in terrane_core::command_names() {
        let namespace = command.split_once('.').map(|(ns, _)| ns).unwrap_or(command);
        if !grantable.contains(namespace) {
            continue;
        }
        match classify_public_command(command) {
            PublicCommandDisposition::GrantGated {
                namespace: classified,
                app_arg_index: 0,
            } if classified == namespace => {}
            PublicCommandDisposition::Refuse { .. } => {}
            other => bad.push(format!("{command}: {other:?}")),
        }
    }
    assert!(
        bad.is_empty(),
        "grantable public commands need explicit app arg extractors or refusal: {bad:?}"
    );
}

#[test]
fn public_query_inventory_covers_every_registered_query() {
    let queries = terrane_core::query_names();
    assert_eq!(queries, vec!["app.exists", "replica.peer"]);
    for query in queries {
        assert_eq!(
            classify_public_query_name(query),
            PublicQueryDisposition::Allow,
            "{query} should be explicitly classified"
        );
    }
}

#[test]
fn resource_commands_need_grants_and_never_prompt_for_missing_apps() {
    let dir = tempdir().unwrap();
    let mut core = terrane_host::open_at_log_path(dir.path().join("log.bin")).unwrap();
    app(&mut core, "demo");

    assert_eq!(
        authorize_public_command(
            &core,
            "kv.set",
            &["demo".into(), "key".into(), "value".into()]
        )
        .unwrap(),
        PublicCommandAuthz::NeedsGrant {
            app: "demo".into(),
            namespace: "kv".into()
        }
    );

    grant(&mut core, "demo", "kv");
    assert_eq!(
        authorize_public_command(
            &core,
            "kv.set",
            &["demo".into(), "key".into(), "value".into()]
        )
        .unwrap(),
        PublicCommandAuthz::Allow
    );

    assert!(matches!(
        authorize_public_command(
            &core,
            "kv.set",
            &["missing".into(), "key".into(), "value".into()]
        )
        .unwrap(),
        PublicCommandAuthz::Refuse { reason } if reason == "no such app: missing"
    ));
    assert!(matches!(
        authorize_public_command(&core, "kv.set", &[]).unwrap(),
        PublicCommandAuthz::Refuse { reason } if reason.contains("args[0]")
    ));
}

#[test]
fn dangerous_and_effect_commands_are_refused() {
    let dir = tempdir().unwrap();
    let core = terrane_host::open_at_log_path(dir.path().join("log.bin")).unwrap();

    for command in [
        "kv.storage.set",
        "kv.storage.clear",
        "js-runtime.run",
        "wasm-runtime.run",
        "net.fetch",
        "model.ask",
        "harness.generate-app",
        "harness.run-js",
        "app.remove",
        "auth.grant",
    ] {
        assert!(
            matches!(
                authorize_public_command(&core, command, &["demo".into()]).unwrap(),
                PublicCommandAuthz::Refuse { .. }
            ),
            "{command} should be refused"
        );
    }
}

#[test]
fn allowlisted_commands_and_queries_stay_available() {
    let dir = tempdir().unwrap();
    let core = terrane_host::open_at_log_path(dir.path().join("log.bin")).unwrap();

    for command in ["app.add", "app.import", "replica.init"] {
        assert_eq!(
            authorize_public_command(&core, command, &[]).unwrap(),
            PublicCommandAuthz::Allow
        );
    }
    assert_eq!(
        authorize_public_query("app", "exists").unwrap(),
        PublicQueryAuthz::Allow
    );
    assert_eq!(
        authorize_public_query("replica", "replica.peer").unwrap(),
        PublicQueryAuthz::Allow
    );
    assert!(matches!(
        authorize_public_query("kv", "get").unwrap(),
        PublicQueryAuthz::Refuse { reason } if reason.contains("kv.get")
    ));
}
