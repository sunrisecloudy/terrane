import { PlatformError } from "./errors.js";
import { canonicalJson, id, nowIso, prettyJson, sha256 } from "./util.js";

const TEXT_CELL_TYPES = new Set(["markdown", "prompt", "code"]);
const CELL_TYPES = new Set([...TEXT_CELL_TYPES, "output", "artifact"]);
const WRITE_OPS = new Set([
  "notebook.init",
  "batch",
  "cell.insert",
  "cell.delete",
  "cell.move",
  "text.insert",
  "text.delete",
  "text.replace",
  "metadata.set",
  "metadata.delete",
  "output.append",
  "comment.add",
  "comment.resolve",
  "checkpoint.create",
]);
const PROPOSAL_OPS = new Set(["proposal.create"]);
const APPROVAL_OPS = new Set(["proposal.accept", "proposal.reject"]);
const MAX_UPDATES = 4096;
const MAX_CELL_SOURCE_BYTES = 262_144;

export class NotebookCrdtService {
  constructor({ database }) {
    this.database = database;
  }

  open(params, context) {
    const notebookId = notebookIdFromParams(params, { create: true });
    const actor = actorFromContext(context, params);
    const existing = this.database.crdtNotebook(context.appId, notebookId);
    if (!existing) {
      assertApprovedAppPermission(context, "notebook.write");
      this.database.createCrdtNotebook({
        appId: context.appId,
        notebookId,
        title: stringOrDefault(params.title, "Untitled notebook"),
        actor,
      });
    } else {
      this.database.assertCrdtNotebookPermission({
        appId: context.appId,
        notebookId,
        actorId: actor.actorId,
        permission: "notebook.read",
      });
    }
    return this.materializedResult(context.appId, notebookId);
  }

  applyLocal(params, context) {
    const notebookId = notebookIdFromParams(params);
    const actor = actorFromContext(context, params);
    const operation = operationFromParams(params);
    const permission = permissionForOperation(operation.type);
    try {
      assertApprovedAppPermission(context, permission);
      this.assertNotebookAccess({ appId: context.appId, notebookId, actor, permission });
      return this.applyOperation({ appId: context.appId, notebookId, actor, operation });
    } catch (error) {
      this.auditRejected({ appId: context.appId, notebookId, actor, operation, error });
      throw error;
    }
  }

  proposeAiPatch(params, context) {
    const notebookId = notebookIdFromParams(params);
    const actor = {
      actorId: context.aiActorId ?? params.aiActorId ?? (context.actorKind === "ai" ? context.actorId : null) ?? "actor_reference_ai",
      actorKind: "ai",
    };
    assertApprovedAppPermission(context, "notebook.propose");
    this.assertNotebookAccess({ appId: context.appId, notebookId, actor, permission: "notebook.propose" });
    const proposal = params.proposal && typeof params.proposal === "object" && !Array.isArray(params.proposal)
      ? params.proposal
      : params;
    const operation = {
      opId: proposal.opId ?? id("crdt_op"),
      seq: proposal.seq ?? this.database.nextCrdtSeq(context.appId, notebookId),
      type: "proposal.create",
      proposalId: proposal.proposalId ?? id("proposal"),
      modelId: requiredString(proposal, "modelId"),
      promptHash: proposal.promptHash ?? proposal.promptContextHash ?? requiredString(proposal, "contextHash"),
      contextHash: proposal.contextHash ?? proposal.promptContextHash ?? proposal.promptHash,
      promptContextHash: proposal.promptContextHash ?? proposal.contextHash ?? proposal.promptHash,
      patchSummary: proposal.patchSummary ?? null,
      proposedSource: proposal.proposedSource ?? null,
      affectedCellIds: Array.isArray(proposal.affectedCellIds) ? proposal.affectedCellIds : [],
      baseFrontier: proposal.baseFrontier ?? this.database.crdtHead(context.appId, notebookId)?.frontier ?? { version: 0, heads: [] },
      operations: Array.isArray(proposal.operations)
        ? proposal.operations
        : proposal.proposedSource && Array.isArray(proposal.affectedCellIds) && proposal.affectedCellIds[0]
          ? [{ type: "text.replace", cellId: proposal.affectedCellIds[0], text: proposal.proposedSource }]
          : [],
    };
    return this.applyOperation({ appId: context.appId, notebookId, actor, operation });
  }

  acceptProposal(params, context) {
    return this.proposalDecision(params, context, "proposal.accept");
  }

  rejectProposal(params, context) {
    return this.proposalDecision(params, context, "proposal.reject");
  }

  proposalDecision(params, context, type) {
    const notebookId = notebookIdFromParams(params);
    const actor = actorFromContext(context, params);
    assertApprovedAppPermission(context, "notebook.approve");
    this.assertNotebookAccess({ appId: context.appId, notebookId, actor, permission: "notebook.approve" });
    const operation = {
      opId: params.opId ?? id("crdt_op"),
      seq: params.seq ?? this.database.nextCrdtSeq(context.appId, notebookId),
      type,
      proposalId: requiredString(params, "proposalId"),
      approvalId: params.approvalId ?? null,
    };
    return this.applyOperation({ appId: context.appId, notebookId, actor, operation });
  }

  snapshot(params, context) {
    const notebookId = notebookIdFromParams(params);
    const actor = actorFromContext(context, params);
    assertApprovedAppPermission(context, "notebook.read");
    this.assertNotebookAccess({ appId: context.appId, notebookId, actor, permission: "notebook.read" });
    return this.materializedResult(context.appId, notebookId);
  }

  checkout(params, context) {
    const notebookId = notebookIdFromParams(params);
    const actor = actorFromContext(context, params);
    assertApprovedAppPermission(context, "notebook.read");
    this.assertNotebookAccess({ appId: context.appId, notebookId, actor, permission: "notebook.read" });
    const frontier = params.frontier && typeof params.frontier === "object" ? params.frontier : {};
    const version = Number.isInteger(frontier.version) ? frontier.version : null;
    return this.materializedResult(context.appId, notebookId, { version });
  }

  syncPull(params, context) {
    const notebookId = notebookIdFromParams(params);
    const actor = actorFromContext(context, params);
    assertApprovedAppPermission(context, "notebook.sync");
    this.assertNotebookAccess({ appId: context.appId, notebookId, actor, permission: "notebook.sync" });
    const afterSeq = Number.isInteger(params.afterSeq) ? params.afterSeq : 0;
    const updates = this.database.crdtAcceptedUpdates(context.appId, notebookId, { afterSeq }).map((row) => ({
      updateId: row.update_id,
      seq: row.seq,
      actorId: row.actor_id,
      actorKind: row.actor_kind,
      operation: JSON.parse(row.operation_json),
      contentHash: row.content_hash,
    }));
    const head = this.database.crdtHead(context.appId, notebookId) ?? { version: 0, frontier: { version: 0, heads: [] } };
    return { ok: true, notebookId, updates, frontier: head.frontier, cursor: { afterSeq: head.version } };
  }

  syncPush(params, context) {
    const notebookId = notebookIdFromParams(params);
    const actor = actorFromContext(context, params);
    assertApprovedAppPermission(context, "notebook.sync");
    this.assertNotebookAccess({ appId: context.appId, notebookId, actor, permission: "notebook.sync" });
    const updates = Array.isArray(params.updates) ? params.updates : [];
    const accepted = [];
    const duplicates = [];
    const rejected = [];
    for (const update of [...updates].sort(compareImportedUpdates)) {
      const operation = update.operation ?? update;
      if (this.database.crdtUpdateByOpId(context.appId, notebookId, operation.opId)) {
        duplicates.push(operation.opId);
        continue;
      }
      const permission = permissionForOperation(operation.type);
      try {
        assertApprovedAppPermission(context, permission);
        this.assertNotebookAccess({ appId: context.appId, notebookId, actor, permission });
        const result = this.applyOperation({ appId: context.appId, notebookId, actor, operation });
        accepted.push(result.updateId);
      } catch (error) {
        rejected.push({
          opId: operation?.opId ?? null,
          error: error instanceof PlatformError ? error.code : "internal_error",
        });
        this.auditRejected({ appId: context.appId, notebookId, actor, operation, error });
      }
    }
    const snapshot = this.materializedResult(context.appId, notebookId);
    return { ok: true, notebookId, accepted, duplicates, rejected, frontier: snapshot.frontier, notebook: snapshot.notebook };
  }

  subscribe(params, context) {
    const notebookId = notebookIdFromParams(params);
    const actor = actorFromContext(context, params);
    assertApprovedAppPermission(context, "notebook.read");
    this.assertNotebookAccess({ appId: context.appId, notebookId, actor, permission: "notebook.read" });
    return {
      ok: true,
      notebookId,
      subscriptionId: id("notebook_sub"),
      eventName: `notebook.${notebookId}.updates`,
      transport: "reference-host-poll",
    };
  }

  assertNotebookAccess({ appId, notebookId, actor, permission }) {
    const notebook = this.database.crdtNotebook(appId, notebookId);
    if (!notebook) {
      throw new PlatformError("unknown_notebook", `Notebook not found: ${notebookId}`, { appId, notebookId });
    }
    this.database.ensureCrdtActor({ appId, actor });
    this.database.assertCrdtNotebookPermission({ appId, notebookId, actorId: actor.actorId, permission });
  }

  applyOperation({ appId, notebookId, actor, operation }) {
    validateOperation(operation);
    if (this.database.crdtUpdateByOpId(appId, notebookId, operation.opId)) {
      return { ...this.materializedResult(appId, notebookId), status: "duplicate", opId: operation.opId };
    }
    const update = {
      updateId: id("crdt_update"),
      appId,
      notebookId,
      actor,
      seq: Number.isInteger(operation.seq) ? operation.seq : this.database.nextCrdtSeq(appId, notebookId),
      operation,
    };
    const candidate = [
      ...this.database.crdtAcceptedUpdates(appId, notebookId).map((row) => ({
        updateId: row.update_id,
        appId: row.app_id,
        notebookId: row.notebook_id,
        actor: { actorId: row.actor_id, actorKind: row.actor_kind },
        seq: row.seq,
        operation: JSON.parse(row.operation_json),
      })),
      update,
    ];
    let materialized;
    try {
      materialized = materializeNotebook(candidate);
    } catch (error) {
      this.auditRejected({ appId, notebookId, actor, operation, error });
      throw error;
    }
    this.database.insertCrdtAcceptedUpdate(update, materialized);
    return {
      ok: true,
      status: "accepted",
      notebookId,
      opId: operation.opId,
      updateId: update.updateId,
      frontier: materialized.frontier,
      notebook: materialized.notebook,
    };
  }

  auditRejected({ appId, notebookId, actor, operation, error }) {
    this.database.insertCrdtRejectedUpdate({
      updateId: id("crdt_update"),
      appId,
      notebookId,
      actor,
      seq: Number.isInteger(operation?.seq) ? operation.seq : this.database.nextCrdtSeq(appId, notebookId),
      operation: operation ?? null,
      errorCode: error instanceof PlatformError ? error.code : "internal_error",
    });
  }

  materializedResult(appId, notebookId, options = {}) {
    const updates = this.database.crdtAcceptedUpdates(appId, notebookId).map((row) => ({
      updateId: row.update_id,
      appId: row.app_id,
      notebookId: row.notebook_id,
      actor: { actorId: row.actor_id, actorKind: row.actor_kind },
      seq: row.seq,
      operation: JSON.parse(row.operation_json),
    }));
    const materialized = materializeNotebook(updates, options);
    this.database.upsertCrdtHead({ appId, notebookId, materialized });
    return { ok: true, notebookId, frontier: materialized.frontier, notebook: materialized.notebook };
  }
}

export function materializeNotebook(updates, { version = null } = {}) {
  if (updates.length > MAX_UPDATES) {
    throw new PlatformError("resource_budget_exceeded", "Notebook update stream exceeds resource budget", { maxUpdates: MAX_UPDATES });
  }
  const selected = Number.isInteger(version) ? updates.slice(0, version) : [...updates];
  selected.sort(compareUpdates);
  const notebook = emptyNotebook();
  for (const update of selected) {
    applyOperationToNotebook(notebook, update);
  }
  validateNotebookDocument(notebook);
  return {
    frontier: {
      version: selected.length,
      heads: selected.length ? [selected.at(-1).operation.opId] : [],
    },
    notebook,
    contentHash: `sha256:${sha256(canonicalJson(notebook))}`,
  };
}

function applyOperationToNotebook(notebook, update) {
  const op = update.operation;
  switch (op.type) {
    case "cell.insert": {
      if (notebook.cells.some((cell) => cell.id === op.cellId)) {
        throw new PlatformError("conflict_rejected", "cell already exists", { cellId: op.cellId });
      }
      if (!CELL_TYPES.has(op.cellType ?? "markdown")) {
        throw new PlatformError("schema_error", "cell type is outside the notebook profile", { cellType: op.cellType });
      }
      notebook.cells.splice(clampInteger(op.index, 0, notebook.cells.length), 0, {
        id: op.cellId,
        type: op.cellType ?? "markdown",
        source: op.source ?? "",
        metadata: cloneJson(op.metadata ?? {}),
        outputs: [],
        createdBy: update.actor.actorId,
        updatedBy: update.actor.actorId,
      });
      break;
    }
    case "cell.delete": {
      const index = notebook.cells.findIndex((cell) => cell.id === op.cellId);
      if (index === -1) throw new PlatformError("unknown_notebook", "cell.delete references an unknown cell", { cellId: op.cellId });
      notebook.cells.splice(index, 1);
      break;
    }
    case "cell.move": {
      const index = notebook.cells.findIndex((cell) => cell.id === op.cellId);
      if (index === -1) throw new PlatformError("unknown_notebook", "cell.move references an unknown cell", { cellId: op.cellId });
      const [cell] = notebook.cells.splice(index, 1);
      notebook.cells.splice(clampInteger(op.index, 0, notebook.cells.length), 0, cell);
      break;
    }
    case "text.insert": {
      const cell = textCell(notebook, op.cellId);
      const index = op.index === "end" ? cell.source.length : clampInteger(op.index, 0, cell.source.length);
      cell.source = `${cell.source.slice(0, index)}${op.text}${cell.source.slice(index)}`;
      if (op.metadata && typeof op.metadata === "object" && !Array.isArray(op.metadata)) {
        cell.metadata = { ...cell.metadata, ...cloneJson(op.metadata) };
      }
      if (op.updatedBy !== false) cell.updatedBy = update.actor.actorId;
      assertCellBudget(cell);
      break;
    }
    case "text.delete": {
      const cell = textCell(notebook, op.cellId);
      const index = clampInteger(op.index, 0, cell.source.length);
      const count = clampInteger(op.count, 0, cell.source.length - index);
      cell.source = `${cell.source.slice(0, index)}${cell.source.slice(index + count)}`;
      if (op.updatedBy !== false) cell.updatedBy = update.actor.actorId;
      break;
    }
    case "text.replace": {
      const cell = textCell(notebook, op.cellId);
      cell.source = op.text;
      if (op.updatedBy !== false) cell.updatedBy = update.actor.actorId;
      assertCellBudget(cell);
      break;
    }
    case "metadata.set":
      if (op.cellId) cellById(notebook, op.cellId).metadata[op.key] = cloneJson(op.value);
      else notebook.metadata[op.key] = cloneJson(op.value);
      break;
    case "metadata.delete":
      if (op.cellId) delete cellById(notebook, op.cellId).metadata[op.key];
      else delete notebook.metadata[op.key];
      break;
    case "output.append": {
      const cell = cellById(notebook, op.cellId);
      cell.outputs.push(cloneJson(op.output ?? {
        id: op.outputId,
        type: op.outputType ?? "stream",
        mime: op.mime ?? "text/plain",
        text: op.text ?? "",
        createdBy: update.actor.actorId,
      }));
      break;
    }
    case "comment.add":
      if (notebook.comments[op.commentId]) throw new PlatformError("conflict_rejected", "comment already exists", { commentId: op.commentId });
      cellById(notebook, op.cellId);
      notebook.comments[op.commentId] = {
        id: op.commentId,
        cellId: op.cellId,
        body: op.body,
        createdBy: update.actor.actorId,
        resolved: false,
      };
      break;
    case "comment.resolve": {
      const comment = notebook.comments[op.commentId];
      if (!comment) throw new PlatformError("unknown_notebook", "comment.resolve references an unknown comment", { commentId: op.commentId });
      comment.resolved = true;
      comment.resolvedBy = update.actor.actorId;
      break;
    }
    case "proposal.create":
      if (notebook.proposals[op.proposalId]) throw new PlatformError("conflict_rejected", "proposal already exists", { proposalId: op.proposalId });
      notebook.proposals[op.proposalId] = {
        id: op.proposalId,
        actorId: update.actor.actorId,
        createdBy: update.actor.actorId,
        actorKind: update.actor.actorKind,
        modelId: op.modelId,
        promptHash: op.promptHash ?? op.promptContextHash,
        contextHash: op.contextHash ?? op.promptContextHash,
        promptContextHash: op.promptContextHash ?? op.contextHash ?? op.promptHash,
        patchSummary: op.patchSummary ?? null,
        proposedSource: op.proposedSource ?? null,
        affectedCellIds: cloneJson(op.affectedCellIds ?? []),
        baseFrontier: cloneJson(op.baseFrontier ?? { version: 0, heads: [] }),
        operations: cloneJson(op.operations ?? []),
        status: "pending",
      };
      break;
    case "proposal.accept":
    case "proposal.reject": {
      const proposal = notebook.proposals[op.proposalId];
      if (!proposal) throw new PlatformError("unknown_notebook", "proposal decision references an unknown proposal", { proposalId: op.proposalId });
      const status = op.type === "proposal.accept" ? "accepted" : "rejected";
      proposal.status = status;
      proposal.reviewedBy = update.actor.actorId;
      notebook.approvals[op.approvalId ?? op.proposalId] = {
        id: op.approvalId ?? op.proposalId,
        proposalId: op.proposalId,
        status,
        decision: status,
        actorId: update.actor.actorId,
      };
      if (status === "accepted") {
        for (const [index, proposed] of (proposal.operations ?? []).entries()) {
          applyOperationToNotebook(notebook, {
            ...update,
            operation: { opId: `${op.opId ?? op.proposalId}:accepted:${index}`, ...proposed },
          });
        }
      }
      break;
    }
    case "checkpoint.create":
      notebook.metadata[`checkpoint:${op.checkpointId}`] = cloneJson(op.frontier ?? { version: 0, heads: [] });
      break;
    case "batch":
      for (const [index, item] of (op.ops ?? []).entries()) {
        applyOperationToNotebook(notebook, {
          ...update,
          operation: { opId: `${op.opId}:batch:${index}`, ...item },
        });
      }
      break;
    case "notebook.init":
      for (const [index, cell] of (op.cells ?? []).entries()) {
        applyOperationToNotebook(notebook, {
          ...update,
          operation: {
            opId: `${op.opId}:cell:${index}`,
            type: "cell.insert",
            index,
            cellId: cell.id,
            cellType: cell.type,
            source: cell.source,
            metadata: cell.metadata ?? {},
          },
        });
      }
      notebook.metadata = { ...notebook.metadata, ...cloneJson(op.metadata ?? {}) };
      break;
    default:
      throw new PlatformError("invalid_request", "Unknown notebook CRDT operation", { type: op.type });
  }
}

function emptyNotebook() {
  return {
    metadata: {},
    cells: [],
    comments: {},
    aiRuns: {},
    proposals: {},
    approvals: {},
  };
}

function validateOperation(operation, { requireOpId = true, child = false } = {}) {
  if (!operation || typeof operation !== "object" || Array.isArray(operation)) {
    throw new PlatformError("invalid_request", "notebook operation must be an object", {});
  }
  if (requireOpId && (typeof operation.opId !== "string" || operation.opId.length === 0)) {
    throw new PlatformError("invalid_request", "notebook operation requires opId", { key: "opId" });
  }
  if (typeof operation.type !== "string" || operation.type.length === 0) {
    throw new PlatformError("invalid_request", "notebook operation requires type", { key: "type" });
  }
  if (![...WRITE_OPS, ...PROPOSAL_OPS, ...APPROVAL_OPS].includes(operation.type)) {
    throw new PlatformError("invalid_request", "notebook operation type is unsupported", { type: operation.type });
  }
  if (child && (operation.type === "batch" || PROPOSAL_OPS.has(operation.type) || APPROVAL_OPS.has(operation.type))) {
    throw new PlatformError("schema_error", "nested notebook operations cannot contain batch, proposal, or approval operations", {
      type: operation.type,
    });
  }
  if (operation.type === "batch" && !Array.isArray(operation.ops)) {
    throw new PlatformError("schema_error", "batch requires ops array", {});
  }
  if (operation.type === "batch") {
    for (const item of operation.ops) validateOperation(item, { requireOpId: false, child: true });
  }
  if (operation.type === "notebook.init" && !Array.isArray(operation.cells)) {
    throw new PlatformError("schema_error", "notebook.init requires cells array", {});
  }
  if (operation.type === "notebook.init") {
    for (const cell of operation.cells) {
      if (!cell || typeof cell !== "object" || Array.isArray(cell)) {
        throw new PlatformError("schema_error", "notebook.init cells must be objects", {});
      }
      requiredString(cell, "id");
      if (!CELL_TYPES.has(cell.type ?? "markdown")) {
        throw new PlatformError("schema_error", "notebook.init cell type is outside the notebook profile", {
          cellType: cell.type,
        });
      }
      if (cell.source != null && typeof cell.source !== "string") {
        throw new PlatformError("schema_error", "notebook.init cell source must be a string", {});
      }
    }
  }
  if (WRITE_OPS.has(operation.type) && operation.type.startsWith("cell.") && operation.type !== "cell.move") {
    requiredString(operation, "cellId");
  }
  if (operation.type === "cell.move") requiredString(operation, "cellId");
  if ((operation.type === "text.insert" || operation.type === "text.delete" || operation.type === "output.append") && typeof operation.cellId !== "string") {
    throw new PlatformError("schema_error", `${operation.type} requires cellId`, {});
  }
  if ((operation.type === "text.insert" || operation.type === "text.replace") && typeof operation.text !== "string") {
    throw new PlatformError("schema_error", `${operation.type} requires text`, {});
  }
  if ((operation.type === "metadata.set" || operation.type === "metadata.delete") && typeof operation.key !== "string") {
    throw new PlatformError("schema_error", `${operation.type} requires key`, {});
  }
  if (operation.type === "metadata.set" && !("value" in operation)) {
    throw new PlatformError("schema_error", "metadata.set requires value", {});
  }
  if (operation.type === "comment.add") {
    for (const key of ["commentId", "cellId", "body"]) requiredString(operation, key);
  }
  if (operation.type === "comment.resolve") requiredString(operation, "commentId");
  if (operation.type === "proposal.create") {
    for (const key of ["proposalId", "modelId"]) requiredString(operation, key);
    if (!operation.promptHash && !operation.contextHash && !operation.promptContextHash) {
      throw new PlatformError("schema_error", "proposal.create requires prompt/context hash", {});
    }
    if (operation.operations != null && !Array.isArray(operation.operations)) {
      throw new PlatformError("schema_error", "proposal.create operations must be an array", {});
    }
    for (const item of operation.operations ?? []) validateOperation(item, { requireOpId: false, child: true });
  }
  if (operation.type === "proposal.accept" || operation.type === "proposal.reject") requiredString(operation, "proposalId");
}

function actorFromContext(context, params = {}) {
  return {
    actorId: context.actorId ?? params.actorId ?? "actor_reference_human",
    actorKind: context.actorKind ?? params.actorKind ?? "human",
  };
}

function assertApprovedAppPermission(context, permission) {
  const approved = context.approvedPermissions;
  if (approved instanceof Set && !approved.has(permission)) {
    throw new PlatformError("permission_denied", `App ${context.appId} cannot use ${permission}`, { appId: context.appId, permission });
  }
}

function permissionForOperation(type) {
  if (PROPOSAL_OPS.has(type)) return "notebook.propose";
  if (APPROVAL_OPS.has(type)) return "notebook.approve";
  return "notebook.write";
}

function notebookIdFromParams(params, { create = false } = {}) {
  if (typeof params.notebookId === "string" && params.notebookId.length > 0) return params.notebookId;
  if (create) return id("notebook");
  throw new PlatformError("invalid_request", "notebookId is required", {});
}

function operationFromParams(params) {
  const operation = params.operation;
  if (!operation || typeof operation !== "object" || Array.isArray(operation)) {
    throw new PlatformError("invalid_request", "operation is required", {});
  }
  return operation;
}

function requiredString(value, key) {
  if (typeof value?.[key] !== "string" || value[key].length === 0) {
    throw new PlatformError("schema_error", `${key} is required`, { key });
  }
  return value[key];
}

function stringOrDefault(value, fallback) {
  return typeof value === "string" && value.length > 0 ? value : fallback;
}

function compareUpdates(a, b) {
  if (a.seq !== b.seq) return a.seq - b.seq;
  return String(a.operation.opId).localeCompare(String(b.operation.opId));
}

function compareImportedUpdates(a, b) {
  const opA = a.operation ?? a;
  const opB = b.operation ?? b;
  const seqA = Number.isInteger(opA.seq) ? opA.seq : 0;
  const seqB = Number.isInteger(opB.seq) ? opB.seq : 0;
  if (seqA !== seqB) return seqA - seqB;
  return String(opA.opId ?? "").localeCompare(String(opB.opId ?? ""));
}

function cellById(notebook, cellId) {
  const cell = notebook.cells.find((candidate) => candidate.id === cellId);
  if (!cell) throw new PlatformError("unknown_notebook", "operation references an unknown cell", { cellId });
  return cell;
}

function textCell(notebook, cellId) {
  const cell = cellById(notebook, cellId);
  if (!TEXT_CELL_TYPES.has(cell.type)) {
    throw new PlatformError("schema_error", "collaborative text is only supported on markdown, prompt, and code cells", { cellId, type: cell.type });
  }
  return cell;
}

function assertCellBudget(cell) {
  if (Buffer.byteLength(cell.source) > MAX_CELL_SOURCE_BYTES) {
    throw new PlatformError("schema_error", "cell source exceeds notebook text budget", {
      cellId: cell.id,
      maxBytes: MAX_CELL_SOURCE_BYTES,
    });
  }
}

function validateNotebookDocument(notebook) {
  if (!notebook || typeof notebook !== "object" || Array.isArray(notebook)) {
    throw new PlatformError("schema_error", "materialized notebook must be an object", {});
  }
  for (const key of ["metadata", "comments", "aiRuns", "proposals", "approvals"]) {
    if (!notebook[key] || typeof notebook[key] !== "object" || Array.isArray(notebook[key])) {
      throw new PlatformError("schema_error", `materialized notebook requires object ${key}`, { key });
    }
  }
  if (!Array.isArray(notebook.cells)) {
    throw new PlatformError("schema_error", "materialized notebook requires cells array", {});
  }
  const ids = new Set();
  for (const cell of notebook.cells) {
    if (!cell || typeof cell !== "object" || Array.isArray(cell)) {
      throw new PlatformError("schema_error", "materialized notebook cell must be an object", {});
    }
    if (typeof cell.id !== "string" || cell.id.length === 0 || ids.has(cell.id)) {
      throw new PlatformError("schema_error", "materialized notebook cell id is invalid", { cellId: cell.id ?? null });
    }
    ids.add(cell.id);
    if (!CELL_TYPES.has(cell.type) || typeof cell.source !== "string" || !Array.isArray(cell.outputs)) {
      throw new PlatformError("schema_error", "materialized notebook cell shape is invalid", { cellId: cell.id });
    }
    if (!cell.metadata || typeof cell.metadata !== "object" || Array.isArray(cell.metadata)) {
      throw new PlatformError("schema_error", "materialized notebook cell metadata must be an object", { cellId: cell.id });
    }
  }
}

function clampInteger(value, min, max) {
  if (!Number.isInteger(value)) return min;
  return Math.max(min, Math.min(max, value));
}

function cloneJson(value) {
  return JSON.parse(JSON.stringify(value ?? null));
}

export function serializedCrdtUpdate({ updateId, appId, notebookId, actor, seq, operation, status = "accepted", errorCode = null }) {
  const operationJson = prettyJson(operation);
  return {
    updateId,
    appId,
    notebookId,
    actorId: actor.actorId,
    actorKind: actor.actorKind,
    seq,
    operationJson,
    status,
    errorCode,
    contentHash: `sha256:${sha256(canonicalJson({ appId, notebookId, seq, operation, status, errorCode }))}`,
    createdAt: nowIso(),
  };
}
