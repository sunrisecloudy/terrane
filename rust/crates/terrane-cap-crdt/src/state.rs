use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use loro::LoroDoc;
use terrane_cap_interface::AppId;

/// This capability's slice of State: one Loro document per app. Reacts to
/// `app.removed` by dropping that app's document.
#[derive(Default)]
pub struct CrdtState {
    pub docs: BTreeMap<AppId, LoroDoc>,
}

// A `LoroDoc`'s derived `clone()` is a *reference* clone (it aliases the same
// underlying document), which would let a backend run mutate live State. State
// is cloned into every `js-runtime.run`, so CrdtState must deep-copy: `fork()`
// gives an independent document.
impl Clone for CrdtState {
    fn clone(&self) -> Self {
        CrdtState {
            docs: self
                .docs
                .iter()
                .map(|(app, doc)| (app.clone(), doc.fork()))
                .collect(),
        }
    }
}

// Replay-identity compares live State to State folded from the log. Two
// documents that imported the same updates converge to the same *value* (the
// CRDT guarantee), even if internal bytes/peer metadata differ.
impl PartialEq for CrdtState {
    fn eq(&self, other: &Self) -> bool {
        self.docs.len() == other.docs.len()
            && self.docs.iter().all(|(app, doc)| {
                other
                    .docs
                    .get(app)
                    .is_some_and(|o| doc.get_deep_value() == o.get_deep_value())
            })
    }
}

impl std::fmt::Debug for CrdtState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut m = f.debug_map();
        for (app, doc) in &self.docs {
            m.entry(app, &doc.get_deep_value());
        }
        m.finish()
    }
}

/// A recorded CRDT op: the binary Loro update for `app`'s document. Opaque on
/// purpose — the bytes are Loro's own update format, re-imported verbatim.
#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Update {
    pub app: String,
    pub bytes: Vec<u8>,
}
