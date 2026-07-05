use terrane_cap_interface::{Capability, EventRecord};
use terrane_cap_share::{invited_event, revoked_event, ShareCapability, ShareState};

#[derive(Default)]
struct Store {
    share: ShareState,
}

impl terrane_cap_interface::StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        match namespace {
            "share" => Some(&self.share),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn std::any::Any> {
        match namespace {
            "share" => Some(&mut self.share),
            _ => None,
        }
    }
}

#[test]
fn invite_describe_redacts_token_hash() {
    let cap = ShareCapability;
    let event = invited_event(
        "notes",
        "read",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "hi",
    )
    .unwrap();

    let described = cap.describe(&event).unwrap();
    assert_eq!(described, "share.invited notes read");
    assert!(!described.contains("012345"));
}

#[test]
fn app_removed_clears_share_state() {
    let cap = ShareCapability;
    let mut store = Store::default();
    cap.fold(
        &mut store,
        &invited_event(
            "notes",
            "write",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "",
        )
        .unwrap(),
    )
    .unwrap();
    cap.fold(
        &mut store,
        &terrane_cap_share::redeemed_event(
            "notes",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "replica:abc",
            "write",
        )
        .unwrap(),
    )
    .unwrap();
    cap.fold(
        &mut store,
        &EventRecord {
            kind: "app.removed".to_string(),
            payload: borsh::to_vec(&AppRemoved { id: "notes".to_string() }).unwrap(),
            actor: String::new(),
        },
    )
    .unwrap();

    assert!(store.share.invites.is_empty());
    assert!(store.share.shares.is_empty());
}

#[test]
fn validation_rejects_bad_rights_and_grantee() {
    assert!(invited_event(
        "notes",
        "admin",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "",
    )
    .is_err());
    assert!(revoked_event("notes", "replica:not hex").is_err());
}

#[derive(borsh::BorshSerialize)]
struct AppRemoved {
    id: String,
}
