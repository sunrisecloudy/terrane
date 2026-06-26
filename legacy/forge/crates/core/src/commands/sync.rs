//! Host-callable CRDT sync transport commands.
//!
//! Phase 1.2 of the legacy cutover folds the old native sync bridge into
//! the single `forge_core_handle_command` ABI. Hosts exchange `sync.export`
//! packets and apply them with `sync.import`; the import path authorizes each
//! chunk with the same SS-7 gate as in-process `WorkspaceCore::sync_with`.

use super::super::{authorize_incoming_op, WorkspaceCore};
use super::take_field;
use crate::TrustedMembership;
use forge_domain::{CoreCommand, CoreError, Result};
use std::collections::BTreeSet;

#[derive(serde::Deserialize)]
struct TrustPeerPayload {
    source: String,
    membership: TrustedMembership,
}

fn normalize_source(source: &str) -> Result<String> {
    let source = source.trim();
    if source.is_empty() {
        return Err(CoreError::ValidationError(
            "sync peer source must not be empty".into(),
        ));
    }
    Ok(source.to_string())
}

impl WorkspaceCore {
    /// `sync.trust_peer` — provision the receiver-side trusted SS-7 membership row
    /// for a remote packet source (`peer:<id>`). This is an owner-only shell
    /// command, not packet-provided authority: `sync.import` still fail-closes for
    /// packets whose source has no trusted row.
    pub(in crate::workspace) fn cmd_sync_trust_peer(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        let payload: TrustPeerPayload =
            serde_json::from_value(cmd.payload.clone()).map_err(|e| {
                CoreError::ValidationError(format!("sync.trust_peer payload is malformed: {e}"))
            })?;
        let source = normalize_source(&payload.source)?;
        self.set_peer_membership(source.clone(), payload.membership)?;
        Ok(serde_json::json!({ "source": source }))
    }

    /// `sync.export` — export this workspace's CRDT chunk set as a JSON packet.
    ///
    /// The response is wrapped as `{ packet }` so hosts can pass that object directly
    /// to a peer's `sync.import` command.
    pub(in crate::workspace) fn cmd_sync_export(
        &mut self,
        _cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        let packet = forge_sync::export_packet(self.store())?;
        Ok(serde_json::json!({ "packet": packet }))
    }

    /// `sync.import` — authorize and atomically apply a `sync.export` packet.
    pub(in crate::workspace) fn cmd_sync_import(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        let mut packet: forge_sync::SyncExportPacket = take_field(cmd, "packet")?;
        packet.source = normalize_source(&packet.source)?;
        let source = packet.source.clone();
        let chunks_seen = packet.chunks.len();
        let docs_synced = packet
            .chunks
            .iter()
            .map(|chunk| chunk.doc_id.clone())
            .collect::<BTreeSet<_>>()
            .len();
        let decoded = forge_sync::decode_packet(packet)?;

        let mut audit_rows = Vec::new();
        let mut allowed = Vec::new();
        let mut chunks_denied = 0usize;
        for (envelope, chunk) in decoded {
            let actor = envelope.origin_source.as_deref().unwrap_or(&source);
            if authorize_incoming_op(
                &self.sync_membership,
                self.run_policy.as_ref(),
                &mut self.events,
                &mut audit_rows,
                actor,
                &envelope,
            ) {
                allowed.push(chunk);
            } else {
                chunks_denied += 1;
            }
        }

        let chunks_imported = self.store.apply_remote_chunks_with_audit(
            &allowed,
            &source,
            &self.indexes,
            &audit_rows,
        )?;
        self.refresh_schema_from_store()?;

        Ok(serde_json::json!({
            "source": source,
            "chunks_seen": chunks_seen,
            "chunks_imported": chunks_imported,
            "chunks_denied": chunks_denied,
            "docs_synced": docs_synced,
        }))
    }
}
