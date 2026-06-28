use std::any::Any;

use terrane_cap_interface::Error;

use super::*;

#[derive(Default)]
struct Store {
    builder: BuilderState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "builder" => Some(&self.builder),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "builder" => Some(&mut self.builder),
            _ => None,
        }
    }
}

#[test]
fn requested_generated_and_failed_events_fold_lifecycle() {
    let cap = BuilderCapability;
    let mut store = Store::default();

    cap.fold(
        &mut store,
        &requested_event("draft-1", "demo", "Demo", "make app", "codex").unwrap(),
    )
    .unwrap();
    assert_eq!(store.builder.drafts["draft-1"].prompt, "make app");

    cap.fold(
        &mut store,
        &generated_event(
            "draft-1",
            vec![BuilderFile {
                path: "manifest.json".into(),
                content: "{}".into(),
            }],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(store.builder.drafts["draft-1"].files.len(), 1);
    assert_eq!(store.builder.drafts["draft-1"].error, None);

    cap.fold(&mut store, &failed_event("draft-1", "bad output").unwrap())
        .unwrap();
    assert!(store.builder.drafts["draft-1"].files.is_empty());
    assert_eq!(
        store.builder.drafts["draft-1"].error,
        Some("bad output".into())
    );
}

#[test]
fn validate_id_accepts_safe_ids_only() {
    assert_eq!(validate_id(" app_1-2 ", "app id").unwrap(), "app_1-2");
    assert_eq!(
        validate_id("../escape", "app id").unwrap_err(),
        Error::InvalidInput(
            "app id is unsafe: \"../escape\"; use ASCII letters, digits, '-' or '_'".into()
        )
    );
}
