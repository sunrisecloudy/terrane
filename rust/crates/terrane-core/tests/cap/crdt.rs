//! Engine tests for the `crdt` capability — per-app Loro documents (Map, List,
//! Text) recorded as binary updates and rebuilt by replay. The defining property
//! a CRDT has and `kv` does not — concurrent merge convergence — is proven at the
//! bottom.

use std::fs;
use std::path::Path;

use loro::LoroDoc;
use tempfile::tempdir;
use terrane_core::Core;
use terrane_core::Error;

use crate::helpers::req;

#[test]
fn crdt_map_records_reads_and_replays() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    // Writing to a missing app is rejected, like every other capability.
    assert_eq!(
        core.dispatch(req("crdt.mapSet", &["ghost", "prefs", "k", "v"])),
        Err(Error::AppNotFound("ghost".into()))
    );

    // A write produces exactly one opaque crdt.update record.
    let records = core
        .dispatch(req("crdt.mapSet", &["notes", "prefs", "theme", "dark"]))
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "crdt.update");

    core.dispatch(req("crdt.mapSet", &["notes", "prefs", "lang", "en"]))
        .unwrap();
    core.dispatch(req("crdt.mapDel", &["notes", "prefs", "lang"]))
        .unwrap();

    // Replay rebuilds the document from the recorded updates alone — identical.
    assert!(core.replay_matches().unwrap());
    let reopened = Core::open(&log).unwrap();
    let doc = &reopened.state().crdt.docs["notes"];
    assert_eq!(map_get(doc, "prefs", "theme"), Some("dark".into()));
    assert_eq!(map_get(doc, "prefs", "lang"), None);
}

#[test]
fn crdt_list_and_text_replay() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    // List: push, insert at a position, delete one.
    core.dispatch(req("crdt.listPush", &["notes", "todo", "a"]))
        .unwrap();
    core.dispatch(req("crdt.listPush", &["notes", "todo", "c"]))
        .unwrap();
    core.dispatch(req("crdt.listInsert", &["notes", "todo", "1", "b"]))
        .unwrap();
    core.dispatch(req("crdt.listDel", &["notes", "todo", "0"]))
        .unwrap();

    // Text: insert, then splice in the middle.
    core.dispatch(req(
        "crdt.textInsert",
        &["notes", "body", "0", "hello world"],
    ))
    .unwrap();
    core.dispatch(req("crdt.textInsert", &["notes", "body", "5", " there"]))
        .unwrap();
    core.dispatch(req("crdt.textDel", &["notes", "body", "0", "5"]))
        .unwrap();

    // A bad numeric arg is a clear typed error, not a panic.
    assert!(matches!(
        core.dispatch(req("crdt.listDel", &["notes", "todo", "notanum"])),
        Err(Error::InvalidInput(_))
    ));

    assert!(core.replay_matches().unwrap());
    let reopened = Core::open(&log).unwrap();
    let doc = &reopened.state().crdt.docs["notes"];
    assert_eq!(list_strings(doc, "todo"), vec!["b", "c"]);
    assert_eq!(doc.get_text("body").to_string(), " there world");
}

/// The CRDT payoff `kv` cannot give: two replicas edit concurrently, exchange
/// their recorded updates, and converge to the same value with no lost write.
#[test]
fn concurrent_replicas_merge_without_losing_writes() {
    // Two "devices" with distinct peer ids, both starting from the same doc.
    let alice = LoroDoc::new();
    alice.set_peer_id(1).unwrap();
    let bob = LoroDoc::new();
    bob.set_peer_id(2).unwrap();

    // Concurrent, offline edits to the same map.
    alice.get_map("prefs").insert("theme", "dark").unwrap();
    alice.commit();
    bob.get_map("prefs").insert("lang", "en").unwrap();
    bob.commit();

    // Exchange updates (the same bytes a `crdt.update` event carries).
    let from_alice = alice.export(loro::ExportMode::all_updates()).unwrap();
    let from_bob = bob.export(loro::ExportMode::all_updates()).unwrap();
    bob.import(&from_alice).unwrap();
    alice.import(&from_bob).unwrap();

    // Both converge, and neither write was lost (last-writer-wins kv would drop one).
    assert_eq!(alice.get_deep_value(), bob.get_deep_value());
    assert_eq!(map_get(&alice, "prefs", "theme"), Some("dark".into()));
    assert_eq!(map_get(&alice, "prefs", "lang"), Some("en".into()));
}

/// Collaboration through the *real capability*: two replicas of the same app
/// each record a `crdt.update` for a concurrent write, then exchange updates.
/// Both writes must survive — the regression guard for the per-app-peer bug,
/// where shared PeerIDs collided and a merge silently dropped one write.
#[test]
fn two_app_replicas_merge_with_no_lost_writes() {
    let dir = tempdir().unwrap();
    let make_replica = |sub: &str, item: &str| {
        let mut core = Core::open(dir.path().join(sub).join("log.bin")).unwrap();
        core.dispatch(req("app.add", &["collab", "Collab"]))
            .unwrap();
        core.dispatch(req("crdt.listPush", &["collab", "todos", item]))
            .unwrap();
        core
    };
    let alice = make_replica("a", "buy milk");
    let bob = make_replica("b", "walk dog");

    // Ship Bob's recorded update to Alice (what sync will do over the wire).
    let from_bob = bob.state().crdt.docs["collab"]
        .export(loro::ExportMode::all_updates())
        .unwrap();
    let merged = alice.state().crdt.docs["collab"].fork();
    merged.import(&from_bob).unwrap();

    let items = list_strings(&merged, "todos");
    assert_eq!(items.len(), 2, "both writes survive the merge: {items:?}");
    assert!(items.contains(&"buy milk".to_string()));
    assert!(items.contains(&"walk dog".to_string()));
}

#[test]
fn removing_the_app_drops_its_document() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("crdt.mapSet", &["notes", "prefs", "theme", "dark"]))
        .unwrap();
    assert!(core.state().crdt.docs.contains_key("notes"));

    // crdt reacts to app.removed via broadcast fold — no app→crdt coupling.
    core.dispatch(req("app.remove", &["notes"])).unwrap();
    assert!(core.state().crdt.docs.is_empty());
    assert!(Core::open(&log).unwrap().state().crdt.docs.is_empty());
}

const BACKEND: &str = r#"
var crdt = ctx.resource.crdt;
function handle(input) {
    var verb = input[0];
    if (verb === "set")  { crdt.mapSet("prefs", input[1], input[2]); return "ok"; }
    if (verb === "get")  { var v = crdt.mapGet("prefs", input[1]); return v == null ? "(none)" : v; }
    if (verb === "all")  {
        var a = crdt.mapAll("prefs"); var ks = [];
        for (var k in a) { ks.push(k + "=" + a[k]); }
        ks.sort();
        return ks.join(",");
    }
    if (verb === "push") { crdt.listPush("todo", input[1]); return "" + crdt.listAll("todo").length; }
    if (verb === "list") { return crdt.listAll("todo").join(","); }
    return "?";
}
"#;

#[test]
fn host_run_drives_crdt_resource_and_replays() {
    let dir = tempdir().unwrap();
    let bundle = dir.path().join("notes");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "notes", "name":"Notes","runtime":"js","backend":"main.js", "resources": ["crdt"] }"#,
    )
    .unwrap();
    fs::write(bundle.join("main.js"), BACKEND).unwrap();

    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req(
        "app.add",
        &["notes", "Notes", "--source", path(&bundle)],
    ))
    .unwrap();

    core.dispatch(req("js-runtime.run", &["notes", "set", "theme", "dark"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("ok"));
    core.dispatch(req("js-runtime.run", &["notes", "get", "theme"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("dark"));

    // A list grows across runs; reads come back as a real JS array.
    core.dispatch(req("js-runtime.run", &["notes", "push", "a"]))
        .unwrap();
    core.dispatch(req("js-runtime.run", &["notes", "push", "b"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("2"));
    core.dispatch(req("js-runtime.run", &["notes", "list"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("a,b"));

    // Option-A: the log holds only crdt.update events; replay rebuilds without JS.
    assert!(core.replay_matches().unwrap());
}

fn map_get(doc: &LoroDoc, container: &str, key: &str) -> Option<String> {
    match doc.get_map(container).get_deep_value() {
        loro::LoroValue::Map(m) => m.get(key).and_then(|v| match v {
            loro::LoroValue::String(s) => Some(s.as_ref().to_string()),
            _ => None,
        }),
        _ => None,
    }
}

fn list_strings(doc: &LoroDoc, container: &str) -> Vec<String> {
    match doc.get_list(container).get_deep_value() {
        loro::LoroValue::List(l) => l
            .iter()
            .filter_map(|v| match v {
                loro::LoroValue::String(s) => Some(s.as_ref().to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}
