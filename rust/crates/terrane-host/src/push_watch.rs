//! Host-edge local push delivery.
//!
//! The `push` capability records subscriptions and per-replica outcomes. This
//! module is the edge effect: it reacts to newly committed records and queues a
//! `native.notification.show` request, then records the outcome.

use std::collections::BTreeMap;

use terrane_core::{Error, EventRecord, Request, Result};

use crate::HostCore;

const STALE_CUTOFF_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone)]
struct Match {
    app: String,
    sub_id: String,
    event_seq: u64,
    title: String,
    body: String,
}

pub fn should_process_after_command(command: &str) -> bool {
    !(command.starts_with("push.") || command.starts_with("native."))
}

pub fn process_committed_records(core: &mut HostCore, records: &[EventRecord]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let log = core.log_records()?;
    let mut matches = Vec::new();
    for record in records {
        let Some(event_seq) = event_seq_for_record(&log, record)? else {
            continue;
        };
        collect_matches(core, event_seq, record, &mut matches)?;
    }
    for delivery in coalesce(matches) {
        queue_delivery(core, delivery)?;
    }
    Ok(())
}

fn event_seq_for_record(log: &[EventRecord], record: &EventRecord) -> Result<Option<u64>> {
    let Some(index) = log
        .iter()
        .position(|candidate| {
            candidate.kind == record.kind
                && candidate.payload == record.payload
                && candidate.actor == record.actor
        })
    else {
        return Ok(None);
    };
    Ok(Some(
        u64::try_from(index + 1)
            .map_err(|_| Error::Storage("push event sequence overflow".into()))?,
    ))
}

fn collect_matches(
    core: &HostCore,
    event_seq: u64,
    record: &EventRecord,
    matches: &mut Vec<Match>,
) -> Result<()> {
    if record.kind.starts_with("push.") || record.kind.starts_with("native.") {
        return Ok(());
    }
    let Some(app) = core.app_of_record(record) else {
        return Ok(());
    };
    let Some(subs) = core.state().push.subscriptions.get(&app) else {
        return Ok(());
    };
    for sub in subs.values() {
        if !terrane_cap_push::matches_pattern(&sub.event_pattern, &record.kind) {
            continue;
        }
        if delivered(core, &app, &sub.sub_id, event_seq) {
            continue;
        }
        let (title, body) = terrane_cap_push::render_template(&sub.template, record, None)?;
        matches.push(Match {
            app: app.clone(),
            sub_id: sub.sub_id.clone(),
            event_seq,
            title,
            body,
        });
    }
    Ok(())
}

fn delivered(core: &HostCore, app: &str, sub_id: &str, event_seq: u64) -> bool {
    core.state()
        .push
        .deliveries
        .get(app)
        .and_then(|app_history| app_history.get(sub_id))
        .is_some_and(|events| events.contains_key(&event_seq))
}

fn coalesce(matches: Vec<Match>) -> Vec<Match> {
    let mut grouped: BTreeMap<(String, String), Vec<Match>> = BTreeMap::new();
    for item in matches {
        grouped
            .entry((item.app.clone(), item.sub_id.clone()))
            .or_default()
            .push(item);
    }
    let mut out = Vec::new();
    for ((app, sub_id), mut items) in grouped {
        if items.len() == 1 {
            if let Some(item) = items.pop() {
                out.push(item);
            }
            continue;
        }
        items.sort_by_key(|item| item.event_seq);
        let last_seq = items.last().map(|item| item.event_seq).unwrap_or_default();
        out.push(Match {
            title: format!("{} changes in {}", items.len(), app),
            body: "Multiple matching updates arrived together.".to_string(),
            app,
            sub_id,
            event_seq: last_seq,
        });
    }
    out
}

fn queue_delivery(core: &mut HostCore, delivery: Match) -> Result<()> {
    let request_id = format!("push-{}-{}", delivery.sub_id, delivery.event_seq);
    let native = core.dispatch(Request::trusted_host(
        "native.notification.show",
        vec![
            delivery.app.clone(),
            request_id,
            delivery.title,
            delivery.body,
        ],
    ));
    let (status, detail) = match native {
        Ok(_) => ("delivered", None),
        Err(err) => ("failed", Some(err.to_string())),
    };
    let mut args = vec![
        delivery.app,
        delivery.sub_id,
        delivery.event_seq.to_string(),
        status.to_string(),
    ];
    if let Some(detail) = detail {
        args.push(detail);
    }
    core.dispatch(Request::trusted_host("push.record-delivery", args))?;
    Ok(())
}

pub fn stale_cutoff_ms() -> u64 {
    STALE_CUTOFF_MS
}
