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
        72,
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
        31,
        "grant-gated commands: {grant_gated:?}"
    );
    assert_eq!(refused.len(), 39, "refused commands: {refused:?}");
    assert_eq!(allowed, vec!["app.add", "replica.init"]);
}

#[test]
fn grantable_command_inventory_requires_explicit_extractors_or_refusal() {
    let grantable: BTreeSet<_> = terrane_core::grant_resource_namespaces()
        .into_iter()
        .collect();
    assert_eq!(
        grantable,
        BTreeSet::from([
            "build",
            "crdt",
            "crypto",
            "kv",
            "local-model",
            "native",
            "net",
            "relational_db",
            "search",
            "stt",
            "sysinfo"
        ])
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
    assert_eq!(
        queries,
        vec!["app.exists", "native.supports", "replica.peer"]
    );
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
        "kv.public.set",
        "kv.public.rm",
        "kv.public.import",
        "js-runtime.run",
        "wasm-runtime.run",
        "net.fetch",
        "model.ask",
        "local-model.register",
        "local-model.pull",
        "local-model.rm",
        "harness.generate-app",
        "harness.run-js",
        "app.import",
        "app.remove",
        "auth.grant",
        "native.platform.observe",
        "native.complete",
        "native.fail",
        "native.cancel",
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

    for command in ["app.add", "replica.init"] {
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
    assert_eq!(
        authorize_public_query("native", "supports").unwrap(),
        PublicQueryAuthz::Allow
    );
    assert!(matches!(
        authorize_public_query("kv", "get").unwrap(),
        PublicQueryAuthz::Refuse { reason } if reason.contains("kv.get")
    ));
}

#[test]
fn no_allowlisted_public_command_can_emit_storage_configuration() {
    let mut bad = Vec::new();
    for command in terrane_core::command_names() {
        if classify_public_command(command) != PublicCommandDisposition::Allow {
            continue;
        }
        let namespace = command.split_once('.').map(|(ns, _)| ns).unwrap_or(command);
        let doc = terrane_core::capability_doc(namespace, true).unwrap();
        let command_doc = doc
            .commands
            .iter()
            .find(|doc| doc.name == command)
            .unwrap_or_else(|| panic!("missing command doc for {command}"));
        if command_doc
            .emits
            .iter()
            .any(|event| event == "kv.storage.configured")
        {
            bad.push(command);
        }
    }
    assert!(
        bad.is_empty(),
        "allowlisted public commands must not emit kv.storage.configured: {bad:?}"
    );
}

#[test]
fn public_dispatch_refuses_app_import_storage_side_channel_before_effect() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = terrane_host::open_at_log_path(&log).unwrap();

    let err = terrane_host::dispatch_public_on_core(
        &mut core,
        "app.import",
        &[
            "/tmp/missing-bundle".into(),
            "--storage".into(),
            "sqlite".into(),
            "--path".into(),
            "/tmp/evil.db".into(),
        ],
    )
    .unwrap_err();
    assert!(
        err.contains("app.import installs bundles"),
        "app.import should be refused before effect validation: {err}"
    );
    let records = terrane_core::read_log(&log).unwrap();
    assert!(
        records
            .iter()
            .all(|record| record.kind != "kv.storage.configured"),
        "refused app.import must not append kv.storage.configured: {records:?}"
    );
}

#[test]
fn public_dispatch_helpers_refuse_ungranted_resource_commands_before_decide() {
    let dir = tempdir().unwrap();
    let mut core = terrane_host::open_at_log_path(dir.path().join("log.bin")).unwrap();
    app(&mut core, "demo");

    let dry_run = terrane_host::dry_run_public_on_core(
        &core,
        "kv.rm",
        &["demo".into(), "missing-key".into()],
    )
    .unwrap_err();
    assert!(dry_run.contains("permission required"), "{dry_run}");
    assert!(
        !dry_run.contains("KeyNotFound"),
        "dry run must not leak decide-time key existence: {dry_run}"
    );

    let commit = terrane_host::dispatch_public_on_core(
        &mut core,
        "kv.set",
        &["demo".into(), "key".into(), "value".into()],
    )
    .unwrap_err();
    assert!(commit.contains("permission required"), "{commit}");

    grant(&mut core, "demo", "kv");
    let outcome = terrane_host::dispatch_public_on_core(
        &mut core,
        "kv.set",
        &["demo".into(), "key".into(), "value".into()],
    )
    .unwrap();
    assert_eq!(outcome.records.len(), 1);
}
