use tempfile::tempdir;
use terrane_core::{Core, Effect, EffectRunner, Error, EventRecord, Result, State};

use crate::helpers::req;

#[derive(Clone, Copy)]
struct MediaRunner;

impl EffectRunner for MediaRunner {
    fn run(&self, effect: &Effect, _state: &State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::MediaTransform {
                app,
                source_hash,
                ops_json,
                dest_name,
                ..
            } => Ok(vec![
                terrane_cap_media::transformed_event(
                    app,
                    source_hash,
                    ops_json,
                    dest_name,
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    42,
                    "image/jpeg",
                )?,
                terrane_cap_blob::stored_event(
                    app,
                    dest_name,
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    42,
                    "image/jpeg",
                )?,
            ]),
            other => Err(Error::InvalidInput(format!("unexpected effect: {other:?}"))),
        }
    }
}

#[test]
fn media_transform_records_refs_and_replays_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, MediaRunner).unwrap();
    core.dispatch(req("app.add", &["gallery", "Gallery"]))
        .unwrap();
    core.dispatch(req(
        "blob.link",
        &[
            "gallery",
            "photo.png",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "12",
            "image/png",
        ],
    ))
    .unwrap();
    core.dispatch(req(
        "media.transform",
        &[
            "gallery",
            "photo.png",
            r#"[{"op":"thumbnail","size":64}]"#,
            "__thumb__/photo.png",
        ],
    ))
    .unwrap();

    assert_eq!(
        core.state().media.transforms["gallery"]["__thumb__/photo.png"].dest_mime,
        "image/jpeg"
    );
    assert_eq!(
        core.state().blob.blobs["gallery"]["__thumb__/photo.png"].hash,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
    assert!(core.replay_matches().unwrap());
    assert_eq!(Core::open(&log).unwrap().state().media, core.state().media);
}

#[test]
fn media_transform_rejects_bad_inputs_before_effect() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, MediaRunner).unwrap();
    core.dispatch(req("app.add", &["gallery", "Gallery"]))
        .unwrap();

    assert_eq!(
        core.dispatch(req(
            "media.transform",
            &["gallery", "missing", "[]", "out.png"]
        )),
        Err(Error::KeyNotFound("gallery".into(), "missing".into()))
    );
    core.dispatch(req(
        "blob.link",
        &[
            "gallery",
            "movie.mp4",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "12",
            "video/mp4",
        ],
    ))
    .unwrap();
    assert!(matches!(
        core.dispatch(req(
            "media.transform",
            &[
                "gallery",
                "movie.mp4",
                r#"[{"op":"thumbnail","size":64}]"#,
                "out.jpg",
            ],
        )),
        Err(Error::InvalidInput(_))
    ));
}
