use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    AppId, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Effect, Error, EventPattern,
    EventRecord, EventSpec, GrantResourceSpec, ReadValue, ResourceMethod, ResourceReadCtx, Result,
    StateStore,
};

mod doc;
pub mod ops;

pub const MAX_PIXEL_BUDGET: u64 = 64_000_000;
pub const MAX_TRANSFORMS_PER_APP: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TransformRecord {
    pub source_hash: String,
    pub ops_json: String,
    pub dest_hash: String,
    pub dest_size: u64,
    pub dest_mime: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaState {
    pub transforms: BTreeMap<AppId, BTreeMap<String, TransformRecord>>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Transformed {
    pub app: String,
    pub source_hash: String,
    pub ops_json: String,
    pub dest_name: String,
    pub dest_hash: String,
    pub dest_size: u64,
    pub dest_mime: String,
}

pub fn transformed_event(
    app: impl Into<String>,
    source_hash: impl Into<String>,
    ops_json: impl Into<String>,
    dest_name: impl Into<String>,
    dest_hash: impl Into<String>,
    dest_size: u64,
    dest_mime: impl Into<String>,
) -> Result<EventRecord> {
    encode_event(
        "media.transformed",
        &Transformed {
            app: app.into(),
            source_hash: source_hash.into(),
            ops_json: ops_json.into(),
            dest_name: dest_name.into(),
            dest_hash: dest_hash.into(),
            dest_size,
            dest_mime: dest_mime.into(),
        },
    )
}

pub struct MediaCapability;

impl Capability for MediaCapability {
    fn namespace(&self) -> &'static str {
        "media"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec {
                name: "media.transform",
            }],
            events: vec![EventSpec {
                kind: "media.transformed",
            }],
            queries: Vec::new(),
            resources: vec![ResourceMethod::Read {
                name: "info",
                params: &["blobName"],
            }],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "media",
                &["call", "read"],
                "Inspect and transform this app's stored media.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::media_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "media.transform" => decide_transform(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "info" => read_info(ctx, args),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: media.{other}"
            ))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "media.transformed" => {
                let e: Transformed = decode_event(record)?;
                let state = state_mut::<MediaState>(state, "media")?;
                let names = state.transforms.entry(e.app).or_default();
                names.insert(
                    e.dest_name,
                    TransformRecord {
                        source_hash: e.source_hash,
                        ops_json: e.ops_json,
                        dest_hash: e.dest_hash,
                        dest_size: e.dest_size,
                        dest_mime: e.dest_mime,
                    },
                );
                trim_keep_last(names);
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                state_mut::<MediaState>(state, "media")?.transforms.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "media.transformed" => {
                let e: Transformed = decode_event(record).ok()?;
                let prefix_len = e.source_hash.len().min(12);
                let names = ops::op_names(&e.ops_json).join(",");
                Some(format!(
                    "media.transformed source={} ops=[{}] dest={} {} bytes {}",
                    &e.source_hash[..prefix_len],
                    names,
                    e.dest_name,
                    e.dest_size,
                    e.dest_mime
                ))
            }
            _ => None,
        }
    }
}

fn decide_transform(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let source_name = arg(args, 1, "source_name")?;
    let ops_json = arg(args, 2, "ops_json")?;
    let dest_name = arg(args, 3, "dest_name")?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_blob_name(&source_name)?;
    validate_blob_name(&dest_name)?;
    let source = state_ref::<terrane_cap_blob::BlobState>(ctx.state, "blob")?
        .blobs
        .get(&app)
        .and_then(|names| names.get(&source_name))
        .cloned()
        .ok_or_else(|| Error::KeyNotFound(app.clone(), source_name.clone()))?;
    ops::validate_ops_for_mime(&source.mime, &ops_json)?;
    Ok(Decision::Effect(Effect::MediaTransform {
        app,
        source_hash: source.hash,
        source_mime: source.mime,
        ops_json,
        dest_name,
    }))
}

fn read_info(ctx: ResourceReadCtx<'_>, args: &[String]) -> Result<ReadValue> {
    let name = args.first().map(String::as_str).unwrap_or_default();
    validate_blob_name(name)?;
    let meta = state_ref::<terrane_cap_blob::BlobState>(ctx.state, "blob")?
        .blobs
        .get(ctx.app)
        .and_then(|names| names.get(name))
        .cloned()
        .ok_or_else(|| Error::KeyNotFound(ctx.app.to_string(), name.to_string()))?;
    let host = ctx
        .host
        .ok_or_else(|| Error::Runtime("media.info requires a live host".into()))?;
    Ok(ReadValue::OptString(Some(host.sample(
        "media.info",
        &[
            ctx.app.to_string(),
            name.to_string(),
            meta.hash,
            meta.size.to_string(),
            meta.mime,
        ],
    )?)))
}

fn validate_blob_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::InvalidInput("blob name must not be empty".into()));
    }
    if name.len() > terrane_cap_blob::MAX_NAME_LEN {
        return Err(Error::InvalidInput(format!(
            "blob name exceeds {} bytes",
            terrane_cap_blob::MAX_NAME_LEN
        )));
    }
    Ok(())
}

fn trim_keep_last(names: &mut BTreeMap<String, TransformRecord>) {
    while names.len() > MAX_TRANSFORMS_PER_APP {
        let Some(first) = names.keys().next().cloned() else {
            break;
        };
        names.remove(&first);
    }
}
