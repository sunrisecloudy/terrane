use std::any::Any;

use terrane_cap_blob::{BlobMeta, BlobState};
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryValue, Result,
    StateStore,
};
use terrane_cap_media::{transformed_event, MediaCapability, MediaState};

#[derive(Default)]
struct Store {
    blob: BlobState,
    media: MediaState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "blob" => Some(&self.blob),
            "media" => Some(&self.media),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "blob" => Some(&mut self.blob),
            "media" => Some(&mut self.media),
            _ => None,
        }
    }
}

struct AppBus {
    exists: bool,
}

impl CapBus for AppBus {
    fn query(&self, cap: &str, name: &str, _args: &[String]) -> Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(self.exists)),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

fn ctx<'a>(store: &'a Store, bus: &'a AppBus) -> CommandCtx<'a> {
    CommandCtx { state: store, bus }
}

#[test]
fn transform_decides_media_effect_from_blob_metadata() {
    let mut store = Store::default();
    store.blob.blobs.entry("gallery".into()).or_default().insert(
        "photo.png".into(),
        BlobMeta {
            hash: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            size: 128,
            mime: "image/png".into(),
        },
    );
    let ops = r#"[{"op":"thumbnail","size":64}]"#.to_string();
    let decision = MediaCapability
        .decide(
            ctx(&store, &AppBus { exists: true }),
            "media.transform",
            &[
                "gallery".into(),
                "photo.png".into(),
                ops.clone(),
                "__thumb__/photo.png".into(),
            ],
        )
        .unwrap();

    assert_eq!(
        decision,
        Decision::Effect(Effect::MediaTransform {
            app: "gallery".into(),
            source_hash: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            source_mime: "image/png".into(),
            ops_json: ops,
            dest_name: "__thumb__/photo.png".into(),
        })
    );
}

#[test]
fn validation_errors_are_typed() {
    let store = Store::default();
    assert_eq!(
        MediaCapability
            .decide(
                ctx(&store, &AppBus { exists: false }),
                "media.transform",
                &["ghost".into(), "a".into(), "[]".into(), "b".into()],
            )
            .unwrap_err(),
        Error::AppNotFound("ghost".into())
    );
    assert!(matches!(
        terrane_cap_media::ops::parse_ops("not json").unwrap_err(),
        Error::InvalidInput(_)
    ));
    assert!(matches!(
        terrane_cap_media::ops::parse_ops(r#"[{"op":"rotate","degrees":45}]"#).unwrap_err(),
        Error::InvalidInput(_)
    ));
}

#[test]
fn transformed_event_folds_and_app_removed_clears() {
    let cap = MediaCapability;
    let mut store = Store::default();
    let event = transformed_event(
        "gallery",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        r#"[{"op":"thumbnail","size":64}]"#,
        "__thumb__/a.png",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        123,
        "image/jpeg",
    )
    .unwrap();
    cap.fold(&mut store, &event).unwrap();
    assert_eq!(
        store.media.transforms["gallery"]["__thumb__/a.png"].dest_mime,
        "image/jpeg"
    );

    let removed = encode_event(
        "app.removed",
        &Removed {
            id: "gallery".into(),
        },
    )
    .unwrap();
    cap.fold(&mut store, &removed).unwrap();
    assert!(store.media.transforms.is_empty());
}

#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
struct Removed {
    id: String,
}
