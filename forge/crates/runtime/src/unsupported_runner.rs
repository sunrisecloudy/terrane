//! Structured runtime-run stubs for native targets without a JS backend.
//!
//! The iOS Forge FFI cutover needs non-JS commands (`legacy.core_step`,
//! `sync.export`, `sync.import`) to build and load through `forge-core`, but
//! rquickjs-sys does not ship bindings for `aarch64-apple-ios-sim`. Until the
//! planned CR-12 JavaScriptCore/alternate backend lands, runtime execution
//! commands fail closed with `PlatformUnavailable`.

use crate::bridge::HostBridge;
use crate::{JsEngine, Program};
use forge_domain::{ActorContext, AppResult, CoreError, Manifest, Result, RunRecord};
use forge_policy::DecisionContext;

fn unavailable<T>() -> Result<T> {
    Err(CoreError::PlatformUnavailable(
        "JavaScript runtime backend is not available on this target".to_string(),
    ))
}

pub fn record_run(
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _input: &serde_json::Value,
    _seed: u64,
    _time_start: u64,
    _bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    unavailable()
}

#[allow(clippy::too_many_arguments)]
pub fn record_run_with_engine(
    _engine: &dyn JsEngine,
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _input: &serde_json::Value,
    _seed: u64,
    _time_start: u64,
    _bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    unavailable()
}

#[allow(clippy::too_many_arguments)]
pub fn record_run_with_context(
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _context: Box<dyn DecisionContext>,
    _input: &serde_json::Value,
    _seed: u64,
    _time_start: u64,
    _bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    unavailable()
}

#[allow(clippy::too_many_arguments)]
pub fn record_dispatch(
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _action_ref: &str,
    _payload: &serde_json::Value,
    _seed: u64,
    _time_start: u64,
    _bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    unavailable()
}

#[allow(clippy::too_many_arguments)]
pub fn record_dispatch_with_context(
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _context: Box<dyn DecisionContext>,
    _action_ref: &str,
    _payload: &serde_json::Value,
    _seed: u64,
    _time_start: u64,
    _bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    unavailable()
}

pub fn replay_dispatch(
    _run: &RunRecord,
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    unavailable()
}

#[allow(clippy::too_many_arguments)]
pub fn record_notification(
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _action_ref: &str,
    _notification: &serde_json::Value,
    _seed: u64,
    _time_start: u64,
    _bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    unavailable()
}

#[allow(clippy::too_many_arguments)]
pub fn record_notification_with_context(
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _context: Box<dyn DecisionContext>,
    _action_ref: &str,
    _notification: &serde_json::Value,
    _seed: u64,
    _time_start: u64,
    _bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    unavailable()
}

pub fn replay_notification(
    _run: &RunRecord,
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _action_ref: &str,
    _bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    unavailable()
}

pub fn replay(
    _run: &RunRecord,
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    unavailable()
}

pub fn replay_with_engine(
    _engine: &dyn JsEngine,
    _run: &RunRecord,
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    unavailable()
}

pub fn run_once(
    _program: &Program,
    _manifest: &Manifest,
    _actor: &ActorContext,
    _input: &serde_json::Value,
    _seed: u64,
    _time_start: u64,
    _bridge: &mut dyn HostBridge,
) -> Result<AppResult> {
    unavailable()
}
