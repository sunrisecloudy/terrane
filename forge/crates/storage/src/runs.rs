//! Run record persistence (replay source, prd-merged/01 CR-9): the full
//! `RunRecord` JSON `runtime.replay` reads, provenance-gated on save and load.

use forge_domain::{Result, RunRecord};
use rusqlite::{params, OptionalExtension};

use crate::errors::{map_json, map_sql};
use crate::store::{now_ms, Store};

impl Store {
    // --- Runs (replay source, prd-merged/01 CR-9) ------------------------

    /// Persist a full `RunRecord` as JSON for `runtime.replay`. Re-saving the
    /// same `run_id` overwrites (idempotent record-and-replace).
    ///
    /// The record's `code_hash` is its provenance + replay key, so it is
    /// validated against the canonical `sha256:` form before it is allowed to
    /// land in the substrate (prd-merged/01 CR-9; review 013/014). A record
    /// carrying a divergent string (the runtime's old `fnv1a64:…`, an uppercase
    /// digest, a truncated body) is rejected with a `ValidationError` here,
    /// rather than persisting a row the pipeline could never reproduce.
    pub fn save_run(&self, run: &RunRecord) -> Result<()> {
        run.validate_code_hash()?;
        let json = serde_json::to_string(run).map_err(|e| map_json("save_run", e))?;
        self.conn
            .execute(
                "INSERT INTO runs (run_id, applet_id, record_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(run_id) DO UPDATE SET
                     applet_id = excluded.applet_id,
                     record_json = excluded.record_json,
                     created_at = excluded.created_at",
                params![run.run_id.as_str(), run.applet_id.as_str(), json, now_ms()],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// Persist a full `RunRecord` inside the CALLER's open transaction (the
    /// tx-scoped form of [`save_run`](Self::save_run)). Used so a `runtime.run` can
    /// commit the run record AND its `allow` SC-12 egress audit rows
    /// ([`append_audit_tx`](Self::append_audit_tx)) in ONE `Store::transact`: a real
    /// served egress (the durable effect) and its `network.egress`/`secret.use` rows
    /// then land — or roll back — together, so a crash between them can never leave a
    /// served egress durable without its audit trail (spec/audit-log.md §2). Same
    /// `code_hash` provenance re-validation + idempotent record-and-replace as the
    /// stand-alone form.
    pub fn save_run_tx(tx: &rusqlite::Transaction<'_>, run: &RunRecord) -> Result<()> {
        run.validate_code_hash()?;
        let json = serde_json::to_string(run).map_err(|e| map_json("save_run_tx", e))?;
        tx.execute(
            "INSERT INTO runs (run_id, applet_id, record_json, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(run_id) DO UPDATE SET
                 applet_id = excluded.applet_id,
                 record_json = excluded.record_json,
                 created_at = excluded.created_at",
            params![run.run_id.as_str(), run.applet_id.as_str(), json, now_ms()],
        )
        .map_err(map_sql)?;
        Ok(())
    }

    /// Load a `RunRecord` by id, reconstructed from its stored JSON.
    ///
    /// The provenance contract is re-checked on read: a corrupted or legacy row
    /// (e.g. a `fnv1a64:…` `code_hash` written before this guard existed, or a
    /// digest mangled in the file) surfaces a `ValidationError` instead of
    /// silently handing back a record the pipeline can never reproduce
    /// (prd-merged/01 CR-9; review 013/014).
    pub fn load_run(&self, run_id: &str) -> Result<Option<RunRecord>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT record_json FROM runs WHERE run_id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql)?;
        match json {
            Some(s) => {
                let run: RunRecord =
                    serde_json::from_str(&s).map_err(|e| map_json("load_run", e))?;
                run.validate_code_hash()?;
                Ok(Some(run))
            }
            None => Ok(None),
        }
    }
}
