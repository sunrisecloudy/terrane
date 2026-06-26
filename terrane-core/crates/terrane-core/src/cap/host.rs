//! The `host` capability — running a bundled JS backend in QuickJS.
//!
//! `host.run` is special-cased in [`Core::dispatch`](crate::Core::dispatch): it
//! needs `&mut self` to re-dispatch the backend's `kv.*` writes, which a pure
//! `decide` (`&State`) cannot do. This capability exists for the registry /
//! namespace contract and to reject any *direct* `host.*` command. It owns no
//! State slice and folds nothing — a run's only records are ordinary `kv.*`
//! events, so Option-A replay rebuilds state without ever re-running JS.

use terrane_domain::{Error, EventRecord, Result};

use super::Capability;
use crate::{Decision, State};

pub struct HostCapability;

impl Capability for HostCapability {
    fn namespace(&self) -> &'static str {
        "host"
    }

    fn decide(&self, _state: &State, name: &str, _args: &[String]) -> Result<Decision> {
        // host.run is executed by Core::dispatch directly and never reaches here.
        Err(Error::InvalidInput(format!(
            "{name}: host commands are executed by the core, not decided"
        )))
    }

    fn fold(&self, _state: &mut State, _record: &EventRecord) -> Result<()> {
        Ok(())
    }
}
