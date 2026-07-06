use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};

pub const MAX_SUBSCRIPTIONS_PER_APP: usize = 32;
pub const DELIVERY_HISTORY_LIMIT: usize = 512;
pub const MAX_PATTERN_BYTES: usize = 128;
pub const MAX_TEMPLATE_BYTES: usize = 512;
pub const MAX_DETAIL_BYTES: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PushState {
    pub subscriptions: BTreeMap<String, BTreeMap<String, PushSubscription>>,
    pub deliveries: BTreeMap<String, BTreeMap<String, BTreeMap<u64, PushDelivery>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PushSubscription {
    pub app: String,
    pub sub_id: String,
    pub event_pattern: String,
    pub template: String,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PushDelivery {
    pub status: PushDeliveryStatus,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PushDeliveryStatus {
    Delivered,
    Failed,
}

impl PushDeliveryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Subscribed {
    pub app: String,
    pub sub_id: String,
    pub event_pattern: String,
    pub template: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Unsubscribed {
    pub app: String,
    pub sub_id: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Delivered {
    pub app: String,
    pub sub_id: String,
    pub event_seq: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Failed {
    pub app: String,
    pub sub_id: String,
    pub event_seq: u64,
    pub detail: String,
}

pub fn prune_delivery_history(app_history: &mut BTreeMap<String, BTreeMap<u64, PushDelivery>>) {
    let mut keys = Vec::new();
    for (sub_id, events) in app_history.iter() {
        for event_seq in events.keys() {
            keys.push((*event_seq, sub_id.clone()));
        }
    }
    if keys.len() <= DELIVERY_HISTORY_LIMIT {
        return;
    }
    keys.sort();
    let remove_count = keys.len() - DELIVERY_HISTORY_LIMIT;
    let remove: BTreeSet<(String, u64)> = keys
        .into_iter()
        .take(remove_count)
        .map(|(seq, sub_id)| (sub_id, seq))
        .collect();
    for (sub_id, seq) in remove {
        if let Some(events) = app_history.get_mut(&sub_id) {
            events.remove(&seq);
        }
    }
    app_history.retain(|_, events| !events.is_empty());
}
