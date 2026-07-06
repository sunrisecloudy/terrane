use terrane_cap_interface::{state_ref, ReadValue, ResourceMethod, ResourceReadCtx, Result};

use crate::json_escape;
use crate::types::PushState;

pub(crate) fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Call {
            name: "subscribe",
            params: &["pattern", "template"],
        },
        ResourceMethod::Call {
            name: "unsubscribe",
            params: &["subId"],
        },
        ResourceMethod::Read {
            name: "list",
            params: &[],
        },
    ]
}

pub(crate) fn read(ctx: ResourceReadCtx<'_>, name: &str, _args: &[String]) -> Result<ReadValue> {
    match name {
        "list" => Ok(ReadValue::OptString(Some(list_json(ctx.state, ctx.app)?))),
        other => Err(terrane_cap_interface::Error::InvalidInput(format!(
            "unknown resource read: push.{other}"
        ))),
    }
}

pub fn list_json(state: &dyn terrane_cap_interface::StateStore, app: &str) -> Result<String> {
    let state = state_ref::<PushState>(state, "push")?;
    let mut out = String::from("[");
    let mut first = true;
    if let Some(subs) = state.subscriptions.get(app) {
        for sub in subs.values() {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"subId\":\"{}\",\"eventPattern\":\"{}\",\"template\":\"{}\"}}",
                json_escape(&sub.sub_id),
                json_escape(&sub.event_pattern),
                json_escape(&sub.template)
            ));
        }
    }
    out.push(']');
    Ok(out)
}
