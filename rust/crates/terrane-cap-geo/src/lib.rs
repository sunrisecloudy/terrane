//! The `geo` capability — replay-safe one-shot location reads. A location fix
//! is an [`Effect`](terrane_cap_interface::Effect), not a pure decide: the edge
//! observes OS/browser geolocation once, applies the requested precision before
//! record construction, and records `geo.observed`. Replay folds that event and
//! never asks a location provider again.

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut,
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Effect, Error, EventPattern,
    EventRecord, EventSpec, GrantResourceSpec, QueryCtx, QuerySpec, QueryValue, ReadValue,
    ResourceMethod, ResourceReadCtx, Result, StateStore,
};

mod doc;
pub mod types;

pub use types::{fix_json, round_for_precision, validate_fix, GeoFix, GeoPrecision, GeoState};
use types::{MAX_FIXES_PER_APP, RATE_LIMIT_MS};

#[derive(BorshSerialize, BorshDeserialize)]
struct Observed {
    app: String,
    lat_e7: i64,
    lon_e7: i64,
    accuracy_m: u32,
    precision: String,
    observed_at: u64,
}

/// Build the recorded event for a completed location observation. The caller is
/// the edge and must pass already-rounded coordinates for the granted precision.
pub fn observed_event(
    app: &str,
    lat_e7: i64,
    lon_e7: i64,
    accuracy_m: u32,
    precision: &str,
    observed_at: u64,
) -> Result<EventRecord> {
    validate_fix(lat_e7, lon_e7)?;
    let precision = GeoPrecision::parse(precision)?.as_str().to_string();
    encode_event(
        "geo.observed",
        &Observed {
            app: app.to_string(),
            lat_e7,
            lon_e7,
            accuracy_m,
            precision,
            observed_at,
        },
    )
}

pub struct GeoCapability;

impl Capability for GeoCapability {
    fn namespace(&self) -> &'static str {
        "geo"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec { name: "geo.locate" }],
            events: vec![EventSpec {
                kind: "geo.observed",
            }],
            queries: vec![QuerySpec {
                name: "geo.supports",
            }],
            resources: vec![
                ResourceMethod::Call {
                    name: "current",
                    params: &["precision"],
                },
                ResourceMethod::Call {
                    name: "peek",
                    params: &["precision"],
                },
                ResourceMethod::Read {
                    name: "last",
                    params: &[],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "geo",
                &["call", "read"],
                "Recorded and transient one-shot geolocation reads.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::geo_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "geo.locate" | "geo.current" | "geo.peek" => {
                let app = arg(args, 0, "app")?;
                let precision = precision_arg(args)?;
                ensure_app_exists(ctx.bus, &app)?;
                let effect = Effect::GeoLocate {
                    app,
                    precision: precision.as_str().to_string(),
                };
                if name == "geo.peek" {
                    Ok(Decision::TransientEffect(effect))
                } else {
                    Ok(Decision::Effect(effect))
                }
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "geo.observed" => {
                let e: Observed = decode_event(record)?;
                validate_fix(e.lat_e7, e.lon_e7)?;
                let geo = state_mut::<GeoState>(state, "geo")?;
                enforce_rate_limit(geo, &e.app, e.observed_at)?;
                let fixes = geo
                    .fixes
                    .entry(e.app)
                    .or_default();
                fixes.push_back(GeoFix {
                    lat_e7: e.lat_e7,
                    lon_e7: e.lon_e7,
                    accuracy_m: e.accuracy_m,
                    precision: e.precision,
                    observed_at: e.observed_at,
                });
                while fixes.len() > MAX_FIXES_PER_APP {
                    fixes.pop_front();
                }
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                state_mut::<GeoState>(state, "geo")?.fixes.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        if record.kind == "geo.observed" {
            let e: Observed = decode_event(record).ok()?;
            Some(format!(
                "geo.observed {} (precision={}, accuracy_m={}, observed_at={})",
                e.app, e.precision, e.accuracy_m, e.observed_at
            ))
        } else {
            None
        }
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, _args: &[String]) -> Result<QueryValue> {
        match name {
            "supports" | "geo.supports" => {
                let supports = ctx
                    .state
                    .get("geo")
                    .and_then(|slice| slice.downcast_ref::<GeoState>())
                    .map(|state| state.supports)
                    .unwrap_or(false);
                Ok(QueryValue::Bool(supports))
            }
            other => Err(Error::InvalidInput(format!("unknown query: geo.{other}"))),
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
                    .get("geo")
                    .and_then(|slice| slice.downcast_ref::<GeoState>())
                    .and_then(|state| state.fixes.get(ctx.app))
                    .and_then(|fixes| fixes.back())
                    .map(fix_json);
                Ok(ReadValue::OptString(last))
            }
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: geo.{other}"
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
            "current" | "peek" => {
                let record = records
                    .first()
                    .ok_or_else(|| Error::Runtime("geo observation produced no result".into()))?;
                let e: Observed = decode_event(record)?;
                Ok(ReadValue::OptString(Some(fix_json(&GeoFix {
                    lat_e7: e.lat_e7,
                    lon_e7: e.lon_e7,
                    accuracy_m: e.accuracy_m,
                    precision: e.precision,
                    observed_at: e.observed_at,
                }))))
            }
            other => Err(Error::InvalidInput(format!(
                "geo.{other} is not a callable resource"
            ))),
        }
    }
}

fn precision_arg(args: &[String]) -> Result<GeoPrecision> {
    match args.get(1) {
        Some(raw) => GeoPrecision::parse(raw),
        None => Ok(GeoPrecision::Coarse),
    }
}

fn enforce_rate_limit(state: &GeoState, app: &str, observed_at: u64) -> Result<()> {
    let Some(last) = state
        .fixes
        .get(app)
        .and_then(|fixes| fixes.back())
    else {
        return Ok(());
    };
    if observed_at < last.observed_at.saturating_add(RATE_LIMIT_MS) {
        return Err(Error::InvalidInput(
            "geo.locate rate limit: at most one recorded fix per app per 10 seconds".to_string(),
        ));
    }
    Ok(())
}
