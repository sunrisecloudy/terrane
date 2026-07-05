use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, state_mut, EventRecord, Result, StateStore,
};

use crate::types::{
    prune_delivery_history, Delivered, Failed, PushDelivery, PushDeliveryStatus, PushState,
    PushSubscription, Subscribed, Unsubscribed,
};

pub fn subscribed_event(
    app: &str,
    sub_id: &str,
    event_pattern: &str,
    template: &str,
) -> Result<EventRecord> {
    encode_event(
        "push.subscribed",
        &Subscribed {
            app: app.to_string(),
            sub_id: sub_id.to_string(),
            event_pattern: event_pattern.to_string(),
            template: template.to_string(),
        },
    )
}

pub fn unsubscribed_event(app: &str, sub_id: &str) -> Result<EventRecord> {
    encode_event(
        "push.unsubscribed",
        &Unsubscribed {
            app: app.to_string(),
            sub_id: sub_id.to_string(),
        },
    )
}

pub fn delivered_event(app: &str, sub_id: &str, event_seq: u64) -> Result<EventRecord> {
    encode_event(
        "push.delivered",
        &Delivered {
            app: app.to_string(),
            sub_id: sub_id.to_string(),
            event_seq,
        },
    )
}

pub fn failed_event(app: &str, sub_id: &str, event_seq: u64, detail: &str) -> Result<EventRecord> {
    encode_event(
        "push.failed",
        &Failed {
            app: app.to_string(),
            sub_id: sub_id.to_string(),
            event_seq,
            detail: detail.to_string(),
        },
    )
}

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "push.subscribed" => {
            let event: Subscribed = decode_event(record)?;
            let state = state_mut::<PushState>(state, "push")?;
            state
                .subscriptions
                .entry(event.app.clone())
                .or_default()
                .insert(
                    event.sub_id.clone(),
                    PushSubscription {
                        app: event.app,
                        sub_id: event.sub_id,
                        event_pattern: event.event_pattern,
                        template: event.template,
                    },
                );
        }
        "push.unsubscribed" => {
            let event: Unsubscribed = decode_event(record)?;
            let state = state_mut::<PushState>(state, "push")?;
            if let Some(subs) = state.subscriptions.get_mut(&event.app) {
                subs.remove(&event.sub_id);
                if subs.is_empty() {
                    state.subscriptions.remove(&event.app);
                }
            }
        }
        "push.delivered" => {
            let event: Delivered = decode_event(record)?;
            let state = state_mut::<PushState>(state, "push")?;
            let history = state
                .deliveries
                .entry(event.app)
                .or_default();
            history.entry(event.sub_id).or_default().insert(
                event.event_seq,
                PushDelivery {
                    status: PushDeliveryStatus::Delivered,
                    detail: None,
                },
            );
            prune_delivery_history(history);
        }
        "push.failed" => {
            let event: Failed = decode_event(record)?;
            let state = state_mut::<PushState>(state, "push")?;
            let history = state
                .deliveries
                .entry(event.app)
                .or_default();
            history.entry(event.sub_id).or_default().insert(
                event.event_seq,
                PushDelivery {
                    status: PushDeliveryStatus::Failed,
                    detail: Some(event.detail),
                },
            );
            prune_delivery_history(history);
        }
        "app.removed" => {
            let event = decode_app_removed(record)?;
            let state = state_mut::<PushState>(state, "push")?;
            state.subscriptions.remove(&event.id);
            state.deliveries.remove(&event.id);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "push.subscribed" => {
            let event: Subscribed = decode_event(record).ok()?;
            Some(format!(
                "push.subscribed {} {} {}",
                event.app, event.sub_id, event.event_pattern
            ))
        }
        "push.unsubscribed" => {
            let event: Unsubscribed = decode_event(record).ok()?;
            Some(format!("push.unsubscribed {} {}", event.app, event.sub_id))
        }
        "push.delivered" => {
            let event: Delivered = decode_event(record).ok()?;
            Some(format!(
                "push.delivered {} {} #{}",
                event.app, event.sub_id, event.event_seq
            ))
        }
        "push.failed" => {
            let event: Failed = decode_event(record).ok()?;
            Some(format!(
                "push.failed {} {} #{}",
                event.app, event.sub_id, event.event_seq
            ))
        }
        _ => None,
    }
}

pub(crate) fn app_of(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "push.subscribed" => decode_event::<Subscribed>(record).ok().map(|e| e.app),
        "push.unsubscribed" => decode_event::<Unsubscribed>(record).ok().map(|e| e.app),
        "push.delivered" => decode_event::<Delivered>(record).ok().map(|e| e.app),
        "push.failed" => decode_event::<Failed>(record).ok().map(|e| e.app),
        _ => None,
    }
}
