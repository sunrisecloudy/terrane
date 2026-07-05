use crate::abi::{Decision, Error, EventRecord, Result, RuntimeOutput, RuntimeRequest};
use crate::doc::CapabilityDoc;
use crate::manifest::{CapManifest, GrantResourceSpec, ResourceMethod};
use crate::runtime::{CommandCtx, QueryCtx, QueryValue, ReadValue, ResourceReadCtx, RuntimeCtx};
use crate::state::StateStore;

/// A per-backend-run cap on recorded `Decision::Effect` calls for one resource
/// method. Capabilities whose recorded effect should not run unbounded in a
/// single backend run (e.g. replayed wall-clock observations) declare one via
/// [`Capability::recorded_call_per_run_limit`]. The runtime host refuses the
/// call that would exceed `limit`, returning a typed [`Error`] whose message
/// names `escape_hint` (owned by the capability) so app authors know the
/// unrecorded fallback. The transient variant (and any non-recorded call) is
/// never gated. `None` (the default) means uncapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedCallCap {
    pub limit: usize,
    /// Hint appended to the rejection error (e.g. an unrecorded escape hatch).
    pub escape_hint: &'static str,
}

/// A self-contained slice of engine behaviour.
pub trait Capability {
    fn namespace(&self) -> &'static str;

    fn manifest(&self) -> CapManifest {
        CapManifest::empty()
    }

    fn doc(&self, include_internal: bool) -> CapabilityDoc;

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision>;

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()>;

    fn describe(&self, record: &EventRecord) -> Option<String> {
        let _ = record;
        None
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        let _ = ctx;
        let _ = args;
        Err(Error::InvalidInput(format!("unknown query: {name}")))
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        let _ = ctx;
        let _ = args;
        Err(Error::InvalidInput(format!(
            "unknown resource read: {}.{name}",
            self.namespace()
        )))
    }

    fn resource_api(&self) -> Vec<ResourceMethod> {
        self.manifest().resources
    }

    /// Translate the events one `ResourceMethod::Call` just recorded into the
    /// value handed back to the calling app backend.
    fn resource_call_output(
        &self,
        state: &dyn StateStore,
        app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        let _ = (state, app, records);
        Err(Error::InvalidInput(format!(
            "{}.{method} is not a callable resource",
            self.namespace()
        )))
    }

    fn grant_resource_specs(&self) -> Vec<GrantResourceSpec> {
        self.manifest().grant_resources
    }

    fn run_runtime(&self, ctx: RuntimeCtx, request: RuntimeRequest) -> Result<RuntimeOutput> {
        let _ = ctx;
        let _ = request;
        Err(Error::InvalidInput(format!(
            "{} is not a runtime capability",
            self.namespace()
        )))
    }

    /// Per-backend-run cap, if any, on recorded `Decision::Effect` calls to the
    /// given resource method. `None` (the default) = uncapped. Enforced by the
    /// runtime host: a single backend run may make at most `limit` recorded
    /// calls; the (limit+1)-th is rejected with a typed error naming
    /// `escape_hint`. Transient (unrecorded) calls are never gated.
    fn recorded_call_per_run_limit(&self, _method: &str) -> Option<RecordedCallCap> {
        None
    }
}
