//! The `time` capability — replay-safe wall-clock reads. Reading the clock is
//! an [`Effect`](crate::Effect), not a pure decide: the edge observes
//! `SystemTime` once and records the observation as a `time.observed` event, so
//! replay folds the recorded fact and never consults a clock (the same contract
//! as `net.fetch`). A transient sibling `time.live()` is live and unrecorded,
//! for display-only timestamps (parallel to `net.get`).

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, CapManifest,
    Capability, CommandCtx, CommandSpec, Decision, Effect, Error, EventPattern, EventRecord,
    EventSpec, GrantResourceSpec, ReadValue, RecordedCallCap, ResourceMethod, ResourceReadCtx,
    Result, StateStore,
};

mod doc;

/// Per-app last recorded wall-clock observation, in UTC epoch milliseconds, kept
/// under the `time` state slice. Replay rebuilds this by folding
/// `time.observed` events; it holds only the *last* value per app — observations
/// are facts, not an ordered history (the event log is the ordering primitive).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimeState {
    pub last: BTreeMap<String, u64>,
}

/// Soft cap on recorded `time.now()` calls in one backend run, so a loop can't
/// bloat the event log. The transient `time.live()` is uncapped.
pub const MAX_OBSERVATIONS_PER_RUN: usize = 32;

#[derive(BorshSerialize, BorshDeserialize)]
struct Observed {
    app: String,
    epoch_ms: u64,
}

/// Build the recorded event for one completed wall-clock observation. Called by
/// the edge runner once it has performed the read, so the `"time.observed"` kind
/// and payload shape stay owned by this capability (the `fetched_event` pattern).
pub fn observed_event(app: &str, epoch_ms: u64) -> Result<EventRecord> {
    encode_event(
        "time.observed",
        &Observed {
            app: app.to_string(),
            epoch_ms,
        },
    )
}

/// Convert a wall-clock reading to UTC epoch milliseconds, failing with a typed
/// error if the clock reads before the Unix epoch (the `u64` payload can't
/// represent it).
pub fn system_time_to_epoch_ms(time: SystemTime) -> Result<u64> {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Runtime("wall clock reads before the Unix epoch".into()))?;
    Ok(duration.as_millis() as u64)
}

pub struct TimeCapability;

impl Capability for TimeCapability {
    fn namespace(&self) -> &'static str {
        "time"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec { name: "time.now" }],
            events: vec![EventSpec {
                kind: "time.observed",
            }],
            queries: Vec::new(),
            resources: vec![
                ResourceMethod::Call {
                    name: "now",
                    params: &[],
                },
                ResourceMethod::Call {
                    name: "live",
                    params: &[],
                },
                ResourceMethod::Read {
                    name: "last",
                    params: &[],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "time",
                &["call", "read"],
                "Recorded wall-clock time observations.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::time_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "time.now" => {
                let app = terrane_cap_interface::arg(args, 0, "app")?;
                ensure_app_exists(ctx.bus, &app)?;
                Ok(Decision::Effect(Effect::ObserveTime { app }))
            }
            "time.live" => {
                let app = terrane_cap_interface::arg(args, 0, "app")?;
                ensure_app_exists(ctx.bus, &app)?;
                Ok(Decision::TransientEffect(Effect::ObserveTime { app }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "time.observed" => {
                let e: Observed = decode_event(record)?;
                state_mut::<TimeState>(state, "time")?
                    .last
                    .insert(e.app, e.epoch_ms);
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                state_mut::<TimeState>(state, "time")?.last.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        if record.kind == "time.observed" {
            let e: Observed = decode_event(record).ok()?;
            Some(format!("time.observed {} (epoch_ms={})", e.app, e.epoch_ms))
        } else {
            None
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        _args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "last" => {
                let last = ctx
                    .state
                    .get("time")
                    .and_then(|slice| slice.downcast_ref::<TimeState>())
                    .and_then(|state| state.last.get(ctx.app))
                    .copied();
                Ok(ReadValue::OptString(last.map(|ms| ms.to_string())))
            }
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: time.{other}"
            ))),
        }
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        match method {
            "now" | "live" => {
                let record = records
                    .first()
                    .ok_or_else(|| Error::Runtime("time observation produced no result".into()))?;
                let e: Observed = decode_event(record)?;
                Ok(ReadValue::OptString(Some(e.epoch_ms.to_string())))
            }
            other => Err(Error::InvalidInput(format!(
                "time.{other} is not a callable resource"
            ))),
        }
    }

    fn recorded_call_per_run_limit(&self, method: &str) -> Option<RecordedCallCap> {
        match method {
            "now" => Some(RecordedCallCap {
                limit: MAX_OBSERVATIONS_PER_RUN,
                escape_hint: "use ctx.resource.time.live() for display-only timestamps",
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;