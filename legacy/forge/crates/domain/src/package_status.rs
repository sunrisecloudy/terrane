//! Legacy webapp package registry statuses (Q8: `package.*` namespace).
//!
//! Distinct from v1 workspace `AppletLifecycle` — these values mirror the shell
//! `platform.sqlite` schema (`apps.status`, `app_versions.status`) and the
//! generated `forge/data/app-status-enums.json`.

use serde::{Deserialize, Serialize};
use std::fmt;

/// `apps.status` values for the legacy webapp registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageAppStatus {
    Enabled,
    Disabled,
    Quarantined,
}

impl PackageAppStatus {
    pub const ALL: [PackageAppStatus; 3] =
        [PackageAppStatus::Enabled, PackageAppStatus::Disabled, PackageAppStatus::Quarantined];

    pub fn as_str(self) -> &'static str {
        match self {
            PackageAppStatus::Enabled => "enabled",
            PackageAppStatus::Disabled => "disabled",
            PackageAppStatus::Quarantined => "quarantined",
        }
    }
}

impl fmt::Display for PackageAppStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `app_versions.status` values for the legacy webapp registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageVersionStatus {
    Enabled,
    Installed,
    RolledBack,
    Quarantined,
    Uninstalled,
}

impl PackageVersionStatus {
    pub const ALL: [PackageVersionStatus; 5] = [
        PackageVersionStatus::Enabled,
        PackageVersionStatus::Installed,
        PackageVersionStatus::RolledBack,
        PackageVersionStatus::Quarantined,
        PackageVersionStatus::Uninstalled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PackageVersionStatus::Enabled => "enabled",
            PackageVersionStatus::Installed => "installed",
            PackageVersionStatus::RolledBack => "rolled-back",
            PackageVersionStatus::Quarantined => "quarantined",
            PackageVersionStatus::Uninstalled => "uninstalled",
        }
    }
}

impl fmt::Display for PackageVersionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Trust level stored on `app_versions.trust_level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TrustLevel {
    #[default]
    Developer,
    Trusted,
    Untrusted,
}

impl TrustLevel {
    pub const ALL: [TrustLevel; 3] =
        [TrustLevel::Developer, TrustLevel::Trusted, TrustLevel::Untrusted];

    pub const DEFAULT: TrustLevel = TrustLevel::Developer;

    pub fn as_str(self) -> &'static str {
        match self {
            TrustLevel::Developer => "developer",
            TrustLevel::Trusted => "trusted",
            TrustLevel::Untrusted => "untrusted",
        }
    }
}

impl fmt::Display for TrustLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Allowed `runtime_snapshots.type` values (plus import-only extensions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SnapshotType {
    BugReport,
    PreInstall,
    PreMigration,
    PostTest,
    Golden,
    Manual,
    DebugBundle,
    /// Import-only: not accepted for `platform.create_snapshot`.
    Backup,
    /// Import-only: not accepted for `platform.create_snapshot`.
    TestFixture,
}

impl SnapshotType {
    pub const CREATABLE: [SnapshotType; 7] = [
        SnapshotType::BugReport,
        SnapshotType::PreInstall,
        SnapshotType::PreMigration,
        SnapshotType::PostTest,
        SnapshotType::Golden,
        SnapshotType::Manual,
        SnapshotType::DebugBundle,
    ];

    pub const IMPORT_ONLY: [SnapshotType; 2] =
        [SnapshotType::Backup, SnapshotType::TestFixture];

    pub fn as_str(self) -> &'static str {
        match self {
            SnapshotType::BugReport => "bug-report",
            SnapshotType::PreInstall => "pre-install",
            SnapshotType::PreMigration => "pre-migration",
            SnapshotType::PostTest => "post-test",
            SnapshotType::Golden => "golden",
            SnapshotType::Manual => "manual",
            SnapshotType::DebugBundle => "debug-bundle",
            SnapshotType::Backup => "backup",
            SnapshotType::TestFixture => "test-fixture",
        }
    }

    pub fn is_creatable(self) -> bool {
        Self::CREATABLE.contains(&self)
    }
}

impl fmt::Display for SnapshotType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod data_catalog_generator {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("forge workspace root")
    }

    fn write_if_changed(path: &Path, contents: &str) {
        let existing = fs::read_to_string(path).unwrap_or_default();
        if existing != contents {
            fs::write(path, contents).expect("write generated data file");
        }
    }

    fn generated_snapshot_types_json() -> String {
        let types: Vec<&str> = SnapshotType::CREATABLE.iter().map(|t| t.as_str()).collect();
        let import_only: Vec<&str> = SnapshotType::IMPORT_ONLY.iter().map(|t| t.as_str()).collect();
        serde_json::json!({
            "types": types,
            "importOnly": import_only,
        })
        .to_string()
            + "\n"
    }

    fn generated_app_status_enums_json() -> String {
        let app_status: Vec<&str> = PackageAppStatus::ALL.iter().map(|s| s.as_str()).collect();
        let version_status: Vec<&str> =
            PackageVersionStatus::ALL.iter().map(|s| s.as_str()).collect();
        serde_json::json!({
            "app_status": app_status,
            "version_status": version_status,
        })
        .to_string()
            + "\n"
    }

    fn generated_trust_levels_json() -> String {
        let levels: Vec<&str> = TrustLevel::ALL.iter().map(|l| l.as_str()).collect();
        serde_json::json!({
            "levels": levels,
            "default": TrustLevel::DEFAULT.as_str(),
        })
        .to_string()
            + "\n"
    }

    #[test]
    fn generated_data_files_match_domain_enums() {
        let data_dir = repo_root().join("data");
        write_if_changed(&data_dir.join("snapshot-types.json"), &generated_snapshot_types_json());
        write_if_changed(
            &data_dir.join("app-status-enums.json"),
            &generated_app_status_enums_json(),
        );
        write_if_changed(&data_dir.join("trust-levels.json"), &generated_trust_levels_json());

        assert_eq!(
            fs::read_to_string(data_dir.join("snapshot-types.json")).unwrap(),
            generated_snapshot_types_json()
        );
        assert_eq!(
            fs::read_to_string(data_dir.join("app-status-enums.json")).unwrap(),
            generated_app_status_enums_json()
        );
        assert_eq!(
            fs::read_to_string(data_dir.join("trust-levels.json")).unwrap(),
            generated_trust_levels_json()
        );
    }

    #[test]
    fn enum_json_roundtrips() {
        for status in PackageAppStatus::ALL {
            let token = status.as_str();
            let back: PackageAppStatus = serde_json::from_str(&format!("\"{token}\"")).unwrap();
            assert_eq!(back, status);
        }
        for status in PackageVersionStatus::ALL {
            let token = status.as_str();
            let back: PackageVersionStatus = serde_json::from_str(&format!("\"{token}\"")).unwrap();
            assert_eq!(back, status);
        }
        for level in TrustLevel::ALL {
            let token = level.as_str();
            let back: TrustLevel = serde_json::from_str(&format!("\"{token}\"")).unwrap();
            assert_eq!(back, level);
        }
        for snapshot in SnapshotType::CREATABLE {
            let token = snapshot.as_str();
            let back: SnapshotType = serde_json::from_str(&format!("\"{token}\"")).unwrap();
            assert_eq!(back, snapshot);
        }
    }
}