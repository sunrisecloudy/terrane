//! Typed ID newtypes. Stringly-typed under the hood (stable, sync-friendly,
//! human-inspectable in the SQLite file) but distinct types at the API so a
//! `RunId` can never be passed where an `AppletId` is expected.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! string_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                $name(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                $name(s.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                $name(s)
            }
        }
    };
}

string_id!(
    /// Unit of sync, membership, and export — also the single portable SQLite
    /// file (prd-merged DECISIONS E1).
    WorkspaceId
);
string_id!(
    /// A runnable applet or script inside a workspace.
    AppletId
);
string_id!(
    /// A deterministic execution record (prd-merged/01 CR-9).
    RunId
);
string_id!(
    /// An actor (device/user) making requests.
    ActorId
);
string_id!(
    /// Correlates a command to its response.
    RequestId
);
string_id!(
    /// A core-emitted event.
    EventId
);
string_id!(
    /// A logical collection (≈ table). prd-merged/02 §2.
    CollectionId
);
string_id!(
    /// A logical record within a collection.
    RecordId
);
string_id!(
    /// A CRDT document id (prd-merged/02 DL-2 granularity).
    DocId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_transparent_in_json() {
        let id = AppletId::new("app_notes");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"app_notes\"");
        let back: AppletId = serde_json::from_str("\"app_notes\"").unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn display_and_as_str_agree() {
        let id = RunId::new("run_1");
        assert_eq!(id.to_string(), "run_1");
        assert_eq!(id.as_str(), "run_1");
    }
}
