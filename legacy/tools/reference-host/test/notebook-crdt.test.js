import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { BridgeDispatcher } from "../src/bridge-dispatcher.js";
import { CoreEngine } from "../src/core.js";
import { examplesDir } from "../src/paths.js";
import { readPackage } from "../src/package-validator.js";
import { PlatformDatabase } from "../src/platform-database.js";
import { createPlatformKeypair, signPackage } from "../src/signing.js";

test("notebook bridge covers open, local edits, snapshots, checkout, sync, and subscribe", async () => {
  const { db, dispatcher, sessionId } = installNotebookApp();
  try {
    const open = await call(dispatcher, sessionId, "notebook.open", { notebookId: "notebook_team", title: "Team notes" });
    assert.equal(open.ok, true);
    assert.equal(open.result.notebookId, "notebook_team");

    const insert = await call(dispatcher, sessionId, "notebook.apply_local", {
      notebookId: "notebook_team",
      operation: {
        opId: "op_cell",
        seq: 1,
        type: "cell.insert",
        cellId: "cell_intro",
        cellType: "markdown",
        source: "Hello",
      },
    });
    assert.equal(insert.ok, true);

    const text = await call(dispatcher, sessionId, "notebook.apply_local", {
      notebookId: "notebook_team",
      operation: {
        opId: "op_text",
        seq: 2,
        type: "text.insert",
        cellId: "cell_intro",
        index: 5,
        text: ", notebook",
      },
    });
    assert.equal(text.ok, true);
    assert.equal(text.result.notebook.cells[0].source, "Hello, notebook");

    const duplicate = await call(dispatcher, sessionId, "notebook.apply_local", {
      notebookId: "notebook_team",
      operation: { opId: "op_text", seq: 2, type: "text.insert", cellId: "cell_intro", index: 5, text: ", notebook" },
    });
    assert.equal(duplicate.ok, true);
    assert.equal(duplicate.result.status, "duplicate");

    const snapshot = await call(dispatcher, sessionId, "notebook.snapshot", { notebookId: "notebook_team" });
    assert.equal(snapshot.ok, true);
    assert.equal(snapshot.result.frontier.version, 2);
    assert.equal(snapshot.result.notebook.cells[0].source, "Hello, notebook");

    const checkout = await call(dispatcher, sessionId, "notebook.checkout", {
      notebookId: "notebook_team",
      frontier: { version: 1 },
    });
    assert.equal(checkout.ok, true);
    assert.equal(checkout.result.notebook.cells[0].source, "Hello");

    const pull = await call(dispatcher, sessionId, "notebook.sync_pull", { notebookId: "notebook_team", afterSeq: 1 });
    assert.equal(pull.ok, true);
    assert.equal(pull.result.updates.length, 1);
    assert.equal(pull.result.updates[0].operation.opId, "op_text");

    const subscribe = await call(dispatcher, sessionId, "notebook.subscribe", { notebookId: "notebook_team" });
    assert.equal(subscribe.ok, true);
    assert.equal(subscribe.result.eventName, "notebook.notebook_team.updates");

    assert.equal(db.crdtAcceptedUpdates("notes-lite", "notebook_team").length, 2);
    assert.equal(db.all("SELECT COUNT(*) AS count FROM crdt_documents").at(0).count, 2);
  } finally {
    db.close();
  }
});

test("AI proposals require approval and canonical AI writes are denied", async () => {
  const { db, dispatcher, sessionId } = installNotebookApp();
	  try {
	    await call(dispatcher, sessionId, "notebook.open", { notebookId: "notebook_ai", title: "AI review" });
	    await call(dispatcher, sessionId, "notebook.apply_local", {
	      notebookId: "notebook_ai",
	      operation: {
	        opId: "op_ai_seed_cell",
	        seq: 1,
	        type: "cell.insert",
	        cellId: "cell_intro",
	        cellType: "markdown",
	        source: "",
	      },
	    });
	    grantActor(db, "notes-lite", "notebook_ai", "actor_reference_ai", "ai", ["notebook.read", "notebook.propose", "notebook.sync"]);

    const proposed = await call(dispatcher, sessionId, "notebook.propose_ai_patch", {
      notebookId: "notebook_ai",
      proposalId: "proposal_1",
      modelId: "reference-model",
      promptHash: "sha256:prompt",
      contextHash: "sha256:context",
      affectedCellIds: ["cell_intro"],
      operations: [{ type: "text.insert", cellId: "cell_intro", index: 0, text: "Draft" }],
    });
    assert.equal(proposed.ok, true);
    assert.equal(proposed.result.notebook.proposals.proposal_1.status, "pending");

    const accepted = await call(dispatcher, sessionId, "notebook.accept_proposal", {
      notebookId: "notebook_ai",
      proposalId: "proposal_1",
    });
    assert.equal(accepted.ok, true);
    assert.equal(accepted.result.notebook.proposals.proposal_1.status, "accepted");
    assert.equal(accepted.result.notebook.approvals.proposal_1.actorId, "actor_reference_human");

    const denied = await dispatcher.dispatch(
      {
        id: "req_ai_write",
        method: "notebook.apply_local",
        params: {
          notebookId: "notebook_ai",
          operation: { opId: "op_ai_write", type: "cell.insert", cellId: "cell_ai", source: "Nope" },
        },
      },
      { appId: "notes-lite", sessionId, actorId: "actor_reference_ai", actorKind: "ai" },
    );
    assert.equal(denied.ok, false);
    assert.equal(denied.error.code, "permission_denied");
    assert.equal(db.all("SELECT status, error_code FROM crdt_updates WHERE status = 'rejected'").length >= 1, true);
  } finally {
    db.close();
  }
});

test("sync_push accepts offline out-of-order updates and rejects unknown notebooks", async () => {
  const { db, dispatcher, sessionId } = installNotebookApp();
  try {
    await call(dispatcher, sessionId, "notebook.open", { notebookId: "notebook_offline", title: "Offline edits" });
    const pushed = await call(dispatcher, sessionId, "notebook.sync_push", {
      notebookId: "notebook_offline",
      updates: [
        { opId: "op_offline_text", seq: 2, type: "text.insert", cellId: "cell_offline", index: 4, text: " sync" },
        { opId: "op_offline_cell", seq: 1, type: "cell.insert", cellId: "cell_offline", cellType: "markdown", source: "Base" },
      ],
    });
    assert.equal(pushed.ok, true);
    assert.deepEqual(pushed.result.rejected, []);
    assert.equal(pushed.result.accepted.length, 2);
    assert.equal(pushed.result.notebook.cells[0].source, "Base sync");

    const unknown = await call(dispatcher, sessionId, "notebook.snapshot", { notebookId: "notebook_missing" });
    assert.equal(unknown.ok, false);
    assert.equal(unknown.error.code, "unknown_notebook");
  } finally {
    db.close();
  }
});

function installNotebookApp() {
  const db = new PlatformDatabase();
  const pkg = readPackage(path.join(examplesDir, "notes-lite"));
  const manifest = {
    ...pkg.manifest,
    permissions: [
      ...pkg.manifest.permissions,
      "notebook.read",
      "notebook.write",
      "notebook.propose",
      "notebook.approve",
      "notebook.sync",
    ],
    capabilities: {
      required: [...pkg.manifest.capabilities.required, "notebook.read"],
      optional: [
        ...pkg.manifest.capabilities.optional,
        "notebook.write",
        "notebook.propose",
        "notebook.approve",
        "notebook.sync",
      ],
    },
  };
  const signed = signPackage({ manifest, files: pkg.files, keypair: createPlatformKeypair() });
  db.insertInstalledPackage({
    manifest,
    files: pkg.files,
    hashes: signed.hashes,
    validation: pkg.validation,
    signature: signed.signature,
    contentHashesDocument: signed.contentHashesDocument,
  });
  const dispatcher = new BridgeDispatcher({ database: db, core: new CoreEngine() });
  const sessionId = db.createRuntimeSession({ appId: "notes-lite" });
  return { db, dispatcher, sessionId };
}

function grantActor(db, appId, notebookId, actorId, actorKind, permissions) {
  db.ensureCrdtActor({ appId, actor: { actorId, actorKind } });
  for (const permission of permissions) {
    db.grantCrdtPermission({ appId, notebookId, actorId, permission });
  }
}

async function call(dispatcher, sessionId, method, params) {
  return dispatcher.dispatch(
    { id: `req_${method.replaceAll(".", "_")}`, method, params },
    { appId: "notes-lite", sessionId, actorId: "actor_reference_human", actorKind: "human" },
  );
}
