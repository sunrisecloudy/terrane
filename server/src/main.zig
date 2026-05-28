const std = @import("std");
const core_api = @import("zig_core");
const sqlite = @cImport({
    @cInclude("sqlite3.h");
});

const max_request_bytes = 1024 * 1024;
const runtime_version = "0.1.0";

pub fn main() !void {
    const allocator = std.heap.page_allocator;
    const port = try parsePort(allocator);
    const address = try std.net.Address.parseIp("127.0.0.1", port);
    var server = try address.listen(.{ .reuse_address = true });
    defer server.deinit();

    std.debug.print("native-ai zig server listening on http://127.0.0.1:{d}\n", .{port});

    while (true) {
        var connection = try server.accept();
        defer connection.stream.close();
        handleConnection(allocator, connection.stream) catch |err| {
            std.debug.print("server connection error: {}\n", .{err});
        };
    }
}

fn parsePort(allocator: std.mem.Allocator) !u16 {
    const args = try std.process.argsAlloc(allocator);
    defer std.process.argsFree(allocator, args);

    var index: usize = 1;
    while (index < args.len) : (index += 1) {
        if (std.mem.eql(u8, args[index], "--port")) {
            if (index + 1 >= args.len) return error.MissingPortValue;
            return try std.fmt.parseInt(u16, args[index + 1], 10);
        }
    }

    return 8088;
}

fn handleConnection(allocator: std.mem.Allocator, stream: std.net.Stream) !void {
    var buffer: [max_request_bytes + 4096]u8 = undefined;
    const read_len = try stream.read(&buffer);
    if (read_len == 0) return;
    const request = buffer[0..read_len];

    const parsed = parseRequest(request) catch {
        return writeJson(stream, 400, "{\"ok\":false,\"error\":{\"code\":\"invalid_request\",\"message\":\"Malformed HTTP request\",\"details\":{}}}");
    };

    if (std.mem.eql(u8, parsed.method, "GET") and std.mem.eql(u8, parsed.path, "/health")) {
        return writeJson(stream, 200, "{\"ok\":true,\"version\":\"0.1.0\",\"target\":\"zig-server\"}");
    }

    if (std.mem.eql(u8, parsed.method, "POST") and std.mem.eql(u8, parsed.path, "/core/step")) {
        return handleCoreStep(allocator, stream, parsed.body);
    }

    if (std.mem.eql(u8, parsed.method, "POST") and std.mem.eql(u8, parsed.path, "/bridge")) {
        return handleBridge(allocator, stream, parsed.body, parsed.app_id, parsed.mount_token, parsed.session_id);
    }

    if (std.mem.eql(u8, parsed.method, "POST") and std.mem.eql(u8, parsed.path, "/webapps/validate")) {
        return handleWebappValidate(allocator, stream, parsed.body);
    }

    if (std.mem.eql(u8, parsed.method, "POST") and std.mem.eql(u8, parsed.path, "/control/command")) {
        return handleControlCommand(allocator, stream, parsed.body, parsed.control_token);
    }

    if (std.mem.eql(u8, parsed.method, "POST") and
        (std.mem.startsWith(u8, parsed.path, "/db/") or std.mem.startsWith(u8, parsed.path, "/control/db/")))
    {
        return handleDbControlEndpoint(allocator, stream, parsed.path, parsed.body, parsed.control_token);
    }

    if (std.mem.eql(u8, parsed.method, "GET") and std.mem.eql(u8, parsed.path, "/webapps/examples")) {
        return writeJson(stream, 200, "{\"ok\":true,\"examples\":[\"notes-lite\",\"task-workbench\",\"file-transformer\",\"api-dashboard\",\"core-replay-lab\"]}");
    }

    if (std.mem.eql(u8, parsed.method, "GET") and std.mem.eql(u8, parsed.path, "/webapps/examples.json")) {
        return writeJson(stream, 200, "{\"ok\":true,\"examples\":[{\"id\":\"api-dashboard\",\"name\":\"API Dashboard\"},{\"id\":\"core-replay-lab\",\"name\":\"Core Replay Lab\"},{\"id\":\"file-transformer\",\"name\":\"File Transformer\"},{\"id\":\"notes-lite\",\"name\":\"Notes Lite\"},{\"id\":\"task-workbench\",\"name\":\"Task Workbench\"}]}");
    }

    const examples_prefix = "/webapps/examples/";
    if (std.mem.eql(u8, parsed.method, "GET") and std.mem.startsWith(u8, parsed.path, examples_prefix)) {
        return handleExampleAsset(allocator, stream, parsed.path[examples_prefix.len..]);
    }

    return writeJson(stream, 404, "{\"ok\":false,\"error\":{\"code\":\"not_found\",\"message\":\"Route not found\",\"details\":{}}}");
}

fn handleCoreStep(allocator: std.mem.Allocator, stream: std.net.Stream, body: []const u8) !void {
    const output = coreStepAlloc(allocator, body) catch {
        return writeJson(stream, 500, "{\"ok\":false,\"error\":{\"code\":\"core_step_failed\",\"message\":\"core_step_json failed\",\"details\":{}}}");
    };
    defer allocator.free(output);

    return writeJson(stream, 200, output);
}

fn handleBridge(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    body: []const u8,
    app_id: ?[]const u8,
    mount_token: ?[]const u8,
    session_id: ?[]const u8,
) !void {
    const channel_app_id = app_id orelse {
        return writeBridgeError(allocator, stream, "unknown", "bridge.unauthorized_channel", "Bridge calls require a channel-derived app id");
    };
    _ = mount_token orelse {
        return writeBridgeError(allocator, stream, "unknown", "bridge.unauthorized_channel", "Bridge calls require a channel-derived mount token");
    };

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, body, .{}) catch {
        return writeBridgeError(allocator, stream, "unknown", "invalid_request", "Bridge request body must be valid JSON");
    };
    defer parsed.deinit();

    const root = parsed.value;
    if (root != .object) {
        return writeBridgeError(allocator, stream, "unknown", "invalid_request", "Bridge request body must be an object");
    }

    validateBridgeRequest(root) catch |err| {
        return writeBridgeError(allocator, stream, bridgeRequestIdOrUnknown(root), "invalid_request", bridgeValidationMessage(err));
    };
    const id = valueString(root.object.get("id")).?;
    const method = valueString(root.object.get("method")).?;
    const params = root.object.get("params").?;

    if (std.mem.eql(u8, method, "core.step")) {
        if (valueString(params.object.get("app"))) |requested_app| {
            if (!std.mem.eql(u8, requested_app, channel_app_id)) {
                return writeBridgeError(allocator, stream, id, "permission_denied", "core.step app field does not match the channel-derived app id");
            }
        }

        const params_json = try jsonValueAlloc(allocator, params);
        defer allocator.free(params_json);
        const result_json = coreStepAlloc(allocator, params_json) catch {
            return writeBridgeError(allocator, stream, id, "core_error", "core.step failed");
        };
        defer allocator.free(result_json);
        return writeBridgeOkRaw(allocator, stream, id, result_json);
    }

    if (std.mem.eql(u8, method, "runtime.capabilities")) {
        const result_json = try serverCapabilitiesJson(allocator);
        defer allocator.free(result_json);
        return writeBridgeOkRaw(allocator, stream, id, result_json);
    }

    if (std.mem.startsWith(u8, method, "storage.")) {
        return handleStorageBridge(allocator, stream, id, method, params, channel_app_id);
    }

    if (std.mem.eql(u8, method, "app.log")) {
        return handleAppLogBridge(allocator, stream, id, params, channel_app_id, session_id);
    }

    if (isKnownUnsupportedBridgeMethod(method)) {
        return writeBridgeError(allocator, stream, id, "platform_unsupported", "Bridge method is not implemented on zig-server");
    }

    return writeBridgeError(allocator, stream, id, "unknown_method", "Unknown bridge method");
}

const BridgeValidationError = error{
    UnknownTopLevelField,
    InvalidId,
    InvalidMethod,
    InvalidParams,
    InvalidTimestamp,
};

fn validateBridgeRequest(root: std.json.Value) BridgeValidationError!void {
    var iterator = root.object.iterator();
    while (iterator.next()) |entry| {
        const key = entry.key_ptr.*;
        if (std.mem.eql(u8, key, "id") or
            std.mem.eql(u8, key, "method") or
            std.mem.eql(u8, key, "params") or
            std.mem.eql(u8, key, "timestamp"))
        {
            continue;
        }
        return error.UnknownTopLevelField;
    }

    const id = root.object.get("id") orelse return error.InvalidId;
    if (id != .string or id.string.len == 0) return error.InvalidId;

    const method = root.object.get("method") orelse return error.InvalidMethod;
    if (method != .string) return error.InvalidMethod;

    const params = root.object.get("params") orelse return error.InvalidParams;
    if (params != .object) return error.InvalidParams;

    if (root.object.get("timestamp")) |timestamp| {
        if (timestamp != .integer and timestamp != .float) return error.InvalidTimestamp;
    }
}

fn bridgeRequestIdOrUnknown(root: std.json.Value) []const u8 {
    if (root == .object) {
        if (valueString(root.object.get("id"))) |id| {
            if (id.len > 0) return id;
        }
    }
    return "unknown";
}

fn bridgeValidationMessage(err: BridgeValidationError) []const u8 {
    return switch (err) {
        error.UnknownTopLevelField => "Bridge request contains unknown top-level fields",
        error.InvalidId => "Bridge request id must be a non-empty string",
        error.InvalidMethod => "Bridge request method must be a string",
        error.InvalidParams => "Bridge request params must be an object",
        error.InvalidTimestamp => "Bridge request timestamp must be a number",
    };
}

fn handleStorageBridge(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    id: []const u8,
    method: []const u8,
    params: std.json.Value,
    app_id: []const u8,
) !void {
    const prefix = try std.fmt.allocPrint(allocator, "{s}:", .{app_id});
    defer allocator.free(prefix);

    if (std.mem.eql(u8, method, "storage.get")) {
        const key = valueString(params.object.get("key")) orelse {
            return writeBridgeError(allocator, stream, id, "invalid_request", "storage.get requires key");
        };
        if (!std.mem.startsWith(u8, key, prefix)) {
            return writeBridgeError(allocator, stream, id, "permission_denied", "Storage key must begin with app storage prefix");
        }
        const result_json = storageGetResultJson(allocator, app_id, key, params.object.get("defaultValue")) catch {
            return writeBridgeError(allocator, stream, id, "storage_error", "storage.get failed");
        };
        defer allocator.free(result_json);
        return writeBridgeOkRaw(allocator, stream, id, result_json);
    }

    if (std.mem.eql(u8, method, "storage.set")) {
        const key = valueString(params.object.get("key")) orelse {
            return writeBridgeError(allocator, stream, id, "invalid_request", "storage.set requires key");
        };
        if (!std.mem.startsWith(u8, key, prefix)) {
            return writeBridgeError(allocator, stream, id, "permission_denied", "Storage key must begin with app storage prefix");
        }
        const value = if (params.object.get("value")) |value_param|
            try jsonValueAlloc(allocator, value_param)
        else
            try allocator.dupe(u8, "null");
        defer allocator.free(value);
        storageSet(app_id, key, value) catch {
            return writeBridgeError(allocator, stream, id, "storage_error", "storage.set failed");
        };
        const result_json = try std.fmt.allocPrint(allocator, "{{\"ok\":true,\"bytesWritten\":{d}}}", .{value.len});
        defer allocator.free(result_json);
        return writeBridgeOkRaw(allocator, stream, id, result_json);
    }

    if (std.mem.eql(u8, method, "storage.remove")) {
        const key = valueString(params.object.get("key")) orelse {
            return writeBridgeError(allocator, stream, id, "invalid_request", "storage.remove requires key");
        };
        if (!std.mem.startsWith(u8, key, prefix)) {
            return writeBridgeError(allocator, stream, id, "permission_denied", "Storage key must begin with app storage prefix");
        }
        storageRemove(app_id, key) catch {
            return writeBridgeError(allocator, stream, id, "storage_error", "storage.remove failed");
        };
        return writeBridgeOkRaw(allocator, stream, id, "{\"ok\":true}");
    }

    if (std.mem.eql(u8, method, "storage.list")) {
        const prefix_param = valueString(params.object.get("prefix")) orelse {
            return writeBridgeError(allocator, stream, id, "invalid_request", "storage.list requires prefix");
        };
        if (!std.mem.startsWith(u8, prefix_param, prefix)) {
            return writeBridgeError(allocator, stream, id, "permission_denied", "Storage key must begin with app storage prefix");
        }
        const result_json = storageListResultJson(allocator, app_id, prefix_param) catch {
            return writeBridgeError(allocator, stream, id, "storage_error", "storage.list failed");
        };
        defer allocator.free(result_json);
        return writeBridgeOkRaw(allocator, stream, id, result_json);
    }

    return writeBridgeError(allocator, stream, id, "unknown_method", "Unknown storage method");
}

fn handleAppLogBridge(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    id: []const u8,
    params: std.json.Value,
    app_id: []const u8,
    session_id: ?[]const u8,
) !void {
    const level = valueString(params.object.get("level")) orelse {
        return writeBridgeError(allocator, stream, id, "invalid_request", "app.log requires level");
    };
    if (!isLogLevel(level)) {
        return writeBridgeError(allocator, stream, id, "invalid_request", "app.log level must be debug, info, warn, or error");
    }

    const message = valueString(params.object.get("message")) orelse {
        return writeBridgeError(allocator, stream, id, "invalid_request", "app.log requires message");
    };

    logAppMessage(allocator, app_id, session_id, level, message) catch {
        return writeBridgeError(allocator, stream, id, "storage_error", "app.log failed");
    };
    std.debug.print("[app.log] {s} {s}: {s}\n", .{ app_id, level, message });
    return writeBridgeOkRaw(allocator, stream, id, "{\"ok\":true}");
}

fn handleWebappValidate(allocator: std.mem.Allocator, stream: std.net.Stream, body: []const u8) !void {
    const report = try validateWebappPackage(allocator, body);
    defer allocator.free(report);
    return writeJson(stream, 200, report);
}

fn handleDbControlEndpoint(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    path: []const u8,
    body: []const u8,
    provided_token: ?[]const u8,
) !void {
    requireControlToken(allocator, provided_token) catch {
        return writeControlError(allocator, stream, 401, "control_auth_required", "A valid X-Platform-Control-Token header is required");
    };

    const args = parseControlArgs(allocator, body) catch {
        return writeControlError(allocator, stream, 400, "invalid_request", "Control request body must be a JSON object");
    };
    defer if (args) |*parsed| parsed.deinit();
    const root = if (args) |parsed| parsed.value else null;
    const app_id = if (root) |value| valueString(value.object.get("appId")) else null;

    if (std.mem.eql(u8, path, "/db/snapshot") or std.mem.eql(u8, path, "/control/db/snapshot")) {
        const result_json = try dbSnapshotJson(allocator);
        defer allocator.free(result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, path, "/db/app-storage") or std.mem.eql(u8, path, "/control/db/app-storage")) {
        const actual_app_id = app_id orelse {
            return writeControlError(allocator, stream, 400, "invalid_request", "db.query_app_storage requires appId");
        };
        const result_json = try queryAppStorageRowsJson(allocator, actual_app_id);
        defer allocator.free(result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, path, "/db/bridge-calls") or std.mem.eql(u8, path, "/control/db/bridge-calls")) {
        const result_json = try queryBridgeCallsRowsJson(allocator, app_id);
        defer allocator.free(result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, path, "/db/app-versions") or std.mem.eql(u8, path, "/control/db/app-versions")) {
        if (app_id == null) {
            return writeControlError(allocator, stream, 400, "invalid_request", "db.query_app_versions requires appId");
        }
        return writeControlOkRaw(allocator, stream, "[]");
    }
    if (std.mem.eql(u8, path, "/db/core-events") or std.mem.eql(u8, path, "/control/db/core-events")) {
        return writeControlOkRaw(allocator, stream, "[]");
    }
    if (std.mem.eql(u8, path, "/db/test-runs") or std.mem.eql(u8, path, "/control/db/test-runs")) {
        return writeControlOkRaw(allocator, stream, "[]");
    }
    if (std.mem.eql(u8, path, "/db/export-debug-bundle") or std.mem.eql(u8, path, "/control/db/export-debug-bundle")) {
        const result_json = try dbDebugBundleJson(allocator);
        defer allocator.free(result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }

    return writeControlError(allocator, stream, 404, "not_found", "Control route not found");
}

fn handleControlCommand(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    body: []const u8,
    provided_token: ?[]const u8,
) !void {
    requireControlToken(allocator, provided_token) catch {
        return writeControlError(allocator, stream, 401, "control_auth_required", "A valid X-Platform-Control-Token header is required");
    };

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, body, .{}) catch {
        return writeControlError(allocator, stream, 400, "invalid_request", "Control command body must be valid JSON");
    };
    defer parsed.deinit();

    const root = parsed.value;
    if (root != .object) {
        return writeControlError(allocator, stream, 400, "invalid_request", "Control command body must be an object");
    }

    const tool = valueString(root.object.get("tool")) orelse {
        return writeControlError(allocator, stream, 400, "invalid_request", "Control command requires tool");
    };
    const args = if (root.object.get("args")) |args_value| args_value else null;
    if (args) |args_value| {
        if (args_value != .object) {
            return writeControlError(allocator, stream, 400, "invalid_request", "Control command args must be an object");
        }
    }

    if (std.mem.eql(u8, tool, "platform.health")) {
        return writeControlOkRaw(allocator, stream, "{\"name\":\"zig-server\",\"version\":\"0.1.0\",\"targets\":[\"zig-server\"],\"db\":\"sqlite\"}");
    }
    if (std.mem.eql(u8, tool, "runtime.capabilities")) {
        const result_json = try serverCapabilitiesJson(allocator);
        defer allocator.free(result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.snapshot")) {
        const result_json = try dbSnapshotJson(allocator);
        defer allocator.free(result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.query_app_storage")) {
        const app_id = controlStringArg(args, "appId") orelse {
            return writeControlError(allocator, stream, 400, "invalid_request", "db.query_app_storage requires appId");
        };
        const result_json = try queryAppStorageRowsJson(allocator, app_id);
        defer allocator.free(result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.query_bridge_calls") or std.mem.eql(u8, tool, "runtime.bridge_calls")) {
        const result_json = try queryBridgeCallsRowsJson(allocator, controlStringArg(args, "appId"));
        defer allocator.free(result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.query_app_versions")) {
        if (controlStringArg(args, "appId") == null) {
            return writeControlError(allocator, stream, 400, "invalid_request", "db.query_app_versions requires appId");
        }
        return writeControlOkRaw(allocator, stream, "[]");
    }
    if (std.mem.eql(u8, tool, "db.query_core_events")) {
        return writeControlOkRaw(allocator, stream, "[]");
    }
    if (std.mem.eql(u8, tool, "db.query_test_runs")) {
        return writeControlOkRaw(allocator, stream, "[]");
    }
    if (std.mem.eql(u8, tool, "db.export_debug_bundle")) {
        const result_json = try dbDebugBundleJson(allocator);
        defer allocator.free(result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }

    return writeControlError(allocator, stream, 400, "unknown_tool", "Unknown control tool");
}

fn requireControlToken(allocator: std.mem.Allocator, provided_token: ?[]const u8) !void {
    const expected = try std.process.getEnvVarOwned(allocator, "NATIVE_AI_SERVER_CONTROL_TOKEN");
    defer allocator.free(expected);
    if (expected.len == 0) return error.ControlAuthRequired;
    const actual = provided_token orelse return error.ControlAuthRequired;
    if (!std.mem.eql(u8, actual, expected)) return error.ControlAuthRequired;
}

fn parseControlArgs(allocator: std.mem.Allocator, body: []const u8) !?std.json.Parsed(std.json.Value) {
    const trimmed = std.mem.trim(u8, body, " \t\r\n");
    if (trimmed.len == 0) return null;
    var parsed = try std.json.parseFromSlice(std.json.Value, allocator, trimmed, .{});
    errdefer parsed.deinit();
    if (parsed.value != .object) return error.InvalidControlArgs;
    return parsed;
}

fn controlStringArg(args: ?std.json.Value, name: []const u8) ?[]const u8 {
    const value = args orelse return null;
    if (value != .object) return null;
    return valueString(value.object.get(name));
}

fn handleExampleAsset(allocator: std.mem.Allocator, stream: std.net.Stream, rel_path: []const u8) !void {
    if (rel_path.len == 0 or containsAny(rel_path, &.{ "..", "\\", "//" })) {
        return writeJson(stream, 400, "{\"ok\":false,\"error\":{\"code\":\"invalid_request\",\"message\":\"Invalid example asset path\",\"details\":{}}}");
    }

    const file_path = try std.fs.path.join(allocator, &.{ "webapps", "examples", rel_path });
    defer allocator.free(file_path);
    const file = std.fs.cwd().openFile(file_path, .{}) catch {
        return writeJson(stream, 404, "{\"ok\":false,\"error\":{\"code\":\"not_found\",\"message\":\"Example asset not found\",\"details\":{}}}");
    };
    defer file.close();
    const body = try file.readToEndAlloc(allocator, max_request_bytes);
    defer allocator.free(body);
    return writeStatic(stream, 200, contentTypeForPath(rel_path), body);
}

fn validateWebappPackage(allocator: std.mem.Allocator, body: []const u8) ![]u8 {
    var errors: std.ArrayList([]const u8) = .empty;
    defer errors.deinit(allocator);

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, body, .{}) catch {
        try errors.append(allocator, "invalid_package_json");
        return validationReportAlloc(allocator, errors.items);
    };
    defer parsed.deinit();

    const root = parsed.value;
    if (root != .object) {
        try errors.append(allocator, "invalid_package_shape");
        return validationReportAlloc(allocator, errors.items);
    }

    const manifest = root.object.get("manifest") orelse {
        try errors.append(allocator, "missing_manifest");
        return validationReportAlloc(allocator, errors.items);
    };
    if (manifest != .object) {
        try errors.append(allocator, "invalid_manifest");
        return validationReportAlloc(allocator, errors.items);
    }

    const files = root.object.get("files") orelse {
        try errors.append(allocator, "missing_files");
        return validationReportAlloc(allocator, errors.items);
    };
    if (files != .array) {
        try errors.append(allocator, "invalid_files");
        return validationReportAlloc(allocator, errors.items);
    }

    const required_files = [_][]const u8{ "manifest.json", "index.html", "styles.css", "app.js" };
    for (required_files) |file_path| {
        if (findPackageFile(files, file_path) == null) {
            try errors.append(allocator, "missing_required_file");
        }
    }

    const required_manifest_fields = [_][]const u8{
        "id",
        "name",
        "version",
        "runtimeVersion",
        "dataVersion",
        "entry",
        "description",
        "permissions",
        "storagePrefix",
        "capabilities",
        "resourceBudget",
        "networkPolicy",
    };
    for (required_manifest_fields) |field| {
        if (manifest.object.get(field) == null) {
            try errors.append(allocator, "missing_manifest_field");
        }
    }

    if (manifest.object.get("networkAllowlist") != null) {
        try errors.append(allocator, "removed_manifest_field");
    }
    if (valueString(manifest.object.get("entry"))) |entry| {
        if (!std.mem.eql(u8, entry, "index.html")) {
            try errors.append(allocator, "invalid_entry");
        }
    }
    if (valueString(manifest.object.get("id"))) |app_id| {
        if (valueString(manifest.object.get("storagePrefix"))) |prefix| {
            const expected = try std.fmt.allocPrint(allocator, "{s}:", .{app_id});
            defer allocator.free(expected);
            if (!std.mem.eql(u8, prefix, expected)) {
                try errors.append(allocator, "invalid_storage_prefix");
            }
        }
    }
    if (manifest.object.get("dataVersion")) |data_version| {
        if (data_version != .integer or data_version.integer < 1) {
            try errors.append(allocator, "invalid_data_version");
        }
    }

    if (findPackageFile(files, "index.html")) |html| {
        if (containsAny(html, &.{ "<script>", "onclick=", "onchange=", "javascript:" })) try errors.append(allocator, "forbidden_html_policy");
        if (containsAny(html, &.{ "src=\"http://", "src=\"https://", "src='http://", "src='https://" })) try errors.append(allocator, "forbidden_remote_script");
        if (containsAny(html, &.{ "<iframe", "<object", "<embed", "<applet" })) try errors.append(allocator, "forbidden_embedded_context");
        if (hasInteractiveWithoutTestId(html)) try errors.append(allocator, "missing_testid");
    }
    if (findPackageFile(files, "styles.css")) |css| {
        if (containsAny(css, &.{ "@import", "url(http:", "url(https:", "url(/", "url(data:" })) try errors.append(allocator, "forbidden_css_url");
    }
    if (findPackageFile(files, "app.js")) |js| {
        if (containsAny(js, &.{ "eval(", "new Function(", "import(" })) try errors.append(allocator, "forbidden_eval");
        if (containsAny(js, &.{ "fetch(", "XMLHttpRequest", "WebSocket", "EventSource" })) try errors.append(allocator, "forbidden_network_api");
        if (containsAny(js, &.{ "localStorage", "sessionStorage", "indexedDB", "document.cookie" })) try errors.append(allocator, "forbidden_storage_api");
        if (containsAny(js, &.{ "webkit.messageHandlers", "chrome.webview", "Android.", "shell.exec", "native.exec" })) try errors.append(allocator, "forbidden_bridge_method");
        if (hasUnknownRuntimeBridgeCall(js)) try errors.append(allocator, "forbidden_bridge_method");
    }

    return validationReportAlloc(allocator, errors.items);
}

fn coreStepAlloc(allocator: std.mem.Allocator, body: []const u8) ![]u8 {
    const core = core_api.core_create() orelse {
        return error.CoreCreateFailed;
    };
    defer core_api.core_destroy(core);

    var output: core_api.ZigCoreBuffer = undefined;
    const code = core_api.core_step_json(core, body.ptr, body.len, &output);
    if (code != 0) {
        return error.CoreStepFailed;
    }
    defer core_api.core_free(output);

    return allocator.dupe(u8, output.ptr[0..output.len]);
}

fn openPlatformDb(allocator: std.mem.Allocator) !*sqlite.sqlite3 {
    const raw_path = std.process.getEnvVarOwned(allocator, "NATIVE_AI_SERVER_DB") catch |err| switch (err) {
        error.EnvironmentVariableNotFound => try allocator.dupe(u8, "server-platform.sqlite"),
        else => return err,
    };
    defer allocator.free(raw_path);
    const path_z = try allocator.dupeZ(u8, raw_path);
    defer allocator.free(path_z);

    var db: ?*sqlite.sqlite3 = null;
    if (sqlite.sqlite3_open(path_z.ptr, &db) != sqlite.SQLITE_OK) {
        return error.StorageOpenFailed;
    }
    errdefer _ = sqlite.sqlite3_close(db);

    if (sqlite.sqlite3_exec(db, "PRAGMA foreign_keys = ON;", null, null, null) != sqlite.SQLITE_OK) {
        return error.StorageSchemaFailed;
    }

    const schema =
        "CREATE TABLE IF NOT EXISTS app_storage (" ++
        "app_id TEXT NOT NULL, " ++
        "key TEXT NOT NULL, " ++
        "value_json TEXT, " ++
        "updated_at TEXT NOT NULL, " ++
        "PRIMARY KEY(app_id, key)" ++
        ");" ++
        "CREATE INDEX IF NOT EXISTS idx_app_storage_app_updated ON app_storage(app_id, updated_at);" ++
        "CREATE TABLE IF NOT EXISTS runtime_sessions (" ++
        "session_id TEXT PRIMARY KEY, " ++
        "target TEXT NOT NULL, " ++
        "platform TEXT NOT NULL, " ++
        "runtime_version TEXT NOT NULL, " ++
        "active_app_id TEXT, " ++
        "active_install_id TEXT, " ++
        "started_at TEXT NOT NULL, " ++
        "ended_at TEXT, " ++
        "status TEXT NOT NULL DEFAULT 'running', " ++
        "capabilities_json TEXT, " ++
        "metadata_json TEXT" ++
        ");" ++
        "CREATE TABLE IF NOT EXISTS bridge_calls (" ++
        "bridge_call_id TEXT PRIMARY KEY, " ++
        "session_id TEXT NOT NULL REFERENCES runtime_sessions(session_id) ON DELETE CASCADE, " ++
        "app_id TEXT, " ++
        "install_id TEXT, " ++
        "method TEXT NOT NULL, " ++
        "params_json TEXT, " ++
        "result_json TEXT, " ++
        "error_json TEXT, " ++
        "duration_ms INTEGER, " ++
        "created_at TEXT NOT NULL" ++
        ");" ++
        "CREATE INDEX IF NOT EXISTS idx_bridge_calls_session_created ON bridge_calls(session_id, created_at);" ++
        "CREATE INDEX IF NOT EXISTS idx_bridge_calls_app_method ON bridge_calls(app_id, method);";
    if (sqlite.sqlite3_exec(db, schema, null, null, null) != sqlite.SQLITE_OK) {
        return error.StorageSchemaFailed;
    }
    return db.?;
}

fn storageGetResultJson(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    key: []const u8,
    default_value: ?std.json.Value,
) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, key);

    if (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        const value_json = sqliteColumnText(statement, 0);
        return std.fmt.allocPrint(allocator, "{{\"value\":{s}}}", .{if (value_json.len == 0) "null" else value_json});
    }

    if (default_value) |value| {
        const default_json = try jsonValueAlloc(allocator, value);
        defer allocator.free(default_json);
        return std.fmt.allocPrint(allocator, "{{\"value\":{s}}}", .{default_json});
    }
    return allocator.dupe(u8, "{\"value\":null}");
}

fn storageSet(app_id: []const u8, key: []const u8, value_json: []const u8) !void {
    const allocator = std.heap.page_allocator;
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, datetime('now')) " ++
            "ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, key);
    bindText(statement, 3, value_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) {
        return error.StorageWriteFailed;
    }
}

fn storageRemove(app_id: []const u8, key: []const u8) !void {
    const allocator = std.heap.page_allocator;
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "DELETE FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, key);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) {
        return error.StorageWriteFailed;
    }
}

fn storageListResultJson(allocator: std.mem.Allocator, app_id: []const u8, prefix: []const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT key FROM app_storage WHERE app_id = ? AND key LIKE ? ORDER BY key", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    const like_prefix = try std.fmt.allocPrint(allocator, "{s}%", .{prefix});
    defer allocator.free(like_prefix);
    bindText(statement, 2, like_prefix);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{\"keys\":[");
    var count: usize = 0;
    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        const key = sqliteColumnText(statement, 0);
        const escaped = try escapeJsonString(allocator, key);
        defer allocator.free(escaped);
        if (count > 0) try out.writer.writeAll(",");
        try out.writer.print("\"{s}\"", .{escaped});
        count += 1;
    }
    try out.writer.writeAll("]}");
    return out.toOwnedSlice();
}

fn dbSnapshotJson(allocator: std.mem.Allocator) ![]u8 {
    const storage = try queryAppStorageRowsJson(allocator, null);
    defer allocator.free(storage);
    const bridge_calls = try queryBridgeCallsRowsJson(allocator, null);
    defer allocator.free(bridge_calls);
    const runtime_sessions = try queryRuntimeSessionsRowsJson(allocator);
    defer allocator.free(runtime_sessions);

    return std.fmt.allocPrint(
        allocator,
        "{{\"app_storage\":{s},\"bridge_calls\":{s},\"runtime_sessions\":{s},\"app_versions\":[],\"core_events\":[],\"test_runs\":[]}}",
        .{ storage, bridge_calls, runtime_sessions },
    );
}

fn dbDebugBundleJson(allocator: std.mem.Allocator) ![]u8 {
    const storage = try queryAppStorageRowsJson(allocator, null);
    defer allocator.free(storage);
    const bridge_calls = try queryBridgeCallsRowsJson(allocator, null);
    defer allocator.free(bridge_calls);
    const runtime_sessions = try queryRuntimeSessionsRowsJson(allocator);
    defer allocator.free(runtime_sessions);

    return std.fmt.allocPrint(
        allocator,
        "{{\"exportId\":\"server-debug-bundle\",\"type\":\"debug-bundle\",\"runtimeVersion\":\"{s}\",\"source\":{{\"platform\":\"server\",\"target\":\"zig-server\"}},\"appStorage\":{s},\"debug\":{{\"runtimeSessions\":{s},\"bridgeCalls\":{s},\"coreEvents\":[],\"testRuns\":[]}}}}",
        .{ runtime_version, storage, runtime_sessions, bridge_calls },
    );
}

fn queryAppStorageRowsJson(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    const sql = if (app_id == null)
        "SELECT app_id, key, value_json, updated_at FROM app_storage ORDER BY app_id, key"
    else
        "SELECT app_id, key, value_json, updated_at FROM app_storage WHERE app_id = ? ORDER BY key";
    if (sqlite.sqlite3_prepare_v2(db, sql, -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    if (app_id) |actual_app_id| {
        bindText(statement, 1, actual_app_id);
    }

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("[");
    var count: usize = 0;
    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        if (count > 0) try out.writer.writeAll(",");
        try out.writer.writeAll("{\"app_id\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 0));
        try out.writer.writeAll(",\"key\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 1));
        try out.writer.writeAll(",\"value_json\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 2));
        try out.writer.writeAll(",\"updated_at\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 3));
        try out.writer.writeAll("}");
        count += 1;
    }
    try out.writer.writeAll("]");
    return out.toOwnedSlice();
}

fn queryRuntimeSessionsRowsJson(allocator: std.mem.Allocator) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, ended_at, status, capabilities_json, metadata_json FROM runtime_sessions ORDER BY started_at",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("[");
    var count: usize = 0;
    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        if (count > 0) try out.writer.writeAll(",");
        try out.writer.writeAll("{\"session_id\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 0));
        try out.writer.writeAll(",\"target\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 1));
        try out.writer.writeAll(",\"platform\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 2));
        try out.writer.writeAll(",\"runtime_version\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 3));
        try out.writer.writeAll(",\"active_app_id\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 4));
        try out.writer.writeAll(",\"active_install_id\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 5));
        try out.writer.writeAll(",\"started_at\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 6));
        try out.writer.writeAll(",\"ended_at\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 7));
        try out.writer.writeAll(",\"status\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 8));
        try out.writer.writeAll(",\"capabilities_json\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 9));
        try out.writer.writeAll(",\"metadata_json\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 10));
        try out.writer.writeAll("}");
        count += 1;
    }
    try out.writer.writeAll("]");
    return out.toOwnedSlice();
}

fn queryBridgeCallsRowsJson(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    const sql = if (app_id == null)
        "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at FROM bridge_calls ORDER BY created_at"
    else
        "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at FROM bridge_calls WHERE app_id = ? ORDER BY created_at";
    if (sqlite.sqlite3_prepare_v2(db, sql, -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    if (app_id) |actual_app_id| {
        bindText(statement, 1, actual_app_id);
    }

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("[");
    var count: usize = 0;
    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        if (count > 0) try out.writer.writeAll(",");
        try out.writer.writeAll("{\"bridge_call_id\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 0));
        try out.writer.writeAll(",\"session_id\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 1));
        try out.writer.writeAll(",\"app_id\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 2));
        try out.writer.writeAll(",\"install_id\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 3));
        try out.writer.writeAll(",\"method\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 4));
        try out.writer.writeAll(",\"params_json\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 5));
        try out.writer.writeAll(",\"result_json\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 6));
        try out.writer.writeAll(",\"error_json\":");
        try appendJsonNullableString(allocator, &out, sqliteColumnNullableText(statement, 7));
        try out.writer.writeAll(",\"duration_ms\":");
        try appendJsonNullableInt(&out, statement, 8);
        try out.writer.writeAll(",\"created_at\":");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 9));
        try out.writer.writeAll("}");
        count += 1;
    }
    try out.writer.writeAll("]");
    return out.toOwnedSlice();
}

fn logAppMessage(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    level: []const u8,
    message: []const u8,
) !void {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    const actual_session_id = session_id orelse "server-dev-session";
    try ensureRuntimeSession(db, actual_session_id, app_id);

    const params_json = try appLogParamsJson(allocator, level, message);
    defer allocator.free(params_json);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO bridge_calls (bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at) " ++
            "VALUES ('bridge_' || lower(hex(randomblob(16))), ?, ?, NULL, 'app.log', ?, '{\"ok\":true}', NULL, 0, datetime('now'))",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, actual_session_id);
    bindText(statement, 2, app_id);
    bindText(statement, 3, params_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) {
        return error.StorageWriteFailed;
    }
}

fn ensureRuntimeSession(db: *sqlite.sqlite3, session_id: []const u8, app_id: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR IGNORE INTO runtime_sessions (session_id, target, platform, runtime_version, active_app_id, started_at, status, capabilities_json, metadata_json) " ++
            "VALUES (?, 'zig-server', 'server', ?, ?, datetime('now'), 'running', NULL, '{\"source\":\"bridge\"}')",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, session_id);
    bindText(statement, 2, runtime_version);
    bindText(statement, 3, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) {
        return error.StorageWriteFailed;
    }
}

fn appLogParamsJson(allocator: std.mem.Allocator, level: []const u8, message: []const u8) ![]u8 {
    const escaped_level = try escapeJsonString(allocator, level);
    defer allocator.free(escaped_level);
    const escaped_message = try escapeJsonString(allocator, message);
    defer allocator.free(escaped_message);
    return std.fmt.allocPrint(allocator, "{{\"level\":\"{s}\",\"message\":\"{s}\",\"data\":\"redacted\"}}", .{ escaped_level, escaped_message });
}

fn bindText(statement: ?*sqlite.sqlite3_stmt, index: c_int, value: []const u8) void {
    _ = sqlite.sqlite3_bind_text(statement, index, value.ptr, @intCast(value.len), null);
}

fn sqliteColumnText(statement: ?*sqlite.sqlite3_stmt, index: c_int) []const u8 {
    const raw = sqlite.sqlite3_column_text(statement, index) orelse return "";
    const len: usize = @intCast(sqlite.sqlite3_column_bytes(statement, index));
    return @as([*]const u8, @ptrCast(raw))[0..len];
}

fn sqliteColumnNullableText(statement: ?*sqlite.sqlite3_stmt, index: c_int) ?[]const u8 {
    if (sqlite.sqlite3_column_type(statement, index) == sqlite.SQLITE_NULL) return null;
    return sqliteColumnText(statement, index);
}

const ParsedRequest = struct {
    method: []const u8,
    path: []const u8,
    body: []const u8,
    app_id: ?[]const u8,
    session_id: ?[]const u8,
    mount_token: ?[]const u8,
    control_token: ?[]const u8,
};

fn parseRequest(request: []const u8) !ParsedRequest {
    const header_end = std.mem.indexOf(u8, request, "\r\n\r\n") orelse return error.MissingHeaderEnd;
    const headers = request[0..header_end];
    const body = request[header_end + 4 ..];
    const line_end = std.mem.indexOf(u8, headers, "\r\n") orelse return error.MissingRequestLine;
    const request_line = headers[0..line_end];

    var parts = std.mem.splitScalar(u8, request_line, ' ');
    const method = parts.next() orelse return error.MissingMethod;
    const raw_path = parts.next() orelse return error.MissingPath;
    const path_end = std.mem.indexOfScalar(u8, raw_path, '?') orelse raw_path.len;

    return .{
        .method = method,
        .path = raw_path[0..path_end],
        .body = body,
        .app_id = headerValue(headers, "x-app-id"),
        .session_id = headerValue(headers, "x-runtime-session-id"),
        .mount_token = headerValue(headers, "x-mount-token"),
        .control_token = headerValue(headers, "x-platform-control-token"),
    };
}

fn headerValue(headers: []const u8, name: []const u8) ?[]const u8 {
    var lines = std.mem.splitSequence(u8, headers, "\r\n");
    _ = lines.next();
    while (lines.next()) |line| {
        const colon = std.mem.indexOfScalar(u8, line, ':') orelse continue;
        const key = std.mem.trim(u8, line[0..colon], " \t");
        if (!std.ascii.eqlIgnoreCase(key, name)) continue;
        return std.mem.trim(u8, line[colon + 1 ..], " \t");
    }
    return null;
}

fn writeJson(stream: std.net.Stream, status: u16, body: []const u8) !void {
    return writeBody(stream, status, "application/json; charset=utf-8", body);
}

fn writeStatic(stream: std.net.Stream, status: u16, content_type: []const u8, body: []const u8) !void {
    return writeBody(stream, status, content_type, body);
}

fn writeBody(stream: std.net.Stream, status: u16, content_type: []const u8, body: []const u8) !void {
    const reason = switch (status) {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        else => "OK",
    };
    var header_buffer: [256]u8 = undefined;
    const header = try std.fmt.bufPrint(
        &header_buffer,
        "HTTP/1.1 {d} {s}\r\ncontent-type: {s}\r\ncontent-length: {d}\r\nconnection: close\r\n\r\n",
        .{ status, reason, content_type, body.len },
    );
    try stream.writeAll(header);
    try stream.writeAll(body);
}

fn contentTypeForPath(path: []const u8) []const u8 {
    if (std.mem.endsWith(u8, path, ".html")) return "text/html; charset=utf-8";
    if (std.mem.endsWith(u8, path, ".css")) return "text/css; charset=utf-8";
    if (std.mem.endsWith(u8, path, ".js")) return "text/javascript; charset=utf-8";
    if (std.mem.endsWith(u8, path, ".json")) return "application/json; charset=utf-8";
    return "text/plain; charset=utf-8";
}

fn writeBridgeOkRaw(allocator: std.mem.Allocator, stream: std.net.Stream, id: []const u8, result_json: []const u8) !void {
    const escaped_id = try escapeJsonString(allocator, id);
    defer allocator.free(escaped_id);
    const body = try std.fmt.allocPrint(allocator, "{{\"id\":\"{s}\",\"ok\":true,\"result\":{s}}}", .{ escaped_id, result_json });
    defer allocator.free(body);
    return writeJson(stream, 200, body);
}

fn writeBridgeError(allocator: std.mem.Allocator, stream: std.net.Stream, id: []const u8, code: []const u8, message: []const u8) !void {
    const escaped_id = try escapeJsonString(allocator, id);
    defer allocator.free(escaped_id);
    const escaped_code = try escapeJsonString(allocator, code);
    defer allocator.free(escaped_code);
    const escaped_message = try escapeJsonString(allocator, message);
    defer allocator.free(escaped_message);
    const body = try std.fmt.allocPrint(
        allocator,
        "{{\"id\":\"{s}\",\"ok\":false,\"error\":{{\"code\":\"{s}\",\"message\":\"{s}\",\"details\":{{}}}}}}",
        .{ escaped_id, escaped_code, escaped_message },
    );
    defer allocator.free(body);
    return writeJson(stream, 200, body);
}

fn writeControlOkRaw(allocator: std.mem.Allocator, stream: std.net.Stream, result_json: []const u8) !void {
    const body = try std.fmt.allocPrint(allocator, "{{\"ok\":true,\"result\":{s}}}", .{result_json});
    defer allocator.free(body);
    return writeJson(stream, 200, body);
}

fn writeControlError(allocator: std.mem.Allocator, stream: std.net.Stream, status: u16, code: []const u8, message: []const u8) !void {
    const escaped_code = try escapeJsonString(allocator, code);
    defer allocator.free(escaped_code);
    const escaped_message = try escapeJsonString(allocator, message);
    defer allocator.free(escaped_message);
    const body = try std.fmt.allocPrint(
        allocator,
        "{{\"ok\":false,\"error\":{{\"code\":\"{s}\",\"message\":\"{s}\",\"details\":{{}}}}}}",
        .{ escaped_code, escaped_message },
    );
    defer allocator.free(body);
    return writeJson(stream, status, body);
}

fn serverCapabilitiesJson(allocator: std.mem.Allocator) ![]u8 {
    return std.fmt.allocPrint(
        allocator,
        "{{\"runtimeVersion\":\"{s}\",\"platform\":\"server\",\"target\":\"zig-server\",\"devMode\":false,\"features\":{{\"core.step\":true,\"runtime.capabilities\":true,\"storage.get\":true,\"storage.set\":true,\"storage.remove\":true,\"storage.list\":true,\"dialog.openFile\":false,\"dialog.saveFile\":false,\"notification.toast\":false,\"network.request\":false,\"app.log\":true}},\"limits\":{{\"maxPackageBytes\":1048576,\"maxFileBytes\":524288}}}}",
        .{runtime_version},
    );
}

fn validationReportAlloc(allocator: std.mem.Allocator, errors: []const []const u8) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    const ok = errors.len == 0;
    try out.writer.print(
        "{{\"ok\":{},\"status\":\"{s}\",\"checks\":[{{\"name\":\"package-policy\",\"status\":\"{s}\"}}],\"errors\":[",
        .{ ok, if (ok) "accepted" else "rejected", if (ok) "pass" else "fail" },
    );
    for (errors, 0..) |validation_error, index| {
        if (index > 0) try out.writer.writeAll(",");
        const escaped = try escapeJsonString(allocator, validation_error);
        defer allocator.free(escaped);
        try out.writer.print("\"{s}\"", .{escaped});
    }
    try out.writer.writeAll("],\"warnings\":[]}");
    return out.toOwnedSlice();
}

fn valueString(value: ?std.json.Value) ?[]const u8 {
    const actual = value orelse return null;
    if (actual != .string) return null;
    return actual.string;
}

fn isLogLevel(level: []const u8) bool {
    const levels = [_][]const u8{ "debug", "info", "warn", "error" };
    for (levels) |candidate| {
        if (std.mem.eql(u8, level, candidate)) return true;
    }
    return false;
}

fn findPackageFile(files: std.json.Value, file_path: []const u8) ?[]const u8 {
    if (files != .array) return null;
    for (files.array.items) |file| {
        if (file != .object) continue;
        const path = valueString(file.object.get("path")) orelse continue;
        if (!std.mem.eql(u8, path, file_path)) continue;
        return valueString(file.object.get("content"));
    }
    return null;
}

fn containsAny(source: []const u8, needles: []const []const u8) bool {
    for (needles) |needle| {
        if (std.mem.indexOf(u8, source, needle) != null) return true;
    }
    return false;
}

fn hasInteractiveWithoutTestId(html: []const u8) bool {
    const tags = [_][]const u8{ "button", "input", "select", "textarea", "a" };
    var index: usize = 0;
    while (std.mem.indexOfScalarPos(u8, html, index, '<')) |start| {
        index = start + 1;
        for (tags) |tag| {
            if (!tagStartsAt(html, start, tag)) continue;
            const end = std.mem.indexOfScalarPos(u8, html, start, '>') orelse return true;
            const attrs = html[start..end];
            if (std.mem.indexOf(u8, attrs, "data-testid") == null) return true;
            index = end + 1;
            break;
        }
    }
    return false;
}

fn tagStartsAt(html: []const u8, start: usize, tag: []const u8) bool {
    const name_start = start + 1;
    const name_end = name_start + tag.len;
    if (name_end > html.len) return false;
    if (!std.mem.eql(u8, html[name_start..name_end], tag)) return false;
    if (name_end == html.len) return false;
    const next = html[name_end];
    return next == ' ' or next == '\t' or next == '\n' or next == '\r' or next == '>' or next == '/';
}

fn isKnownUnsupportedBridgeMethod(method: []const u8) bool {
    const methods = [_][]const u8{
        "dialog.openFile",
        "dialog.saveFile",
        "notification.toast",
        "network.request",
    };
    for (methods) |candidate| {
        if (std.mem.eql(u8, method, candidate)) return true;
    }
    return false;
}

fn hasUnknownRuntimeBridgeCall(source: []const u8) bool {
    var index: usize = 0;
    while (std.mem.indexOfPos(u8, source, index, "AppRuntime.call")) |call_start| {
        index = call_start + "AppRuntime.call".len;
        const open = std.mem.indexOfScalarPos(u8, source, index, '(') orelse return false;
        var cursor = open + 1;
        while (cursor < source.len and (source[cursor] == ' ' or source[cursor] == '\t' or source[cursor] == '\n' or source[cursor] == '\r')) {
            cursor += 1;
        }
        if (cursor >= source.len or (source[cursor] != '"' and source[cursor] != '\'')) continue;
        const quote = source[cursor];
        const method_start = cursor + 1;
        const method_end = std.mem.indexOfScalarPos(u8, source, method_start, quote) orelse return true;
        if (!isAllowedRuntimeBridgeMethod(source[method_start..method_end])) return true;
        index = method_end + 1;
    }
    return false;
}

fn isAllowedRuntimeBridgeMethod(method: []const u8) bool {
    const methods = [_][]const u8{
        "core.step",
        "storage.get",
        "storage.set",
        "storage.remove",
        "storage.list",
        "dialog.openFile",
        "dialog.saveFile",
        "notification.toast",
        "network.request",
        "app.log",
        "runtime.capabilities",
    };
    for (methods) |candidate| {
        if (std.mem.eql(u8, method, candidate)) return true;
    }
    return false;
}

fn jsonValueAlloc(allocator: std.mem.Allocator, value: std.json.Value) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try std.json.Stringify.value(value, .{}, &out.writer);
    return out.toOwnedSlice();
}

fn appendJsonString(allocator: std.mem.Allocator, out: *std.io.Writer.Allocating, text: []const u8) !void {
    const escaped = try escapeJsonString(allocator, text);
    defer allocator.free(escaped);
    try out.writer.print("\"{s}\"", .{escaped});
}

fn appendJsonNullableString(allocator: std.mem.Allocator, out: *std.io.Writer.Allocating, text: ?[]const u8) !void {
    if (text) |actual| {
        return appendJsonString(allocator, out, actual);
    }
    try out.writer.writeAll("null");
}

fn appendJsonNullableInt(out: *std.io.Writer.Allocating, statement: ?*sqlite.sqlite3_stmt, index: c_int) !void {
    if (sqlite.sqlite3_column_type(statement, index) == sqlite.SQLITE_NULL) {
        return out.writer.writeAll("null");
    }
    try out.writer.print("{d}", .{sqlite.sqlite3_column_int64(statement, index)});
}

fn escapeJsonString(allocator: std.mem.Allocator, text: []const u8) ![]u8 {
    var out: std.ArrayList(u8) = .empty;
    for (text) |char| {
        switch (char) {
            '"' => try out.appendSlice(allocator, "\\\""),
            '\\' => try out.appendSlice(allocator, "\\\\"),
            '\n' => try out.appendSlice(allocator, "\\n"),
            '\r' => try out.appendSlice(allocator, "\\r"),
            '\t' => try out.appendSlice(allocator, "\\t"),
            else => try out.append(allocator, char),
        }
    }
    return out.toOwnedSlice(allocator);
}
