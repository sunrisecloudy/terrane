use crate::abi::{Decision, Error, EventRecord, Result, RuntimeOutput, RuntimeRequest};
use crate::doc::CapabilityDoc;
use crate::manifest::{CapManifest, ResourceMethod};
use crate::runtime::{CommandCtx, QueryCtx, QueryValue, ReadValue, ResourceReadCtx, RuntimeCtx};
use crate::state::StateStore;

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

    fn run_runtime(&self, ctx: RuntimeCtx, request: RuntimeRequest) -> Result<RuntimeOutput> {
        let _ = ctx;
        let _ = request;
        Err(Error::InvalidInput(format!(
            "{} is not a runtime capability",
            self.namespace()
        )))
    }
}
