//! Debug-gated `control.*` commands (forge-core-plan B6, Q1).
//!
//! Pure DevControlPlane algorithms live in `forge-controlcore`; these handlers
//! are thin JSON adapters over that crate. The commands are registered only when
//! the non-default `control` cargo feature is enabled (native debug shells and
//! the `control-invoke` helper). Release `forge-cli` builds omit the feature, so
//! `control.*` names are rejected as unknown commands.

use forge_controlcore::{
    backup_content_hash_from_payload, backup_validate_from_payload,
    compare_snapshots_from_payload, generate_token_from_payload,
    json_matches_subset_from_payload, package_hashes_from_payload,
    sign_payload_from_payload, validate_package_from_payload, verify_signature_from_payload,
};
use forge_domain::{CoreCommand, Result};

use super::WorkspaceCore;

impl WorkspaceCore {
    pub(super) fn cmd_control_compare_snapshot(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        control_gate(cmd)?;
        compare_snapshots_from_payload(&cmd.payload)
    }

    pub(super) fn cmd_control_json_matches_subset(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        control_gate(cmd)?;
        json_matches_subset_from_payload(&cmd.payload)
    }

    pub(super) fn cmd_control_package_validate(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        control_gate(cmd)?;
        validate_package_from_payload(&cmd.payload)
    }

    pub(super) fn cmd_control_package_hashes(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        control_gate(cmd)?;
        package_hashes_from_payload(&cmd.payload)
    }

    pub(super) fn cmd_control_backup_validate(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        control_gate(cmd)?;
        backup_validate_from_payload(&cmd.payload)
    }

    pub(super) fn cmd_control_backup_content_hash(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        control_gate(cmd)?;
        backup_content_hash_from_payload(&cmd.payload)
    }

    pub(super) fn cmd_control_generate_token(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        control_gate(cmd)?;
        generate_token_from_payload(&cmd.payload)
    }

    pub(super) fn cmd_control_sign_payload(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        control_gate(cmd)?;
        sign_payload_from_payload(&cmd.payload)
    }

    pub(super) fn cmd_control_verify_signature(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        control_gate(cmd)?;
        verify_signature_from_payload(&cmd.payload)
    }
}

fn control_gate(_cmd: &CoreCommand) -> Result<()> {
    #[cfg(not(feature = "control"))]
    {
        Err(forge_domain::CoreError::PlatformUnavailable(
            "control.* commands are disabled in this build (enable the `control` feature)".into(),
        ))
    }
    #[cfg(feature = "control")]
    {
        Ok(())
    }
}