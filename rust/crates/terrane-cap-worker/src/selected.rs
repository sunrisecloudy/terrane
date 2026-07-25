use std::any::Any;

use terrane_cap_interface::{Capability, Effect, EventRecord, Result, StateStore};

pub type HeadlessEffect = fn(&dyn StateStore, &Effect) -> Result<Option<Vec<EventRecord>>>;

pub struct SelectedCapability {
    pub capability: Box<dyn Capability>,
    pub state: Option<(&'static str, Box<dyn Any>)>,
    pub background_work: fn(&dyn terrane_cap_interface::StateStore) -> bool,
    pub headless_effect: HeadlessEffect,
}

#[allow(dead_code)]
fn no_background_work(_state: &dyn terrane_cap_interface::StateStore) -> bool {
    false
}

#[allow(dead_code)]
fn no_headless_effect(
    _state: &dyn StateStore,
    _effect: &Effect,
) -> Result<Option<Vec<EventRecord>>> {
    Ok(None)
}

macro_rules! selected {
    ($feature:literal, $capability:path) => {
        #[cfg(feature = $feature)]
        pub fn build() -> SelectedCapability {
            SelectedCapability {
                capability: Box::new($capability),
                state: None,
                background_work: no_background_work,
                headless_effect: no_headless_effect,
            }
        }
    };
    ($feature:literal, $capability:path, $namespace:literal, $state:path) => {
        #[cfg(feature = $feature)]
        pub fn build() -> SelectedCapability {
            SelectedCapability {
                capability: Box::new($capability),
                state: Some(($namespace, Box::new(<$state>::default()))),
                background_work: no_background_work,
                headless_effect: no_headless_effect,
            }
        }
    };
    ($feature:literal, $capability:path, $namespace:literal, $state:path, $background:path) => {
        #[cfg(feature = $feature)]
        pub fn build() -> SelectedCapability {
            SelectedCapability {
                capability: Box::new($capability),
                state: Some(($namespace, Box::new(<$state>::default()))),
                background_work: $background,
                headless_effect: no_headless_effect,
            }
        }
    };
}

#[cfg(feature = "cap-automation")]
fn automation_background(state: &dyn terrane_cap_interface::StateStore) -> bool {
    terrane_cap_interface::state_ref::<terrane_cap_automation::AutomationState>(state, "automation")
        .is_ok_and(|state| state.rules.values().any(|rules| !rules.is_empty()))
}

#[cfg(feature = "cap-job")]
fn job_background(state: &dyn terrane_cap_interface::StateStore) -> bool {
    terrane_cap_interface::state_ref::<terrane_cap_job_queue::JobState>(state, "job").is_ok_and(
        |state| {
            state
                .jobs
                .values()
                .flat_map(|jobs| jobs.values())
                .any(|job| matches!(job.status.as_str(), "queued" | "running"))
        },
    )
}

#[cfg(feature = "cap-scheduler")]
fn scheduler_background(state: &dyn terrane_cap_interface::StateStore) -> bool {
    terrane_cap_interface::state_ref::<terrane_cap_scheduler::SchedulerState>(state, "scheduler")
        .is_ok_and(|state| {
            state
                .schedules
                .values()
                .any(|schedules| !schedules.is_empty())
        })
}

#[cfg(feature = "cap-stream")]
fn stream_background(state: &dyn terrane_cap_interface::StateStore) -> bool {
    terrane_cap_interface::state_ref::<terrane_cap_stream::StreamState>(state, "stream")
        .is_ok_and(|state| state.streams.values().any(|streams| !streams.is_empty()))
}

#[cfg(feature = "cap-webhook")]
fn webhook_background(state: &dyn terrane_cap_interface::StateStore) -> bool {
    terrane_cap_interface::state_ref::<terrane_cap_webhook::WebhookState>(state, "webhook")
        .is_ok_and(|state| state.routes.values().any(|routes| !routes.is_empty()))
}

#[cfg(feature = "cap-time")]
fn time_headless(_state: &dyn StateStore, effect: &Effect) -> Result<Option<Vec<EventRecord>>> {
    let Effect::ObserveTime { app } = effect else {
        return Ok(None);
    };
    let epoch_ms = terrane_cap_time::system_time_to_epoch_ms(std::time::SystemTime::now())?;
    Ok(Some(vec![terrane_cap_time::observed_event(app, epoch_ms)?]))
}

#[cfg(feature = "cap-webhook")]
fn webhook_headless(state: &dyn StateStore, effect: &Effect) -> Result<Option<Vec<EventRecord>>> {
    let Effect::WebhookRegister { app, name, verb } = effect else {
        return Ok(None);
    };
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| {
        terrane_cap_interface::Error::Runtime(format!("mint webhook token: {error}"))
    })?;
    let mut token = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(token, "{byte:02x}");
    }
    let webhook =
        terrane_cap_interface::state_ref::<terrane_cap_webhook::WebhookState>(state, "webhook")?;
    let event = if webhook
        .routes
        .get(app)
        .and_then(|routes| routes.get(name))
        .is_some()
    {
        terrane_cap_webhook::rotated_event(app, name, verb, token)?
    } else {
        terrane_cap_webhook::registered_event(app, name, verb, token)?
    };
    Ok(Some(vec![event]))
}

selected!(
    "cap-agent",
    terrane_cap_agent::AgentCapability,
    "agent",
    terrane_cap_agent::AgentState
);
selected!(
    "cap-applescript",
    terrane_cap_applescript::AppleScriptCapability,
    "applescript",
    terrane_cap_applescript::AppleScriptState
);
selected!(
    "cap-automation",
    terrane_cap_automation::AutomationCapability,
    "automation",
    terrane_cap_automation::AutomationState,
    automation_background
);
selected!(
    "cap-browser",
    terrane_cap_browser::BrowserCapability,
    "browser",
    terrane_cap_browser::BrowserState
);
selected!("cap-build", terrane_cap_build::BuildCapability);
selected!(
    "cap-builder",
    terrane_cap_builder::BuilderCapability,
    "builder",
    terrane_cap_builder::BuilderState
);
selected!(
    "cap-common",
    terrane_cap_common::CommonCapability,
    "common",
    terrane_cap_common::CommonState
);
selected!(
    "cap-control-room",
    terrane_cap_control_room::ControlRoomCapability
);
selected!(
    "cap-crdt",
    terrane_cap_crdt::CrdtCapability,
    "crdt",
    terrane_cap_crdt::CrdtState
);
selected!("cap-crypto", terrane_cap_crypto::CryptoCapability);
selected!(
    "cap-document",
    terrane_cap_document::DocumentCapability,
    "document",
    terrane_cap_document::DocumentState
);
selected!(
    "cap-geo",
    terrane_cap_geo::GeoCapability,
    "geo",
    terrane_cap_geo::GeoState
);
selected!(
    "cap-harness",
    terrane_cap_harness::HarnessCapability,
    "harness",
    terrane_cap_harness::HarnessState
);
selected!(
    "cap-history",
    terrane_cap_history::HistoryCapability,
    "history",
    terrane_cap_history::HistoryState
);
selected!(
    "cap-interop",
    terrane_cap_interop::InteropCapability,
    "interop",
    terrane_cap_interop::InteropState
);
selected!(
    "cap-job",
    terrane_cap_job_queue::JobQueueCapability,
    "job",
    terrane_cap_job_queue::JobState,
    job_background
);
selected!(
    "cap-js-runtime",
    terrane_cap_js_runtime::JsRuntimeCapability
);
selected!(
    "cap-local-model",
    terrane_cap_local_model::LocalModelCapability,
    "local-model",
    terrane_cap_local_model::LocalModelState
);
selected!(
    "cap-mcp",
    terrane_cap_mcp_client::McpClientCapability,
    "mcp",
    terrane_cap_mcp_client::McpClientState
);
selected!(
    "cap-media",
    terrane_cap_media::MediaCapability,
    "media",
    terrane_cap_media::MediaState
);
selected!(
    "cap-migration",
    terrane_cap_migration::MigrationCapability,
    "migration",
    terrane_cap_migration::MigrationState
);
selected!(
    "cap-model",
    terrane_cap_model::ModelCapability,
    "model",
    terrane_cap_model::ModelState
);
selected!(
    "cap-native",
    terrane_cap_native::NativeCapability,
    "native",
    terrane_cap_native::NativeState
);
selected!(
    "cap-net",
    terrane_cap_net::NetCapability,
    "net",
    terrane_cap_net::NetState
);
selected!(
    "cap-org",
    terrane_cap_org::OrgCapability,
    "org",
    terrane_cap_org::OrgState
);
selected!(
    "cap-presence",
    terrane_cap_presence::PresenceCapability,
    "presence",
    terrane_cap_presence::PresenceState
);
selected!(
    "cap-publish",
    terrane_cap_publish::PublishCapability,
    "publish",
    terrane_cap_publish::PublishState
);
selected!(
    "cap-push",
    terrane_cap_push::PushCapability,
    "push",
    terrane_cap_push::PushState
);
selected!(
    "cap-query",
    terrane_cap_query::QueryCapability,
    "query",
    terrane_cap_query::QueryState
);
selected!(
    "cap-relational-db",
    terrane_cap_relational_db::RelationalDbCapability
);
selected!(
    "cap-scheduler",
    terrane_cap_scheduler::SchedulerCapability,
    "scheduler",
    terrane_cap_scheduler::SchedulerState,
    scheduler_background
);
selected!("cap-search", terrane_cap_search::SearchCapability);
selected!(
    "cap-share",
    terrane_cap_share::ShareCapability,
    "share",
    terrane_cap_share::ShareState
);
selected!(
    "cap-stream",
    terrane_cap_stream::StreamCapability,
    "stream",
    terrane_cap_stream::StreamState,
    stream_background
);
selected!(
    "cap-stt",
    terrane_cap_stt::SttCapability,
    "stt",
    terrane_cap_stt::SttState
);
selected!(
    "cap-sync",
    terrane_cap_sync::SyncCapability,
    "sync",
    terrane_cap_sync::SyncState
);
selected!("cap-sysinfo", terrane_cap_sysinfo::SysinfoCapability);
#[cfg(feature = "cap-time")]
pub fn build() -> SelectedCapability {
    SelectedCapability {
        capability: Box::new(terrane_cap_time::TimeCapability),
        state: Some(("time", Box::new(terrane_cap_time::TimeState::default()))),
        background_work: no_background_work,
        headless_effect: time_headless,
    }
}
selected!(
    "cap-tts",
    terrane_cap_tts::TtsCapability,
    "tts",
    terrane_cap_tts::TtsState
);
selected!(
    "cap-wasm-runtime",
    terrane_cap_wasm_runtime::WasmRuntimeCapability
);
selected!(
    "cap-web-publish",
    terrane_cap_web_publish::WebPublishCapability,
    "web-publish",
    terrane_cap_web_publish::WebPublishState
);
#[cfg(feature = "cap-webhook")]
pub fn build() -> SelectedCapability {
    SelectedCapability {
        capability: Box::new(terrane_cap_webhook::WebhookCapability),
        state: Some((
            "webhook",
            Box::new(terrane_cap_webhook::WebhookState::default()),
        )),
        background_work: webhook_background,
        headless_effect: webhook_headless,
    }
}

#[cfg(not(any(
    feature = "cap-agent",
    feature = "cap-applescript",
    feature = "cap-automation",
    feature = "cap-browser",
    feature = "cap-build",
    feature = "cap-builder",
    feature = "cap-common",
    feature = "cap-control-room",
    feature = "cap-crdt",
    feature = "cap-crypto",
    feature = "cap-document",
    feature = "cap-geo",
    feature = "cap-harness",
    feature = "cap-history",
    feature = "cap-interop",
    feature = "cap-job",
    feature = "cap-js-runtime",
    feature = "cap-local-model",
    feature = "cap-mcp",
    feature = "cap-media",
    feature = "cap-migration",
    feature = "cap-model",
    feature = "cap-native",
    feature = "cap-net",
    feature = "cap-org",
    feature = "cap-presence",
    feature = "cap-publish",
    feature = "cap-push",
    feature = "cap-query",
    feature = "cap-relational-db",
    feature = "cap-scheduler",
    feature = "cap-search",
    feature = "cap-share",
    feature = "cap-stream",
    feature = "cap-stt",
    feature = "cap-sync",
    feature = "cap-sysinfo",
    feature = "cap-time",
    feature = "cap-tts",
    feature = "cap-wasm-runtime",
    feature = "cap-web-publish",
    feature = "cap-webhook"
)))]
pub fn build() -> SelectedCapability {
    panic!("terrane-cap-worker was built without a capability feature")
}
