use terrane_cap_interface::{
    param, CapabilityDoc, CapabilityManifestDoc, ExampleDoc, InternalNote, ParamDoc, ResourceDoc,
    ResourceMethodDoc, SchemaDoc,
};

/// A `read` method on `ctx.resource.sysinfo`. `returns` and `summary` are
/// required by the doc-completeness tests, so every method states both.
fn read_method(name: &str, params: &[ParamDoc], returns: &str, summary: &str) -> ResourceMethodDoc {
    ResourceMethodDoc {
        name: name.to_string(),
        kind: "read".to_string(),
        params: params.to_vec(),
        returns: returns.to_string(),
        summary: summary.to_string(),
        errors: vec![
            "sysinfo not granted".to_string(),
            "no live host (pure core)".to_string(),
            "unknown resource read".to_string(),
        ],
    }
}

pub fn sysinfo_doc(include_internal: bool) -> CapabilityDoc {
    let methods = vec![
        read_method(
            "snapshot",
            &[],
            "JSON object with cpu, memory, disk, network, battery, system, and processes sections",
            "Sample every section at once — the one call a live dashboard polls.",
        ),
        read_method(
            "cpu",
            &[],
            "JSON with overall usage %, per-core usage, core count, and load average",
            "Sample CPU usage (overall and per-core) plus load average.",
        ),
        read_method(
            "memory",
            &[],
            "JSON with total/used/free/available bytes, usage %, and swap totals",
            "Sample RAM and swap usage.",
        ),
        read_method(
            "disk",
            &[],
            "JSON array of volumes with name, mount point, total/used/free bytes, and usage %",
            "Sample per-volume disk capacity.",
        ),
        read_method(
            "network",
            &[],
            "JSON with per-interface and total download/upload rates (bytes/s) and cumulative totals",
            "Sample network throughput since the previous read plus session totals.",
        ),
        read_method(
            "battery",
            &[],
            "JSON with present flag, charge %, charging state, and time remaining (best-effort; empty on desktops)",
            "Sample battery/power state where the host exposes it.",
        ),
        read_method(
            "system",
            &[],
            "JSON with host name, OS name/version, kernel, architecture, CPU brand, and uptime seconds",
            "Read static host and OS identity plus uptime.",
        ),
        read_method(
            "processes",
            &[
                param(
                    "sortBy",
                    "Ranking key: \"cpu\" (default) or \"memory\".",
                    "string",
                ),
                param("limit", "Maximum rows to return (default 8).", "u32"),
            ],
            "JSON array of top processes with pid, name, cpu %, and memory bytes",
            "List the top processes by CPU or memory.",
        ),
    ];

    CapabilityDoc {
        namespace: "sysinfo".to_string(),
        title: "Live system metrics".to_string(),
        summary: "Live host metrics — CPU, memory, disk, network, battery, and top processes — \
                  for monitoring apps. Reads sample the edge and record nothing."
            .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: Vec::new(),
            queries: Vec::new(),
            events: Vec::new(),
            subscriptions: Vec::new(),
            resource_methods: methods.clone(),
        },
        commands: Vec::new(),
        queries: Vec::new(),
        events: Vec::new(),
        resources: vec![ResourceDoc {
            namespace: "sysinfo".to_string(),
            summary: "Live, non-recorded reads of host system metrics. Each call samples the \
                      current state; polling drives a live view without touching the event log."
                .to_string(),
            methods,
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Poll a full snapshot from an app backend".to_string(),
            summary: "Feature-detect the resource, then read every section in one call for a \
                      dashboard tick."
                .to_string(),
            language: "js".to_string(),
            code: "var sys = ctx.resource.sysinfo;\nif (!sys) return \"sysinfo not granted\";\n\
                   return sys.snapshot();"
                .to_string(),
            expected: "a JSON string with cpu/memory/disk/network/battery/system/processes"
                .to_string(),
        }],
        constraints: vec![
            "Reads are live samples of the host, not folded state, and record no events — replay \
             is unaffected by how often an app polls."
                .to_string(),
            "Sampling happens only at the edge (the host's effect runner); a pure core with no \
             live host returns an error."
                .to_string(),
            "Rate-based fields (network throughput) are measured against the previous read, so \
             the first sample after start reports zero."
                .to_string(),
            "Battery and some fields are best-effort and platform-dependent; absent data is \
             reported as empty or a present:false flag rather than an error."
                .to_string(),
        ],
        limits: Vec::new(),
        compatibility: vec![
            "Metric availability depends on the host platform; the deterministic core stays pure \
             and never links a metrics backend."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Live-read boundary".to_string(),
                body: "Reads route through ResourceReadCtx::host (LiveHost), supplied by the edge \
                       runner via EffectRunner::live(). Nothing is recorded, so this is neither an \
                       Effect nor a Commit — it is the read counterpart that observes the outside \
                       world without entering the log."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}
