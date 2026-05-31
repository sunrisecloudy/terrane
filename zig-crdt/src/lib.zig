const std = @import("std");
const builtin = @import("builtin");

const ffi_allocator = std.heap.page_allocator;
const max_input_bytes = 1024 * 1024;
const max_updates = 4096;
const max_cell_source_bytes = 262_144;
const max_notebook_json_bytes = 1024 * 1024;

comptime {
    if (builtin.abi == .android) {
        @export(&androidGetAuxVal, .{ .name = "getauxval" });
    }
}

pub const ZigCrdtBuffer = extern struct {
    ptr: [*]u8,
    len: usize,
};

const CrdtError = error{
    PayloadTooLarge,
    InvalidJson,
    InvalidRequest,
    PermissionDenied,
    SchemaError,
    ConflictRejected,
    StaleFrontier,
    UnknownNotebook,
    SyncUnavailable,
};

pub const ZigCrdt = struct {
    updates: std.ArrayList(UpdateRecord) = .empty,

    fn deinit(self: *ZigCrdt, allocator: std.mem.Allocator) void {
        for (self.updates.items) |*record| {
            record.deinit(allocator);
        }
        self.updates.deinit(allocator);
    }

    fn applyJson(self: *ZigCrdt, allocator: std.mem.Allocator, input: []const u8) ![]u8 {
        const parsed = try parseInput(allocator, input);
        defer parsed.deinit();

        const context = try TrustedContext.fromJson(parsed.value.object.get("context") orelse return CrdtError.InvalidRequest);
        const operation = parsed.value.object.get("operation") orelse return CrdtError.InvalidRequest;
        const normalized = try OperationEnvelope.fromJson(allocator, context, operation);
        defer normalized.deinit(allocator);

        if (self.hasOp(normalized.op_id)) {
            var notebook = try materializeUpdates(allocator, self.updates.items, null);
            defer notebook.deinit(allocator);
            return notebookResultJson(allocator, "duplicate", normalized.op_id, notebook);
        }

        assertOperationPermission(context, normalized.kind) catch |err| {
            return errorResultJson(allocator, codeForError(err), messageForError(err));
        };
        var candidate = try cloneRecordsWith(allocator, self.updates.items, normalized);
        defer freeRecordList(allocator, &candidate);
        var notebook = try materializeUpdates(allocator, candidate.items, null);
        defer notebook.deinit(allocator);

        if (notebook.rejected_code) |code| {
            return errorResultJson(allocator, code, notebook.rejected_message orelse "operation rejected");
        }

        try self.updates.append(allocator, try UpdateRecord.fromEnvelope(allocator, normalized));
        return notebookResultJson(allocator, "accepted", normalized.op_id, notebook);
    }

    fn mergeJson(self: *ZigCrdt, allocator: std.mem.Allocator, input: []const u8) ![]u8 {
        const parsed = try parseInput(allocator, input);
        defer parsed.deinit();
        const context = try TrustedContext.fromJson(parsed.value.object.get("context") orelse return CrdtError.InvalidRequest);
        const updates_value = parsed.value.object.get("updates") orelse return CrdtError.InvalidRequest;
        if (updates_value != .array) return CrdtError.InvalidRequest;

        var pending = std.ArrayList(UpdateRecord).empty;
        defer freeRecordList(allocator, &pending);
        var accepted: usize = 0;
        var duplicates: usize = 0;
        var rejected: usize = 0;

        for (updates_value.array.items) |update| {
            if (update != .object) return CrdtError.InvalidRequest;
            const operation_value = update.object.get("operation") orelse update;
            const normalized = OperationEnvelope.fromJson(allocator, context, operation_value) catch {
                rejected += 1;
                continue;
            };
            defer normalized.deinit(allocator);

            if (self.hasOp(normalized.op_id)) {
                duplicates += 1;
                continue;
            }
            assertOperationPermission(context, normalized.kind) catch {
                rejected += 1;
                continue;
            };

            try pending.append(allocator, try UpdateRecord.fromEnvelope(allocator, normalized));
            accepted += 1;
        }

        var candidate = std.ArrayList(UpdateRecord).empty;
        defer freeRecordList(allocator, &candidate);
        for (self.updates.items) |record| {
            try candidate.append(allocator, try record.clone(allocator));
        }
        for (pending.items) |record| {
            try candidate.append(allocator, try record.clone(allocator));
        }
        var notebook = try materializeUpdates(allocator, candidate.items, null);
        defer notebook.deinit(allocator);
        if (notebook.rejected_code) |code| {
            return errorResultJson(allocator, code, notebook.rejected_message orelse "merge rejected");
        }
        for (pending.items) |record| {
            try self.updates.append(allocator, try record.clone(allocator));
        }
        const snapshot = try notebook.toJson(allocator);
        defer allocator.free(snapshot);
        const frontier = try frontierJson(allocator, self.updates.items.len);
        defer allocator.free(frontier);
        return std.fmt.allocPrint(
            allocator,
            "{{\"ok\":true,\"accepted\":{},\"duplicates\":{},\"rejected\":{},\"frontier\":{s},\"notebook\":{s}}}",
            .{ accepted, duplicates, rejected, frontier, snapshot },
        );
    }

    fn materializeJson(self: *ZigCrdt, allocator: std.mem.Allocator, input: []const u8) ![]u8 {
        if (input.len > max_input_bytes) return CrdtError.PayloadTooLarge;
        var limit: ?usize = null;
        if (input.len > 0) {
            const parsed = try parseInput(allocator, input);
            defer parsed.deinit();
            if (parsed.value.object.get("frontier")) |frontier| {
                if (frontier == .object) {
                    if (frontier.object.get("version")) |version_value| {
                        if (version_value == .integer and version_value.integer >= 0) {
                            limit = @intCast(version_value.integer);
                        }
                    }
                }
            }
        }
        var notebook = try materializeUpdates(allocator, self.updates.items, limit);
        defer notebook.deinit(allocator);
        if (notebook.rejected_code) |code| {
            return errorResultJson(allocator, code, notebook.rejected_message orelse "materialization rejected");
        }
        return notebookResultJson(allocator, "materialized", null, notebook);
    }

    fn hasOp(self: *const ZigCrdt, op_id: []const u8) bool {
        for (self.updates.items) |record| {
            if (std.mem.eql(u8, record.op_id, op_id)) return true;
        }
        return false;
    }
};

const TrustedContext = struct {
    app_id: []const u8,
    notebook_id: []const u8,
    actor_id: []const u8,
    actor_kind: []const u8,
    permissions: []const std.json.Value,

    fn fromJson(value: std.json.Value) !TrustedContext {
        if (value != .object) return CrdtError.InvalidRequest;
        const app_id = try requiredString(value, "appId");
        const notebook_id = try requiredString(value, "notebookId");
        const actor_id = try requiredString(value, "actorId");
        const actor_kind = try requiredString(value, "actorKind");
        const permissions_value = value.object.get("permissions") orelse return CrdtError.PermissionDenied;
        if (permissions_value != .array) return CrdtError.PermissionDenied;
        return .{
            .app_id = app_id,
            .notebook_id = notebook_id,
            .actor_id = actor_id,
            .actor_kind = actor_kind,
            .permissions = permissions_value.array.items,
        };
    }

    fn hasPermission(self: TrustedContext, permission: []const u8) bool {
        for (self.permissions) |entry| {
            if (entry == .string and std.mem.eql(u8, entry.string, permission)) return true;
        }
        return false;
    }
};

const OperationKind = enum {
    notebook_init,
    batch,
    cell_insert,
    cell_delete,
    cell_move,
    text_insert,
    text_delete,
    text_replace,
    metadata_set,
    metadata_delete,
    output_append,
    comment_add,
    comment_resolve,
    proposal_create,
    proposal_accept,
    proposal_reject,
    checkpoint_create,
};

const OperationEnvelope = struct {
    op_id: []u8,
    seq: u64,
    actor_id: []u8,
    actor_kind: []u8,
    kind: OperationKind,
    body_json: []u8,

    fn fromJson(allocator: std.mem.Allocator, context: TrustedContext, operation: std.json.Value) !OperationEnvelope {
        if (operation != .object) return CrdtError.InvalidRequest;
        const op_id = try allocator.dupe(u8, try requiredString(operation, "opId"));
        errdefer allocator.free(op_id);
        const type_name = try requiredString(operation, "type");
        const kind = parseKind(type_name) orelse return CrdtError.InvalidRequest;
        const seq = optionalInteger(operation, "seq") orelse 0;
        const body_json = try std.json.Stringify.valueAlloc(allocator, operation, .{});
        errdefer allocator.free(body_json);
        return .{
            .op_id = op_id,
            .seq = @intCast(seq),
            .actor_id = try allocator.dupe(u8, context.actor_id),
            .actor_kind = try allocator.dupe(u8, context.actor_kind),
            .kind = kind,
            .body_json = body_json,
        };
    }

    fn deinit(self: OperationEnvelope, allocator: std.mem.Allocator) void {
        allocator.free(self.op_id);
        allocator.free(self.actor_id);
        allocator.free(self.actor_kind);
        allocator.free(self.body_json);
    }
};

const UpdateRecord = struct {
    op_id: []u8,
    seq: u64,
    actor_id: []u8,
    actor_kind: []u8,
    kind: OperationKind,
    body_json: []u8,

    fn fromEnvelope(allocator: std.mem.Allocator, source: OperationEnvelope) !UpdateRecord {
        return .{
            .op_id = try allocator.dupe(u8, source.op_id),
            .seq = source.seq,
            .actor_id = try allocator.dupe(u8, source.actor_id),
            .actor_kind = try allocator.dupe(u8, source.actor_kind),
            .kind = source.kind,
            .body_json = try allocator.dupe(u8, source.body_json),
        };
    }

    fn clone(self: UpdateRecord, allocator: std.mem.Allocator) !UpdateRecord {
        return .{
            .op_id = try allocator.dupe(u8, self.op_id),
            .seq = self.seq,
            .actor_id = try allocator.dupe(u8, self.actor_id),
            .actor_kind = try allocator.dupe(u8, self.actor_kind),
            .kind = self.kind,
            .body_json = try allocator.dupe(u8, self.body_json),
        };
    }

    fn deinit(self: *UpdateRecord, allocator: std.mem.Allocator) void {
        allocator.free(self.op_id);
        allocator.free(self.actor_id);
        allocator.free(self.actor_kind);
        allocator.free(self.body_json);
    }
};

const Cell = struct {
    id: []u8,
    cell_type: []u8,
    source: std.ArrayList(u8) = .empty,
    metadata_json: []u8,
    outputs: std.ArrayList([]u8) = .empty,
    created_by: []u8,
    updated_by: []u8,
    deleted: bool = false,

    fn deinit(self: *Cell, allocator: std.mem.Allocator) void {
        allocator.free(self.id);
        allocator.free(self.cell_type);
        self.source.deinit(allocator);
        allocator.free(self.metadata_json);
        for (self.outputs.items) |output| allocator.free(output);
        self.outputs.deinit(allocator);
        allocator.free(self.created_by);
        allocator.free(self.updated_by);
    }
};

const KeyValue = struct {
    key: []u8,
    value_json: []u8,

    fn deinit(self: *KeyValue, allocator: std.mem.Allocator) void {
        allocator.free(self.key);
        allocator.free(self.value_json);
    }
};

const Comment = struct {
    id: []u8,
    cell_id: []u8,
    body: []u8,
    created_by: []u8,
    resolved: bool = false,
    resolved_by: ?[]u8 = null,

    fn deinit(self: *Comment, allocator: std.mem.Allocator) void {
        allocator.free(self.id);
        allocator.free(self.cell_id);
        allocator.free(self.body);
        allocator.free(self.created_by);
        if (self.resolved_by) |value| allocator.free(value);
    }
};

const Proposal = struct {
    id: []u8,
    actor_id: []u8,
    model_id: []u8,
    prompt_hash: []u8,
    context_hash: []u8,
    status: []u8,
    affected_json: []u8,
    operations_json: []u8,
    base_frontier_json: []u8,

    fn deinit(self: *Proposal, allocator: std.mem.Allocator) void {
        allocator.free(self.id);
        allocator.free(self.actor_id);
        allocator.free(self.model_id);
        allocator.free(self.prompt_hash);
        allocator.free(self.context_hash);
        allocator.free(self.status);
        allocator.free(self.affected_json);
        allocator.free(self.operations_json);
        allocator.free(self.base_frontier_json);
    }
};

const Approval = struct {
    proposal_id: []u8,
    status: []u8,
    actor_id: []u8,

    fn deinit(self: *Approval, allocator: std.mem.Allocator) void {
        allocator.free(self.proposal_id);
        allocator.free(self.status);
        allocator.free(self.actor_id);
    }
};

const Notebook = struct {
    metadata: std.ArrayList(KeyValue) = .empty,
    cells: std.ArrayList(Cell) = .empty,
    comments: std.ArrayList(Comment) = .empty,
    proposals: std.ArrayList(Proposal) = .empty,
    approvals: std.ArrayList(Approval) = .empty,
    checkpoints: std.ArrayList(KeyValue) = .empty,
    applied_ops: usize = 0,
    rejected_code: ?[]const u8 = null,
    rejected_message: ?[]const u8 = null,

    fn deinit(self: *Notebook, allocator: std.mem.Allocator) void {
        for (self.metadata.items) |*item| item.deinit(allocator);
        self.metadata.deinit(allocator);
        for (self.cells.items) |*cell| cell.deinit(allocator);
        self.cells.deinit(allocator);
        for (self.comments.items) |*comment| comment.deinit(allocator);
        self.comments.deinit(allocator);
        for (self.proposals.items) |*proposal| proposal.deinit(allocator);
        self.proposals.deinit(allocator);
        for (self.approvals.items) |*approval| approval.deinit(allocator);
        self.approvals.deinit(allocator);
        for (self.checkpoints.items) |*checkpoint| checkpoint.deinit(allocator);
        self.checkpoints.deinit(allocator);
    }

    fn toJson(self: Notebook, allocator: std.mem.Allocator) ![]u8 {
        var out: std.ArrayList(u8) = .empty;
        try out.appendSlice(allocator, "{\"metadata\":{");
        for (self.metadata.items, 0..) |item, index| {
            if (index > 0) try out.append(allocator, ',');
            try appendQuoted(allocator, &out, item.key);
            try out.append(allocator, ':');
            try out.appendSlice(allocator, item.value_json);
        }
        try out.appendSlice(allocator, "},\"cells\":[");
        var visible_index: usize = 0;
        for (self.cells.items) |cell| {
            if (cell.deleted) continue;
            if (visible_index > 0) try out.append(allocator, ',');
            visible_index += 1;
            try out.appendSlice(allocator, "{\"id\":");
            try appendQuoted(allocator, &out, cell.id);
            try out.appendSlice(allocator, ",\"type\":");
            try appendQuoted(allocator, &out, cell.cell_type);
            try out.appendSlice(allocator, ",\"source\":");
            try appendQuoted(allocator, &out, cell.source.items);
            try out.appendSlice(allocator, ",\"metadata\":");
            try out.appendSlice(allocator, cell.metadata_json);
            try out.appendSlice(allocator, ",\"outputs\":[");
            for (cell.outputs.items, 0..) |output, index| {
                if (index > 0) try out.append(allocator, ',');
                try out.appendSlice(allocator, output);
            }
            try out.appendSlice(allocator, "],\"createdBy\":");
            try appendQuoted(allocator, &out, cell.created_by);
            try out.appendSlice(allocator, ",\"updatedBy\":");
            try appendQuoted(allocator, &out, cell.updated_by);
            try out.append(allocator, '}');
        }
        try out.appendSlice(allocator, "],\"comments\":{");
        for (self.comments.items, 0..) |comment, index| {
            if (index > 0) try out.append(allocator, ',');
            try appendQuoted(allocator, &out, comment.id);
            try out.appendSlice(allocator, ":{\"id\":");
            try appendQuoted(allocator, &out, comment.id);
            try out.appendSlice(allocator, ",\"cellId\":");
            try appendQuoted(allocator, &out, comment.cell_id);
            try out.appendSlice(allocator, ",\"body\":");
            try appendQuoted(allocator, &out, comment.body);
            try out.appendSlice(allocator, ",\"createdBy\":");
            try appendQuoted(allocator, &out, comment.created_by);
            try out.appendSlice(allocator, ",\"resolved\":");
            try out.appendSlice(allocator, if (comment.resolved) "true" else "false");
            if (comment.resolved_by) |resolved_by| {
                try out.appendSlice(allocator, ",\"resolvedBy\":");
                try appendQuoted(allocator, &out, resolved_by);
            }
            try out.append(allocator, '}');
        }
        try out.appendSlice(allocator, "},\"aiRuns\":{},\"proposals\":{");
        for (self.proposals.items, 0..) |proposal, index| {
            if (index > 0) try out.append(allocator, ',');
            try appendQuoted(allocator, &out, proposal.id);
            try out.appendSlice(allocator, ":{\"id\":");
            try appendQuoted(allocator, &out, proposal.id);
            try out.appendSlice(allocator, ",\"actorId\":");
            try appendQuoted(allocator, &out, proposal.actor_id);
            try out.appendSlice(allocator, ",\"modelId\":");
            try appendQuoted(allocator, &out, proposal.model_id);
            try out.appendSlice(allocator, ",\"promptHash\":");
            try appendQuoted(allocator, &out, proposal.prompt_hash);
            try out.appendSlice(allocator, ",\"contextHash\":");
            try appendQuoted(allocator, &out, proposal.context_hash);
            try out.appendSlice(allocator, ",\"affectedCellIds\":");
            try out.appendSlice(allocator, proposal.affected_json);
            try out.appendSlice(allocator, ",\"baseFrontier\":");
            try out.appendSlice(allocator, proposal.base_frontier_json);
            try out.appendSlice(allocator, ",\"operations\":");
            try out.appendSlice(allocator, proposal.operations_json);
            try out.appendSlice(allocator, ",\"status\":");
            try appendQuoted(allocator, &out, proposal.status);
            try out.append(allocator, '}');
        }
        try out.appendSlice(allocator, "},\"approvals\":{");
        for (self.approvals.items, 0..) |approval, index| {
            if (index > 0) try out.append(allocator, ',');
            try appendQuoted(allocator, &out, approval.proposal_id);
            try out.appendSlice(allocator, ":{\"proposalId\":");
            try appendQuoted(allocator, &out, approval.proposal_id);
            try out.appendSlice(allocator, ",\"status\":");
            try appendQuoted(allocator, &out, approval.status);
            try out.appendSlice(allocator, ",\"actorId\":");
            try appendQuoted(allocator, &out, approval.actor_id);
            try out.append(allocator, '}');
        }
        try out.appendSlice(allocator, "}}");
        if (out.items.len > max_notebook_json_bytes) return CrdtError.SchemaError;
        return out.toOwnedSlice(allocator);
    }

    fn reject(self: *Notebook, code: []const u8, message: []const u8) void {
        self.rejected_code = code;
        self.rejected_message = message;
    }
};

fn materializeUpdates(allocator: std.mem.Allocator, original_updates: []const UpdateRecord, limit: ?usize) !Notebook {
    if (original_updates.len > max_updates) return CrdtError.PayloadTooLarge;
    var updates = std.ArrayList(UpdateRecord).empty;
    defer updates.deinit(allocator);
    const bounded = if (limit) |n| @min(n, original_updates.len) else original_updates.len;
    for (original_updates[0..bounded]) |record| {
        try updates.append(allocator, record);
    }
    std.mem.sort(UpdateRecord, updates.items, {}, compareUpdates);

    var notebook = Notebook{};
    errdefer notebook.deinit(allocator);
    for (updates.items) |record| {
        if (notebook.rejected_code != null) break;
        try applyRecord(allocator, &notebook, record);
        notebook.applied_ops += 1;
    }
    return notebook;
}

fn compareUpdates(_: void, a: UpdateRecord, b: UpdateRecord) bool {
    if (a.seq != b.seq) return a.seq < b.seq;
    return std.mem.order(u8, a.op_id, b.op_id) == .lt;
}

fn applyRecord(allocator: std.mem.Allocator, notebook: *Notebook, record: UpdateRecord) !void {
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, record.body_json, .{}) catch {
        notebook.reject("schema_error", "stored operation JSON is invalid");
        return;
    };
    defer parsed.deinit();
    const operation = parsed.value;
    try applyOperationValue(allocator, notebook, operation, record, record.kind);
}

fn applyOperationValue(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value, record: UpdateRecord, kind: OperationKind) anyerror!void {
    switch (kind) {
        .notebook_init => try applyNotebookInit(allocator, notebook, operation, record),
        .batch => try applyBatch(allocator, notebook, operation, record),
        .cell_insert => try applyCellInsert(allocator, notebook, operation, record),
        .cell_delete => applyCellDelete(notebook, operation),
        .cell_move => applyCellMove(notebook, operation),
        .text_insert => try applyTextInsert(allocator, notebook, operation, record),
        .text_delete => applyTextDelete(allocator, notebook, operation, record),
        .text_replace => try applyTextReplace(allocator, notebook, operation, record),
        .metadata_set => try applyMetadataSet(allocator, notebook, operation),
        .metadata_delete => applyMetadataDelete(notebook, operation),
        .output_append => try applyOutputAppend(allocator, notebook, operation),
        .comment_add => try applyCommentAdd(allocator, notebook, operation, record),
        .comment_resolve => try applyCommentResolve(allocator, notebook, operation, record),
        .proposal_create => try applyProposalCreate(allocator, notebook, operation, record),
        .proposal_accept => try applyProposalDecision(allocator, notebook, operation, record, "accepted"),
        .proposal_reject => try applyProposalDecision(allocator, notebook, operation, record, "rejected"),
        .checkpoint_create => try applyCheckpointCreate(allocator, notebook, operation),
    }
}

fn applyNotebookInit(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value, record: UpdateRecord) !void {
    const cells_value = operation.object.get("cells") orelse {
        notebook.reject("schema_error", "notebook.init requires cells array");
        return;
    };
    if (cells_value != .array) {
        notebook.reject("schema_error", "notebook.init requires cells array");
        return;
    }
    if (operation.object.get("metadata")) |metadata| {
        if (metadata == .object) {
            var entries = metadata.object.iterator();
            while (entries.next()) |entry| {
                const value_json = try std.json.Stringify.valueAlloc(allocator, entry.value_ptr.*, .{});
                try setKeyValue(allocator, &notebook.metadata, entry.key_ptr.*, value_json);
            }
        }
    }
    for (cells_value.array.items, 0..) |cell_value, index| {
        if (cell_value != .object) {
            notebook.reject("schema_error", "notebook.init cells must be objects");
            return;
        }
        try insertCellFromObject(allocator, notebook, cell_value, record, index);
        if (notebook.rejected_code != null) return;
    }
}

fn applyBatch(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value, record: UpdateRecord) !void {
    const ops_value = operation.object.get("ops") orelse {
        notebook.reject("schema_error", "batch requires ops array");
        return;
    };
    if (ops_value != .array) {
        notebook.reject("schema_error", "batch requires ops array");
        return;
    }
    for (ops_value.array.items) |child| {
        if (child != .object) {
            notebook.reject("schema_error", "batch child operation must be an object");
            return;
        }
        const type_name = requiredString(child, "type") catch {
            notebook.reject("schema_error", "batch child operation requires type");
            return;
        };
        const child_kind = parseKind(type_name) orelse {
            notebook.reject("invalid_request", "batch child operation type is unsupported");
            return;
        };
        if (child_kind == .batch or child_kind == .proposal_create or child_kind == .proposal_accept or child_kind == .proposal_reject) {
            notebook.reject("schema_error", "nested notebook operations cannot contain batch, proposal, or approval operations");
            return;
        }
        try applyOperationValue(allocator, notebook, child, record, child_kind);
        if (notebook.rejected_code != null) return;
    }
}

fn applyCellInsert(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value, record: UpdateRecord) !void {
    try insertCellFromObject(allocator, notebook, operation, record, optionalInteger(operation, "index") orelse notebook.cells.items.len);
}

fn insertCellFromObject(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value, record: UpdateRecord, index_hint: usize) !void {
    const cell_id = cellIdOrId(operation) catch {
        notebook.reject("schema_error", "cell.insert requires cellId");
        return;
    };
    if (findCell(notebook, cell_id) != null) {
        notebook.reject("conflict_rejected", "cell already exists");
        return;
    }
    const cell_type = optionalString(operation, "cellType") orelse optionalString(operation, "type") orelse "markdown";
    if (!isCellType(cell_type)) {
        notebook.reject("schema_error", "cell type is outside the notebook profile");
        return;
    }
    const source = optionalString(operation, "source") orelse "";
    if (source.len > max_cell_source_bytes) {
        notebook.reject("schema_error", "cell source exceeds notebook text budget");
        return;
    }
    const metadata_json = if (operation.object.get("metadata")) |metadata| try std.json.Stringify.valueAlloc(allocator, metadata, .{}) else try allocator.dupe(u8, "{}");
    var cell = Cell{
        .id = try allocator.dupe(u8, cell_id),
        .cell_type = try allocator.dupe(u8, cell_type),
        .metadata_json = metadata_json,
        .created_by = try allocator.dupe(u8, record.actor_id),
        .updated_by = try allocator.dupe(u8, record.actor_id),
    };
    try cell.source.appendSlice(allocator, source);
    if (operation.object.get("outputs")) |outputs| {
        if (outputs == .array) {
            for (outputs.array.items) |output| {
                try cell.outputs.append(allocator, try std.json.Stringify.valueAlloc(allocator, output, .{}));
            }
        }
    }
    const index = @min(index_hint, notebook.cells.items.len);
    try notebook.cells.insert(allocator, index, cell);
}

fn applyCellDelete(notebook: *Notebook, operation: std.json.Value) void {
    const cell_id = requiredString(operation, "cellId") catch {
        notebook.reject("schema_error", "cell.delete requires cellId");
        return;
    };
    const cell = findCell(notebook, cell_id) orelse {
        notebook.reject("unknown_notebook", "cell.delete references an unknown cell");
        return;
    };
    cell.deleted = true;
}

fn applyCellMove(notebook: *Notebook, operation: std.json.Value) void {
    const cell_id = requiredString(operation, "cellId") catch {
        notebook.reject("schema_error", "cell.move requires cellId");
        return;
    };
    const to = optionalInteger(operation, "index") orelse {
        notebook.reject("schema_error", "cell.move requires index");
        return;
    };
    const from = findCellIndex(notebook, cell_id) orelse {
        notebook.reject("unknown_notebook", "cell.move references an unknown cell");
        return;
    };
    const cell = notebook.cells.orderedRemove(from);
    const bounded = @min(to, notebook.cells.items.len);
    notebook.cells.insertAssumeCapacity(bounded, cell);
}

fn applyTextInsert(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value, record: UpdateRecord) !void {
    const cell = mutableTextCell(notebook, operation) orelse return;
    const text = requiredString(operation, "text") catch {
        notebook.reject("schema_error", "text.insert requires text");
        return;
    };
    const index = @min(optionalInteger(operation, "index") orelse cell.source.items.len, cell.source.items.len);
    try cell.source.insertSlice(allocator, index, text);
    if (operation.object.get("metadata")) |metadata| {
        const merged = try mergeObjectJson(allocator, cell.metadata_json, metadata);
        allocator.free(cell.metadata_json);
        cell.metadata_json = merged;
    }
    if (cell.source.items.len > max_cell_source_bytes) {
        notebook.reject("schema_error", "cell source exceeds notebook text budget");
        return;
    }
    if (!optionalBool(operation, "updatedBy", true)) return;
    allocator.free(cell.updated_by);
    cell.updated_by = try allocator.dupe(u8, record.actor_id);
}

fn applyTextDelete(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value, record: UpdateRecord) void {
    const cell = mutableTextCell(notebook, operation) orelse return;
    const index = optionalInteger(operation, "index") orelse {
        notebook.reject("schema_error", "text.delete requires index");
        return;
    };
    const count = optionalInteger(operation, "count") orelse {
        notebook.reject("schema_error", "text.delete requires count");
        return;
    };
    if (index > cell.source.items.len) {
        notebook.reject("conflict_rejected", "text.delete index is outside source");
        return;
    }
    const end = @min(index + count, cell.source.items.len);
    cell.source.replaceRange(allocator, index, end - index, "") catch {
        notebook.reject("conflict_rejected", "text.delete failed");
        return;
    };
    if (!optionalBool(operation, "updatedBy", true)) return;
    allocator.free(cell.updated_by);
    cell.updated_by = allocator.dupe(u8, record.actor_id) catch return;
}

fn applyTextReplace(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value, record: UpdateRecord) !void {
    const cell = mutableTextCell(notebook, operation) orelse return;
    const text = requiredString(operation, "text") catch {
        notebook.reject("schema_error", "text.replace requires text");
        return;
    };
    if (text.len > max_cell_source_bytes) {
        notebook.reject("schema_error", "cell source exceeds notebook text budget");
        return;
    }
    try cell.source.replaceRange(allocator, 0, cell.source.items.len, text);
    if (!optionalBool(operation, "updatedBy", true)) return;
    allocator.free(cell.updated_by);
    cell.updated_by = try allocator.dupe(u8, record.actor_id);
}

fn applyMetadataSet(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value) !void {
    const key = requiredString(operation, "key") catch {
        notebook.reject("schema_error", "metadata.set requires key");
        return;
    };
    const value = operation.object.get("value") orelse {
        notebook.reject("schema_error", "metadata.set requires value");
        return;
    };
    const value_json = try std.json.Stringify.valueAlloc(allocator, value, .{});
    try setKeyValue(allocator, &notebook.metadata, key, value_json);
}

fn applyMetadataDelete(notebook: *Notebook, operation: std.json.Value) void {
    const key = requiredString(operation, "key") catch {
        notebook.reject("schema_error", "metadata.delete requires key");
        return;
    };
    removeKeyValue(&notebook.metadata, key);
}

fn applyOutputAppend(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value) !void {
    const cell_id = requiredString(operation, "cellId") catch {
        notebook.reject("schema_error", "output.append requires cellId");
        return;
    };
    const cell = findCell(notebook, cell_id) orelse {
        notebook.reject("unknown_notebook", "output.append references an unknown cell");
        return;
    };
    const value = operation.object.get("output") orelse {
        notebook.reject("schema_error", "output.append requires output");
        return;
    };
    try cell.outputs.append(allocator, try std.json.Stringify.valueAlloc(allocator, value, .{}));
}

fn applyCommentAdd(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value, record: UpdateRecord) !void {
    const comment_id = requiredString(operation, "commentId") catch {
        notebook.reject("schema_error", "comment.add requires commentId");
        return;
    };
    const cell_id = requiredString(operation, "cellId") catch {
        notebook.reject("schema_error", "comment.add requires cellId");
        return;
    };
    if (findCell(notebook, cell_id) == null) {
        notebook.reject("unknown_notebook", "comment.add references an unknown cell");
        return;
    }
    if (findComment(notebook, comment_id) != null) {
        notebook.reject("conflict_rejected", "comment already exists");
        return;
    }
    try notebook.comments.append(allocator, .{
        .id = try allocator.dupe(u8, comment_id),
        .cell_id = try allocator.dupe(u8, cell_id),
        .body = try allocator.dupe(u8, requiredString(operation, "body") catch ""),
        .created_by = try allocator.dupe(u8, record.actor_id),
    });
}

fn applyCommentResolve(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value, record: UpdateRecord) !void {
    const comment_id = requiredString(operation, "commentId") catch {
        notebook.reject("schema_error", "comment.resolve requires commentId");
        return;
    };
    const comment = findComment(notebook, comment_id) orelse {
        notebook.reject("unknown_notebook", "comment.resolve references an unknown comment");
        return;
    };
    comment.resolved = true;
    if (comment.resolved_by) |old| allocator.free(old);
    comment.resolved_by = try allocator.dupe(u8, record.actor_id);
}

fn applyProposalCreate(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value, record: UpdateRecord) !void {
    const proposal_id = requiredString(operation, "proposalId") catch {
        notebook.reject("schema_error", "proposal.create requires proposalId");
        return;
    };
    if (findProposal(notebook, proposal_id) != null) {
        notebook.reject("conflict_rejected", "proposal already exists");
        return;
    }
    const operations = operation.object.get("operations") orelse {
        notebook.reject("schema_error", "proposal.create requires operations");
        return;
    };
    if (operations != .array) {
        notebook.reject("schema_error", "proposal operations must be an array");
        return;
    }
    const affected_json = if (operation.object.get("affectedCellIds")) |affected|
        try std.json.Stringify.valueAlloc(allocator, affected, .{})
    else
        try allocator.dupe(u8, "[]");
    const base_frontier_json = if (operation.object.get("baseFrontier")) |base_frontier|
        try std.json.Stringify.valueAlloc(allocator, base_frontier, .{})
    else
        try allocator.dupe(u8, "[]");
    try notebook.proposals.append(allocator, .{
        .id = try allocator.dupe(u8, proposal_id),
        .actor_id = try allocator.dupe(u8, record.actor_id),
        .model_id = try allocator.dupe(u8, requiredString(operation, "modelId") catch ""),
        .prompt_hash = try allocator.dupe(u8, optionalString(operation, "promptHash") orelse optionalString(operation, "promptContextHash") orelse ""),
        .context_hash = try allocator.dupe(u8, optionalString(operation, "contextHash") orelse optionalString(operation, "promptContextHash") orelse ""),
        .status = try allocator.dupe(u8, "pending"),
        .affected_json = affected_json,
        .operations_json = try std.json.Stringify.valueAlloc(allocator, operations, .{}),
        .base_frontier_json = base_frontier_json,
    });
}

fn applyProposalDecision(
    allocator: std.mem.Allocator,
    notebook: *Notebook,
    operation: std.json.Value,
    record: UpdateRecord,
    status: []const u8,
) !void {
    const proposal_id = requiredString(operation, "proposalId") catch {
        notebook.reject("schema_error", "proposal decision requires proposalId");
        return;
    };
    const proposal = findProposal(notebook, proposal_id) orelse {
        notebook.reject("unknown_notebook", "proposal decision references an unknown proposal");
        return;
    };
    allocator.free(proposal.status);
    proposal.status = try allocator.dupe(u8, status);
    try notebook.approvals.append(allocator, .{
        .proposal_id = try allocator.dupe(u8, proposal_id),
        .status = try allocator.dupe(u8, status),
        .actor_id = try allocator.dupe(u8, record.actor_id),
    });
    if (std.mem.eql(u8, status, "accepted")) {
        var parsed_ops = std.json.parseFromSlice(std.json.Value, allocator, proposal.operations_json, .{}) catch {
            notebook.reject("schema_error", "accepted proposal operations JSON is invalid");
            return;
        };
        defer parsed_ops.deinit();
        if (parsed_ops.value != .array) {
            notebook.reject("schema_error", "accepted proposal operations must be an array");
            return;
        }
        for (parsed_ops.value.array.items) |child| {
            if (child != .object) {
                notebook.reject("schema_error", "accepted proposal child operation must be an object");
                return;
            }
            const type_name = requiredString(child, "type") catch {
                notebook.reject("schema_error", "accepted proposal child operation requires type");
                return;
            };
            const child_kind = parseKind(type_name) orelse {
                notebook.reject("invalid_request", "accepted proposal child operation type is unsupported");
                return;
            };
            if (child_kind == .batch or child_kind == .proposal_create or child_kind == .proposal_accept or child_kind == .proposal_reject) {
                notebook.reject("schema_error", "accepted proposal cannot contain nested proposal operations");
                return;
            }
            try applyOperationValue(allocator, notebook, child, record, child_kind);
            if (notebook.rejected_code != null) return;
        }
    }
}

fn applyCheckpointCreate(allocator: std.mem.Allocator, notebook: *Notebook, operation: std.json.Value) !void {
    const checkpoint_id = requiredString(operation, "checkpointId") catch {
        notebook.reject("schema_error", "checkpoint.create requires checkpointId");
        return;
    };
    const frontier_json = if (operation.object.get("frontier")) |frontier|
        try std.json.Stringify.valueAlloc(allocator, frontier, .{})
    else
        try allocator.dupe(u8, "[]");
    try setKeyValue(allocator, &notebook.checkpoints, checkpoint_id, frontier_json);
}

fn mutableTextCell(notebook: *Notebook, operation: std.json.Value) ?*Cell {
    const cell_id = requiredString(operation, "cellId") catch {
        notebook.reject("schema_error", "text operation requires cellId");
        return null;
    };
    const cell = findCell(notebook, cell_id) orelse {
        notebook.reject("unknown_notebook", "text operation references an unknown cell");
        return null;
    };
    if (!std.mem.eql(u8, cell.cell_type, "markdown") and !std.mem.eql(u8, cell.cell_type, "prompt") and !std.mem.eql(u8, cell.cell_type, "code")) {
        notebook.reject("schema_error", "collaborative text is only supported on markdown, prompt, and code cells");
        return null;
    }
    return cell;
}

fn findCell(notebook: *Notebook, cell_id: []const u8) ?*Cell {
    for (notebook.cells.items) |*cell| {
        if (!cell.deleted and std.mem.eql(u8, cell.id, cell_id)) return cell;
    }
    return null;
}

fn findCellIndex(notebook: *Notebook, cell_id: []const u8) ?usize {
    for (notebook.cells.items, 0..) |cell, index| {
        if (!cell.deleted and std.mem.eql(u8, cell.id, cell_id)) return index;
    }
    return null;
}

fn findComment(notebook: *Notebook, comment_id: []const u8) ?*Comment {
    for (notebook.comments.items) |*comment| {
        if (std.mem.eql(u8, comment.id, comment_id)) return comment;
    }
    return null;
}

fn findProposal(notebook: *Notebook, proposal_id: []const u8) ?*Proposal {
    for (notebook.proposals.items) |*proposal| {
        if (std.mem.eql(u8, proposal.id, proposal_id)) return proposal;
    }
    return null;
}

fn setKeyValue(allocator: std.mem.Allocator, list: *std.ArrayList(KeyValue), key: []const u8, value_json: []u8) !void {
    for (list.items) |*item| {
        if (std.mem.eql(u8, item.key, key)) {
            allocator.free(item.value_json);
            item.value_json = value_json;
            return;
        }
    }
    try list.append(allocator, .{
        .key = try allocator.dupe(u8, key),
        .value_json = value_json,
    });
    std.mem.sort(KeyValue, list.items, {}, compareKeyValues);
}

fn removeKeyValue(list: *std.ArrayList(KeyValue), key: []const u8) void {
    for (list.items, 0..) |item, index| {
        if (std.mem.eql(u8, item.key, key)) {
            _ = list.orderedRemove(index);
            return;
        }
    }
}

fn compareKeyValues(_: void, a: KeyValue, b: KeyValue) bool {
    return std.mem.order(u8, a.key, b.key) == .lt;
}

fn assertOperationPermission(context: TrustedContext, kind: OperationKind) !void {
    const required: []const u8 = switch (kind) {
        .proposal_create => "notebook.propose",
        .proposal_accept, .proposal_reject => "notebook.approve",
        else => "notebook.write",
    };
    if (!context.hasPermission(required)) return CrdtError.PermissionDenied;
}

fn parseKind(value: []const u8) ?OperationKind {
    const names = .{
        .{ "notebook.init", OperationKind.notebook_init },
        .{ "batch", OperationKind.batch },
        .{ "cell.insert", OperationKind.cell_insert },
        .{ "cell.delete", OperationKind.cell_delete },
        .{ "cell.move", OperationKind.cell_move },
        .{ "text.insert", OperationKind.text_insert },
        .{ "text.delete", OperationKind.text_delete },
        .{ "text.replace", OperationKind.text_replace },
        .{ "metadata.set", OperationKind.metadata_set },
        .{ "metadata.delete", OperationKind.metadata_delete },
        .{ "output.append", OperationKind.output_append },
        .{ "comment.add", OperationKind.comment_add },
        .{ "comment.resolve", OperationKind.comment_resolve },
        .{ "proposal.create", OperationKind.proposal_create },
        .{ "proposal.accept", OperationKind.proposal_accept },
        .{ "proposal.reject", OperationKind.proposal_reject },
        .{ "checkpoint.create", OperationKind.checkpoint_create },
    };
    inline for (names) |entry| {
        if (std.mem.eql(u8, value, entry[0])) return entry[1];
    }
    return null;
}

fn isCellType(value: []const u8) bool {
    return std.mem.eql(u8, value, "markdown") or
        std.mem.eql(u8, value, "code") or
        std.mem.eql(u8, value, "output") or
        std.mem.eql(u8, value, "artifact") or
        std.mem.eql(u8, value, "prompt");
}

fn parseInput(allocator: std.mem.Allocator, input: []const u8) !std.json.Parsed(std.json.Value) {
    if (input.len > max_input_bytes) return CrdtError.PayloadTooLarge;
    if (input.len == 0) {
        return std.json.parseFromSlice(std.json.Value, allocator, "{}", .{});
    }
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, input, .{}) catch return CrdtError.InvalidJson;
    if (parsed.value != .object) {
        parsed.deinit();
        return CrdtError.InvalidRequest;
    }
    return parsed;
}

fn requiredString(value: std.json.Value, field: []const u8) ![]const u8 {
    if (value != .object) return CrdtError.InvalidRequest;
    const child = value.object.get(field) orelse return CrdtError.InvalidRequest;
    if (child != .string) return CrdtError.InvalidRequest;
    return child.string;
}

fn optionalString(value: std.json.Value, field: []const u8) ?[]const u8 {
    if (value != .object) return null;
    const child = value.object.get(field) orelse return null;
    if (child != .string) return null;
    return child.string;
}

fn optionalInteger(value: std.json.Value, field: []const u8) ?usize {
    if (value != .object) return null;
    const child = value.object.get(field) orelse return null;
    if (child == .integer and child.integer >= 0) return @intCast(child.integer);
    return null;
}

fn optionalBool(value: std.json.Value, field: []const u8, default_value: bool) bool {
    if (value != .object) return default_value;
    const child = value.object.get(field) orelse return default_value;
    if (child == .bool) return child.bool;
    return default_value;
}

fn cellIdOrId(value: std.json.Value) ![]const u8 {
    if (optionalString(value, "cellId")) |cell_id| return cell_id;
    if (optionalString(value, "id")) |id_value| return id_value;
    return CrdtError.InvalidRequest;
}

fn mergeObjectJson(allocator: std.mem.Allocator, existing_json: []const u8, patch: std.json.Value) ![]u8 {
    if (patch != .object) return allocator.dupe(u8, existing_json);
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, existing_json, .{}) catch
        return std.json.Stringify.valueAlloc(allocator, patch, .{});
    defer parsed.deinit();
    if (parsed.value != .object) return std.json.Stringify.valueAlloc(allocator, patch, .{});

    var out: std.ArrayList(u8) = .empty;
    errdefer out.deinit(allocator);
    try out.append(allocator, '{');
    var count: usize = 0;

    var existing = parsed.value.object.iterator();
    while (existing.next()) |entry| {
        if (patch.object.get(entry.key_ptr.*) != null) continue;
        if (count > 0) try out.append(allocator, ',');
        try appendQuoted(allocator, &out, entry.key_ptr.*);
        try out.append(allocator, ':');
        const value_json = try std.json.Stringify.valueAlloc(allocator, entry.value_ptr.*, .{});
        defer allocator.free(value_json);
        try out.appendSlice(allocator, value_json);
        count += 1;
    }

    var additions = patch.object.iterator();
    while (additions.next()) |entry| {
        if (count > 0) try out.append(allocator, ',');
        try appendQuoted(allocator, &out, entry.key_ptr.*);
        try out.append(allocator, ':');
        const value_json = try std.json.Stringify.valueAlloc(allocator, entry.value_ptr.*, .{});
        defer allocator.free(value_json);
        try out.appendSlice(allocator, value_json);
        count += 1;
    }

    try out.append(allocator, '}');
    return out.toOwnedSlice(allocator);
}

fn cloneRecordsWith(allocator: std.mem.Allocator, records: []const UpdateRecord, source: OperationEnvelope) !std.ArrayList(UpdateRecord) {
    var out = std.ArrayList(UpdateRecord).empty;
    errdefer freeRecordList(allocator, &out);
    for (records) |record| {
        try out.append(allocator, try record.clone(allocator));
    }
    try out.append(allocator, try UpdateRecord.fromEnvelope(allocator, source));
    return out;
}

fn freeRecordList(allocator: std.mem.Allocator, records: *std.ArrayList(UpdateRecord)) void {
    for (records.items) |*record| record.deinit(allocator);
    records.deinit(allocator);
}

fn notebookResultJson(allocator: std.mem.Allocator, status: []const u8, op_id: ?[]const u8, notebook: Notebook) ![]u8 {
    const snapshot = try notebook.toJson(allocator);
    defer allocator.free(snapshot);
    const frontier = try frontierJson(allocator, notebook.applied_ops);
    defer allocator.free(frontier);
    const escaped_status = try escapeJsonString(allocator, status);
    defer allocator.free(escaped_status);
    if (op_id) |id_value| {
        const escaped_op = try escapeJsonString(allocator, id_value);
        defer allocator.free(escaped_op);
        return std.fmt.allocPrint(
            allocator,
            "{{\"ok\":true,\"status\":\"{s}\",\"opId\":\"{s}\",\"frontier\":{s},\"notebook\":{s}}}",
            .{ escaped_status, escaped_op, frontier, snapshot },
        );
    }
    return std.fmt.allocPrint(
        allocator,
        "{{\"ok\":true,\"status\":\"{s}\",\"frontier\":{s},\"notebook\":{s}}}",
        .{ escaped_status, frontier, snapshot },
    );
}

fn errorResultJson(allocator: std.mem.Allocator, code: []const u8, message: []const u8) ![]u8 {
    const escaped_code = try escapeJsonString(allocator, code);
    defer allocator.free(escaped_code);
    const escaped_message = try escapeJsonString(allocator, message);
    defer allocator.free(escaped_message);
    return std.fmt.allocPrint(
        allocator,
        "{{\"ok\":false,\"error\":{{\"code\":\"{s}\",\"message\":\"{s}\"}}}}",
        .{ escaped_code, escaped_message },
    );
}

fn frontierJson(allocator: std.mem.Allocator, version: usize) ![]u8 {
    return std.fmt.allocPrint(allocator, "{{\"version\":{},\"heads\":[]}}", .{version});
}

fn appendQuoted(allocator: std.mem.Allocator, out: *std.ArrayList(u8), value: []const u8) !void {
    const escaped = try escapeJsonString(allocator, value);
    defer allocator.free(escaped);
    try out.append(allocator, '"');
    try out.appendSlice(allocator, escaped);
    try out.append(allocator, '"');
}

fn escapeJsonString(allocator: std.mem.Allocator, value: []const u8) ![]u8 {
    var out: std.ArrayList(u8) = .empty;
    for (value) |char| {
        switch (char) {
            '"' => try out.appendSlice(allocator, "\\\""),
            '\\' => try out.appendSlice(allocator, "\\\\"),
            '\n' => try out.appendSlice(allocator, "\\n"),
            '\r' => try out.appendSlice(allocator, "\\r"),
            '\t' => try out.appendSlice(allocator, "\\t"),
            0x08 => try out.appendSlice(allocator, "\\b"),
            0x0c => try out.appendSlice(allocator, "\\f"),
            else => {
                if (char < 0x20) {
                    try out.writer(allocator).print("\\u{x:0>4}", .{char});
                } else {
                    try out.append(allocator, char);
                }
            },
        }
    }
    return out.toOwnedSlice(allocator);
}

pub export fn crdt_create() ?*ZigCrdt {
    const crdt = ffi_allocator.create(ZigCrdt) catch return null;
    crdt.* = .{};
    return crdt;
}

pub export fn crdt_destroy(crdt: ?*ZigCrdt) void {
    if (crdt) |ptr| {
        ptr.deinit(ffi_allocator);
        ffi_allocator.destroy(ptr);
    }
}

pub export fn crdt_apply_json(crdt: ?*ZigCrdt, input_ptr: ?[*]const u8, input_len: usize, output: ?*ZigCrdtBuffer) i32 {
    return callCrdt(crdt, input_ptr, input_len, output, .apply);
}

pub export fn crdt_merge_json(crdt: ?*ZigCrdt, input_ptr: ?[*]const u8, input_len: usize, output: ?*ZigCrdtBuffer) i32 {
    return callCrdt(crdt, input_ptr, input_len, output, .merge);
}

pub export fn crdt_materialize_json(crdt: ?*ZigCrdt, input_ptr: ?[*]const u8, input_len: usize, output: ?*ZigCrdtBuffer) i32 {
    return callCrdt(crdt, input_ptr, input_len, output, .materialize);
}

pub export fn crdt_free(buffer: ZigCrdtBuffer) void {
    if (buffer.len == 0) return;
    ffi_allocator.free(buffer.ptr[0..buffer.len]);
}

const CCall = enum { apply, merge, materialize };

fn callCrdt(crdt: ?*ZigCrdt, input_ptr: ?[*]const u8, input_len: usize, output: ?*ZigCrdtBuffer, mode: CCall) i32 {
    if (crdt == null or input_ptr == null or output == null) return -1;
    const input = input_ptr.?[0..input_len];
    const bytes = switch (mode) {
        .apply => crdt.?.applyJson(ffi_allocator, input),
        .merge => crdt.?.mergeJson(ffi_allocator, input),
        .materialize => crdt.?.materializeJson(ffi_allocator, input),
    } catch |err| {
        const code = codeForError(err);
        const message = messageForError(err);
        const bytes = errorResultJson(ffi_allocator, code, message) catch return -2;
        output.?.* = .{ .ptr = bytes.ptr, .len = bytes.len };
        return 0;
    };
    output.?.* = .{ .ptr = bytes.ptr, .len = bytes.len };
    return 0;
}

fn codeForError(err: anyerror) []const u8 {
    return switch (err) {
        CrdtError.PayloadTooLarge => "resource_budget_exceeded",
        CrdtError.InvalidJson, CrdtError.InvalidRequest => "invalid_request",
        CrdtError.PermissionDenied => "permission_denied",
        CrdtError.SchemaError => "schema_error",
        CrdtError.ConflictRejected => "conflict_rejected",
        CrdtError.StaleFrontier => "stale_frontier",
        CrdtError.UnknownNotebook => "unknown_notebook",
        CrdtError.SyncUnavailable => "sync_unavailable",
        else => "internal_error",
    };
}

fn messageForError(err: anyerror) []const u8 {
    return switch (err) {
        CrdtError.PayloadTooLarge => "CRDT payload exceeds the notebook budget",
        CrdtError.InvalidJson => "CRDT input must be valid JSON",
        CrdtError.InvalidRequest => "CRDT input envelope is invalid",
        CrdtError.PermissionDenied => "CRDT operation is not permitted for actor",
        CrdtError.SchemaError => "CRDT operation violates the notebook schema",
        CrdtError.ConflictRejected => "CRDT operation conflicts with notebook state",
        CrdtError.StaleFrontier => "CRDT operation uses a stale frontier",
        CrdtError.UnknownNotebook => "CRDT notebook or object is unknown",
        CrdtError.SyncUnavailable => "CRDT sync is unavailable",
        else => "CRDT operation failed",
    };
}

fn androidGetAuxVal(key: usize) callconv(.c) usize {
    const at_pagesz = 6;
    if (key == at_pagesz) return 4096;
    return 0;
}

test "applies notebook cell and text operations" {
    var crdt = ZigCrdt{};
    defer crdt.deinit(std.testing.allocator);
    const insert = try makeEnvelope("op_001", "cell.insert", "\"cellId\":\"cell_intro\",\"cellType\":\"markdown\",\"source\":\"Hello\"");
    defer std.testing.allocator.free(insert);
    const result = try crdt.applyJson(std.testing.allocator, insert);
    defer std.testing.allocator.free(result);
    try std.testing.expect(std.mem.indexOf(u8, result, "\"source\":\"Hello\"") != null);

    const text = try makeEnvelope("op_002", "text.insert", "\"cellId\":\"cell_intro\",\"index\":5,\"text\":\", world\"");
    defer std.testing.allocator.free(text);
    const text_result = try crdt.applyJson(std.testing.allocator, text);
    defer std.testing.allocator.free(text_result);
    try std.testing.expect(std.mem.indexOf(u8, text_result, "\"source\":\"Hello, world\"") != null);
}

test "rejects AI canonical writes without notebook.write" {
    var crdt = ZigCrdt{};
    defer crdt.deinit(std.testing.allocator);
    const input = try std.fmt.allocPrint(
        std.testing.allocator,
        "{{\"context\":{{\"appId\":\"app\",\"notebookId\":\"nb\",\"actorId\":\"ai_1\",\"actorKind\":\"ai\",\"permissions\":[\"notebook.propose\"]}},\"operation\":{{\"opId\":\"op_ai\",\"type\":\"cell.insert\",\"cellId\":\"cell_ai\"}}}}",
        .{},
    );
    defer std.testing.allocator.free(input);
    const result = try crdt.applyJson(std.testing.allocator, input);
    defer std.testing.allocator.free(result);
    try std.testing.expect(std.mem.indexOf(u8, result, "\"code\":\"permission_denied\"") != null);
}

test "duplicate operations are idempotent" {
    var crdt = ZigCrdt{};
    defer crdt.deinit(std.testing.allocator);
    const input = try makeEnvelope("op_dup", "metadata.set", "\"key\":\"title\",\"value\":\"Notebook\"");
    defer std.testing.allocator.free(input);
    const first = try crdt.applyJson(std.testing.allocator, input);
    defer std.testing.allocator.free(first);
    const second = try crdt.applyJson(std.testing.allocator, input);
    defer std.testing.allocator.free(second);
    try std.testing.expect(std.mem.indexOf(u8, second, "\"status\":\"duplicate\"") != null);
    try std.testing.expectEqual(@as(usize, 1), crdt.updates.items.len);
}

test "out of order merge converges by sequence and op id" {
    var crdt = ZigCrdt{};
    defer crdt.deinit(std.testing.allocator);
    const input =
        \\{"context":{"appId":"app","notebookId":"nb","actorId":"actor_a","actorKind":"human","permissions":["notebook.write","notebook.propose","notebook.approve"]},"updates":[
        \\{"operation":{"opId":"op_002","seq":2,"type":"text.insert","cellId":"cell_a","index":1,"text":"B"}},
        \\{"operation":{"opId":"op_001","seq":1,"type":"cell.insert","cellId":"cell_a","cellType":"markdown","source":"A"}}
        \\]}
    ;
    const result = try crdt.mergeJson(std.testing.allocator, input);
    defer std.testing.allocator.free(result);
    try std.testing.expect(std.mem.indexOf(u8, result, "\"source\":\"AB\"") != null);
}

test "fixture notebook profile supports init batch outputs and proposal acceptance" {
    var crdt = ZigCrdt{};
    defer crdt.deinit(std.testing.allocator);

    const seed =
        \\{"context":{"appId":"notebook-app","notebookId":"nb","actorId":"actor_seed","actorKind":"system","permissions":["notebook.write","notebook.propose","notebook.approve"]},"operation":{"opId":"op_seed","seq":1,"type":"notebook.init","metadata":{"title":"Fixture"},"cells":[{"id":"cell_prompt","type":"prompt","source":"Summarize risks.","metadata":{},"outputs":[]},{"id":"cell_code","type":"code","source":"print(\"start\")","metadata":{},"outputs":[]}]}}
    ;
    const seed_result = try crdt.applyJson(std.testing.allocator, seed);
    defer std.testing.allocator.free(seed_result);
    try std.testing.expect(std.mem.indexOf(u8, seed_result, "\"title\":\"Fixture\"") != null);

    const batch =
        \\{"context":{"appId":"notebook-app","notebookId":"nb","actorId":"actor_alice","actorKind":"human","permissions":["notebook.write"]},"operation":{"opId":"op_batch","seq":2,"type":"batch","ops":[{"type":"text.insert","cellId":"cell_prompt","index":"end","text":" Now.","metadata":{"status":"draft"}},{"type":"cell.move","cellId":"cell_code","index":0},{"type":"output.append","cellId":"cell_code","output":{"id":"output_1","type":"stream","mime":"text/plain","text":"start\n","createdBy":"actor_alice"}}]}}
    ;
    const batch_result = try crdt.applyJson(std.testing.allocator, batch);
    defer std.testing.allocator.free(batch_result);
    try std.testing.expect(std.mem.indexOf(u8, batch_result, "\"status\":\"draft\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, batch_result, "\"output_1\"") != null);

    const proposal =
        \\{"context":{"appId":"notebook-app","notebookId":"nb","actorId":"actor_ai","actorKind":"ai","permissions":["notebook.propose"]},"operation":{"opId":"op_propose","seq":3,"type":"proposal.create","proposalId":"proposal_1","modelId":"glm-4.5","promptContextHash":"sha256:context","affectedCellIds":["cell_prompt"],"operations":[{"type":"text.replace","cellId":"cell_prompt","text":"Accepted proposal text."}]}}
    ;
    const proposal_result = try crdt.applyJson(std.testing.allocator, proposal);
    defer std.testing.allocator.free(proposal_result);
    try std.testing.expect(std.mem.indexOf(u8, proposal_result, "\"status\":\"pending\"") != null);

    const accept =
        \\{"context":{"appId":"notebook-app","notebookId":"nb","actorId":"actor_reviewer","actorKind":"human","permissions":["notebook.approve"]},"operation":{"opId":"op_accept","seq":4,"type":"proposal.accept","proposalId":"proposal_1"}}
    ;
    const accept_result = try crdt.applyJson(std.testing.allocator, accept);
    defer std.testing.allocator.free(accept_result);
    try std.testing.expect(std.mem.indexOf(u8, accept_result, "\"status\":\"accepted\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, accept_result, "\"source\":\"Accepted proposal text.\"") != null);
}

test "C ABI allocates and frees an output buffer" {
    const crdt = crdt_create().?;
    defer crdt_destroy(crdt);
    const input = "{\"frontier\":{\"version\":0}}";
    var output: ZigCrdtBuffer = undefined;
    try std.testing.expectEqual(@as(i32, 0), crdt_materialize_json(crdt, input.ptr, input.len, &output));
    try std.testing.expect(output.len > 0);
    crdt_free(output);
}

fn makeEnvelope(op_id: []const u8, kind: []const u8, fields: []const u8) ![]u8 {
    return std.fmt.allocPrint(
        std.testing.allocator,
        "{{\"context\":{{\"appId\":\"app\",\"notebookId\":\"nb\",\"actorId\":\"actor_a\",\"actorKind\":\"human\",\"permissions\":[\"notebook.write\",\"notebook.propose\",\"notebook.approve\"]}},\"operation\":{{\"opId\":\"{s}\",\"type\":\"{s}\",{s}}}}}",
        .{ op_id, kind, fields },
    );
}
