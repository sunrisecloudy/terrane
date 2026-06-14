//! Export format versions + the run-log inclusion policy / [`ExportOptions`].

/// The open-format version written to / required from a bundle's `meta` table
/// (`export_format_version`). Bumped only on an incompatible bundle-layout
/// change; an importer refuses a version it does not understand rather than
/// silently reinterpreting unknown data (spec §Versioning).
pub const EXPORT_FORMAT_VERSION: i64 = 1;

/// The physical storage schema version recorded alongside the open-format
/// version (`forge_storage_schema_version`). Lets a future importer migrate an
/// older physical layout explicitly.
pub const STORAGE_SCHEMA_VERSION: i64 = 1;

/// Policy for whether run records + logs travel in the bundle (spec: run logs
/// are policy-dependent and default-excluded for privacy; include them only for
/// an explicit debug/backup bundle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunLogPolicy {
    /// Default: omit `runs` and `run_logs` from the bundle (privacy).
    Exclude,
    /// Debug/backup bundle: include `runs` and `run_logs` (ordered).
    Include,
}

/// Options controlling what an export contains.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// The workspace identifier stamped into the bundle's `meta` (so an import
    /// can carry the source identity). Empty is allowed (anonymous bundle).
    pub workspace_id: String,
    /// Whether run records + logs are included (default: excluded).
    pub run_logs: RunLogPolicy,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            workspace_id: String::new(),
            run_logs: RunLogPolicy::Exclude,
        }
    }
}

impl ExportOptions {
    /// A bundle stamped with `workspace_id`, run logs excluded (the default).
    pub fn new(workspace_id: impl Into<String>) -> Self {
        ExportOptions {
            workspace_id: workspace_id.into(),
            run_logs: RunLogPolicy::Exclude,
        }
    }

    /// Include run records + logs (debug/backup bundle).
    pub fn with_run_logs(mut self) -> Self {
        self.run_logs = RunLogPolicy::Include;
        self
    }
}
