//! In-memory presence hub.
//!
//! This is host-edge state: live-only, process-local, and intentionally absent
//! from the event log. The sync-v2 WebSocket route can attach peer sockets here;
//! backend `ctx.resource.presence.publish` already flows through the same hub.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use terrane_cap_interface::{Error, Result};
use terrane_cap_presence::{
    ChannelLimits, PresenceState, DEFAULT_MAX_RATE_PER_SEC, DEFAULT_MAX_PAYLOAD_BYTES,
};
use terrane_core::State;

const RATE_WINDOW: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceFrame {
    pub app: String,
    pub channel: String,
    pub from_peer: String,
    pub payload: String,
}

#[derive(Default)]
struct Hub {
    peers: BTreeMap<(String, String), BTreeSet<String>>,
    delivered: Vec<PresenceFrame>,
    rate: BTreeMap<(String, String, String), RateBucket>,
}

struct RateBucket {
    window_start: Instant,
    count: u32,
}

pub fn publish(
    state: &State,
    app: &str,
    channel: &str,
    payload: &str,
) -> Result<Vec<PresenceFrame>> {
    let limits = limits_for(&state.presence, app, channel);
    if payload.len() > limits.max_payload as usize {
        return Err(Error::InvalidInput(format!(
            "presence payload for {app}/{channel} exceeds {} bytes",
            limits.max_payload
        )));
    }

    let from_peer = local_peer_id(state);
    let mut hub = hub()
        .lock()
        .map_err(|_| Error::Runtime("presence hub lock poisoned".into()))?;
    hub.check_rate(app, channel, &from_peer, limits.max_rate)?;
    let frame = PresenceFrame {
        app: app.to_string(),
        channel: channel.to_string(),
        from_peer,
        payload: payload.to_string(),
    };
    let recipients = hub
        .peers
        .get(&(app.to_string(), channel.to_string()))
        .cloned()
        .unwrap_or_default();
    let delivered = recipients
        .into_iter()
        .filter(|peer| peer != &frame.from_peer)
        .map(|_| frame.clone())
        .collect::<Vec<_>>();
    hub.delivered.extend(delivered.iter().cloned());
    Ok(delivered)
}

pub fn peers_json(app: &str, channel: &str) -> Result<String> {
    let peers = peers(app, channel)?;
    serde_json::to_string(&peers)
        .map_err(|e| Error::Runtime(format!("presence peers encode failed: {e}")))
}

pub fn peers(app: &str, channel: &str) -> Result<Vec<String>> {
    let hub = hub()
        .lock()
        .map_err(|_| Error::Runtime("presence hub lock poisoned".into()))?;
    Ok(hub
        .peers
        .get(&(app.to_string(), channel.to_string()))
        .map(|peers| peers.iter().cloned().collect())
        .unwrap_or_default())
}

pub fn join_peer(app: &str, channel: &str, peer: &str) -> Result<()> {
    let mut hub = hub()
        .lock()
        .map_err(|_| Error::Runtime("presence hub lock poisoned".into()))?;
    hub.peers
        .entry((app.to_string(), channel.to_string()))
        .or_default()
        .insert(peer.to_string());
    Ok(())
}

pub fn leave_peer(app: &str, channel: &str, peer: &str) -> Result<()> {
    let mut hub = hub()
        .lock()
        .map_err(|_| Error::Runtime("presence hub lock poisoned".into()))?;
    if let Some(peers) = hub.peers.get_mut(&(app.to_string(), channel.to_string())) {
        peers.remove(peer);
    }
    Ok(())
}

pub fn take_delivered() -> Result<Vec<PresenceFrame>> {
    let mut hub = hub()
        .lock()
        .map_err(|_| Error::Runtime("presence hub lock poisoned".into()))?;
    Ok(std::mem::take(&mut hub.delivered))
}

pub fn reset_for_tests() -> Result<()> {
    let mut hub = hub()
        .lock()
        .map_err(|_| Error::Runtime("presence hub lock poisoned".into()))?;
    *hub = Hub::default();
    Ok(())
}

impl Hub {
    fn check_rate(
        &mut self,
        app: &str,
        channel: &str,
        peer: &str,
        max_rate: u32,
    ) -> Result<()> {
        let now = Instant::now();
        let key = (app.to_string(), channel.to_string(), peer.to_string());
        let bucket = self.rate.entry(key).or_insert(RateBucket {
            window_start: now,
            count: 0,
        });
        if now.duration_since(bucket.window_start) >= RATE_WINDOW {
            bucket.window_start = now;
            bucket.count = 0;
        }
        if bucket.count >= max_rate {
            return Err(Error::Runtime(format!(
                "presence rate limit exceeded for {app}/{channel}: {max_rate}/sec"
            )));
        }
        bucket.count += 1;
        Ok(())
    }
}

fn limits_for(state: &PresenceState, app: &str, channel: &str) -> ChannelLimits {
    state
        .channels
        .get(app)
        .and_then(|channels| channels.get(channel))
        .cloned()
        .unwrap_or(ChannelLimits {
            max_payload: DEFAULT_MAX_PAYLOAD_BYTES as u32,
            max_rate: DEFAULT_MAX_RATE_PER_SEC,
        })
}

fn local_peer_id(state: &State) -> String {
    state
        .replica
        .peer
        .map(|id| format!("{id:x}"))
        .unwrap_or_else(|| "local".to_string())
}

fn hub() -> &'static Mutex<Hub> {
    static HUB: OnceLock<Mutex<Hub>> = OnceLock::new();
    HUB.get_or_init(|| Mutex::new(Hub::default()))
}
