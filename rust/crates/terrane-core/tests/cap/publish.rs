use tempfile::tempdir;
use terrane_core::{Core, Effect, EffectRunner, EventRecord, State};

use crate::helpers::req;

const PUBKEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const PUBKEY_2: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";
const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct PublishEdge {
    pubkey: &'static str,
}

impl EffectRunner for PublishEdge {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::InstallSignedBundle { source } => Ok(vec![
                terrane_cap_publish::trusted_event(self.pubkey, "veha")?,
                terrane_cap_app::added_event("demo", "Demo", Some(source.clone()), "js")?,
                terrane_cap_publish::installed_event("demo", "1.0.0", HASH, self.pubkey, "veha")?,
            ]),
            other => Err(terrane_core::Error::Runtime(format!(
                "unexpected effect: {other:?}"
            ))),
        }
    }
}

#[test]
fn publish_install_records_tofu_provenance_and_replays() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), PublishEdge { pubkey: PUBKEY })
        .unwrap();

    let records = core
        .dispatch(req("publish.install", &["demo.terrane"]))
        .unwrap();
    assert_eq!(records[0].kind, "publish.trusted");
    assert_eq!(records[1].kind, "app.added");
    assert_eq!(records[2].kind, "publish.installed");
    assert_eq!(core.state().publish.trusted[PUBKEY], "veha");
    assert_eq!(core.state().publish.provenance["demo"].publisher_pubkey, PUBKEY);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn publish_fold_drops_app_provenance_but_keeps_trust() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), PublishEdge { pubkey: PUBKEY_2 })
        .unwrap();

    core.dispatch(req("publish.install", &["demo.terrane"]))
        .unwrap();
    core.dispatch(req("app.remove", &["demo"])).unwrap();

    assert!(!core.state().publish.provenance.contains_key("demo"));
    assert!(core.state().publish.trusted.contains_key(PUBKEY_2));
    assert!(core.replay_matches().unwrap());
}
