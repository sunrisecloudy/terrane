//! The `js-runtime` capability — running a bundled JS backend in QuickJS.
//!
//! The engine stays deterministic by letting this capability execute JS exactly
//! once and by recording only the ordinary resource-write events produced during
//! the run. Replay folds those recorded events and never re-runs JavaScript.

use terrane_cap_interface::{
    arg, ensure_app_exists, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error,
    EventRecord, Result, RuntimeCtx, RuntimeOutput, RuntimeRequest, StateStore,
};

mod bundle;
mod sandbox;

pub use bundle::{read_manifest, BundleManifest, JsRuntimeBundle};
pub use sandbox::run_js_bundle;

pub struct JsRuntimeCapability;

impl Capability for JsRuntimeCapability {
    fn namespace(&self) -> &'static str {
        "js-runtime"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec {
                name: "js-runtime.run",
            }],
            events: Vec::new(),
            queries: Vec::new(),
            resources: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "js-runtime.run" => {
                let app = arg(args, 0, "app")?;
                ensure_app_exists(ctx.bus, &app)?;
                Ok(Decision::Runtime(RuntimeRequest {
                    app,
                    input: args.get(1..).unwrap_or_default().to_vec(),
                }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, _state: &mut dyn StateStore, _record: &EventRecord) -> Result<()> {
        Ok(())
    }

    fn run_runtime(&self, ctx: RuntimeCtx, request: RuntimeRequest) -> Result<RuntimeOutput> {
        let bundle = bundle::load_bundle(&ctx.source)?;
        let output = run_js_bundle(
            &request.app,
            &request.input,
            &JsRuntimeBundle {
                source: bundle.source,
                name: if bundle.name.is_empty() {
                    ctx.app_name
                } else {
                    bundle.name
                },
                resources: bundle.resources,
            },
            ctx.host,
        )?;
        Ok(RuntimeOutput { output })
    }
}
