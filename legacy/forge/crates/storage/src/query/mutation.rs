//! The mutation plan (DL-17) carried in a fixture's `mutations[]` or a
//! `transact` group. The CRDT write path (`crdt_write`) consumes these.

use serde::Deserialize;

/// A mutation as carried in a fixture's `mutations[]` or a `transact` group.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Mutation {
    Insert {
        collection: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        fields: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        logical_at: Option<i64>,
    },
    Update {
        collection: String,
        id: String,
        #[serde(default)]
        fields: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        logical_at: Option<i64>,
    },
    Patch {
        collection: String,
        id: String,
        #[serde(default)]
        fields: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        logical_at: Option<i64>,
    },
    Delete {
        collection: String,
        id: String,
        #[serde(default)]
        logical_at: Option<i64>,
    },
    Transact {
        items: Vec<Mutation>,
    },
}
