//! Shared helpers for the forge-runtime integration suites.
//!
//! Each integration test binary compiles this whole module, so a helper used by
//! only one suite looks "dead" to the others — allow it crate-wide for tests.
#![allow(dead_code)]

use forge_domain::{ActorContext, Capabilities, DbGrant, Limits, Manifest, Role, StorageGrant};
use forge_runtime::Program;

/// A manifest granting `app/*` storage (read+write), the `tasks` collection
/// (read+write), and UI — the common spine-demo capability set.
///
/// The wall-clock budget is set generously (30s) rather than the production
/// default: these tests run trivial *completing* programs and assert success /
/// divergence, never a wall-clock suspension, so the wall clock is purely a
/// hang-backstop here. A generous budget keeps record+replay (two QuickJS realm
/// builds per test) green on noisy CI runners where a cold/contended build could
/// otherwise trip a tight wall budget mid-load and flip a completing run into a
/// spurious `ResourceLimitExceeded`.
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
        limits: Limits {
            wall_ms: 30_000,
            ..Limits::default()
        },
    }
}

/// A manifest with *small* limits whose **deterministic limiter is the relevant
/// count/byte budget** (`max_host_calls`, `storage_bytes`, `log_bytes`), not the
/// wall clock. The wall-clock budget is a *generous backstop* (30s) against a
/// true hang — it must never be the limiter that wins a race.
///
/// This is the manifest for the *assertion-sensitive* budget tests: they assert
/// an exact recorded-call count, an exact error substring, or a precise
/// completed/failed shape, all of which the count/byte budgets produce
/// deterministically. A tight `wall_ms` (the old value) used to trip *before*
/// the count/byte budget under CPU contention on noisy CI runners, flipping the
/// error variant/message, dropping a recorded call, or leaving an interrupted
/// state that never reached the asserted shape — the flake this fixes. The
/// non-looping containment probes (no-ambient-globals, eval/Function poisoning)
/// also use it so a contended realm build can't trip the wall mid-load.
///
/// CPU-exhaustion corpus cases instead use [`cpu_tight_manifest`], where the
/// wall clock *is* the intended fast limiter (any budget tripping yields
/// `ResourceLimitExceeded`, which is all those cases assert).
///
/// Storage is granted broadly under the applet-scoped `app/*` prefix so
/// flood/seam tests reach the limits, not a capability denial. (A bare `*`
/// grant is rejected at policy-build time as overly broad — forge-policy review
/// 006 P2 — so these fixtures use a scoped wildcard and `app/`-prefixed keys.)
pub fn small_limits_manifest() -> Manifest {
    let mut m = spine_manifest();
    m.capabilities.storage = StorageGrant {
        read: vec!["app/*".into()],
        write: vec!["app/*".into()],
    };
    m.limits = Limits {
        // Generous backstop: the count/byte caps are the deterministic limiters;
        // the wall clock only guards against an actual hang (never wins a race).
        wall_ms: 30_000,
        fuel: 2_000_000,
        memory_bytes: 8 * 1024 * 1024,
        max_host_calls: 1_000,
        storage_bytes: 1024 * 1024,
        log_bytes: 64 * 1024,
    };
    m
}

/// A manifest for the **CPU-exhaustion containment corpus** cases (hot loops,
/// catastrophic regex). Here the wall clock is intentionally the *fast* limiter:
/// QuickJS invokes the interrupt handler roughly every fixed number of bytecode
/// ops, but the wall time per interval depends on how expensive those ops are
/// (an empty `while(true){}` burns the fuel-tick budget in ~1s, while a loop
/// body doing arithmetic can take tens of seconds to reach the same tick count),
/// so a fuel-tick budget is *not* a reliable wall bound for these. A tight
/// `wall_ms` contains every CPU case in a fraction of a second. The corpus test
/// only asserts the outcome is `ResourceLimitExceeded` (and that it was fast),
/// never *which* budget tripped, so the wall clock winning the race is correct
/// here — unlike the assertion-sensitive tests above.
pub fn cpu_tight_manifest() -> Manifest {
    let mut m = small_limits_manifest();
    m.limits.wall_ms = 500;
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
