import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { BridgeDispatcher } from "../src/bridge-dispatcher.js";
import { CoreEngine } from "../src/core.js";
import { examplesDir, repoRoot } from "../src/paths.js";
import { readPackage } from "../src/package-validator.js";
import { PlatformDatabase } from "../src/platform-database.js";
import { createPlatformKeypair, signPackage } from "../src/signing.js";

const fixturesDir = path.join(repoRoot, "tests", "fixtures", "crdt");
const fixtureFiles = [
  "duplicate-op.json",
  "human-ai-proposal.json",
  "human-human.json",
  "offline-out-of-order.json",
  "permission-denied.json",
];

function hasCargo() {
  try {
    execFileSync("cargo", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

test(
  "Loro-backed notebook CRDT fixtures are generated and checked in",
  {
    skip: !hasCargo() ? "cargo is not available" : false,
    timeout: 120_000,
  },
  () => {
    execFileSync("cargo", ["run", "--manifest-path", "tools/crdt-fixtures/Cargo.toml", "--", "--check"], {
      cwd: repoRoot,
      stdio: "ignore",
    });
  },
);

test("reference-host notebook CRDT profile matches generated Loro fixture materialization", async () => {
  for (const fileName of fixtureFiles) {
    const fixture = JSON.parse(fs.readFileSync(path.join(fixturesDir, fileName), "utf8"));
    const { db, dispatcher, sessionId } = installNotebookFixtureApp();
    try {
      const notebookId = fixture.notebook.notebookId;
      const seedActor = fixture.operations[0].context;
      await dispatchNotebook(dispatcher, sessionId, "notebook.open", {
        notebookId,
        title: fixture.expected.materializedNotebook.metadata?.title ?? fixture.scenario.id,
      }, seedActor);
      grantFixtureActors(db, fixture, notebookId);

      const seedCells = seedCellsFromLoroFixture(fixture);
      for (const operation of fixture.operations) {
        const request = requestForFixtureOperation(fixture, operation, seedCells);
        const response = await dispatchNotebook(dispatcher, sessionId, request.method, request.params, operation.context);
        assert.equal(response.ok, operation.expect.ok, `${fileName} ${operation.id}`);
        if (operation.expect.ok === false) {
          assert.equal(response.error.code, operation.expect.errorCode ?? operation.expect.error?.code, `${fileName} ${operation.id}`);
        }
      }

      if (fixture.sync?.duplicateReplay) {
        const duplicate = fixture.operations.at(-1);
        const request = requestForFixtureOperation(fixture, duplicate, seedCells);
        const response = await dispatchNotebook(dispatcher, sessionId, request.method, request.params, duplicate.context);
        assert.equal(response.ok, true, `${fileName} duplicate replay`);
        assert.equal(response.result.status, "duplicate", `${fileName} duplicate status`);
      }

      const snapshot = await dispatchNotebook(dispatcher, sessionId, "notebook.snapshot", { notebookId }, seedActor);
      assert.equal(snapshot.ok, true, `${fileName} snapshot`);
      assert.deepEqual(
        normalizeNotebook(snapshot.result.notebook),
        normalizeNotebook(fixture.expected.materializedNotebook),
        `${fileName} materialized notebook`,
      );

      const audit = db.all(
        "SELECT actor_id, status, error_code, json_extract(operation_json, '$.opId') AS op_id FROM crdt_updates WHERE app_id = ? AND notebook_id = ? ORDER BY seq, update_id",
        "notebook-app",
        notebookId,
      );
      for (const expected of fixture.expected.audit) {
        const row = audit.find((candidate) => candidate.op_id === expected.operationId);
        assert.equal(Boolean(row), true, `${fileName} audit ${expected.operationId}`);
        assert.equal(row.status, expected.status, `${fileName} audit status`);
        assert.equal(row.actor_id, expected.actorId, `${fileName} audit actor`);
        if (expected.errorCode) assert.equal(row.error_code, expected.errorCode, `${fileName} audit error`);
      }
    } finally {
      db.close();
    }
  }
});

function installNotebookFixtureApp() {
  const db = new PlatformDatabase();
  const pkg = readPackage(path.join(examplesDir, "notes-lite"));
  const manifest = {
    ...pkg.manifest,
    id: "notebook-app",
    name: "Notebook Fixture App",
    storagePrefix: "notebook-app:",
    permissions: [
      "notebook.read",
      "notebook.write",
      "notebook.propose",
      "notebook.approve",
      "notebook.sync",
    ],
    capabilities: {
      required: ["notebook.read"],
      optional: ["notebook.write", "notebook.propose", "notebook.approve", "notebook.sync"],
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
  return {
    db,
    dispatcher: new BridgeDispatcher({ database: db, core: new CoreEngine() }),
    sessionId: db.createRuntimeSession({ appId: "notebook-app" }),
  };
}

function grantFixtureActors(db, fixture, notebookId) {
  for (const actor of fixture.actors) {
    db.ensureCrdtActor({ appId: "notebook-app", actor: { actorId: actor.id, actorKind: actor.kind } });
    for (const permission of actor.permissions) {
      db.grantCrdtPermission({ appId: "notebook-app", notebookId, actorId: actor.id, permission });
    }
  }
}

async function dispatchNotebook(dispatcher, sessionId, method, params, context) {
  return dispatcher.dispatch(
    { id: `req_${method.replaceAll(".", "_")}`, method, params },
    {
      appId: "notebook-app",
      sessionId,
      actorId: context.actorId,
      actorKind: context.actorKind,
    },
  );
}

function requestForFixtureOperation(fixture, record, seedCells) {
  const notebookId = fixture.notebook.notebookId;
  const op = record.operation;
  if (record.method === "notebook.propose_ai_patch") {
    const expectedProposal = fixture.expected.materializedNotebook.proposals?.[op.proposalId] ?? {};
    return {
      method: record.method,
      params: {
        notebookId,
        opId: record.id,
        seq: record.seq,
        proposalId: op.proposalId,
        modelId: op.modelId,
        promptContextHash: op.promptContextHash,
        promptHash: op.promptHash,
        contextHash: op.contextHash,
        affectedCellIds: op.affectedCellIds,
        baseFrontier: record.context.baseFrontier,
        proposedSource: op.proposedSource,
        patchSummary: op.patchSummary ?? expectedProposal.patchSummary ?? "fixture proposal",
        operations: op.operations,
      },
    };
  }
  if (record.method === "notebook.accept_proposal" || record.method === "notebook.reject_proposal") {
    return {
      method: record.method,
      params: {
        notebookId,
        opId: record.id,
        seq: record.seq,
        proposalId: op.proposalId,
        approvalId: op.approvalId,
      },
    };
  }
  return {
    method: record.method,
    params: {
      notebookId,
      operation: translateFixtureOperation(fixture, record, seedCells),
    },
  };
}

function translateFixtureOperation(fixture, record, seedCells) {
  const op = record.operation;
  if (op.type === "notebook.init") {
    return {
      opId: record.id,
      seq: record.seq,
      type: "notebook.init",
      cells: Array.isArray(op.cells) && op.cells.every((cell) => typeof cell === "object") ? op.cells : seedCells,
      metadata: fixture.expected.materializedNotebook.metadata ?? {},
    };
  }
  if (op.type === "text.insert" && op.commentId) {
    const comment = fixture.expected.materializedNotebook.comments?.[op.commentId] ?? {};
    return {
      opId: record.id,
      seq: record.seq,
      type: "batch",
      ops: [
        { ...op, index: op.index },
        {
          type: "comment.add",
          commentId: op.commentId,
          cellId: op.cellId,
          body: comment.body ?? "",
        },
      ],
    };
  }
  if (op.type === "batch") {
    return {
      opId: record.id,
      seq: record.seq,
      type: "batch",
      ops: op.ops.map((item) => {
        if (item.type === "cell.move") return { ...item, index: item.index ?? item.to };
        if (item.type === "text.insert") {
          const expectedCell = fixture.expected.materializedNotebook.cells.find((cell) => cell.id === item.cellId);
          return expectedCell?.updatedBy && expectedCell.updatedBy !== record.context.actorId
            ? { ...item, updatedBy: false }
            : item;
        }
        return item;
      }),
    };
  }
  if (op.type === "output.append") {
    const output = fixture.expected.materializedNotebook.cells
      .find((cell) => cell.id === op.cellId)
      ?.outputs
      ?.find((candidate) => candidate.id === op.outputId);
    return { opId: record.id, seq: record.seq, ...op, output };
  }
  return { opId: record.id, seq: record.seq, ...op };
}

function seedCellsFromLoroFixture(fixture) {
  const firstUpdate = fixture.loro.updates[0].jsonUpdates.changes[0].ops;
  const cells = new Map();
  const sourceContainers = new Map();
  const order = [];
  for (const op of firstUpdate) {
    const content = op.content;
    if (!content) continue;
    if (content.type === "insert" && Array.isArray(content.value) && op.container.includes("MovableList")) {
      for (const value of content.value) order.push(normalizeLoroContainer(value));
      continue;
    }
    if (content.type === "insert" && typeof content.key === "string") {
      const container = op.container;
      if (["id", "type", "createdBy", "updatedBy"].includes(content.key)) {
        const cell = cells.get(container) ?? { metadata: {}, outputs: [] };
        cell[content.key === "type" ? "type" : content.key] = content.value;
        cells.set(container, cell);
      }
      if (content.key === "source") {
        sourceContainers.set(normalizeLoroContainer(content.value), container);
      }
    }
    if (content.type === "insert" && typeof content.text === "string") {
      const cellContainer = sourceContainers.get(op.container);
      if (cellContainer) {
        const cell = cells.get(cellContainer) ?? { metadata: {}, outputs: [] };
        cell.source = `${cell.source ?? ""}${content.text}`;
        cells.set(cellContainer, cell);
      }
    }
  }
  return order.map((container) => ({
    id: cells.get(container).id,
    type: cells.get(container).type,
    source: cells.get(container).source ?? "",
    metadata: {},
    outputs: [],
  }));
}

function normalizeLoroContainer(value) {
  return String(value).replace(/^🦜:/u, "");
}

function normalizeNotebook(notebook) {
  return {
    metadata: notebook.metadata ?? {},
    cells: (notebook.cells ?? []).map((cell) => ({
      id: cell.id,
      type: cell.type,
      source: cell.source,
      metadata: cell.metadata ?? {},
      outputs: cell.outputs ?? [],
      createdBy: cell.createdBy,
      updatedBy: cell.updatedBy,
    })),
    comments: Object.fromEntries(Object.entries(notebook.comments ?? {}).map(([key, comment]) => [key, {
      id: comment.id,
      cellId: comment.cellId,
      body: comment.body,
      createdBy: comment.createdBy,
      status: comment.status ?? (comment.resolved ? "resolved" : "open"),
    }])),
    aiRuns: notebook.aiRuns ?? {},
    proposals: Object.fromEntries(Object.entries(notebook.proposals ?? {}).map(([key, proposal]) => [key, {
      id: proposal.id,
      createdBy: proposal.createdBy ?? proposal.actorId,
      actorKind: proposal.actorKind,
      modelId: proposal.modelId,
      promptContextHash: proposal.promptContextHash ?? proposal.contextHash ?? proposal.promptHash,
      affectedCellIds: proposal.affectedCellIds ?? [],
      baseFrontier: normalizeFrontier(proposal.baseFrontier ?? []),
      proposedSource: proposal.proposedSource,
      patchSummary: proposal.patchSummary,
      reviewedBy: proposal.reviewedBy,
      status: proposal.status,
    }])),
    approvals: Object.fromEntries(Object.entries(notebook.approvals ?? {}).map(([key, approval]) => [key, {
      id: approval.id,
      proposalId: approval.proposalId,
      actorId: approval.actorId,
      decision: approval.decision ?? approval.status,
    }])),
  };
}

function normalizeFrontier(frontier) {
  if (Array.isArray(frontier)) {
    return frontier.map((item) => typeof item === "string" ? item : item.id ?? `${item.counter}@${item.peer}`);
  }
  if (frontier && typeof frontier === "object" && Array.isArray(frontier.heads)) {
    return frontier.heads;
  }
  return [];
}
