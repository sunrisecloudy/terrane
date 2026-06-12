//! Shared helpers for the forge-runtime integration suites.
//!
//! Each integration test binary compiles this whole module, so a helper used by
//! only one suite looks "dead" to the others — allow it crate-wide for tests.
#![allow(dead_code)]

use forge_domain::{ActorContext, Capabilities, DbGrant, Limits, Manifest, Role, StorageGrant};
use forge_runtime::Program;

/// A manifest granting `app/*` storage (read+write), the `tasks` collection
/// (read+write), and UI — the common spine-demo capability set.
pub fn spine_manifest() -> Manifest {
    Manifest {
        entrypoint: "main.ts".into(),
        min_api: "forge-api@0.1".into(),
        deterministic: true,
        capabilities: Capabilities {
            storage: StorageGrant {
                read: vec!["app/*".into()],
                write: vec!["app/*".into()],
            },
            db: DbGrant {
                read: vec!["tasks".into()],
                write: vec!["tasks".into()],
            },
            ui: true,
        },
        limits: Limits::default(),
    }
}

/// A manifest with *small* limits so containment/suspension is fast and CI never
/// hangs: a tight wall-clock + fuel budget, an 8 MiB memory ceiling, and a
/// modest host-call cap. Storage is granted broadly (`*`) so flood/seam tests
/// reach the limits, not a capability denial.
pub fn small_limits_manifest() -> Manifest {
    let mut m = spine_manifest();
    m.capabilities.storage = StorageGrant {
        read: vec!["*".into()],
        write: vec!["*".into()],
    };
    m.limits = Limits {
        wall_ms: 200,
        fuel: 2_000_000,
        memory_bytes: 8 * 1024 * 1024,
        max_host_calls: 1_000,
        storage_bytes: 1024 * 1024,
        log_bytes: 64 * 1024,
    };
    m
}

/// The owner actor (permits running in M0a).
pub fn owner() -> ActorContext {
    ActorContext::owner("dev")
}

/// A read-only actor (cannot run applets, prd-merged/07 SC-10).
pub fn viewer() -> ActorContext {
    ActorContext {
        actor: "v".into(),
        role: Role::Viewer,
    }
}

/// Build a `Program` for the test applet from JS source.
pub fn program(source: &str) -> Program {
    Program::new(forge_domain::AppletId::new("app_test"), source)
}
