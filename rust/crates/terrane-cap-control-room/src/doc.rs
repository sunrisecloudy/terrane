use terrane_cap_interface::{
    CapabilityDoc, CapabilityManifestDoc, ExampleDoc, InternalNote, ResourceDoc, ResourceMethodDoc,
};

pub fn control_room_doc(include_internal: bool) -> CapabilityDoc {
    let method = ResourceMethodDoc {
        name: "catalog".to_string(),
        kind: "read".to_string(),
        params: Vec::new(),
        returns: "JSON object containing redacted live app, grant, model, storage, MCP, and capability metadata".to_string(),
        summary: "Build a read-only Control Room snapshot from registered capability docs and safely aggregated folded state.".to_string(),
        errors: vec![
            "control-room not granted".to_string(),
            "state projection unavailable".to_string(),
        ],
    };
    CapabilityDoc {
        namespace: "control-room".to_string(),
        title: "Control Room catalog".to_string(),
        summary: "Read-only, redacted inventory of Terrane apps, capabilities, MCP surfaces, grants, runtimes, models, and safe data aggregates.".to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "user".to_string(),
            "app-author".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: Vec::new(),
            queries: Vec::new(),
            events: Vec::new(),
            subscriptions: Vec::new(),
            resource_methods: vec![method.clone()],
        },
        commands: Vec::new(),
        queries: Vec::new(),
        events: Vec::new(),
        resources: vec![ResourceDoc {
            namespace: "control-room".to_string(),
            summary: "Metadata-only management view. It never returns raw app records, prompts, responses, passwords, tokens, secret bytes, connection transport details, or selector JSON.".to_string(),
            methods: vec![method],
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Read the catalog from the built-in app".to_string(),
            summary: "The Control Room backend feature-detects its read-only resource and returns the JSON snapshot unchanged.".to_string(),
            language: "js".to_string(),
            code: "if (!ctx.resource['control-room']) return JSON.stringify({error:'not granted'});\nreturn ctx.resource['control-room'].catalog();".to_string(),
            expected: "a metadata-only catalog with facts labelled static or live".to_string(),
        }],
        constraints: vec![
            "Read-only: this capability declares no commands, writes, calls, or effects.".to_string(),
            "Secret-bearing fields and raw app content are excluded by construction.".to_string(),
            "Static contract documentation and live folded-state facts carry separate factKind labels.".to_string(),
            "An unavailable safe aggregate is reported as unavailable, never guessed from raw data.".to_string(),
        ],
        limits: Vec::new(),
        compatibility: vec![
            "Hosts without the optional live manifest/policy sampler still return the registered capability and folded-state catalog.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Trust boundary".to_string(),
                body: "The capability reads public registry documentation through CapBus and selected aggregate counts from folded state. Optional host sampling accepts only app sources already recorded by the app catalog and returns a sanitized manifest projection.".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}
