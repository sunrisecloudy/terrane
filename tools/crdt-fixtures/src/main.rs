use loro::{
    CommitOptions, Container, ExportMode, Frontiers, ImportBlobMetadata, ImportStatus, LoroDoc,
    LoroList, LoroMap, LoroMovableList, LoroText, ToJson, ValueOrContainer, VersionRange,
    VersionVector, ID,
};
use serde_json::{json, Map, Value};
use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

type GenResult<T> = Result<T, Box<dyn Error>>;

const APP_ID: &str = "notebook-app";
const LORO_CHECKOUT: &str = "external-lib/loro";
const LORO_PINNED_COMMIT: &str = "ab91df67e322d01f75621742ff83d0fb4a000e79";
const NOTEBOOK_SCHEMA: &str = "notebook-crdt-v0";
const FIXTURE_SCHEMA: &str = "terrane-notebook-crdt-fixture-v0.1";

#[derive(Clone)]
struct Actor {
    id: &'static str,
    kind: &'static str,
    peer: u64,
    permissions: &'static [&'static str],
}

#[derive(Clone)]
struct LoroBlob {
    id: String,
    kind: &'static str,
    actor_id: String,
    bytes: Vec<u8>,
    start_vv: VersionVector,
    end_vv: VersionVector,
    json_updates: Option<Value>,
}

struct CommitOutcome {
    op_id: String,
    blob: LoroBlob,
    frontier: Frontiers,
}

fn main() -> GenResult<()> {
    let mut out_dir = PathBuf::from("tests/fixtures/crdt");
    let mut check = false;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                let value = args
                    .next()
                    .ok_or_else(|| boxed_error("--out requires a path"))?;
                out_dir = PathBuf::from(value);
            }
            "--check" => check = true,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => return Err(boxed_error(format!("unknown argument: {other}"))),
        }
    }

    let fixtures = vec![
        scenario_human_human()?,
        scenario_human_ai_proposal()?,
        scenario_offline_out_of_order()?,
        scenario_duplicate_op()?,
        scenario_permission_denied()?,
    ];

    if !check {
        fs::create_dir_all(&out_dir)?;
    }

    let mut changed = Vec::new();
    for (file_name, value) in fixtures {
        let path = out_dir.join(file_name);
        let rendered = render_json(&value)?;
        if check {
            let current = fs::read_to_string(&path)?;
            if current != rendered {
                changed.push(path);
            }
        } else {
            fs::write(path, rendered)?;
        }
    }

    if check && !changed.is_empty() {
        let joined = changed
            .into_iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(boxed_error(format!("fixtures are stale: {joined}")));
    }

    Ok(())
}

fn print_help() {
    println!("Usage: cargo run --manifest-path tools/crdt-fixtures/Cargo.toml -- [--out tests/fixtures/crdt] [--check]");
}

fn scenario_human_human() -> GenResult<(&'static str, Value)> {
    let seed = actor(
        "actor_seed",
        "system",
        100,
        &["notebook.read", "notebook.write"],
    );
    let alice = actor(
        "actor_alice",
        "human",
        201,
        &["notebook.read", "notebook.write", "notebook.sync"],
    );
    let bob = actor(
        "actor_bob",
        "human",
        202,
        &["notebook.read", "notebook.write", "notebook.sync"],
    );
    let notebook_id = "notebook_fixture_human_human";

    let base = LoroDoc::new();
    configure_doc(&base, seed.peer)?;
    let seed_commit = commit_op(&base, &seed, "op_hh_seed", 10, || {
        init_notebook(&base, "Human Human Fixture")?;
        add_cell_at(&base, 0, "cell_intro", "markdown", "Meeting notes", seed.id)?;
        add_cell_at(
            &base,
            1,
            "cell_calc",
            "code",
            "print(\"baseline\")",
            seed.id,
        )?;
        Ok(())
    })?;
    let base_snapshot = snapshot_blob("hh-base-snapshot", &seed, &base)?;
    let base_vv = base.oplog_vv();

    let alice_doc = doc_from_snapshot(&base_snapshot.bytes, alice.peer)?;
    let alice_commit = commit_op(&alice_doc, &alice, "op_hh_alice_edit", 20, || {
        let intro = text_at(&alice_doc, "notebook/cells/0/source")?;
        intro.insert(intro.len_unicode(), "\nAlice adds action items.")?;
        let cell = map_at(&alice_doc, "notebook/cells/0")?;
        cell.insert("updatedBy", alice.id)?;
        let metadata = map_at(&alice_doc, "notebook/cells/0/metadata")?;
        metadata.insert("status", "draft")?;
        add_comment(
            &alice_doc,
            "comment_hh_1",
            "cell_intro",
            alice.id,
            "Check owner before publishing.",
        )?;
        Ok(())
    })?;

    let bob_doc = doc_from_snapshot(&base_snapshot.bytes, bob.peer)?;
    let bob_commit = commit_op(&bob_doc, &bob, "op_hh_bob_edit_move", 30, || {
        let intro = text_at(&bob_doc, "notebook/cells/0/source")?;
        intro.insert(0, "Reviewed: ")?;
        let cell = map_at(&bob_doc, "notebook/cells/0")?;
        cell.insert("updatedBy", bob.id)?;
        let cells = movable_list_at(&bob_doc, "notebook/cells")?;
        cells.mov(1, 0)?;
        Ok(())
    })?;

    let merged_a = doc_from_snapshot(&base_snapshot.bytes, 901)?;
    let status_bob_first = merged_a.import(&bob_commit.blob.bytes)?;
    let status_alice_second = merged_a.import(&alice_commit.blob.bytes)?;

    let merged_b = doc_from_snapshot(&base_snapshot.bytes, 902)?;
    let status_alice_first = merged_b.import(&alice_commit.blob.bytes)?;
    let status_bob_second = merged_b.import(&bob_commit.blob.bytes)?;
    assert_same_materialized(&merged_a, &merged_b, "human-human replay orders diverged")?;

    let final_snapshot = snapshot_blob("hh-final-snapshot", &seed, &merged_a)?;
    let operations = vec![
        op_record(
            1,
            &seed_commit.op_id,
            "notebook.apply_local",
            &seed,
            &Frontiers::default(),
            json!({
                "type": "notebook.init",
                "title": "Human Human Fixture",
                "metadata": {
                    "createdBy": "fixture-generator",
                    "schemaVersion": NOTEBOOK_SCHEMA,
                    "title": "Human Human Fixture"
                },
                "cells": [
                    {
                        "id": "cell_intro",
                        "type": "markdown",
                        "source": "Meeting notes",
                        "metadata": {},
                        "outputs": []
                    },
                    {
                        "id": "cell_calc",
                        "type": "code",
                        "source": "print(\"baseline\")",
                        "metadata": {},
                        "outputs": []
                    }
                ]
            }),
            accepted_expect(),
        ),
        op_record(
            2,
            &alice_commit.op_id,
            "notebook.apply_local",
            &alice,
            &seed_commit.frontier,
            json!({
                "type": "batch",
                "ops": [
                    {
                        "type": "text.insert",
                        "cellId": "cell_intro",
                        "path": "source",
                        "index": "end",
                        "text": "\nAlice adds action items.",
                        "metadata": {"status": "draft"}
                    },
                    {
                        "type": "comment.add",
                        "commentId": "comment_hh_1",
                        "cellId": "cell_intro",
                        "body": "Check owner before publishing."
                    }
                ]
            }),
            accepted_expect(),
        ),
        op_record(
            3,
            &bob_commit.op_id,
            "notebook.apply_local",
            &bob,
            &seed_commit.frontier,
            json!({
                "type": "batch",
                "ops": [
                    {"type": "text.insert", "cellId": "cell_intro", "path": "source", "index": 0, "text": "Reviewed: ", "updatedBy": false},
                    {"type": "cell.move", "cellId": "cell_calc", "from": 1, "index": 0}
                ]
            }),
            accepted_expect(),
        ),
    ];

    let fixture = fixture(
        "human-human",
        "Concurrent human edits merge collaborative text, metadata, comments, and movable cell order.",
        notebook_id,
        &[seed.clone(), alice.clone(), bob.clone()],
        operations,
        vec![seed_commit.blob, alice_commit.blob, bob_commit.blob],
        vec![base_snapshot, final_snapshot],
        json!({
            "replayOrders": [
                {
                    "id": "bob-then-alice",
                    "imports": [
                        {"blobId": "op_hh_bob_edit_move-update", "status": import_status_json(&status_bob_first)},
                        {"blobId": "op_hh_alice_edit-update", "status": import_status_json(&status_alice_second)}
                    ]
                },
                {
                    "id": "alice-then-bob",
                    "imports": [
                        {"blobId": "op_hh_alice_edit-update", "status": import_status_json(&status_alice_first)},
                        {"blobId": "op_hh_bob_edit_move-update", "status": import_status_json(&status_bob_second)}
                    ]
                }
            ],
            "converged": true
        }),
        expected(&merged_a, vec![
            audit_accept(&seed_commit.op_id, seed.id),
            audit_accept(&alice_commit.op_id, alice.id),
            audit_accept(&bob_commit.op_id, bob.id),
        ])?,
    )?;

    assert_ne!(vv_json(&base_vv), vv_json(&merged_a.oplog_vv()));
    Ok(("human-human.json", fixture))
}

fn scenario_human_ai_proposal() -> GenResult<(&'static str, Value)> {
    let seed = actor(
        "actor_seed",
        "system",
        300,
        &["notebook.read", "notebook.write"],
    );
    let ai = actor(
        "actor_ai_assistant",
        "ai",
        401,
        &["notebook.read", "notebook.propose"],
    );
    let reviewer = actor(
        "actor_reviewer",
        "human",
        302,
        &["notebook.read", "notebook.write", "notebook.approve"],
    );
    let notebook_id = "notebook_fixture_human_ai_proposal";

    let doc = LoroDoc::new();
    configure_doc(&doc, seed.peer)?;
    let seed_commit = commit_op(&doc, &seed, "op_ai_seed", 10, || {
        init_notebook(&doc, "Human AI Proposal Fixture")?;
        add_cell_at(
            &doc,
            0,
            "cell_prompt",
            "prompt",
            "Summarize risks.",
            seed.id,
        )?;
        Ok(())
    })?;

    configure_doc(&doc, ai.peer)?;
    let proposal_base = doc.state_frontiers();
    let proposal_commit = commit_op(&doc, &ai, "op_ai_create_proposal", 20, || {
        create_ai_proposal(
            &doc,
            "proposal_ai_001",
            ai.id,
            "glm-4.5",
            "sha256:prompt-context-001",
            &proposal_base,
            "cell_prompt",
            "Summarize risks as three bullet points.",
        )
    })?;

    configure_doc(&doc, reviewer.peer)?;
    let accept_commit = commit_op(&doc, &reviewer, "op_ai_accept_proposal", 30, || {
        let source = text_at(&doc, "notebook/cells/0/source")?;
        source.delete(0, source.len_unicode())?;
        source.insert(0, "Summarize risks as three bullet points.")?;
        map_at(&doc, "notebook/cells/0")?.insert("updatedBy", reviewer.id)?;
        let proposal = map_at(&doc, "notebook/proposals/proposal_ai_001")?;
        proposal.insert("status", "accepted")?;
        proposal.insert("reviewedBy", reviewer.id)?;
        add_approval(
            &doc,
            "approval_ai_001",
            "proposal_ai_001",
            reviewer.id,
            "accepted",
        )?;
        Ok(())
    })?;

    let final_snapshot = snapshot_blob("ai-final-snapshot", &reviewer, &doc)?;
    let operations = vec![
        op_record(
            1,
            &seed_commit.op_id,
            "notebook.apply_local",
            &seed,
            &Frontiers::default(),
            json!({
                "type": "notebook.init",
                "metadata": {
                    "createdBy": "fixture-generator",
                    "schemaVersion": NOTEBOOK_SCHEMA,
                    "title": "Human AI Proposal Fixture"
                },
                "cells": [
                    {
                        "id": "cell_prompt",
                        "type": "prompt",
                        "source": "Summarize risks.",
                        "metadata": {},
                        "outputs": []
                    }
                ]
            }),
            accepted_expect(),
        ),
        op_record(
            2,
            &proposal_commit.op_id,
            "notebook.propose_ai_patch",
            &ai,
            &proposal_base,
            json!({
                "type": "proposal.create",
                "proposalId": "proposal_ai_001",
                "modelId": "glm-4.5",
                "promptHash": "sha256:prompt-context-001",
                "contextHash": "sha256:prompt-context-001",
                "promptContextHash": "sha256:prompt-context-001",
                "affectedCellIds": ["cell_prompt"],
                "proposedSource": "Summarize risks as three bullet points.",
                "patchSummary": "replace prompt source",
                "operations": [
                    {
                        "type": "text.replace",
                        "cellId": "cell_prompt",
                        "text": "Summarize risks as three bullet points."
                    }
                ]
            }),
            accepted_expect(),
        ),
        op_record(
            3,
            &accept_commit.op_id,
            "notebook.accept_proposal",
            &reviewer,
            &proposal_commit.frontier,
            json!({
                "type": "proposal.accept",
                "proposalId": "proposal_ai_001",
                "approvalId": "approval_ai_001"
            }),
            accepted_expect(),
        ),
    ];

    let fixture = fixture(
        "human-ai-proposal",
        "AI creates a proposal under proposal-only permissions; a human reviewer accepts it into canonical notebook state.",
        notebook_id,
        &[seed.clone(), ai.clone(), reviewer.clone()],
        operations,
        vec![seed_commit.blob, proposal_commit.blob, accept_commit.blob],
        vec![final_snapshot],
        json!({"proposalPolicy": "ai proposal-only actor cannot mutate canonical cells until a human accepts"}),
        expected(&doc, vec![
            audit_accept(&seed_commit.op_id, seed.id),
            audit_accept(&proposal_commit.op_id, ai.id),
            audit_accept(&accept_commit.op_id, reviewer.id),
        ])?,
    )?;

    Ok(("human-ai-proposal.json", fixture))
}

fn scenario_offline_out_of_order() -> GenResult<(&'static str, Value)> {
    let seed = actor(
        "actor_seed",
        "system",
        500,
        &["notebook.read", "notebook.write"],
    );
    let alice = actor(
        "actor_alice",
        "human",
        501,
        &["notebook.read", "notebook.write", "notebook.sync"],
    );
    let bob = actor(
        "actor_bob",
        "human",
        502,
        &["notebook.read", "notebook.write", "notebook.sync"],
    );
    let notebook_id = "notebook_fixture_offline_out_of_order";

    let base = LoroDoc::new();
    configure_doc(&base, seed.peer)?;
    let seed_commit = commit_op(&base, &seed, "op_offline_seed", 10, || {
        init_notebook(&base, "Offline Out Of Order Fixture")?;
        add_cell_at(&base, 0, "cell_code", "code", "print(\"start\")", seed.id)?;
        Ok(())
    })?;
    let base_snapshot = snapshot_blob("offline-base-snapshot", &seed, &base)?;

    let alice_doc = doc_from_snapshot(&base_snapshot.bytes, alice.peer)?;
    let alice_commit = commit_op(&alice_doc, &alice, "op_offline_alice_edit", 20, || {
        let source = text_at(&alice_doc, "notebook/cells/0/source")?;
        source.insert(source.len_unicode(), "\nprint(\"offline alice\")")?;
        map_at(&alice_doc, "notebook/cells/0")?.insert("updatedBy", alice.id)?;
        Ok(())
    })?;

    let bob_doc = doc_from_snapshot(&base_snapshot.bytes, bob.peer)?;
    let bob_commit = commit_op(&bob_doc, &bob, "op_offline_bob_output", 30, || {
        add_output(
            &bob_doc,
            "notebook/cells/0/outputs",
            "output_bob_001",
            "text/plain",
            "start\n",
            bob.id,
        )
    })?;

    let replay = LoroDoc::new();
    let early_status = replay.import(&bob_commit.blob.bytes)?;
    let base_status = replay.import(&base_snapshot.bytes)?;
    let alice_status = replay.import(&alice_commit.blob.bytes)?;

    let ordered = doc_from_snapshot(&base_snapshot.bytes, 903)?;
    let ordered_alice_status = ordered.import(&alice_commit.blob.bytes)?;
    let ordered_bob_status = ordered.import(&bob_commit.blob.bytes)?;
    assert_same_materialized(&replay, &ordered, "offline replay did not converge")?;

    let final_snapshot = snapshot_blob("offline-final-snapshot", &seed, &replay)?;
    let operations = vec![
        op_record(
            1,
            &seed_commit.op_id,
            "notebook.apply_local",
            &seed,
            &Frontiers::default(),
            json!({
                "type": "notebook.init",
                "metadata": {
                    "createdBy": "fixture-generator",
                    "schemaVersion": NOTEBOOK_SCHEMA,
                    "title": "Offline Out Of Order Fixture"
                },
                "cells": [
                    {
                        "id": "cell_code",
                        "type": "code",
                        "source": "print(\"start\")",
                        "metadata": {},
                        "outputs": []
                    }
                ]
            }),
            accepted_expect(),
        ),
        op_record(
            2,
            &alice_commit.op_id,
            "notebook.apply_local",
            &alice,
            &seed_commit.frontier,
            json!({
                "type": "text.insert",
                "cellId": "cell_code",
                "path": "source",
                "index": "end",
                "text": "\nprint(\"offline alice\")"
            }),
            accepted_expect(),
        ),
        op_record(
            3,
            &bob_commit.op_id,
            "notebook.apply_local",
            &bob,
            &seed_commit.frontier,
            json!({
                "type": "output.append",
                "cellId": "cell_code",
                "outputId": "output_bob_001",
                "mime": "text/plain",
                "output": {
                    "id": "output_bob_001",
                    "type": "stream",
                    "mime": "text/plain",
                    "text": "start\n",
                    "createdBy": "actor_bob"
                }
            }),
            accepted_expect(),
        ),
    ];

    let fixture = fixture(
        "offline-out-of-order",
        "Offline peers produce independent updates; an out-of-order update is pending until the base snapshot arrives, then all peers converge.",
        notebook_id,
        &[seed.clone(), alice.clone(), bob.clone()],
        operations,
        vec![seed_commit.blob, alice_commit.blob, bob_commit.blob],
        vec![base_snapshot, final_snapshot],
        json!({
            "outOfOrderReplay": [
                {"blobId": "op_offline_bob_output-update", "status": import_status_json(&early_status)},
                {"blobId": "offline-base-snapshot", "status": import_status_json(&base_status)},
                {"blobId": "op_offline_alice_edit-update", "status": import_status_json(&alice_status)}
            ],
            "inOrderReplay": [
                {"blobId": "op_offline_alice_edit-update", "status": import_status_json(&ordered_alice_status)},
                {"blobId": "op_offline_bob_output-update", "status": import_status_json(&ordered_bob_status)}
            ],
            "converged": true
        }),
        expected(&replay, vec![
            audit_accept(&seed_commit.op_id, seed.id),
            audit_accept(&alice_commit.op_id, alice.id),
            audit_accept(&bob_commit.op_id, bob.id),
        ])?,
    )?;

    Ok(("offline-out-of-order.json", fixture))
}

fn scenario_duplicate_op() -> GenResult<(&'static str, Value)> {
    let seed = actor(
        "actor_seed",
        "system",
        600,
        &["notebook.read", "notebook.write"],
    );
    let alice = actor(
        "actor_alice",
        "human",
        601,
        &["notebook.read", "notebook.write", "notebook.sync"],
    );
    let notebook_id = "notebook_fixture_duplicate_op";

    let base = LoroDoc::new();
    configure_doc(&base, seed.peer)?;
    let seed_commit = commit_op(&base, &seed, "op_dup_seed", 10, || {
        init_notebook(&base, "Duplicate Op Fixture")?;
        add_cell_at(
            &base,
            0,
            "cell_note",
            "markdown",
            "Duplicate-safe note.",
            seed.id,
        )?;
        Ok(())
    })?;
    let base_snapshot = snapshot_blob("duplicate-base-snapshot", &seed, &base)?;

    let alice_doc = doc_from_snapshot(&base_snapshot.bytes, alice.peer)?;
    let alice_commit = commit_op(&alice_doc, &alice, "op_dup_alice_edit", 20, || {
        let source = text_at(&alice_doc, "notebook/cells/0/source")?;
        source.insert(source.len_unicode(), " Applied once.")?;
        map_at(&alice_doc, "notebook/cells/0")?.insert("updatedBy", alice.id)?;
        Ok(())
    })?;

    let replay = doc_from_snapshot(&base_snapshot.bytes, 904)?;
    let first_status = replay.import(&alice_commit.blob.bytes)?;
    let after_first = materialized(&replay);
    let duplicate_status = replay.import(&alice_commit.blob.bytes)?;
    let after_duplicate = materialized(&replay);
    if after_first != after_duplicate {
        return Err(boxed_error("duplicate update changed materialized state"));
    }

    let final_snapshot = snapshot_blob("duplicate-final-snapshot", &seed, &replay)?;
    let operations = vec![
        op_record(
            1,
            &seed_commit.op_id,
            "notebook.apply_local",
            &seed,
            &Frontiers::default(),
            json!({
                "type": "notebook.init",
                "metadata": {
                    "createdBy": "fixture-generator",
                    "schemaVersion": NOTEBOOK_SCHEMA,
                    "title": "Duplicate Op Fixture"
                },
                "cells": [
                    {
                        "id": "cell_note",
                        "type": "markdown",
                        "source": "Duplicate-safe note.",
                        "metadata": {},
                        "outputs": []
                    }
                ]
            }),
            accepted_expect(),
        ),
        op_record(
            2,
            &alice_commit.op_id,
            "notebook.apply_local",
            &alice,
            &seed_commit.frontier,
            json!({
                "type": "text.insert",
                "cellId": "cell_note",
                "path": "source",
                "index": "end",
                "text": " Applied once."
            }),
            accepted_expect(),
        ),
    ];

    let fixture = fixture(
        "duplicate-op",
        "A duplicated sync update is imported twice and leaves the materialized notebook unchanged after the first import.",
        notebook_id,
        &[seed.clone(), alice.clone()],
        operations,
        vec![seed_commit.blob, alice_commit.blob],
        vec![base_snapshot, final_snapshot],
        json!({
            "duplicateReplay": [
                {"blobId": "op_dup_alice_edit-update", "status": import_status_json(&first_status)},
                {"blobId": "op_dup_alice_edit-update", "status": import_status_json(&duplicate_status)}
            ],
            "idempotent": true
        }),
        expected(&replay, vec![
            audit_accept(&seed_commit.op_id, seed.id),
            audit_accept(&alice_commit.op_id, alice.id),
        ])?,
    )?;

    Ok(("duplicate-op.json", fixture))
}

fn scenario_permission_denied() -> GenResult<(&'static str, Value)> {
    let seed = actor(
        "actor_seed",
        "system",
        700,
        &["notebook.read", "notebook.write"],
    );
    let ai = actor(
        "actor_ai_assistant",
        "ai",
        701,
        &["notebook.read", "notebook.propose"],
    );
    let notebook_id = "notebook_fixture_permission_denied";

    let doc = LoroDoc::new();
    configure_doc(&doc, seed.peer)?;
    let seed_commit = commit_op(&doc, &seed, "op_denied_seed", 10, || {
        init_notebook(&doc, "Permission Denied Fixture")?;
        add_cell_at(
            &doc,
            0,
            "cell_guarded",
            "markdown",
            "Canonical text must be human-approved.",
            seed.id,
        )?;
        Ok(())
    })?;
    let base_snapshot = snapshot_blob("denied-base-snapshot", &seed, &doc)?;
    let denied_frontier = doc.state_frontiers();

    let denied_op = op_record(
        2,
        "op_denied_ai_direct_write",
        "notebook.apply_local",
        &ai,
        &denied_frontier,
        json!({
            "type": "text.insert",
            "cellId": "cell_guarded",
            "path": "source",
            "index": 0,
            "text": "AI direct write. "
        }),
        json!({
            "ok": false,
            "error": {
                "code": "permission_denied",
                "message": "actor_ai_assistant has notebook.propose but not notebook.write; canonical notebook mutation rejected before merge"
            }
        }),
    );

    let operations = vec![
        op_record(
            1,
            &seed_commit.op_id,
            "notebook.apply_local",
            &seed,
            &Frontiers::default(),
            json!({
                "type": "notebook.init",
                "metadata": {
                    "createdBy": "fixture-generator",
                    "schemaVersion": NOTEBOOK_SCHEMA,
                    "title": "Permission Denied Fixture"
                },
                "cells": [
                    {
                        "id": "cell_guarded",
                        "type": "markdown",
                        "source": "Canonical text must be human-approved.",
                        "metadata": {},
                        "outputs": []
                    }
                ]
            }),
            accepted_expect(),
        ),
        denied_op,
    ];

    let fixture = fixture(
        "permission-denied",
        "An AI proposal-only actor attempts a canonical text edit; the host rejects it before applying any Loro update.",
        notebook_id,
        &[seed.clone(), ai.clone()],
        operations,
        vec![seed_commit.blob],
        vec![base_snapshot],
        json!({
            "rejectedBeforeMerge": true,
            "rejectedOperationIds": ["op_denied_ai_direct_write"]
        }),
        expected(&doc, vec![
            audit_accept(&seed_commit.op_id, seed.id),
            json!({
                "operationId": "op_denied_ai_direct_write",
                "actorId": ai.id,
                "status": "rejected",
                "errorCode": "permission_denied",
                "loroApplied": false
            }),
        ])?,
    )?;

    Ok(("permission-denied.json", fixture))
}

fn actor(
    id: &'static str,
    kind: &'static str,
    peer: u64,
    permissions: &'static [&'static str],
) -> Actor {
    Actor {
        id,
        kind,
        peer,
        permissions,
    }
}

fn configure_doc(doc: &LoroDoc, peer: u64) -> GenResult<()> {
    doc.set_peer_id(peer)?;
    doc.set_change_merge_interval(0);
    Ok(())
}

fn commit_op<F>(
    doc: &LoroDoc,
    actor: &Actor,
    op_id: &str,
    timestamp: i64,
    apply: F,
) -> GenResult<CommitOutcome>
where
    F: FnOnce() -> GenResult<()>,
{
    let start_vv = doc.oplog_vv();
    apply()?;
    doc.commit_with(
        CommitOptions::new()
            .commit_msg(op_id)
            .origin(actor.id)
            .timestamp(timestamp),
    );
    let end_vv = doc.oplog_vv();
    let update = doc.export(ExportMode::updates(&start_vv))?;
    let json_updates =
        serde_json::to_value(doc.export_json_updates_without_peer_compression(&start_vv, &end_vv))?;
    let frontier = doc.state_frontiers();

    Ok(CommitOutcome {
        op_id: op_id.to_string(),
        blob: LoroBlob {
            id: format!("{op_id}-update"),
            kind: "update",
            actor_id: actor.id.to_string(),
            bytes: update,
            start_vv,
            end_vv,
            json_updates: Some(json_updates),
        },
        frontier,
    })
}

fn snapshot_blob(id: &str, actor: &Actor, doc: &LoroDoc) -> GenResult<LoroBlob> {
    Ok(LoroBlob {
        id: id.to_string(),
        kind: "snapshot",
        actor_id: actor.id.to_string(),
        bytes: doc.export(ExportMode::Snapshot)?,
        start_vv: VersionVector::default(),
        end_vv: doc.oplog_vv(),
        json_updates: None,
    })
}

fn doc_from_snapshot(snapshot: &[u8], peer: u64) -> GenResult<LoroDoc> {
    let doc = LoroDoc::from_snapshot(snapshot)?;
    configure_doc(&doc, peer)?;
    Ok(doc)
}

fn init_notebook(doc: &LoroDoc, title: &str) -> GenResult<()> {
    let notebook = doc.get_map("notebook");
    let metadata = notebook.insert_container("metadata", LoroMap::new())?;
    metadata.insert("schemaVersion", NOTEBOOK_SCHEMA)?;
    metadata.insert("title", title)?;
    metadata.insert("createdBy", "fixture-generator")?;
    notebook.insert_container("cells", LoroMovableList::new())?;
    notebook.insert_container("comments", LoroMap::new())?;
    notebook.insert_container("aiRuns", LoroMap::new())?;
    notebook.insert_container("proposals", LoroMap::new())?;
    notebook.insert_container("approvals", LoroMap::new())?;
    Ok(())
}

fn add_cell_at(
    doc: &LoroDoc,
    index: usize,
    id: &str,
    cell_type: &str,
    source_text: &str,
    actor_id: &str,
) -> GenResult<()> {
    let cells = movable_list_at(doc, "notebook/cells")?;
    let cell = cells.insert_container(index, LoroMap::new())?;
    cell.insert("id", id)?;
    cell.insert("type", cell_type)?;
    cell.insert("createdBy", actor_id)?;
    cell.insert("updatedBy", actor_id)?;
    let source = cell.insert_container("source", LoroText::new())?;
    source.insert(0, source_text)?;
    cell.insert_container("metadata", LoroMap::new())?;
    cell.insert_container("outputs", LoroList::new())?;
    Ok(())
}

fn add_comment(
    doc: &LoroDoc,
    comment_id: &str,
    cell_id: &str,
    actor_id: &str,
    body: &str,
) -> GenResult<()> {
    let comments = map_at(doc, "notebook/comments")?;
    let comment = comments.insert_container(comment_id, LoroMap::new())?;
    comment.insert("id", comment_id)?;
    comment.insert("cellId", cell_id)?;
    comment.insert("createdBy", actor_id)?;
    comment.insert("status", "open")?;
    let text = comment.insert_container("body", LoroText::new())?;
    text.insert(0, body)?;
    Ok(())
}

fn add_output(
    doc: &LoroDoc,
    outputs_path: &str,
    output_id: &str,
    mime: &str,
    text: &str,
    actor_id: &str,
) -> GenResult<()> {
    let outputs = list_at(doc, outputs_path)?;
    let output = outputs.push_container(LoroMap::new())?;
    output.insert("id", output_id)?;
    output.insert("type", "stream")?;
    output.insert("mime", mime)?;
    output.insert("text", text)?;
    output.insert("createdBy", actor_id)?;
    Ok(())
}

fn create_ai_proposal(
    doc: &LoroDoc,
    proposal_id: &str,
    actor_id: &str,
    model_id: &str,
    prompt_context_hash: &str,
    base_frontier: &Frontiers,
    cell_id: &str,
    proposed_source: &str,
) -> GenResult<()> {
    let proposals = map_at(doc, "notebook/proposals")?;
    let proposal = proposals.insert_container(proposal_id, LoroMap::new())?;
    proposal.insert("id", proposal_id)?;
    proposal.insert("status", "pending")?;
    proposal.insert("createdBy", actor_id)?;
    proposal.insert("actorKind", "ai")?;
    proposal.insert("modelId", model_id)?;
    proposal.insert("promptContextHash", prompt_context_hash)?;
    proposal.insert("affectedCellIds", vec![cell_id.to_string()])?;
    proposal.insert("baseFrontier", frontier_ids(base_frontier))?;
    proposal.insert("proposedSource", proposed_source)?;
    proposal.insert("patchSummary", "replace prompt source")?;
    Ok(())
}

fn add_approval(
    doc: &LoroDoc,
    approval_id: &str,
    proposal_id: &str,
    actor_id: &str,
    decision: &str,
) -> GenResult<()> {
    let approvals = map_at(doc, "notebook/approvals")?;
    let approval = approvals.insert_container(approval_id, LoroMap::new())?;
    approval.insert("id", approval_id)?;
    approval.insert("proposalId", proposal_id)?;
    approval.insert("actorId", actor_id)?;
    approval.insert("decision", decision)?;
    Ok(())
}

fn fixture(
    scenario_id: &str,
    description: &str,
    notebook_id: &str,
    actors: &[Actor],
    mut operations: Vec<Value>,
    updates: Vec<LoroBlob>,
    snapshots: Vec<LoroBlob>,
    sync: Value,
    expected: Value,
) -> GenResult<Value> {
    for operation in &mut operations {
        let context = operation
            .get_mut("context")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| boxed_error("operation missing context object"))?;
        context.insert(
            "notebookId".to_string(),
            Value::String(notebook_id.to_string()),
        );
    }

    let update_values = updates
        .iter()
        .map(blob_json)
        .collect::<GenResult<Vec<_>>>()?;
    let snapshot_values = snapshots
        .iter()
        .map(blob_json)
        .collect::<GenResult<Vec<_>>>()?;

    Ok(json!({
        "schemaVersion": FIXTURE_SCHEMA,
        "generatedBy": {
            "command": "cargo run --manifest-path tools/crdt-fixtures/Cargo.toml",
            "tool": "tools/crdt-fixtures"
        },
        "reference": {
            "library": "Loro",
            "checkout": LORO_CHECKOUT,
            "pinnedCommit": LORO_PINNED_COMMIT,
            "crate": "loro",
            "crateVersion": "1.12.0"
        },
        "scenario": {
            "id": scenario_id,
            "description": description
        },
        "notebook": {
            "appId": APP_ID,
            "notebookId": notebook_id,
            "profile": NOTEBOOK_SCHEMA
        },
        "actors": actors.iter().map(actor_json).collect::<Vec<_>>(),
        "operations": operations,
        "loro": {
            "updates": update_values,
            "snapshots": snapshot_values
        },
        "sync": sync,
        "expected": expected
    }))
}

fn expected(doc: &LoroDoc, audit: Vec<Value>) -> GenResult<Value> {
    let snapshot = doc.export(ExportMode::Snapshot)?;
    let restored = LoroDoc::from_snapshot(&snapshot)?;
    assert_same_materialized(doc, &restored, "snapshot roundtrip changed state")?;

    Ok(json!({
        "materializedNotebook": materialized(doc),
        "loroDeepValue": doc.get_deep_value().to_json_value(),
        "versionVector": vv_json(&doc.oplog_vv()),
        "frontier": frontier_json(&doc.state_frontiers()),
        "snapshotRoundTrip": {
            "materializedMatches": true
        },
        "audit": audit
    }))
}

fn op_record(
    seq: usize,
    id: &str,
    method: &str,
    actor: &Actor,
    base_frontier: &Frontiers,
    operation: Value,
    expect: Value,
) -> Value {
    json!({
        "seq": seq,
        "id": id,
        "method": method,
        "context": {
            "appId": APP_ID,
            "notebookId": "derived-by-host",
            "actorId": actor.id,
            "actorKind": actor.kind,
            "permissions": actor.permissions,
            "baseFrontier": frontier_json(base_frontier)
        },
        "operation": operation,
        "expect": expect
    })
}

fn accepted_expect() -> Value {
    json!({"ok": true})
}

fn audit_accept(operation_id: &str, actor_id: &str) -> Value {
    json!({
        "operationId": operation_id,
        "actorId": actor_id,
        "status": "accepted",
        "loroApplied": true
    })
}

fn actor_json(actor: &Actor) -> Value {
    json!({
        "id": actor.id,
        "kind": actor.kind,
        "loroPeerId": actor.peer,
        "permissions": actor.permissions
    })
}

fn blob_json(blob: &LoroBlob) -> GenResult<Value> {
    let meta = LoroDoc::decode_import_blob_meta(&blob.bytes, true)?;
    Ok(json!({
        "id": blob.id,
        "kind": blob.kind,
        "actorId": blob.actor_id,
        "encoding": "loro-fast",
        "byteLength": blob.bytes.len(),
        "bytesBase64": base64_encode(&blob.bytes),
        "version": {
            "start": vv_json(&blob.start_vv),
            "end": vv_json(&blob.end_vv)
        },
        "metadata": import_blob_meta_json(&meta),
        "jsonUpdates": blob.json_updates.clone()
    }))
}

fn import_blob_meta_json(meta: &ImportBlobMetadata) -> Value {
    json!({
        "mode": meta.mode.to_string(),
        "changeNum": meta.change_num,
        "partialStartVersion": vv_json(&meta.partial_start_vv),
        "partialEndVersion": vv_json(&meta.partial_end_vv),
        "startFrontier": frontier_json(&meta.start_frontiers),
        "endTimestamp": meta.end_timestamp,
        "startTimestamp": meta.start_timestamp
    })
}

fn import_status_json(status: &ImportStatus) -> Value {
    json!({
        "success": version_range_json(&status.success),
        "pending": status
            .pending
            .as_ref()
            .map(version_range_json)
            .unwrap_or(Value::Null)
    })
}

fn version_range_json(range: &VersionRange) -> Value {
    let mut entries = range
        .iter()
        .map(|(peer, (start, end))| (*peer, *start, *end))
        .collect::<Vec<_>>();
    entries.sort_by_key(|(peer, _, _)| *peer);
    Value::Array(
        entries
            .into_iter()
            .map(|(peer, start, end)| {
                json!({
                    "peer": peer,
                    "start": start,
                    "end": end
                })
            })
            .collect(),
    )
}

fn vv_json(vv: &VersionVector) -> Value {
    let mut entries = vv
        .iter()
        .map(|(peer, counter)| (*peer, *counter))
        .collect::<Vec<_>>();
    entries.sort_by_key(|(peer, _)| *peer);
    Value::Array(
        entries
            .into_iter()
            .map(|(peer, counter)| json!({"peer": peer, "counter": counter}))
            .collect(),
    )
}

fn frontier_json(frontiers: &Frontiers) -> Value {
    Value::Array(
        sorted_frontier_ids(frontiers)
            .into_iter()
            .map(|id| json!({"peer": id.peer, "counter": id.counter, "id": id.to_string()}))
            .collect(),
    )
}

fn frontier_ids(frontiers: &Frontiers) -> Vec<String> {
    sorted_frontier_ids(frontiers)
        .into_iter()
        .map(|id| id.to_string())
        .collect()
}

fn sorted_frontier_ids(frontiers: &Frontiers) -> Vec<ID> {
    let mut ids = frontiers.iter().collect::<Vec<_>>();
    ids.sort();
    ids
}

fn materialized(doc: &LoroDoc) -> Value {
    doc.get_map("notebook").get_deep_value().to_json_value()
}

fn assert_same_materialized(a: &LoroDoc, b: &LoroDoc, message: &str) -> GenResult<()> {
    if materialized(a) != materialized(b) {
        return Err(boxed_error(message));
    }
    Ok(())
}

fn text_at(doc: &LoroDoc, path: &str) -> GenResult<LoroText> {
    match container_at(doc, path)? {
        Container::Text(text) => Ok(text),
        other => Err(boxed_error(format!(
            "expected text at {path}, got {other:?}"
        ))),
    }
}

fn map_at(doc: &LoroDoc, path: &str) -> GenResult<LoroMap> {
    match container_at(doc, path)? {
        Container::Map(map) => Ok(map),
        other => Err(boxed_error(format!(
            "expected map at {path}, got {other:?}"
        ))),
    }
}

fn list_at(doc: &LoroDoc, path: &str) -> GenResult<LoroList> {
    match container_at(doc, path)? {
        Container::List(list) => Ok(list),
        other => Err(boxed_error(format!(
            "expected list at {path}, got {other:?}"
        ))),
    }
}

fn movable_list_at(doc: &LoroDoc, path: &str) -> GenResult<LoroMovableList> {
    match container_at(doc, path)? {
        Container::MovableList(list) => Ok(list),
        other => Err(boxed_error(format!(
            "expected movable list at {path}, got {other:?}"
        ))),
    }
}

fn container_at(doc: &LoroDoc, path: &str) -> GenResult<Container> {
    match doc.get_by_str_path(path) {
        Some(ValueOrContainer::Container(container)) => Ok(container),
        Some(ValueOrContainer::Value(value)) => Err(boxed_error(format!(
            "expected container at {path}, got value {value:?}"
        ))),
        None => Err(boxed_error(format!("path not found: {path}"))),
    }
}

fn render_json(value: &Value) -> GenResult<String> {
    let canonical = sort_value(value.clone());
    let mut output = serde_json::to_string_pretty(&canonical)?;
    output.push('\n');
    Ok(output)
}

fn sort_value(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(sort_value).collect()),
        Value::Object(map) => {
            let mut sorted = Map::new();
            let mut entries = map.into_iter().collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            for (key, value) in entries {
                sorted.insert(key, sort_value(value));
            }
            Value::Object(sorted)
        }
        other => other,
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);

        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn boxed_error(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::new(io::ErrorKind::Other, message.into()))
}

#[allow(dead_code)]
fn assert_path_is_within(path: &Path, parent: &Path) -> GenResult<()> {
    if !path.starts_with(parent) {
        return Err(boxed_error(format!(
            "{} is outside {}",
            path.display(),
            parent.display()
        )));
    }
    Ok(())
}
