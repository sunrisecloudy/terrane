use loro::{ExportMode, LoroError};
use terrane_cap_interface::{
    arg, encode_event, ensure_app_exists, join_tail, parse_usize_arg, replica_peer, CommandCtx,
    Decision, Error, Result, StateStore,
};

use crate::state::Update;
use crate::sync::{fork_or_new, from_hex};

pub(crate) fn decide(ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    ensure_app_exists(ctx.bus, &app)?;

    // `crdt.merge` ingests another replica's update rather than authoring one.
    if name == "crdt.merge" {
        return decide_merge(ctx.state, app, args);
    }

    // Apply the op to a fork (never to live State), then export just the new
    // delta. The randomness in a fresh peer id is frozen into the recorded
    // bytes, so replay re-imports it and replay-identity still holds.
    let doc = fork_or_new(ctx.state, &app)?;
    if let Some(peer) = replica_peer(ctx.bus)? {
        let _ = doc.set_peer_id(peer);
    }
    let before = doc.oplog_vv();

    match name {
        "crdt.mapSet" => {
            let cname = arg(args, 1, "doc")?;
            let key = arg(args, 2, "key")?;
            let value = join_tail(args, 3);
            doc.get_map(cname.as_str())
                .insert(key.as_str(), value)
                .map_err(crdt_err)?;
        }
        "crdt.mapDel" => {
            let cname = arg(args, 1, "doc")?;
            let key = arg(args, 2, "key")?;
            doc.get_map(cname.as_str())
                .delete(key.as_str())
                .map_err(crdt_err)?;
        }
        "crdt.listPush" => {
            let cname = arg(args, 1, "doc")?;
            let value = join_tail(args, 2);
            doc.get_list(cname.as_str()).push(value).map_err(crdt_err)?;
        }
        "crdt.listInsert" => {
            let cname = arg(args, 1, "doc")?;
            let index = parse_usize_arg(args, 2, "index")?;
            let value = join_tail(args, 3);
            doc.get_list(cname.as_str())
                .insert(index, value)
                .map_err(crdt_err)?;
        }
        "crdt.listDel" => {
            let cname = arg(args, 1, "doc")?;
            let index = parse_usize_arg(args, 2, "index")?;
            doc.get_list(cname.as_str())
                .delete(index, 1)
                .map_err(crdt_err)?;
        }
        "crdt.textInsert" => {
            let cname = arg(args, 1, "doc")?;
            let index = parse_usize_arg(args, 2, "index")?;
            let text = join_tail(args, 3);
            doc.get_text(cname.as_str())
                .insert(index, &text)
                .map_err(crdt_err)?;
        }
        "crdt.textDel" => {
            let cname = arg(args, 1, "doc")?;
            let index = parse_usize_arg(args, 2, "index")?;
            let len = parse_usize_arg(args, 3, "len")?;
            doc.get_text(cname.as_str())
                .delete(index, len)
                .map_err(crdt_err)?;
        }
        other => return Err(Error::InvalidInput(format!("unknown command: {other}"))),
    }

    doc.commit();
    let bytes = doc
        .export(ExportMode::updates_owned(before))
        .map_err(|e| Error::Storage(format!("crdt export: {e}")))?;
    Ok(Decision::Commit(vec![encode_event(
        "crdt.update",
        &Update { app, bytes },
    )?]))
}

/// `crdt.merge <app> <hex>` — ingest another replica's exported Loro update.
/// Validates by importing into a fork and dedups updates we already have.
fn decide_merge(state: &dyn StateStore, app: String, args: &[String]) -> Result<Decision> {
    let hex = arg(args, 1, "update")?;
    let bytes = from_hex(&hex)?;
    let doc = fork_or_new(state, &app)?;
    let before = doc.oplog_vv();
    doc.import(&bytes)
        .map_err(|e| Error::InvalidInput(format!("crdt.merge: invalid update: {e}")))?;
    if doc.oplog_vv() == before {
        return Ok(Decision::Commit(vec![]));
    }
    Ok(Decision::Commit(vec![encode_event(
        "crdt.update",
        &Update { app, bytes },
    )?]))
}

fn crdt_err(e: LoroError) -> Error {
    Error::Runtime(format!("crdt: {e}"))
}
