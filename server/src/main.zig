const std = @import("std");
const core_api = @import("zig_core");
const sqlite = @cImport({
    @cInclude("sqlite3.h");
});

const max_request_bytes = 1024 * 1024;
const runtime_version = "0.1.0";
const signature_prefix = "native-ai-webapp/sig/v1";

pub fn main() !void {
    const allocator = std.heap.page_allocator;
    try enforceProductionStartupRules(allocator);
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

fn enforceProductionStartupRules(allocator: std.mem.Allocator) !void {
    const args = try std.process.argsAlloc(allocator);
    defer std.process.argsFree(allocator, args);
    if (!isProductionMode(allocator)) return;

    for (args[1..]) |arg| {
        if (isForbiddenProductionFlag(arg)) {
            std.debug.print("fatal: production mode rejects dev-only flag {s}\n", .{arg});
            return error.ProductionDevFlagRejected;
        }
    }
}

fn isProductionMode(allocator: std.mem.Allocator) bool {
    const env = std.process.getEnvVarOwned(allocator, "NATIVE_AI_SERVER_ENV") catch return false;
    defer allocator.free(env);
    return std.ascii.eqlIgnoreCase(env, "production") or std.ascii.eqlIgnoreCase(env, "prod");
}

fn isForbiddenProductionFlag(arg: []const u8) bool {
    const forbidden = [_][]const u8{
        "--control-plane-port",
        "--allow-runtime-mismatch",
        "--allow-unsigned-dev",
    };
    for (forbidden) |candidate| {
        if (std.mem.eql(u8, arg, candidate)) return true;
        if (arg.len > candidate.len and std.mem.startsWith(u8, arg, candidate) and arg[candidate.len] == '=') return true;
    }
    return false;
}

fn isDevControlPath(path: []const u8) bool {
    return std.mem.eql(u8, path, "/control/command") or
        std.mem.startsWith(u8, path, "/control/db/") or
        std.mem.startsWith(u8, path, "/db/") or
        std.mem.eql(u8, path, "/webapps/validate") or
        std.mem.eql(u8, path, "/webapps/install") or
        std.mem.startsWith(u8, path, "/packages/") or
        appIdFromRollbackPath(path) != null;
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

    if (isProductionMode(allocator) and isDevControlPath(parsed.path)) {
        return writeControlError(allocator, stream, 404, "production_control_disabled", "Dev control endpoints are disabled in production mode");
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

    if (std.mem.eql(u8, parsed.method, "POST") and std.mem.eql(u8, parsed.path, "/webapps/install")) {
        return handleWebappInstall(allocator, stream, parsed.body, parsed.control_token);
    }

    if (std.mem.eql(u8, parsed.method, "POST") and
        (std.mem.startsWith(u8, parsed.path, "/packages/") or std.mem.startsWith(u8, parsed.path, "/control/packages/")))
    {
        return handlePackageControlEndpoint(allocator, stream, parsed.path, parsed.body, parsed.control_token);
    }

    if (std.mem.eql(u8, parsed.method, "POST") and appIdFromRollbackPath(parsed.path) != null) {
        return handleAppRollbackEndpoint(allocator, stream, parsed.path, parsed.body, parsed.control_token);
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

    const audit = coreAuditContextAlloc(allocator, body) catch null;
    if (audit) |ctx| {
        defer allocator.free(ctx.event_json);
        defer if (ctx.app_id) |app_id| allocator.free(app_id);
        recordCoreStep(allocator, ctx.app_id, null, ctx.event_json, output) catch |err| {
            std.debug.print("core audit write failed: {}\n", .{err});
        };
    }

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

    if (permissionForBridgeMethod(method)) |permission| {
        const params_json = try jsonValueAlloc(allocator, params);
        defer allocator.free(params_json);
        const permitted = bridgePermissionApproved(allocator, channel_app_id, permission) catch false;
        if (!permitted) {
            const error_json = try bridgeErrorJsonAlloc(allocator, "permission_denied", "Bridge method requires an approved app permission");
            defer allocator.free(error_json);
            logBridgeCall(allocator, channel_app_id, session_id, method, params_json, null, error_json) catch |err| {
                std.debug.print("bridge audit write failed: {}\n", .{err});
            };
            return writeBridgeError(allocator, stream, id, "permission_denied", "Bridge method requires an approved app permission");
        }
    }

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
        logBridgeCall(allocator, channel_app_id, session_id, method, params_json, result_json, null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        const event_json = if (params.object.get("event")) |event_value|
            try jsonValueAlloc(allocator, event_value)
        else
            try allocator.dupe(u8, "{}");
        defer allocator.free(event_json);
        const actual_session_id = session_id orelse "server-dev-session";
        recordCoreStep(allocator, channel_app_id, actual_session_id, event_json, result_json) catch |err| {
            std.debug.print("core audit write failed: {}\n", .{err});
        };
        return writeBridgeOkRaw(allocator, stream, id, result_json);
    }

    if (std.mem.eql(u8, method, "runtime.capabilities")) {
        const result_json = try serverCapabilitiesJson(allocator);
        defer allocator.free(result_json);
        logBridgeCall(allocator, channel_app_id, session_id, method, "{}", result_json, null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return writeBridgeOkRaw(allocator, stream, id, result_json);
    }

    if (std.mem.startsWith(u8, method, "storage.")) {
        return handleStorageBridge(allocator, stream, id, method, params, channel_app_id, session_id);
    }

    if (std.mem.eql(u8, method, "app.log")) {
        return handleAppLogBridge(allocator, stream, id, params, channel_app_id, session_id);
    }

    if (std.mem.eql(u8, method, "notification.toast")) {
        return handleNotificationToastBridge(allocator, stream, id, params, channel_app_id, session_id);
    }

    if (std.mem.eql(u8, method, "network.request")) {
        return handleNetworkRequestBridge(allocator, stream, id, params, channel_app_id, session_id);
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
    session_id: ?[]const u8,
) !void {
    const prefix = try std.fmt.allocPrint(allocator, "{s}:", .{app_id});
    defer allocator.free(prefix);
    const params_json = try jsonValueAlloc(allocator, params);
    defer allocator.free(params_json);

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
        logBridgeCall(allocator, app_id, session_id, method, params_json, result_json, null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
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
        logBridgeCall(allocator, app_id, session_id, method, params_json, result_json, null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
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
        logBridgeCall(allocator, app_id, session_id, method, params_json, "{\"ok\":true}", null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
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
        logBridgeCall(allocator, app_id, session_id, method, params_json, result_json, null) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
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

fn handleNotificationToastBridge(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    id: []const u8,
    params: std.json.Value,
    app_id: []const u8,
    session_id: ?[]const u8,
) !void {
    _ = valueString(params.object.get("message")) orelse {
        return writeBridgeError(allocator, stream, id, "invalid_request", "notification.toast requires message");
    };
    if (params.object.get("level")) |level_value| {
        const level = valueString(level_value) orelse {
            return writeBridgeError(allocator, stream, id, "invalid_request", "notification.toast level must be a string");
        };
        if (!isToastLevel(level)) {
            return writeBridgeError(allocator, stream, id, "invalid_request", "notification.toast level must be info, success, warn, or error");
        }
    }

    const params_json = try jsonValueAlloc(allocator, params);
    defer allocator.free(params_json);
    logBridgeCall(allocator, app_id, session_id, "notification.toast", params_json, "{\"ok\":true}", null) catch |err| {
        std.debug.print("bridge audit write failed: {}\n", .{err});
    };
    return writeBridgeOkRaw(allocator, stream, id, "{\"ok\":true}");
}

fn handleNetworkRequestBridge(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    id: []const u8,
    params: std.json.Value,
    app_id: []const u8,
    session_id: ?[]const u8,
) !void {
    const url = valueString(params.object.get("url")) orelse {
        return writeBridgeError(allocator, stream, id, "invalid_request", "network.request requires url");
    };
    const method_raw = valueString(params.object.get("method")) orelse "GET";
    const method = try upperAsciiAlloc(allocator, method_raw);
    defer allocator.free(method);
    const params_json = try jsonValueAlloc(allocator, params);
    defer allocator.free(params_json);

    const policy_result = networkPolicyAllowsRequest(allocator, app_id, url, method, params) catch |err| switch (err) {
        error.InvalidNetworkUrl => {
            return writeBridgeError(allocator, stream, id, "invalid_request", "network.request url must be absolute");
        },
        else => return writeBridgeError(allocator, stream, id, "network_policy_denied", "network.request is outside manifest.networkPolicy"),
    };
    if (!policy_result) {
        const error_json = try bridgeErrorJsonAlloc(allocator, "network_policy_denied", "network.request is outside manifest.networkPolicy");
        defer allocator.free(error_json);
        logBridgeCall(allocator, app_id, session_id, "network.request", params_json, null, error_json) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return writeBridgeError(allocator, stream, id, "network_policy_denied", "network.request is outside manifest.networkPolicy");
    }

    const result_json = (try networkMockResultJsonAlloc(allocator, app_id, session_id, method, url)) orelse {
        const error_json = try bridgeErrorJsonAlloc(allocator, "network.mock_missing", "No network mock is registered for request");
        defer allocator.free(error_json);
        logBridgeCall(allocator, app_id, session_id, "network.request", params_json, null, error_json) catch |err| {
            std.debug.print("bridge audit write failed: {}\n", .{err});
        };
        return writeBridgeError(allocator, stream, id, "network.mock_missing", "No network mock is registered for request");
    };
    defer allocator.free(result_json);
    logBridgeCall(allocator, app_id, session_id, "network.request", params_json, result_json, null) catch |err| {
        std.debug.print("bridge audit write failed: {}\n", .{err});
    };
    return writeBridgeOkRaw(allocator, stream, id, result_json);
}

fn handleWebappValidate(allocator: std.mem.Allocator, stream: std.net.Stream, body: []const u8) !void {
    const report = try validateWebappPackage(allocator, body);
    defer allocator.free(report);
    return writeJson(stream, 200, report);
}

fn handleWebappInstall(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    body: []const u8,
    provided_token: ?[]const u8,
) !void {
    requireControlToken(allocator, provided_token) catch {
        auditControlCommand(allocator, "/webapps/install", "platform.install_webapp_package", "rejected", "control_auth_required", null, null);
        return writeControlError(allocator, stream, 401, "control_auth_required", "A valid X-Platform-Control-Token header is required");
    };

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, body, .{}) catch {
        auditControlCommand(allocator, "/webapps/install", "platform.install_webapp_package", "rejected", "invalid_request", null, null);
        return writeControlError(allocator, stream, 400, "invalid_request", "Install request body must be valid JSON");
    };
    defer parsed.deinit();

    const root = packageRootValue(parsed.value) orelse {
        auditControlCommand(allocator, "/webapps/install", "platform.install_webapp_package", "rejected", "invalid_request", body, null);
        return writeControlError(allocator, stream, 400, "invalid_request", "Install request requires an inline package object");
    };
    const activate = controlBoolArg(parsed.value, "activate") orelse true;
    const trust_level = controlStringArg(parsed.value, "trustLevel") orelse "developer";
    const result_json = installWebappPackage(allocator, root, activate, trust_level) catch |err| switch (err) {
        error.InvalidWebappPackage => {
            auditControlCommand(allocator, "/webapps/install", "platform.install_webapp_package", "rejected", "invalid_package", body, null);
            return writeControlError(allocator, stream, 400, "invalid_package", "Package validation failed");
        },
        error.InvalidMigration => {
            auditControlCommand(allocator, "/webapps/install", "platform.install_webapp_package", "rejected", "invalid_migration", body, null);
            return writeControlError(allocator, stream, 400, "invalid_migration", "Package migration chain is invalid or incomplete");
        },
        else => return err,
    };
    defer allocator.free(result_json);
    auditControlCommand(allocator, "/webapps/install", "platform.install_webapp_package", "accepted", null, body, result_json);
    return writeControlOkRaw(allocator, stream, result_json);
}

fn handlePackageControlEndpoint(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    path: []const u8,
    body: []const u8,
    provided_token: ?[]const u8,
) !void {
    const tool = controlToolForPackagePath(path);
    requireControlToken(allocator, provided_token) catch {
        auditControlCommand(allocator, path, tool, "rejected", "control_auth_required", null, null);
        return writeControlError(allocator, stream, 401, "control_auth_required", "A valid X-Platform-Control-Token header is required");
    };

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, body, .{}) catch {
        auditControlCommand(allocator, path, tool, "rejected", "invalid_request", null, null);
        return writeControlError(allocator, stream, 400, "invalid_request", "Package control body must be valid JSON");
    };
    defer parsed.deinit();

    const package_root = packageRootValue(parsed.value) orelse {
        auditControlCommand(allocator, path, tool, "rejected", "invalid_request", body, null);
        return writeControlError(allocator, stream, 400, "invalid_request", "Package control request requires an inline package object");
    };

    if (std.mem.eql(u8, tool, "platform.validate_package") or std.mem.eql(u8, tool, "platform.run_policy_audit")) {
        const result_json = try validateWebappPackageValue(allocator, package_root);
        defer allocator.free(result_json);
        auditControlCommand(allocator, path, tool, "accepted", null, body, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }

    if (std.mem.eql(u8, tool, "platform.sign_webapp_package")) {
        const trust_level = controlStringArg(parsed.value, "trustLevel") orelse "developer";
        const result_json = signWebappPackage(allocator, package_root, trust_level) catch |err| switch (err) {
            error.InvalidWebappPackage => {
                auditControlCommand(allocator, path, tool, "rejected", "invalid_package", body, null);
                return writeControlError(allocator, stream, 400, "invalid_package", "Package validation failed");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, path, tool, "accepted", null, body, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }

    auditControlCommand(allocator, path, tool, "rejected", "not_found", body, null);
    return writeControlError(allocator, stream, 404, "not_found", "Package control route not found");
}

fn handleAppRollbackEndpoint(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    path: []const u8,
    body: []const u8,
    provided_token: ?[]const u8,
) !void {
    const tool = "platform.rollback_webapp";
    requireControlToken(allocator, provided_token) catch {
        auditControlCommand(allocator, path, tool, "rejected", "control_auth_required", null, null);
        return writeControlError(allocator, stream, 401, "control_auth_required", "A valid X-Platform-Control-Token header is required");
    };

    const app_id = appIdFromRollbackPath(path) orelse {
        auditControlCommand(allocator, path, tool, "rejected", "invalid_request", body, null);
        return writeControlError(allocator, stream, 400, "invalid_request", "Rollback route requires app id");
    };
    const args = parseControlArgs(allocator, body) catch {
        auditControlCommand(allocator, path, tool, "rejected", "invalid_request", null, null);
        return writeControlError(allocator, stream, 400, "invalid_request", "Rollback request body must be a JSON object");
    };
    defer if (args) |*parsed| parsed.deinit();
    const root = if (args) |parsed| parsed.value else null;
    const target_install_id = if (root) |value| valueString(value.object.get("installId")) else null;
    const snapshot_id = if (root) |value| valueString(value.object.get("snapshotId")) else null;
    const args_json = try controlArgsJsonForAudit(allocator, body);
    defer allocator.free(args_json);

    const result_json = rollbackWebappPackage(allocator, app_id, target_install_id, snapshot_id) catch |err| switch (err) {
        error.AppNotInstalled => {
            auditControlCommand(allocator, path, tool, "rejected", "app_not_installed", args_json, null);
            return writeControlError(allocator, stream, 400, "app_not_installed", "App is not installed");
        },
        error.NoRollbackTarget => {
            auditControlCommand(allocator, path, tool, "rejected", "no_rollback_target", args_json, null);
            return writeControlError(allocator, stream, 400, "no_rollback_target", "No rollback target exists");
        },
        error.RollbackTargetInvalid => {
            auditControlCommand(allocator, path, tool, "rejected", "rollback_target_invalid", args_json, null);
            return writeControlError(allocator, stream, 400, "rollback_target_invalid", "Rollback target is invalid");
        },
        error.RollbackDataVersionIncompatible => {
            auditControlCommand(allocator, path, tool, "rejected", "rollback_data_version_incompatible", args_json, null);
            return writeControlError(allocator, stream, 400, "rollback_data_version_incompatible", "Rollback requires a compatible data version or explicit snapshotId");
        },
        error.SnapshotNotFound => {
            auditControlCommand(allocator, path, tool, "rejected", "snapshot_not_found", args_json, null);
            return writeControlError(allocator, stream, 400, "snapshot_not_found", "Snapshot was not found");
        },
        error.SnapshotInvalid => {
            auditControlCommand(allocator, path, tool, "rejected", "snapshot_invalid", args_json, null);
            return writeControlError(allocator, stream, 400, "snapshot_invalid", "Snapshot cannot be restored for this rollback target");
        },
        else => return err,
    };
    defer allocator.free(result_json);
    auditControlCommand(allocator, path, tool, "accepted", null, args_json, result_json);
    return writeControlOkRaw(allocator, stream, result_json);
}

fn handleDbControlEndpoint(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    path: []const u8,
    body: []const u8,
    provided_token: ?[]const u8,
) !void {
    const audit_tool = controlToolForDbPath(path);
    requireControlToken(allocator, provided_token) catch {
        auditControlCommand(allocator, path, audit_tool, "rejected", "control_auth_required", null, null);
        return writeControlError(allocator, stream, 401, "control_auth_required", "A valid X-Platform-Control-Token header is required");
    };

    const args = parseControlArgs(allocator, body) catch {
        auditControlCommand(allocator, path, audit_tool, "rejected", "invalid_request", null, null);
        return writeControlError(allocator, stream, 400, "invalid_request", "Control request body must be a JSON object");
    };
    defer if (args) |*parsed| parsed.deinit();
    const root = if (args) |parsed| parsed.value else null;
    const app_id = if (root) |value| valueString(value.object.get("appId")) else null;
    const args_json = try controlArgsJsonForAudit(allocator, body);
    defer allocator.free(args_json);

    if (std.mem.eql(u8, path, "/db/snapshot") or std.mem.eql(u8, path, "/control/db/snapshot")) {
        const result_json = try dbSnapshotJson(allocator);
        defer allocator.free(result_json);
        auditControlCommand(allocator, path, audit_tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, path, "/db/app-storage") or std.mem.eql(u8, path, "/control/db/app-storage")) {
        const actual_app_id = app_id orelse {
            auditControlCommand(allocator, path, audit_tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "db.query_app_storage requires appId");
        };
        const result_json = try queryAppStorageRowsJson(allocator, actual_app_id);
        defer allocator.free(result_json);
        auditControlCommand(allocator, path, audit_tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, path, "/db/bridge-calls") or std.mem.eql(u8, path, "/control/db/bridge-calls")) {
        const result_json = try queryBridgeCallsRowsJson(allocator, app_id);
        defer allocator.free(result_json);
        auditControlCommand(allocator, path, audit_tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, path, "/db/app-versions") or std.mem.eql(u8, path, "/control/db/app-versions")) {
        const actual_app_id = app_id orelse {
            auditControlCommand(allocator, path, audit_tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "db.query_app_versions requires appId");
        };
        const result_json = try queryAppVersionsRowsJson(allocator, actual_app_id);
        defer allocator.free(result_json);
        auditControlCommand(allocator, path, audit_tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, path, "/db/core-events") or std.mem.eql(u8, path, "/control/db/core-events")) {
        const result_json = try queryCoreEventsRowsJson(allocator, app_id);
        defer allocator.free(result_json);
        auditControlCommand(allocator, path, audit_tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, path, "/db/test-runs") or std.mem.eql(u8, path, "/control/db/test-runs")) {
        const result_json = try queryTestRunsRowsJson(allocator, app_id);
        defer allocator.free(result_json);
        auditControlCommand(allocator, path, audit_tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, path, "/db/export-debug-bundle") or std.mem.eql(u8, path, "/control/db/export-debug-bundle")) {
        const result_json = try dbDebugBundleJson(allocator);
        defer allocator.free(result_json);
        recordBackupExport(allocator, result_json) catch |err| {
            std.debug.print("debug bundle export record failed: {}\n", .{err});
        };
        auditControlCommand(allocator, path, audit_tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }

    auditControlCommand(allocator, path, audit_tool, "rejected", "not_found", args_json, null);
    return writeControlError(allocator, stream, 404, "not_found", "Control route not found");
}

fn handleControlCommand(
    allocator: std.mem.Allocator,
    stream: std.net.Stream,
    body: []const u8,
    provided_token: ?[]const u8,
) !void {
    requireControlToken(allocator, provided_token) catch {
        auditControlCommand(allocator, "/control/command", "control.command", "rejected", "control_auth_required", null, null);
        return writeControlError(allocator, stream, 401, "control_auth_required", "A valid X-Platform-Control-Token header is required");
    };

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, body, .{}) catch {
        auditControlCommand(allocator, "/control/command", "control.command", "rejected", "invalid_request", null, null);
        return writeControlError(allocator, stream, 400, "invalid_request", "Control command body must be valid JSON");
    };
    defer parsed.deinit();

    const root = parsed.value;
    if (root != .object) {
        auditControlCommand(allocator, "/control/command", "control.command", "rejected", "invalid_request", null, null);
        return writeControlError(allocator, stream, 400, "invalid_request", "Control command body must be an object");
    }

    const tool = valueString(root.object.get("tool")) orelse {
        auditControlCommand(allocator, "/control/command", "control.command", "rejected", "invalid_request", null, null);
        return writeControlError(allocator, stream, 400, "invalid_request", "Control command requires tool");
    };
    const args = if (root.object.get("args")) |args_value| args_value else null;
    if (args) |args_value| {
        if (args_value != .object) {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", null, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "Control command args must be an object");
        }
    }
    const args_json = if (args) |args_value|
        try jsonValueAlloc(allocator, args_value)
    else
        try allocator.dupe(u8, "{}");
    defer allocator.free(args_json);

    if (std.mem.eql(u8, tool, "platform.health")) {
        const result_json = "{\"name\":\"zig-server\",\"version\":\"0.1.0\",\"targets\":[\"zig-server\"],\"db\":\"sqlite\"}";
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.capabilities")) {
        const result_json = try serverCapabilitiesJson(allocator);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.validate_package") or std.mem.eql(u8, tool, "platform.run_policy_audit")) {
        const package_root = (if (args) |args_value| packageRootValue(args_value) else null) orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "Package validation requires an inline package object");
        };
        const result_json = try validateWebappPackageValue(allocator, package_root);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.sign_webapp_package")) {
        const package_root = (if (args) |args_value| packageRootValue(args_value) else null) orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "Package signing requires an inline package object");
        };
        const trust_level = controlStringArg(args, "trustLevel") orelse "developer";
        const result_json = signWebappPackage(allocator, package_root, trust_level) catch |err| switch (err) {
            error.InvalidWebappPackage => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_package", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_package", "Package validation failed");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.install_webapp_package")) {
        const package_root = (if (args) |args_value| packageRootValue(args_value) else null) orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "Package install requires an inline package object");
        };
        const activate = controlBoolArg(args, "activate") orelse true;
        const trust_level = controlStringArg(args, "trustLevel") orelse "developer";
        const result_json = installWebappPackage(allocator, package_root, activate, trust_level) catch |err| switch (err) {
            error.InvalidWebappPackage => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_package", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_package", "Package validation failed");
            },
            error.InvalidMigration => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_migration", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_migration", "Package migration chain is invalid or incomplete");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.list_webapps")) {
        const result_json = try queryRowsJson(allocator, "SELECT id, name, status, active_install_id, active_version, data_version, created_at, updated_at FROM apps ORDER BY id", null);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.list_webapp_versions")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "platform.list_webapp_versions requires appId");
        };
        const result_json = try queryAppVersionsRowsJson(allocator, app_id);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.install_report")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "platform.install_report requires appId");
        };
        const result_json = try queryInstallReportRowsJson(allocator, app_id, controlStringArg(args, "installId"));
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.rollback_webapp")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "platform.rollback_webapp requires appId");
        };
        const result_json = rollbackWebappPackage(allocator, app_id, controlStringArg(args, "installId"), controlStringArg(args, "snapshotId")) catch |err| switch (err) {
            error.AppNotInstalled => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "app_not_installed", args_json, null);
                return writeControlError(allocator, stream, 400, "app_not_installed", "App is not installed");
            },
            error.NoRollbackTarget => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "no_rollback_target", args_json, null);
                return writeControlError(allocator, stream, 400, "no_rollback_target", "No rollback target exists");
            },
            error.RollbackTargetInvalid => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "rollback_target_invalid", args_json, null);
                return writeControlError(allocator, stream, 400, "rollback_target_invalid", "Rollback target is invalid");
            },
            error.RollbackDataVersionIncompatible => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "rollback_data_version_incompatible", args_json, null);
                return writeControlError(allocator, stream, 400, "rollback_data_version_incompatible", "Rollback requires a compatible data version or explicit snapshotId");
            },
            error.SnapshotNotFound => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "snapshot_not_found", args_json, null);
                return writeControlError(allocator, stream, 400, "snapshot_not_found", "Snapshot was not found");
            },
            error.SnapshotInvalid => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "snapshot_invalid", args_json, null);
                return writeControlError(allocator, stream, 400, "snapshot_invalid", "Snapshot cannot be restored for this rollback target");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.create_snapshot")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "platform.create_snapshot requires appId");
        };
        const snapshot_type = controlStringArg(args, "type") orelse "bug-report";
        if (!isAllowedSnapshotType(snapshot_type)) {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "Unsupported snapshot type");
        }
        const result_json = createRuntimeSnapshot(allocator, app_id, snapshot_type, controlStringArg(args, "sessionId")) catch |err| switch (err) {
            error.AppNotInstalled => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "app_not_installed", args_json, null);
                return writeControlError(allocator, stream, 400, "app_not_installed", "App is not installed");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.restore_snapshot")) {
        const snapshot_id = controlStringArg(args, "snapshotId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "platform.restore_snapshot requires snapshotId");
        };
        const result_json = restoreRuntimeSnapshot(allocator, snapshot_id) catch |err| switch (err) {
            error.SnapshotNotFound => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "snapshot_not_found", args_json, null);
                return writeControlError(allocator, stream, 400, "snapshot_not_found", "Snapshot was not found");
            },
            error.SnapshotInvalid => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "snapshot_invalid", args_json, null);
                return writeControlError(allocator, stream, 400, "snapshot_invalid", "Snapshot cannot be restored");
            },
            error.RollbackTargetInvalid => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "snapshot_target_invalid", args_json, null);
                return writeControlError(allocator, stream, 400, "snapshot_target_invalid", "Snapshot target install is invalid");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.network_mock_set")) {
        const args_value = args orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "runtime.network_mock_set requires args");
        };
        const result_json = insertNetworkMockControl(allocator, args_value) catch |err| switch (err) {
            error.InvalidControlArgs => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_request", "runtime.network_mock_set requires urlPattern or match.url and response");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "runtime.network_mock_reset")) {
        const result_json = try resetNetworkMocksControl(allocator, args);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "platform.migration_dry_run") or std.mem.eql(u8, tool, "platform.migration_apply")) {
        const migration_value = if (args) |args_value| args_value.object.get("migration") else null;
        const migration = migration_value orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "Migration command requires migration object");
        };
        if (migration != .object) {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "Migration must be an object");
        }
        const mode = if (std.mem.eql(u8, tool, "platform.migration_apply")) "apply" else "dry-run";
        const result_json = runStorageMigration(allocator, migration, mode) catch |err| switch (err) {
            error.AppNotInstalled => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "app_not_installed", args_json, null);
                return writeControlError(allocator, stream, 400, "app_not_installed", "App is not installed");
            },
            error.InvalidMigration => {
                auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_migration", args_json, null);
                return writeControlError(allocator, stream, 400, "invalid_migration", "Migration is invalid or unsupported");
            },
            else => return err,
        };
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.snapshot")) {
        const result_json = try dbSnapshotJson(allocator);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.query_app_storage")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "db.query_app_storage requires appId");
        };
        const result_json = try queryAppStorageRowsJson(allocator, app_id);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.query_bridge_calls") or std.mem.eql(u8, tool, "runtime.bridge_calls")) {
        const result_json = try queryBridgeCallsRowsJson(allocator, controlStringArg(args, "appId"));
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.query_app_versions")) {
        const app_id = controlStringArg(args, "appId") orelse {
            auditControlCommand(allocator, "/control/command", tool, "rejected", "invalid_request", args_json, null);
            return writeControlError(allocator, stream, 400, "invalid_request", "db.query_app_versions requires appId");
        };
        const result_json = try queryAppVersionsRowsJson(allocator, app_id);
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.query_core_events")) {
        const result_json = try queryCoreEventsRowsJson(allocator, controlStringArg(args, "appId"));
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.query_test_runs")) {
        const result_json = try queryTestRunsRowsJson(allocator, controlStringArg(args, "appId"));
        defer allocator.free(result_json);
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }
    if (std.mem.eql(u8, tool, "db.export_debug_bundle")) {
        const result_json = try dbDebugBundleJson(allocator);
        defer allocator.free(result_json);
        recordBackupExport(allocator, result_json) catch |err| {
            std.debug.print("debug bundle export record failed: {}\n", .{err});
        };
        auditControlCommand(allocator, "/control/command", tool, "accepted", null, args_json, result_json);
        return writeControlOkRaw(allocator, stream, result_json);
    }

    auditControlCommand(allocator, "/control/command", tool, "rejected", "unknown_tool", args_json, null);
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

fn controlArgsJsonForAudit(allocator: std.mem.Allocator, body: []const u8) ![]u8 {
    const trimmed = std.mem.trim(u8, body, " \t\r\n");
    if (trimmed.len == 0) return allocator.dupe(u8, "{}");
    return allocator.dupe(u8, trimmed);
}

fn controlToolForDbPath(path: []const u8) []const u8 {
    if (std.mem.eql(u8, path, "/db/snapshot") or std.mem.eql(u8, path, "/control/db/snapshot")) return "db.snapshot";
    if (std.mem.eql(u8, path, "/db/app-storage") or std.mem.eql(u8, path, "/control/db/app-storage")) return "db.query_app_storage";
    if (std.mem.eql(u8, path, "/db/app-versions") or std.mem.eql(u8, path, "/control/db/app-versions")) return "db.query_app_versions";
    if (std.mem.eql(u8, path, "/db/bridge-calls") or std.mem.eql(u8, path, "/control/db/bridge-calls")) return "db.query_bridge_calls";
    if (std.mem.eql(u8, path, "/db/core-events") or std.mem.eql(u8, path, "/control/db/core-events")) return "db.query_core_events";
    if (std.mem.eql(u8, path, "/db/test-runs") or std.mem.eql(u8, path, "/control/db/test-runs")) return "db.query_test_runs";
    if (std.mem.eql(u8, path, "/db/export-debug-bundle") or std.mem.eql(u8, path, "/control/db/export-debug-bundle")) return "db.export_debug_bundle";
    return "control.db";
}

fn controlToolForPackagePath(path: []const u8) []const u8 {
    if (std.mem.eql(u8, path, "/packages/validate") or std.mem.eql(u8, path, "/control/packages/validate")) return "platform.validate_package";
    if (std.mem.eql(u8, path, "/packages/sign") or std.mem.eql(u8, path, "/control/packages/sign")) return "platform.sign_webapp_package";
    if (std.mem.eql(u8, path, "/packages/policy-audit") or std.mem.eql(u8, path, "/control/packages/policy-audit")) return "platform.run_policy_audit";
    return "control.packages";
}

fn appIdFromRollbackPath(path: []const u8) ?[]const u8 {
    const prefix = "/apps/";
    const suffix = "/rollback";
    if (!std.mem.startsWith(u8, path, prefix) or !std.mem.endsWith(u8, path, suffix)) return null;
    const app_id = path[prefix.len .. path.len - suffix.len];
    if (app_id.len == 0 or std.mem.indexOfScalar(u8, app_id, '/') != null) return null;
    return app_id;
}

fn controlStringArg(args: ?std.json.Value, name: []const u8) ?[]const u8 {
    const value = args orelse return null;
    if (value != .object) return null;
    return valueString(value.object.get(name));
}

fn controlBoolArg(args: ?std.json.Value, name: []const u8) ?bool {
    const value = args orelse return null;
    if (value != .object) return null;
    const actual = value.object.get(name) orelse return null;
    if (actual != .bool) return null;
    return actual.bool;
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

    try collectWebappPackageErrors(allocator, parsed.value, &errors);
    return validationReportAlloc(allocator, errors.items);
}

fn validateWebappPackageValue(allocator: std.mem.Allocator, root: std.json.Value) ![]u8 {
    var errors: std.ArrayList([]const u8) = .empty;
    defer errors.deinit(allocator);
    try collectWebappPackageErrors(allocator, root, &errors);
    return validationReportAlloc(allocator, errors.items);
}

fn packageRootValue(value: std.json.Value) ?std.json.Value {
    if (value != .object) return null;
    if (value.object.get("package")) |package| {
        if (package == .object and package.object.get("manifest") != null and package.object.get("files") != null) {
            return package;
        }
    }
    if (value.object.get("manifest") != null and value.object.get("files") != null) {
        return value;
    }
    return null;
}

fn collectWebappPackageErrors(
    allocator: std.mem.Allocator,
    root: std.json.Value,
    errors: *std.ArrayList([]const u8),
) !void {
    if (root != .object) {
        try errors.append(allocator, "invalid_package_shape");
        return;
    }

    const manifest = root.object.get("manifest") orelse {
        try errors.append(allocator, "missing_manifest");
        return;
    };
    if (manifest != .object) {
        try errors.append(allocator, "invalid_manifest");
        return;
    }

    const files = root.object.get("files") orelse {
        try errors.append(allocator, "missing_files");
        return;
    };
    if (files != .array) {
        try errors.append(allocator, "invalid_files");
        return;
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
    const string_manifest_fields = [_][]const u8{ "id", "name", "version", "runtimeVersion", "entry", "description", "storagePrefix" };
    for (string_manifest_fields) |field| {
        if (manifest.object.get(field)) |value| {
            if (valueString(value) == null) try errors.append(allocator, "invalid_manifest_field");
        }
    }

    if (manifest.object.get("networkAllowlist") != null) {
        try errors.append(allocator, "removed_manifest_field");
    }
    if (valueString(manifest.object.get("id"))) |app_id| {
        if (!isValidAppId(app_id)) try errors.append(allocator, "invalid_manifest_id");
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
    if (manifest.object.get("permissions")) |permissions| {
        if (permissions != .array) {
            try errors.append(allocator, "invalid_permissions");
        } else {
            for (permissions.array.items) |permission| {
                if (valueString(permission) == null) try errors.append(allocator, "invalid_permissions");
            }
        }
    }
    if (manifest.object.get("capabilities")) |capabilities| {
        if (capabilities != .object) try errors.append(allocator, "invalid_capabilities");
    }
    if (manifest.object.get("resourceBudget")) |resource_budget| {
        if (resource_budget != .object) try errors.append(allocator, "invalid_resource_budget");
    }
    if (manifest.object.get("networkPolicy")) |network_policy| {
        if (network_policy != .object) try errors.append(allocator, "invalid_network_policy");
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
}

const PackageFile = struct {
    path: []const u8,
    content: []const u8,
    content_hash: []u8,
};

const PackageHashes = struct {
    manifest_hash: []u8,
    content_hash: []u8,
    permissions_hash: []u8,
    policy_hash: []u8,
    file_records_json: []u8,
    file_hashes_json: []u8,
};

const SmokeTestEvaluation = struct {
    ok: bool,
    status: []const u8,
    result_json: []u8,
    spec_json: []u8,
};

fn signWebappPackage(
    allocator: std.mem.Allocator,
    package_root: std.json.Value,
    trust_level: []const u8,
) ![]u8 {
    var errors: std.ArrayList([]const u8) = .empty;
    defer errors.deinit(allocator);
    try collectWebappPackageErrors(allocator, package_root, &errors);
    if (errors.items.len > 0) return error.InvalidWebappPackage;

    const manifest = package_root.object.get("manifest").?;
    const permissions = manifest.object.get("permissions").?;
    const files_value = package_root.object.get("files").?;
    const actual_trust_level = if (isTrustLevel(trust_level)) trust_level else "developer";
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const signed_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(signed_at);
    const manifest_json = try jsonValueAlloc(allocator, manifest);
    defer allocator.free(manifest_json);
    const package_files = try packageFilesAlloc(allocator, files_value);
    defer freePackageFiles(allocator, package_files);
    const hashes = try packageHashesAlloc(allocator, manifest, manifest_json, permissions, package_files);
    defer freePackageHashes(allocator, hashes);
    const signature_json = try serverSignatureJsonAlloc(allocator, manifest, actual_trust_level, hashes, signed_at);
    defer allocator.free(signature_json);
    const content_hashes_json = try contentHashesDocumentJsonAlloc(allocator, hashes);
    defer allocator.free(content_hashes_json);
    return std.fmt.allocPrint(
        allocator,
        "{{\"signature\":{s},\"hashes\":{{\"manifestHash\":\"{s}\",\"contentHash\":\"{s}\",\"permissionsHash\":\"{s}\",\"policyHash\":\"{s}\",\"fileHashes\":{s},\"fileRecords\":{s}}},\"contentHashesDocument\":{s}}}",
        .{ signature_json, hashes.manifest_hash, hashes.content_hash, hashes.permissions_hash, hashes.policy_hash, hashes.file_hashes_json, hashes.file_records_json, content_hashes_json },
    );
}

fn installWebappPackage(
    allocator: std.mem.Allocator,
    package_root: std.json.Value,
    activate_requested: bool,
    trust_level: []const u8,
) ![]u8 {
    var errors: std.ArrayList([]const u8) = .empty;
    defer errors.deinit(allocator);
    try collectWebappPackageErrors(allocator, package_root, &errors);
    if (errors.items.len > 0) return error.InvalidWebappPackage;

    const manifest = package_root.object.get("manifest").?;
    const files_value = package_root.object.get("files").?;
    const app_id = valueString(manifest.object.get("id")).?;
    const app_name = valueString(manifest.object.get("name")).?;
    const app_version = valueString(manifest.object.get("version")).?;
    const app_runtime_version = valueString(manifest.object.get("runtimeVersion")).?;
    const data_version = manifest.object.get("dataVersion").?.integer;
    const permissions = manifest.object.get("permissions").?;
    const actual_trust_level = if (isTrustLevel(trust_level)) trust_level else "developer";

    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const created_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(created_at);
    const install_id = try randomDbIdAlloc(allocator, db, "install_");
    defer allocator.free(install_id);
    const report_id = try randomDbIdAlloc(allocator, db, "report_");
    defer allocator.free(report_id);

    const manifest_json = try jsonValueAlloc(allocator, manifest);
    defer allocator.free(manifest_json);
    const package_files = try packageFilesAlloc(allocator, files_value);
    defer freePackageFiles(allocator, package_files);
    const hashes = try packageHashesAlloc(allocator, manifest, manifest_json, permissions, package_files);
    defer freePackageHashes(allocator, hashes);
    const signature_json = try serverSignatureJsonAlloc(allocator, manifest, actual_trust_level, hashes, created_at);
    defer allocator.free(signature_json);
    const content_hashes_json = try contentHashesDocumentJsonAlloc(allocator, hashes);
    defer allocator.free(content_hashes_json);
    const security_json = try std.fmt.allocPrint(allocator, "{{\"ok\":true,\"signature\":{s},\"contentHashes\":{s}}}", .{ signature_json, content_hashes_json });
    defer allocator.free(security_json);
    const validation_json = try validationReportAlloc(allocator, &.{});
    defer allocator.free(validation_json);
    const compatibility_json = try std.fmt.allocPrint(allocator, "{{\"ok\":true,\"runtimeVersion\":\"{s}\",\"appRuntimeVersion\":\"{s}\"}}", .{ runtime_version, app_runtime_version });
    defer allocator.free(compatibility_json);
    const smoke_test = try evaluateSmokeTestsAlloc(allocator, package_root, app_id);
    defer freeSmokeTestEvaluation(allocator, smoke_test);

    try execDb(db, "BEGIN IMMEDIATE");
    errdefer execDb(db, "ROLLBACK") catch {};

    const previous_install_id = try activeInstallIdAlloc(allocator, db, app_id);
    defer if (previous_install_id) |previous| allocator.free(previous);
    const existing_data_version = try appDataVersion(db, app_id);
    const requires_approval = try packageAddsPermissions(db, permissions, previous_install_id);
    const activate = activate_requested and !requires_approval and smoke_test.ok;
    const blocked_by_smoke = !smoke_test.ok;
    const version_status = if (activate) "enabled" else if (blocked_by_smoke) "quarantined" else "installed";
    const app_status = if (activate or previous_install_id != null) "enabled" else if (blocked_by_smoke) "quarantined" else "disabled";
    const stored_data_version = if (activate or previous_install_id == null) data_version else existing_data_version orelse data_version;
    const report_status = if (blocked_by_smoke) "failed" else if (requires_approval) "requires-approval" else "accepted";
    const permissions_json = try permissionsReportJsonAlloc(allocator, permissions, activate, requires_approval, previous_install_id);
    defer allocator.free(permissions_json);

    try upsertInstalledApp(db, app_id, app_name, app_status, stored_data_version, created_at);
    try insertAppVersion(db, install_id, app_id, app_version, app_runtime_version, data_version, manifest_json, hashes, signature_json, actual_trust_level, version_status, created_at, activate);
    for (package_files) |file| {
        try insertAppFile(db, install_id, file, created_at);
    }
    try insertAppPermissions(db, install_id, app_id, permissions, activate, created_at);
    try insertInstallReport(db, report_id, app_id, install_id, report_status, validation_json, security_json, permissions_json, compatibility_json, smoke_test.result_json, hashes.content_hash, created_at);
    try insertSmokeTestRun(db, allocator, app_id, smoke_test.status, smoke_test.spec_json, smoke_test.result_json, created_at);
    try insertInstallationEvent(db, allocator, app_id, install_id, "install", null, report_id, created_at, "zig-server", version_status);
    if (activate) {
        if (existing_data_version) |from_data_version| {
            if (data_version < from_data_version) return error.InvalidMigration;
            if (data_version > from_data_version) {
                try applyPackagedMigrationChainForInstall(allocator, db, app_id, install_id, package_files, from_data_version, data_version, created_at);
            }
        }
        if (previous_install_id != null) {
            try markPreviousVersionInstalled(db, previous_install_id.?);
        }
        try insertInstallationEvent(db, allocator, app_id, install_id, "activate", previous_install_id, report_id, created_at, "zig-server", "active");
        try activateInstalledApp(db, app_id, install_id, app_version, data_version, created_at);
    } else if (blocked_by_smoke) {
        try insertInstallationEvent(db, allocator, app_id, install_id, "quarantine", previous_install_id, report_id, created_at, "zig-server", "smoke-test failed");
    }

    const result_json = try installResultJsonAlloc(allocator, app_id, install_id, report_id, app_version, version_status, activate, requires_approval, hashes.content_hash);
    errdefer allocator.free(result_json);
    try execDb(db, "COMMIT");
    return result_json;
}

fn installResultJsonAlloc(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    install_id: []const u8,
    report_id: []const u8,
    app_version: []const u8,
    version_status: []const u8,
    activated: bool,
    requires_approval: bool,
    content_hash: []const u8,
) ![]u8 {
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_install_id = try escapeJsonString(allocator, install_id);
    defer allocator.free(escaped_install_id);
    const escaped_report_id = try escapeJsonString(allocator, report_id);
    defer allocator.free(escaped_report_id);
    const escaped_version = try escapeJsonString(allocator, app_version);
    defer allocator.free(escaped_version);
    const escaped_content_hash = try escapeJsonString(allocator, content_hash);
    defer allocator.free(escaped_content_hash);
    return std.fmt.allocPrint(
        allocator,
        "{{\"appId\":\"{s}\",\"installId\":\"{s}\",\"reportId\":\"{s}\",\"version\":\"{s}\",\"status\":\"{s}\",\"activated\":{},\"requiresUserApproval\":{},\"contentHash\":\"{s}\"}}",
        .{ escaped_app_id, escaped_install_id, escaped_report_id, escaped_version, version_status, activated, requires_approval, escaped_content_hash },
    );
}

const InstalledVersion = struct {
    install_id: []u8,
    version: []u8,
    data_version: i64,
    status: []u8,
};

fn rollbackWebappPackage(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    target_install_id: ?[]const u8,
    snapshot_id: ?[]const u8,
) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    try execDb(db, "BEGIN IMMEDIATE");
    errdefer execDb(db, "ROLLBACK") catch {};

    const active = try activeInstallDetailsAlloc(allocator, db, app_id);
    const active_version = active orelse return error.AppNotInstalled;
    defer freeInstalledVersion(allocator, active_version);

    if (target_install_id) |explicit_target| {
        if (std.mem.eql(u8, explicit_target, active_version.install_id)) return error.RollbackTargetInvalid;
    }

    const target = try rollbackTargetAlloc(allocator, db, app_id, active_version.install_id, target_install_id);
    const target_version = target orelse return error.NoRollbackTarget;
    defer freeInstalledVersion(allocator, target_version);

    if (std.mem.eql(u8, target_version.status, "quarantined") or std.mem.eql(u8, target_version.status, "uninstalled")) {
        return error.RollbackTargetInvalid;
    }

    const created_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(created_at);

    var restored_storage_keys: ?usize = null;
    if (target_version.data_version > active_version.data_version) {
        return error.RollbackDataVersionIncompatible;
    }
    if (target_version.data_version < active_version.data_version and snapshot_id == null) {
        return error.RollbackDataVersionIncompatible;
    }
    if (snapshot_id) |restore_snapshot_id| {
        restored_storage_keys = try restoreSnapshotStorageIntoDb(allocator, db, restore_snapshot_id, app_id, target_version.install_id, created_at);
    }

    try markVersionStatus(db, active_version.install_id, "rolled-back", null);
    try markVersionStatus(db, target_version.install_id, "enabled", created_at);
    try activateInstalledApp(db, app_id, target_version.install_id, target_version.version, target_version.data_version, created_at);
    try insertRollbackInstallationEvent(db, allocator, app_id, target_version.install_id, active_version.install_id, created_at);

    const result_json = try rollbackResultJsonAlloc(allocator, app_id, target_version.install_id, active_version.install_id, target_version.version, target_version.data_version, snapshot_id, restored_storage_keys);
    errdefer allocator.free(result_json);
    try execDb(db, "COMMIT");
    return result_json;
}

fn activeInstallDetailsAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, app_id: []const u8) !?InstalledVersion {
    return installedVersionFromQueryAlloc(
        allocator,
        db,
        "SELECT v.install_id, v.version, v.data_version, v.status FROM apps a JOIN app_versions v ON v.install_id = a.active_install_id WHERE a.id = ?",
        app_id,
        null,
    );
}

fn rollbackTargetAlloc(
    allocator: std.mem.Allocator,
    db: *sqlite.sqlite3,
    app_id: []const u8,
    active_install_id: []const u8,
    target_install_id: ?[]const u8,
) !?InstalledVersion {
    if (target_install_id) |target| {
        return installedVersionFromQueryAlloc(
            allocator,
            db,
            "SELECT install_id, version, data_version, status FROM app_versions WHERE app_id = ? AND install_id = ?",
            app_id,
            target,
        );
    }
    return installedVersionFromQueryAlloc(
        allocator,
        db,
        "SELECT install_id, version, data_version, status FROM app_versions WHERE app_id = ? AND install_id != ? AND status NOT IN ('quarantined','uninstalled') ORDER BY created_at DESC LIMIT 1",
        app_id,
        active_install_id,
    );
}

fn installedVersionFromQueryAlloc(
    allocator: std.mem.Allocator,
    db: *sqlite.sqlite3,
    sql: [:0]const u8,
    first: []const u8,
    second: ?[]const u8,
) !?InstalledVersion {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, sql, -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, first);
    if (second) |value| bindText(statement, 2, value);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return null;
    const install_id = try allocator.dupe(u8, sqliteColumnText(statement, 0));
    errdefer allocator.free(install_id);
    const version = try allocator.dupe(u8, sqliteColumnText(statement, 1));
    errdefer allocator.free(version);
    const status = try allocator.dupe(u8, sqliteColumnText(statement, 3));
    errdefer allocator.free(status);
    return .{
        .install_id = install_id,
        .version = version,
        .data_version = sqlite.sqlite3_column_int64(statement, 2),
        .status = status,
    };
}

fn freeInstalledVersion(allocator: std.mem.Allocator, version: InstalledVersion) void {
    allocator.free(version.install_id);
    allocator.free(version.version);
    allocator.free(version.status);
}

fn rollbackResultJsonAlloc(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    active_install_id: []const u8,
    rolled_back_install_id: []const u8,
    active_version: []const u8,
    data_version: i64,
    snapshot_id: ?[]const u8,
    restored_storage_keys: ?usize,
) ![]u8 {
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_active = try escapeJsonString(allocator, active_install_id);
    defer allocator.free(escaped_active);
    const escaped_rolled_back = try escapeJsonString(allocator, rolled_back_install_id);
    defer allocator.free(escaped_rolled_back);
    const escaped_version = try escapeJsonString(allocator, active_version);
    defer allocator.free(escaped_version);
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.print(
        "{{\"appId\":\"{s}\",\"activeInstallId\":\"{s}\",\"rolledBackInstallId\":\"{s}\",\"activeVersion\":\"{s}\",\"dataVersion\":{d}",
        .{ escaped_app_id, escaped_active, escaped_rolled_back, escaped_version, data_version },
    );
    if (snapshot_id) |actual_snapshot_id| {
        try out.writer.writeAll(",\"dataRollbackSnapshotId\":");
        try appendJsonString(allocator, &out, actual_snapshot_id);
    }
    if (restored_storage_keys) |count| {
        try out.writer.print(",\"restoredStorageKeys\":{d}", .{count});
    }
    try out.writer.writeAll("}");
    return out.toOwnedSlice();
}

const SnapshotActiveApp = struct {
    app_id: []u8,
    install_id: []u8,
    manifest_hash: []u8,
    content_hash: []u8,
};

fn createRuntimeSnapshot(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    snapshot_type: []const u8,
    session_id: ?[]const u8,
) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    return createRuntimeSnapshotInDb(allocator, db, app_id, snapshot_type, session_id);
}

fn createRuntimeSnapshotInDb(
    allocator: std.mem.Allocator,
    db: *sqlite.sqlite3,
    app_id: []const u8,
    snapshot_type: []const u8,
    session_id: ?[]const u8,
) ![]u8 {
    const active = try activeSnapshotAppAlloc(allocator, db, app_id);
    const active_app = active orelse return error.AppNotInstalled;
    defer freeSnapshotActiveApp(allocator, active_app);

    const snapshot_id = try randomDbIdAlloc(allocator, db, "snapshot_");
    defer allocator.free(snapshot_id);
    const created_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(created_at);
    const capabilities = try serverCapabilitiesJson(allocator);
    defer allocator.free(capabilities);
    const storage = try snapshotStorageObjectJsonAlloc(allocator, db, app_id);
    defer allocator.free(storage);
    const bridge_calls = try queryBridgeCallsRowsJson(allocator, app_id);
    defer allocator.free(bridge_calls);
    const core_events = try queryCoreEventsRowsJson(allocator, app_id);
    defer allocator.free(core_events);
    const resource_usage = try snapshotResourceUsageJsonAlloc(allocator, db, app_id);
    defer allocator.free(resource_usage);

    const snapshot_json = try snapshotDocumentJsonAlloc(
        allocator,
        snapshot_id,
        snapshot_type,
        created_at,
        active_app,
        capabilities,
        storage,
        bridge_calls,
        core_events,
        resource_usage,
    );
    errdefer allocator.free(snapshot_json);
    const content_hash = try sha256PrefixedAlloc(allocator, snapshot_json);
    defer allocator.free(content_hash);
    try insertRuntimeSnapshot(db, snapshot_id, session_id, active_app.app_id, active_app.install_id, snapshot_type, snapshot_json, content_hash, created_at);
    return snapshot_json;
}

fn restoreRuntimeSnapshot(allocator: std.mem.Allocator, snapshot_id: []const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    const snapshot_json = try snapshotJsonByIdAlloc(allocator, db, snapshot_id);
    defer allocator.free(snapshot_json);
    const stored_hash = try snapshotContentHashByIdAlloc(allocator, db, snapshot_id);
    defer allocator.free(stored_hash);
    const actual_hash = try sha256PrefixedAlloc(allocator, snapshot_json);
    defer allocator.free(actual_hash);
    if (!std.mem.eql(u8, stored_hash, actual_hash)) return error.SnapshotInvalid;

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, snapshot_json, .{}) catch return error.SnapshotInvalid;
    defer parsed.deinit();
    if (parsed.value != .object) return error.SnapshotInvalid;
    const active_app_value = parsed.value.object.get("activeApp") orelse return error.SnapshotInvalid;
    if (active_app_value != .object) return error.SnapshotInvalid;
    const app_id = valueString(active_app_value.object.get("appId")) orelse return error.SnapshotInvalid;
    const install_id = valueString(active_app_value.object.get("installId")) orelse return error.SnapshotInvalid;
    const storage_value = parsed.value.object.get("storage") orelse return error.SnapshotInvalid;
    if (storage_value != .object) return error.SnapshotInvalid;

    try execDb(db, "BEGIN IMMEDIATE");
    errdefer execDb(db, "ROLLBACK") catch {};

    const target = try installedVersionFromQueryAlloc(
        allocator,
        db,
        "SELECT install_id, version, data_version, status FROM app_versions WHERE app_id = ? AND install_id = ?",
        app_id,
        install_id,
    );
    const target_version = target orelse return error.SnapshotInvalid;
    defer freeInstalledVersion(allocator, target_version);
    if (std.mem.eql(u8, target_version.status, "quarantined") or std.mem.eql(u8, target_version.status, "uninstalled")) {
        return error.RollbackTargetInvalid;
    }

    const current = try activeInstallDetailsAlloc(allocator, db, app_id);
    defer if (current) |version| freeInstalledVersion(allocator, version);
    if (current) |version| {
        if (!std.mem.eql(u8, version.install_id, target_version.install_id)) {
            try markVersionStatus(db, version.install_id, "installed", null);
        }
    }

    const restored_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(restored_at);
    try deleteAppStorageForApp(db, app_id);
    const restored_keys = try restoreSnapshotStorage(allocator, db, app_id, storage_value, restored_at);
    try markVersionStatus(db, target_version.install_id, "enabled", restored_at);
    try activateInstalledApp(db, app_id, target_version.install_id, target_version.version, target_version.data_version, restored_at);

    const result_json = try restoreSnapshotResultJsonAlloc(allocator, snapshot_id, app_id, target_version.install_id, restored_keys);
    errdefer allocator.free(result_json);
    try execDb(db, "COMMIT");
    return result_json;
}

fn activeSnapshotAppAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, app_id: []const u8) !?SnapshotActiveApp {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT a.id, v.install_id, v.manifest_hash, v.content_hash FROM apps a JOIN app_versions v ON v.install_id = a.active_install_id WHERE a.id = ?",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return null;
    const active_app_id = try allocator.dupe(u8, sqliteColumnText(statement, 0));
    errdefer allocator.free(active_app_id);
    const install_id = try allocator.dupe(u8, sqliteColumnText(statement, 1));
    errdefer allocator.free(install_id);
    const manifest_hash = try allocator.dupe(u8, sqliteColumnText(statement, 2));
    errdefer allocator.free(manifest_hash);
    const content_hash = try allocator.dupe(u8, sqliteColumnText(statement, 3));
    errdefer allocator.free(content_hash);
    return .{
        .app_id = active_app_id,
        .install_id = install_id,
        .manifest_hash = manifest_hash,
        .content_hash = content_hash,
    };
}

fn freeSnapshotActiveApp(allocator: std.mem.Allocator, app: SnapshotActiveApp) void {
    allocator.free(app.app_id);
    allocator.free(app.install_id);
    allocator.free(app.manifest_hash);
    allocator.free(app.content_hash);
}

fn snapshotDocumentJsonAlloc(
    allocator: std.mem.Allocator,
    snapshot_id: []const u8,
    snapshot_type: []const u8,
    created_at: []const u8,
    active_app: SnapshotActiveApp,
    capabilities: []const u8,
    storage: []const u8,
    bridge_calls: []const u8,
    core_events: []const u8,
    resource_usage: []const u8,
) ![]u8 {
    const escaped_snapshot_id = try escapeJsonString(allocator, snapshot_id);
    defer allocator.free(escaped_snapshot_id);
    const escaped_type = try escapeJsonString(allocator, snapshot_type);
    defer allocator.free(escaped_type);
    const escaped_created_at = try escapeJsonString(allocator, created_at);
    defer allocator.free(escaped_created_at);
    const escaped_app_id = try escapeJsonString(allocator, active_app.app_id);
    defer allocator.free(escaped_app_id);
    const escaped_install_id = try escapeJsonString(allocator, active_app.install_id);
    defer allocator.free(escaped_install_id);
    const escaped_manifest_hash = try escapeJsonString(allocator, active_app.manifest_hash);
    defer allocator.free(escaped_manifest_hash);
    const escaped_content_hash = try escapeJsonString(allocator, active_app.content_hash);
    defer allocator.free(escaped_content_hash);
    return std.fmt.allocPrint(
        allocator,
        "{{\"snapshotId\":\"{s}\",\"type\":\"{s}\",\"createdAt\":\"{s}\",\"platform\":\"server\",\"target\":\"zig-server\",\"runtimeVersion\":\"{s}\",\"activeApp\":{{\"appId\":\"{s}\",\"installId\":\"{s}\",\"manifestHash\":\"{s}\",\"contentHash\":\"{s}\"}},\"capabilities\":{s},\"storage\":{s},\"bridgeCalls\":{s},\"coreEvents\":{s},\"console\":[],\"resourceUsage\":{s}}}",
        .{ escaped_snapshot_id, escaped_type, escaped_created_at, runtime_version, escaped_app_id, escaped_install_id, escaped_manifest_hash, escaped_content_hash, capabilities, storage, bridge_calls, core_events, resource_usage },
    );
}

fn snapshotStorageObjectJsonAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, app_id: []const u8) ![]u8 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT key, value_json FROM app_storage WHERE app_id = ? ORDER BY key", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{");
    var count: usize = 0;
    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        if (count > 0) try out.writer.writeAll(",");
        try appendJsonString(allocator, &out, sqliteColumnText(statement, 0));
        try out.writer.writeAll(":");
        const value_json = sqliteColumnNullableText(statement, 1) orelse "null";
        try out.writer.writeAll(if (value_json.len == 0) "null" else value_json);
        count += 1;
    }
    try out.writer.writeAll("}");
    return out.toOwnedSlice();
}

fn snapshotResourceUsageJsonAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, app_id: []const u8) ![]u8 {
    const storage_bytes = try int64QueryDb(db, "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ?", app_id);
    const bridge_calls = try int64QueryDb(db, "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ?", app_id);
    const core_events = try int64QueryDb(db, "SELECT COUNT(*) FROM core_events WHERE app_id = ?", app_id);
    return std.fmt.allocPrint(
        allocator,
        "{{\"storageBytes\":{d},\"bridgeCalls\":{d},\"coreEvents\":{d}}}",
        .{ storage_bytes, bridge_calls, core_events },
    );
}

fn int64QueryDb(db: *sqlite.sqlite3, sql: [*:0]const u8, bind_value: []const u8) !i64 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, sql, -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, bind_value);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return error.StorageQueryFailed;
    return sqlite.sqlite3_column_int64(statement, 0);
}

fn insertRuntimeSnapshot(
    db: *sqlite.sqlite3,
    snapshot_id: []const u8,
    session_id: ?[]const u8,
    app_id: []const u8,
    install_id: []const u8,
    snapshot_type: []const u8,
    snapshot_json: []const u8,
    content_hash: []const u8,
    created_at: []const u8,
) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO runtime_snapshots (snapshot_id, session_id, app_id, install_id, type, snapshot_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, snapshot_id);
    bindNullableText(statement, 2, session_id);
    bindText(statement, 3, app_id);
    bindText(statement, 4, install_id);
    bindText(statement, 5, snapshot_type);
    bindText(statement, 6, snapshot_json);
    bindText(statement, 7, content_hash);
    bindText(statement, 8, created_at);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn snapshotJsonByIdAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, snapshot_id: []const u8) ![]u8 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT snapshot_json FROM runtime_snapshots WHERE snapshot_id = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, snapshot_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return error.SnapshotNotFound;
    return allocator.dupe(u8, sqliteColumnText(statement, 0));
}

fn snapshotContentHashByIdAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, snapshot_id: []const u8) ![]u8 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT content_hash FROM runtime_snapshots WHERE snapshot_id = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, snapshot_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return error.SnapshotNotFound;
    return allocator.dupe(u8, sqliteColumnText(statement, 0));
}

fn restoreSnapshotStorageIntoDb(
    allocator: std.mem.Allocator,
    db: *sqlite.sqlite3,
    snapshot_id: []const u8,
    expected_app_id: []const u8,
    expected_install_id: []const u8,
    restored_at: []const u8,
) !usize {
    const snapshot_json = try snapshotJsonByIdAlloc(allocator, db, snapshot_id);
    defer allocator.free(snapshot_json);
    const stored_hash = try snapshotContentHashByIdAlloc(allocator, db, snapshot_id);
    defer allocator.free(stored_hash);
    const actual_hash = try sha256PrefixedAlloc(allocator, snapshot_json);
    defer allocator.free(actual_hash);
    if (!std.mem.eql(u8, stored_hash, actual_hash)) return error.SnapshotInvalid;

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, snapshot_json, .{}) catch return error.SnapshotInvalid;
    defer parsed.deinit();
    if (parsed.value != .object) return error.SnapshotInvalid;
    const active_app_value = parsed.value.object.get("activeApp") orelse return error.SnapshotInvalid;
    if (active_app_value != .object) return error.SnapshotInvalid;
    const app_id = valueString(active_app_value.object.get("appId")) orelse return error.SnapshotInvalid;
    const install_id = valueString(active_app_value.object.get("installId")) orelse return error.SnapshotInvalid;
    if (!std.mem.eql(u8, app_id, expected_app_id) or !std.mem.eql(u8, install_id, expected_install_id)) {
        return error.SnapshotInvalid;
    }
    const storage_value = parsed.value.object.get("storage") orelse return error.SnapshotInvalid;
    if (storage_value != .object) return error.SnapshotInvalid;
    try deleteAppStorageForApp(db, app_id);
    return restoreSnapshotStorage(allocator, db, app_id, storage_value, restored_at);
}

fn deleteAppStorageForApp(db: *sqlite.sqlite3, app_id: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "DELETE FROM app_storage WHERE app_id = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn restoreSnapshotStorage(
    allocator: std.mem.Allocator,
    db: *sqlite.sqlite3,
    app_id: []const u8,
    storage_value: std.json.Value,
    restored_at: []const u8,
) !usize {
    if (storage_value != .object) return error.SnapshotInvalid;
    var restored_keys: usize = 0;
    var iterator = storage_value.object.iterator();
    while (iterator.next()) |entry| {
        const value_json = try jsonValueAlloc(allocator, entry.value_ptr.*);
        defer allocator.free(value_json);
        try insertRestoredStorageValue(db, app_id, entry.key_ptr.*, value_json, restored_at);
        restored_keys += 1;
    }
    return restored_keys;
}

fn insertRestoredStorageValue(db: *sqlite.sqlite3, app_id: []const u8, key: []const u8, value_json: []const u8, restored_at: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, key);
    bindText(statement, 3, value_json);
    bindText(statement, 4, restored_at);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn restoreSnapshotResultJsonAlloc(
    allocator: std.mem.Allocator,
    snapshot_id: []const u8,
    app_id: []const u8,
    active_install_id: []const u8,
    restored_keys: usize,
) ![]u8 {
    const escaped_snapshot_id = try escapeJsonString(allocator, snapshot_id);
    defer allocator.free(escaped_snapshot_id);
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_install_id = try escapeJsonString(allocator, active_install_id);
    defer allocator.free(escaped_install_id);
    return std.fmt.allocPrint(
        allocator,
        "{{\"ok\":true,\"snapshotId\":\"{s}\",\"appId\":\"{s}\",\"activeInstallId\":\"{s}\",\"restoredStorageKeys\":{d}}}",
        .{ escaped_snapshot_id, escaped_app_id, escaped_install_id, restored_keys },
    );
}

fn isAllowedSnapshotType(snapshot_type: []const u8) bool {
    const allowed = [_][]const u8{ "bug-report", "pre-install", "pre-migration", "post-test", "golden", "manual", "debug-bundle" };
    for (allowed) |candidate| {
        if (std.mem.eql(u8, snapshot_type, candidate)) return true;
    }
    return false;
}

const MigrationChange = struct {
    key: []u8,
    value_json: ?[]u8,
};

const MigrationPreview = struct {
    changes: []MigrationChange,
    changed_keys_json: []u8,
    operation_counts_json: []u8,
};

fn runStorageMigration(allocator: std.mem.Allocator, migration: std.json.Value, mode: []const u8) ![]u8 {
    const app_id = valueString(migration.object.get("appId")) orelse return error.InvalidMigration;
    const from_data_version = valueI64(migration.object.get("fromDataVersion")) orelse return error.InvalidMigration;
    const to_data_version = valueI64(migration.object.get("toDataVersion")) orelse return error.InvalidMigration;
    if (from_data_version < 1 or to_data_version != from_data_version + 1) return error.InvalidMigration;
    const steps = migration.object.get("steps") orelse return error.InvalidMigration;
    if (steps != .array) return error.InvalidMigration;

    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const current_data_version = try appDataVersion(db, app_id);
    const actual_data_version = current_data_version orelse return error.AppNotInstalled;
    if (actual_data_version != from_data_version) return error.InvalidMigration;

    const snapshot_json = try createRuntimeSnapshot(allocator, app_id, "pre-migration", null);
    defer allocator.free(snapshot_json);
    const snapshot_id = try snapshotIdFromJsonAlloc(allocator, snapshot_json);
    defer allocator.free(snapshot_id);

    const preview = try previewStorageMigration(allocator, db, app_id, steps.array.items);
    defer freeMigrationPreview(allocator, preview);
    const migration_json = try jsonValueAlloc(allocator, migration);
    defer allocator.free(migration_json);
    const migration_hash = try sha256PrefixedAlloc(allocator, migration_json);
    defer allocator.free(migration_hash);
    const migration_id = try std.fmt.allocPrint(allocator, "migration_{s}_{d}_to_{d}", .{ app_id, from_data_version, to_data_version });
    defer allocator.free(migration_id);
    const run_id = try randomDbIdAlloc(allocator, db, "mrun_");
    defer allocator.free(run_id);
    const started_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(started_at);
    const report_json = try migrationReportJsonAlloc(allocator, preview.changed_keys_json, preview.operation_counts_json);
    defer allocator.free(report_json);

    try insertAppMigrationRecord(db, migration_id, app_id, from_data_version, to_data_version, migration_json, migration_hash, started_at);

    if (std.mem.eql(u8, mode, "apply")) {
        try execDb(db, "BEGIN IMMEDIATE");
        errdefer execDb(db, "ROLLBACK") catch {};
        try applyMigrationChanges(db, preview.changes, started_at, app_id);
        try updateAppDataVersion(db, app_id, to_data_version, started_at);
        try insertMigrationRun(db, run_id, migration_id, app_id, null, mode, "passed", snapshot_id, report_json, started_at, started_at);
        const result_json = try migrationResultJsonAlloc(allocator, run_id, mode, snapshot_id, preview.changed_keys_json, preview.operation_counts_json);
        errdefer allocator.free(result_json);
        try execDb(db, "COMMIT");
        return result_json;
    }

    if (!std.mem.eql(u8, mode, "dry-run")) return error.InvalidMigration;
    try insertMigrationRun(db, run_id, migration_id, app_id, null, mode, "passed", snapshot_id, report_json, started_at, started_at);
    return migrationResultJsonAlloc(allocator, run_id, mode, snapshot_id, preview.changed_keys_json, preview.operation_counts_json);
}

fn applyPackagedMigrationChainForInstall(
    allocator: std.mem.Allocator,
    db: *sqlite.sqlite3,
    app_id: []const u8,
    install_id: []const u8,
    package_files: []const PackageFile,
    from_data_version: i64,
    to_data_version: i64,
    created_at: []const u8,
) !void {
    if (to_data_version <= from_data_version) return;
    const snapshot_json = try createRuntimeSnapshotInDb(allocator, db, app_id, "pre-migration", null);
    defer allocator.free(snapshot_json);
    const snapshot_id = try snapshotIdFromJsonAlloc(allocator, snapshot_json);
    defer allocator.free(snapshot_id);

    var current = from_data_version;
    while (current < to_data_version) : (current += 1) {
        const next = current + 1;
        const migration_path = try std.fmt.allocPrint(allocator, "migrations/{d}_to_{d}.json", .{ current, next });
        defer allocator.free(migration_path);
        const migration_content = findPackageFileContent(package_files, migration_path) orelse return error.InvalidMigration;
        var parsed = std.json.parseFromSlice(std.json.Value, allocator, migration_content, .{}) catch return error.InvalidMigration;
        defer parsed.deinit();
        if (parsed.value != .object) return error.InvalidMigration;
        if (!migrationMatchesInstall(app_id, current, next, parsed.value)) return error.InvalidMigration;

        const steps = parsed.value.object.get("steps") orelse return error.InvalidMigration;
        if (steps != .array) return error.InvalidMigration;
        const preview = try previewStorageMigration(allocator, db, app_id, steps.array.items);
        defer freeMigrationPreview(allocator, preview);
        const migration_json = try jsonValueAlloc(allocator, parsed.value);
        defer allocator.free(migration_json);
        const migration_hash = try sha256PrefixedAlloc(allocator, migration_json);
        defer allocator.free(migration_hash);
        const migration_id = try std.fmt.allocPrint(allocator, "migration_{s}_{d}_to_{d}", .{ app_id, current, next });
        defer allocator.free(migration_id);
        const run_id = try randomDbIdAlloc(allocator, db, "mrun_");
        defer allocator.free(run_id);
        const report_json = try migrationReportJsonAlloc(allocator, preview.changed_keys_json, preview.operation_counts_json);
        defer allocator.free(report_json);

        try insertAppMigrationRecord(db, migration_id, app_id, current, next, migration_json, migration_hash, created_at);
        try applyMigrationChanges(db, preview.changes, created_at, app_id);
        try updateAppDataVersion(db, app_id, next, created_at);
        try insertMigrationRun(db, run_id, migration_id, app_id, install_id, "apply", "passed", snapshot_id, report_json, created_at, created_at);
    }
}

fn migrationMatchesInstall(app_id: []const u8, from_data_version: i64, to_data_version: i64, migration: std.json.Value) bool {
    const migration_app_id = valueString(migration.object.get("appId")) orelse return false;
    const migration_from = valueI64(migration.object.get("fromDataVersion")) orelse return false;
    const migration_to = valueI64(migration.object.get("toDataVersion")) orelse return false;
    return std.mem.eql(u8, migration_app_id, app_id) and migration_from == from_data_version and migration_to == to_data_version;
}

fn previewStorageMigration(
    allocator: std.mem.Allocator,
    db: *sqlite.sqlite3,
    app_id: []const u8,
    steps: []std.json.Value,
) !MigrationPreview {
    var changes: std.ArrayList(MigrationChange) = .empty;
    errdefer {
        freeMigrationChanges(allocator, changes.items);
        changes.deinit(allocator);
    }
    var changed_keys: std.ArrayList([]u8) = .empty;
    errdefer {
        freeStringList(allocator, changed_keys.items);
        changed_keys.deinit(allocator);
    }
    var operation_counts = std.StringArrayHashMap(usize).init(allocator);
    defer operation_counts.deinit();

    for (steps) |step| {
        if (step != .object) return error.InvalidMigration;
        const op = valueString(step.object.get("op")) orelse return error.InvalidMigration;
        try incrementOperationCount(&operation_counts, op);

        if (std.mem.eql(u8, op, "setDefault")) {
            const key = valueString(step.object.get("key")) orelse return error.InvalidMigration;
            const field = valueString(step.object.get("to")) orelse return error.InvalidMigration;
            const default_value = step.object.get("value") orelse std.json.Value.null;
            const current_json = try migrationCurrentValueJsonAlloc(allocator, db, app_id, changes.items, key);
            defer allocator.free(current_json);
            var parsed = std.json.parseFromSlice(std.json.Value, allocator, current_json, .{}) catch return error.InvalidMigration;
            defer parsed.deinit();
            try setDefaultMigrationValue(&parsed.value, field, default_value);
            const next_json = try jsonValueAlloc(allocator, parsed.value);
            defer allocator.free(next_json);
            try appendMigrationChange(allocator, &changes, key, next_json);
            try appendChangedKey(allocator, &changed_keys, key);
        } else if (std.mem.eql(u8, op, "renameKey") or std.mem.eql(u8, op, "moveStorageKey")) {
            const from = valueString(step.object.get("from")) orelse return error.InvalidMigration;
            const to = valueString(step.object.get("to")) orelse return error.InvalidMigration;
            const value_json = try migrationCurrentValueJsonAlloc(allocator, db, app_id, changes.items, from);
            defer allocator.free(value_json);
            try appendMigrationDelete(allocator, &changes, from);
            try appendMigrationChange(allocator, &changes, to, value_json);
            try appendChangedKey(allocator, &changed_keys, from);
            try appendChangedKey(allocator, &changed_keys, to);
        } else if (std.mem.eql(u8, op, "deleteKey") or std.mem.eql(u8, op, "deleteStorageKey")) {
            const key = valueString(step.object.get("key")) orelse return error.InvalidMigration;
            try appendMigrationDelete(allocator, &changes, key);
            try appendChangedKey(allocator, &changed_keys, key);
        } else if (std.mem.eql(u8, op, "copyKey")) {
            const from = valueString(step.object.get("from")) orelse return error.InvalidMigration;
            const to = valueString(step.object.get("to")) orelse return error.InvalidMigration;
            const value_json = try migrationCurrentValueJsonAlloc(allocator, db, app_id, changes.items, from);
            defer allocator.free(value_json);
            try appendMigrationChange(allocator, &changes, to, value_json);
            try appendChangedKey(allocator, &changed_keys, to);
        } else {
            return error.InvalidMigration;
        }
    }

    const changed_keys_json = try stringListJsonAlloc(allocator, changed_keys.items);
    errdefer allocator.free(changed_keys_json);
    const operation_counts_json = try operationCountsJsonAlloc(allocator, operation_counts);
    errdefer allocator.free(operation_counts_json);
    const owned_changes = try changes.toOwnedSlice(allocator);
    changes = .empty;
    freeStringList(allocator, changed_keys.items);
    changed_keys.deinit(allocator);
    return .{
        .changes = owned_changes,
        .changed_keys_json = changed_keys_json,
        .operation_counts_json = operation_counts_json,
    };
}

fn freeMigrationPreview(allocator: std.mem.Allocator, preview: MigrationPreview) void {
    freeMigrationChanges(allocator, preview.changes);
    allocator.free(preview.changes);
    allocator.free(preview.changed_keys_json);
    allocator.free(preview.operation_counts_json);
}

fn freeMigrationChanges(allocator: std.mem.Allocator, changes: []MigrationChange) void {
    for (changes) |change| {
        allocator.free(change.key);
        if (change.value_json) |value| allocator.free(value);
    }
}

fn freeStringList(allocator: std.mem.Allocator, items: []const []u8) void {
    for (items) |item| allocator.free(item);
}

fn incrementOperationCount(counts: *std.StringArrayHashMap(usize), op: []const u8) !void {
    const entry = try counts.getOrPut(op);
    if (!entry.found_existing) entry.value_ptr.* = 0;
    entry.value_ptr.* += 1;
}

fn setDefaultMigrationValue(value: *std.json.Value, field: []const u8, default_value: std.json.Value) !void {
    switch (value.*) {
        .object => |*object| {
            if (object.get(field) == null) {
                try object.put(field, default_value);
            }
        },
        .array => |*array| {
            for (array.items) |*item| {
                try setDefaultMigrationValue(item, field, default_value);
            }
        },
        else => {},
    }
}

fn migrationCurrentValueJsonAlloc(
    allocator: std.mem.Allocator,
    db: *sqlite.sqlite3,
    app_id: []const u8,
    changes: []const MigrationChange,
    key: []const u8,
) ![]u8 {
    var index = changes.len;
    while (index > 0) {
        index -= 1;
        const change = changes[index];
        if (std.mem.eql(u8, change.key, key)) {
            if (change.value_json) |value| return allocator.dupe(u8, value);
            return allocator.dupe(u8, "null");
        }
    }
    const stored = try storageValueJsonForKeyAlloc(allocator, db, app_id, key);
    return stored orelse try allocator.dupe(u8, "null");
}

fn storageValueJsonForKeyAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, app_id: []const u8, key: []const u8) !?[]u8 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, key);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return null;
    const value_json = sqliteColumnNullableText(statement, 0) orelse "null";
    const owned_value = try allocator.dupe(u8, if (value_json.len == 0) "null" else value_json);
    return owned_value;
}

fn appendMigrationChange(allocator: std.mem.Allocator, changes: *std.ArrayList(MigrationChange), key: []const u8, value_json: []const u8) !void {
    const owned_key = try allocator.dupe(u8, key);
    errdefer allocator.free(owned_key);
    const owned_value = try allocator.dupe(u8, value_json);
    errdefer allocator.free(owned_value);
    try changes.append(allocator, .{ .key = owned_key, .value_json = owned_value });
}

fn appendMigrationDelete(allocator: std.mem.Allocator, changes: *std.ArrayList(MigrationChange), key: []const u8) !void {
    const owned_key = try allocator.dupe(u8, key);
    errdefer allocator.free(owned_key);
    try changes.append(allocator, .{ .key = owned_key, .value_json = null });
}

fn appendChangedKey(allocator: std.mem.Allocator, changed_keys: *std.ArrayList([]u8), key: []const u8) !void {
    for (changed_keys.items) |existing| {
        if (std.mem.eql(u8, existing, key)) return;
    }
    const owned_key = try allocator.dupe(u8, key);
    errdefer allocator.free(owned_key);
    try changed_keys.append(allocator, owned_key);
}

fn stringListJsonAlloc(allocator: std.mem.Allocator, items: []const []u8) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("[");
    for (items, 0..) |item, index| {
        if (index > 0) try out.writer.writeAll(",");
        try appendJsonString(allocator, &out, item);
    }
    try out.writer.writeAll("]");
    return out.toOwnedSlice();
}

fn operationCountsJsonAlloc(allocator: std.mem.Allocator, counts: std.StringArrayHashMap(usize)) ![]u8 {
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("{");
    var iterator = counts.iterator();
    var index: usize = 0;
    while (iterator.next()) |entry| : (index += 1) {
        if (index > 0) try out.writer.writeAll(",");
        try appendJsonString(allocator, &out, entry.key_ptr.*);
        try out.writer.print(":{d}", .{entry.value_ptr.*});
    }
    try out.writer.writeAll("}");
    return out.toOwnedSlice();
}

fn applyMigrationChanges(db: *sqlite.sqlite3, changes: []const MigrationChange, updated_at: []const u8, app_id: []const u8) !void {
    for (changes) |change| {
        if (change.value_json) |value_json| {
            try upsertMigrationStorageValue(db, app_id, change.key, value_json, updated_at);
        } else {
            try deleteStorageKey(db, app_id, change.key);
        }
    }
}

fn upsertMigrationStorageValue(db: *sqlite.sqlite3, app_id: []const u8, key: []const u8, value_json: []const u8, updated_at: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, key);
    bindText(statement, 3, value_json);
    bindText(statement, 4, updated_at);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn deleteStorageKey(db: *sqlite.sqlite3, app_id: []const u8, key: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "DELETE FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, key);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn updateAppDataVersion(db: *sqlite.sqlite3, app_id: []const u8, data_version: i64, updated_at: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "UPDATE apps SET data_version = ?, updated_at = ? WHERE id = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    _ = sqlite.sqlite3_bind_int64(statement, 1, data_version);
    bindText(statement, 2, updated_at);
    bindText(statement, 3, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn insertAppMigrationRecord(
    db: *sqlite.sqlite3,
    migration_id: []const u8,
    app_id: []const u8,
    from_data_version: i64,
    to_data_version: i64,
    migration_json: []const u8,
    content_hash: []const u8,
    created_at: []const u8,
) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_migrations (migration_id, app_id, from_data_version, to_data_version, migration_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, migration_id);
    bindText(statement, 2, app_id);
    _ = sqlite.sqlite3_bind_int64(statement, 3, from_data_version);
    _ = sqlite.sqlite3_bind_int64(statement, 4, to_data_version);
    bindText(statement, 5, migration_json);
    bindText(statement, 6, content_hash);
    bindText(statement, 7, created_at);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn insertMigrationRun(
    db: *sqlite.sqlite3,
    run_id: []const u8,
    migration_id: []const u8,
    app_id: []const u8,
    install_id: ?[]const u8,
    mode: []const u8,
    status: []const u8,
    pre_snapshot_id: []const u8,
    report_json: []const u8,
    started_at: []const u8,
    finished_at: []const u8,
) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO migration_runs (migration_run_id, migration_id, app_id, install_id, mode, status, pre_snapshot_id, report_json, started_at, finished_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, run_id);
    bindText(statement, 2, migration_id);
    bindText(statement, 3, app_id);
    bindNullableText(statement, 4, install_id);
    bindText(statement, 5, mode);
    bindText(statement, 6, status);
    bindText(statement, 7, pre_snapshot_id);
    bindText(statement, 8, report_json);
    bindText(statement, 9, started_at);
    bindText(statement, 10, finished_at);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn migrationReportJsonAlloc(allocator: std.mem.Allocator, changed_keys_json: []const u8, operation_counts_json: []const u8) ![]u8 {
    return std.fmt.allocPrint(
        allocator,
        "{{\"changedKeys\":{s},\"operationCounts\":{s}}}",
        .{ changed_keys_json, operation_counts_json },
    );
}

fn migrationResultJsonAlloc(
    allocator: std.mem.Allocator,
    run_id: []const u8,
    mode: []const u8,
    snapshot_id: []const u8,
    changed_keys_json: []const u8,
    operation_counts_json: []const u8,
) ![]u8 {
    const escaped_run_id = try escapeJsonString(allocator, run_id);
    defer allocator.free(escaped_run_id);
    const escaped_mode = try escapeJsonString(allocator, mode);
    defer allocator.free(escaped_mode);
    const escaped_snapshot_id = try escapeJsonString(allocator, snapshot_id);
    defer allocator.free(escaped_snapshot_id);
    return std.fmt.allocPrint(
        allocator,
        "{{\"runId\":\"{s}\",\"mode\":\"{s}\",\"status\":\"passed\",\"snapshotId\":\"{s}\",\"changedKeys\":{s},\"operationCounts\":{s}}}",
        .{ escaped_run_id, escaped_mode, escaped_snapshot_id, changed_keys_json, operation_counts_json },
    );
}

fn snapshotIdFromJsonAlloc(allocator: std.mem.Allocator, snapshot_json: []const u8) ![]u8 {
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, snapshot_json, .{}) catch return error.SnapshotInvalid;
    defer parsed.deinit();
    if (parsed.value != .object) return error.SnapshotInvalid;
    const snapshot_id = valueString(parsed.value.object.get("snapshotId")) orelse return error.SnapshotInvalid;
    return allocator.dupe(u8, snapshot_id);
}

fn valueI64(value: ?std.json.Value) ?i64 {
    const actual = value orelse return null;
    if (actual != .integer) return null;
    return actual.integer;
}

fn packageFilesAlloc(allocator: std.mem.Allocator, files_value: std.json.Value) ![]PackageFile {
    var files: std.ArrayList(PackageFile) = .empty;
    errdefer {
        for (files.items) |file| allocator.free(file.content_hash);
        files.deinit(allocator);
    }
    for (files_value.array.items) |file_value| {
        if (file_value != .object) continue;
        const path = valueString(file_value.object.get("path")) orelse continue;
        const content = valueString(file_value.object.get("content")) orelse continue;
        const content_hash = try sha256PrefixedAlloc(allocator, content);
        try files.append(allocator, .{
            .path = path,
            .content = content,
            .content_hash = content_hash,
        });
    }
    const owned = try files.toOwnedSlice(allocator);
    std.mem.sort(PackageFile, owned, {}, packageFileLessThan);
    return owned;
}

fn freePackageFiles(allocator: std.mem.Allocator, files: []PackageFile) void {
    for (files) |file| {
        allocator.free(file.content_hash);
    }
    allocator.free(files);
}

fn findPackageFileContent(files: []const PackageFile, file_path: []const u8) ?[]const u8 {
    for (files) |file| {
        if (std.mem.eql(u8, file.path, file_path)) return file.content;
    }
    return null;
}

fn packageFileLessThan(_: void, a: PackageFile, b: PackageFile) bool {
    return std.mem.lessThan(u8, a.path, b.path);
}

fn packageHashesAlloc(
    allocator: std.mem.Allocator,
    manifest: std.json.Value,
    manifest_json: []const u8,
    permissions: std.json.Value,
    files: []const PackageFile,
) !PackageHashes {
    const manifest_hash = try sha256PrefixedAlloc(allocator, manifest_json);
    errdefer allocator.free(manifest_hash);
    const permissions_json = try permissionsArrayJsonAlloc(allocator, permissions);
    defer allocator.free(permissions_json);
    const permissions_hash = try sha256PrefixedAlloc(allocator, permissions_json);
    errdefer allocator.free(permissions_hash);
    const policy_json = try policyJsonAlloc(allocator, manifest);
    defer allocator.free(policy_json);
    const policy_hash = try sha256PrefixedAlloc(allocator, policy_json);
    errdefer allocator.free(policy_hash);

    var content_bytes: std.io.Writer.Allocating = .init(allocator);
    errdefer content_bytes.deinit();
    var records: std.io.Writer.Allocating = .init(allocator);
    errdefer records.deinit();
    var file_hashes: std.io.Writer.Allocating = .init(allocator);
    errdefer file_hashes.deinit();
    try records.writer.writeAll("[");
    try file_hashes.writer.writeAll("{");
    for (files, 0..) |file, index| {
        try content_bytes.writer.print("{s}\n{s}\n", .{ file.path, file.content_hash });
        if (index > 0) try records.writer.writeAll(",");
        if (index > 0) try file_hashes.writer.writeAll(",");
        try records.writer.writeAll("{\"path\":");
        try appendJsonString(allocator, &records, file.path);
        try records.writer.writeAll(",\"hash\":");
        try appendJsonString(allocator, &records, file.content_hash);
        try records.writer.writeAll("}");
        try appendJsonString(allocator, &file_hashes, file.path);
        try file_hashes.writer.writeAll(":");
        try appendJsonString(allocator, &file_hashes, file.content_hash);
    }
    try records.writer.writeAll("]");
    try file_hashes.writer.writeAll("}");
    const content_slice = try content_bytes.toOwnedSlice();
    defer allocator.free(content_slice);
    const content_hash = try sha256PrefixedAlloc(allocator, content_slice);
    errdefer allocator.free(content_hash);
    const file_records_json = try records.toOwnedSlice();
    errdefer allocator.free(file_records_json);
    const file_hashes_json = try file_hashes.toOwnedSlice();

    return .{
        .manifest_hash = manifest_hash,
        .content_hash = content_hash,
        .permissions_hash = permissions_hash,
        .policy_hash = policy_hash,
        .file_records_json = file_records_json,
        .file_hashes_json = file_hashes_json,
    };
}

fn freePackageHashes(allocator: std.mem.Allocator, hashes: PackageHashes) void {
    allocator.free(hashes.manifest_hash);
    allocator.free(hashes.content_hash);
    allocator.free(hashes.permissions_hash);
    allocator.free(hashes.policy_hash);
    allocator.free(hashes.file_records_json);
    allocator.free(hashes.file_hashes_json);
}

fn permissionsArrayJsonAlloc(allocator: std.mem.Allocator, permissions: std.json.Value) ![]u8 {
    var items: std.ArrayList([]const u8) = .empty;
    defer items.deinit(allocator);
    if (permissions == .array) {
        for (permissions.array.items) |permission| {
            if (valueString(permission)) |name| {
                try items.append(allocator, name);
            }
        }
    }
    std.mem.sort([]const u8, items.items, {}, stringLessThan);
    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("[");
    for (items.items, 0..) |permission, index| {
        if (index > 0) try out.writer.writeAll(",");
        try appendJsonString(allocator, &out, permission);
    }
    try out.writer.writeAll("]");
    return out.toOwnedSlice();
}

fn stringLessThan(_: void, a: []const u8, b: []const u8) bool {
    return std.mem.lessThan(u8, a, b);
}

fn policyJsonAlloc(allocator: std.mem.Allocator, manifest: std.json.Value) ![]u8 {
    const capabilities = try jsonValueAlloc(allocator, manifest.object.get("capabilities").?);
    defer allocator.free(capabilities);
    const network_policy = try jsonValueAlloc(allocator, manifest.object.get("networkPolicy").?);
    defer allocator.free(network_policy);
    const resource_budget = try jsonValueAlloc(allocator, manifest.object.get("resourceBudget").?);
    defer allocator.free(resource_budget);
    return std.fmt.allocPrint(
        allocator,
        "{{\"capabilities\":{s},\"networkPolicy\":{s},\"resourceBudget\":{s}}}",
        .{ capabilities, network_policy, resource_budget },
    );
}

fn serverSignatureJsonAlloc(
    allocator: std.mem.Allocator,
    manifest: std.json.Value,
    trust_level: []const u8,
    hashes: PackageHashes,
    signed_at: []const u8,
) ![]u8 {
    const key_pair = try serverSigningKeyPair(allocator);
    const public_key_bytes = key_pair.public_key.toBytes();
    const key_id = try serverSigningKeyIdAlloc(allocator, &public_key_bytes);
    defer allocator.free(key_id);
    const payload = try signaturePayloadAlloc(allocator, manifest, trust_level, key_id, hashes, signed_at);
    defer allocator.free(payload);
    const signature = try key_pair.sign(payload, null);
    const signature_bytes = signature.toBytes();
    const signature_b64 = try base64Alloc(allocator, &signature_bytes);
    defer allocator.free(signature_b64);

    const app_id = valueString(manifest.object.get("id")).?;
    const version = valueString(manifest.object.get("version")).?;
    const app_runtime_version = valueString(manifest.object.get("runtimeVersion")).?;
    const data_version = manifest.object.get("dataVersion").?.integer;
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_version = try escapeJsonString(allocator, version);
    defer allocator.free(escaped_version);
    const escaped_runtime_version = try escapeJsonString(allocator, app_runtime_version);
    defer allocator.free(escaped_runtime_version);
    const escaped_trust_level = try escapeJsonString(allocator, trust_level);
    defer allocator.free(escaped_trust_level);
    const escaped_key_id = try escapeJsonString(allocator, key_id);
    defer allocator.free(escaped_key_id);
    const escaped_signature = try escapeJsonString(allocator, signature_b64);
    defer allocator.free(escaped_signature);
    return std.fmt.allocPrint(
        allocator,
        "{{\"appId\":\"{s}\",\"appVersion\":\"{s}\",\"dataVersion\":{d},\"runtimeVersion\":\"{s}\",\"trustLevel\":\"{s}\",\"algorithm\":\"ed25519\",\"keyId\":\"{s}\",\"manifestHash\":\"{s}\",\"contentHash\":\"{s}\",\"permissionsHash\":\"{s}\",\"policyHash\":\"{s}\",\"signedAt\":\"{s}\",\"signedBy\":\"zig-server\",\"signature\":\"{s}\"}}",
        .{ escaped_app_id, escaped_version, data_version, escaped_runtime_version, escaped_trust_level, escaped_key_id, hashes.manifest_hash, hashes.content_hash, hashes.permissions_hash, hashes.policy_hash, signed_at, escaped_signature },
    );
}

fn signaturePayloadAlloc(
    allocator: std.mem.Allocator,
    manifest: std.json.Value,
    trust_level: []const u8,
    key_id: []const u8,
    hashes: PackageHashes,
    signed_at: []const u8,
) ![]u8 {
    const app_id = valueString(manifest.object.get("id")).?;
    const version = valueString(manifest.object.get("version")).?;
    const app_runtime_version = valueString(manifest.object.get("runtimeVersion")).?;
    const data_version = manifest.object.get("dataVersion").?.integer;
    return std.fmt.allocPrint(
        allocator,
        "{s}\n{s}\n{s}\n{d}\n{s}\n{s}\n{s}\n{s}\n{s}\n{s}\n{s}\n{s}",
        .{ signature_prefix, app_id, version, data_version, app_runtime_version, trust_level, key_id, hashes.manifest_hash, hashes.content_hash, hashes.permissions_hash, hashes.policy_hash, signed_at },
    );
}

fn serverSigningKeyPair(allocator: std.mem.Allocator) !std.crypto.sign.Ed25519.KeyPair {
    const seed_source = std.process.getEnvVarOwned(allocator, "NATIVE_AI_SERVER_SIGNING_SEED") catch |err| switch (err) {
        error.EnvironmentVariableNotFound => try allocator.dupe(u8, "native-ai-zig-server-dev-signing-key-v0.4"),
        else => return err,
    };
    defer allocator.free(seed_source);
    var seed: [std.crypto.sign.Ed25519.KeyPair.seed_length]u8 = undefined;
    std.crypto.hash.sha2.Sha256.hash(seed_source, &seed, .{});
    return std.crypto.sign.Ed25519.KeyPair.generateDeterministic(seed);
}

fn serverSigningKeyIdAlloc(allocator: std.mem.Allocator, public_key: []const u8) ![]u8 {
    const public_hash = try sha256HexAlloc(allocator, public_key);
    defer allocator.free(public_hash);
    return std.fmt.allocPrint(allocator, "platform-host:zig-server:{s}", .{public_hash[0..16]});
}

fn base64Alloc(allocator: std.mem.Allocator, input: []const u8) ![]u8 {
    const output = try allocator.alloc(u8, std.base64.standard.Encoder.calcSize(input.len));
    _ = std.base64.standard.Encoder.encode(output, input);
    return output;
}

fn contentHashesDocumentJsonAlloc(allocator: std.mem.Allocator, hashes: PackageHashes) ![]u8 {
    return std.fmt.allocPrint(
        allocator,
        "{{\"algorithm\":\"sha256\",\"manifestHash\":\"{s}\",\"contentHash\":\"{s}\",\"files\":{s}}}",
        .{ hashes.manifest_hash, hashes.content_hash, hashes.file_records_json },
    );
}

fn evaluateSmokeTestsAlloc(allocator: std.mem.Allocator, package_root: std.json.Value, app_id: []const u8) !SmokeTestEvaluation {
    const files = package_root.object.get("files").?;
    const smoke_tests = findPackageFile(files, "smoke-tests.json") orelse {
        return .{
            .ok = true,
            .status = "not-run",
            .result_json = try smokeResultJsonAlloc(allocator, app_id, true, "not-run", 0, 0, "[]", "[]"),
            .spec_json = try allocator.dupe(u8, "[]"),
        };
    };

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, smoke_tests, .{}) catch {
        return smokeInvalidResultAlloc(allocator, app_id, "package.invalid", "smoke-tests.json must parse as JSON");
    };
    defer parsed.deinit();

    if (parsed.value != .array) {
        return smokeInvalidResultAlloc(allocator, app_id, "invalid_smoke_shape", "smoke-tests.json must be an array");
    }

    const html = findPackageFile(files, "index.html") orelse "";
    const app_js = findPackageFile(files, "app.js") orelse "";
    var dynamic_text: std.ArrayList([]const u8) = .empty;
    defer dynamic_text.deinit(allocator);
    var failures: std.io.Writer.Allocating = .init(allocator);
    errdefer failures.deinit();
    try failures.writer.writeAll("[");
    var failure_count: usize = 0;
    var assertions: usize = 0;

    for (parsed.value.array.items) |test_value| {
        if (test_value != .object) {
            try appendSmokeFailure(allocator, &failures, &failure_count, "unnamed", "invalid_smoke_test", "message", "Smoke test must be an object");
            continue;
        }
        const test_name = valueString(test_value.object.get("name")) orelse "unnamed";
        if (test_value.object.get("steps")) |steps| {
            if (steps != .array) {
                try appendSmokeFailure(allocator, &failures, &failure_count, test_name, "invalid_smoke_steps", "message", "Smoke test steps must be an array");
            } else {
                assertions += steps.array.items.len;
                for (steps.array.items) |step_value| {
                    if (step_value != .object) continue;
                    if (valueString(step_value.object.get("selector"))) |selector| {
                        if (!try selectorExists(allocator, html, selector)) {
                            try appendSmokeFailure(allocator, &failures, &failure_count, test_name, "selector.not_found", "selector", selector);
                        }
                    }
                    const step_type = valueString(step_value.object.get("type")) orelse "";
                    if ((std.mem.eql(u8, step_type, "fill") or std.mem.eql(u8, step_type, "select"))) {
                        if (valueString(step_value.object.get("value"))) |value| {
                            try dynamic_text.append(allocator, value);
                        }
                    }
                }
            }
        }
        if (test_value.object.get("expected")) |expected| {
            if (expected != .object) {
                try appendSmokeFailure(allocator, &failures, &failure_count, test_name, "invalid_smoke_expected", "message", "Smoke test expected must be an object");
            } else {
                assertions += expected.object.count();
                if (expected.object.get("bridgeCallsInclude")) |methods| {
                    if (methods == .array) {
                        for (methods.array.items) |method_value| {
                            const method = valueString(method_value) orelse continue;
                            if (!try bridgeMethodReferenced(allocator, app_js, method)) {
                                try appendSmokeFailure(allocator, &failures, &failure_count, test_name, "bridge.call_missing", "method", method);
                            }
                        }
                    }
                }
                if (valueString(expected.object.get("textIncludes"))) |text| {
                    if (!textCanAppear(html, dynamic_text.items, text)) {
                        try appendSmokeFailure(allocator, &failures, &failure_count, test_name, "text.not_found", "text", text);
                    }
                }
            }
        }
    }

    try failures.writer.writeAll("]");
    const failures_json = try failures.toOwnedSlice();
    defer allocator.free(failures_json);
    const spec_json = try allocator.dupe(u8, smoke_tests);
    errdefer allocator.free(spec_json);
    const ok = failure_count == 0;
    return .{
        .ok = ok,
        .status = if (ok) "passed" else "failed",
        .result_json = try smokeResultJsonAlloc(allocator, app_id, ok, if (ok) "passed" else "failed", parsed.value.array.items.len, assertions, failures_json, smoke_tests),
        .spec_json = spec_json,
    };
}

fn freeSmokeTestEvaluation(allocator: std.mem.Allocator, evaluation: SmokeTestEvaluation) void {
    allocator.free(evaluation.result_json);
    allocator.free(evaluation.spec_json);
}

fn smokeInvalidResultAlloc(allocator: std.mem.Allocator, app_id: []const u8, code: []const u8, message: []const u8) !SmokeTestEvaluation {
    var failures: std.io.Writer.Allocating = .init(allocator);
    errdefer failures.deinit();
    try failures.writer.writeAll("[");
    var failure_count: usize = 0;
    try appendSmokeFailure(allocator, &failures, &failure_count, "smoke-tests.json", code, "message", message);
    try failures.writer.writeAll("]");
    const failures_json = try failures.toOwnedSlice();
    defer allocator.free(failures_json);
    return .{
        .ok = false,
        .status = "failed",
        .result_json = try smokeResultJsonAlloc(allocator, app_id, false, "failed", 0, 0, failures_json, "[]"),
        .spec_json = try allocator.dupe(u8, "[]"),
    };
}

fn smokeResultJsonAlloc(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    ok: bool,
    status: []const u8,
    total: usize,
    assertions: usize,
    failures_json: []const u8,
    spec_json: []const u8,
) ![]u8 {
    const escaped_app_id = try escapeJsonString(allocator, app_id);
    defer allocator.free(escaped_app_id);
    const escaped_status = try escapeJsonString(allocator, status);
    defer allocator.free(escaped_status);
    return std.fmt.allocPrint(
        allocator,
        "{{\"ok\":{},\"status\":\"{s}\",\"appId\":\"{s}\",\"total\":{d},\"assertions\":{d},\"failures\":{s},\"spec\":{s}}}",
        .{ ok, escaped_status, escaped_app_id, total, assertions, failures_json, spec_json },
    );
}

fn appendSmokeFailure(
    allocator: std.mem.Allocator,
    out: *std.io.Writer.Allocating,
    count: *usize,
    test_name: []const u8,
    code: []const u8,
    detail_key: []const u8,
    detail_value: []const u8,
) !void {
    if (count.* > 0) try out.writer.writeAll(",");
    try out.writer.writeAll("{\"test\":");
    try appendJsonString(allocator, out, test_name);
    try out.writer.writeAll(",\"code\":");
    try appendJsonString(allocator, out, code);
    try out.writer.writeAll(",");
    try appendJsonString(allocator, out, detail_key);
    try out.writer.writeAll(":");
    try appendJsonString(allocator, out, detail_value);
    try out.writer.writeAll("}");
    count.* += 1;
}

fn selectorExists(allocator: std.mem.Allocator, html: []const u8, selector: []const u8) !bool {
    if (std.mem.startsWith(u8, selector, "#")) {
        return htmlAttrValueExists(allocator, html, "id", selector[1..]);
    }
    if (std.mem.indexOf(u8, selector, "data-testid=")) |start| {
        const value_start = start + "data-testid=".len;
        if (value_start < selector.len and (selector[value_start] == '"' or selector[value_start] == '\'')) {
            const quote = selector[value_start];
            const actual_start = value_start + 1;
            const actual_end = std.mem.indexOfScalarPos(u8, selector, actual_start, quote) orelse selector.len;
            return htmlAttrValueExists(allocator, html, "data-testid", selector[actual_start..actual_end]);
        }
    }
    return std.mem.indexOf(u8, html, selector) != null;
}

fn htmlAttrValueExists(allocator: std.mem.Allocator, html: []const u8, attr: []const u8, value: []const u8) !bool {
    const double = try std.fmt.allocPrint(allocator, "{s}=\"{s}\"", .{ attr, value });
    defer allocator.free(double);
    if (std.mem.indexOf(u8, html, double) != null) return true;
    const single = try std.fmt.allocPrint(allocator, "{s}='{s}'", .{ attr, value });
    defer allocator.free(single);
    return std.mem.indexOf(u8, html, single) != null;
}

fn bridgeMethodReferenced(allocator: std.mem.Allocator, app_js: []const u8, method: []const u8) !bool {
    const double = try std.fmt.allocPrint(allocator, "\"{s}\"", .{method});
    defer allocator.free(double);
    if (std.mem.indexOf(u8, app_js, double) != null) return true;
    const single = try std.fmt.allocPrint(allocator, "'{s}'", .{method});
    defer allocator.free(single);
    return std.mem.indexOf(u8, app_js, single) != null;
}

fn textCanAppear(html: []const u8, dynamic_text: []const []const u8, text: []const u8) bool {
    if (std.mem.indexOf(u8, html, text) != null) return true;
    for (dynamic_text) |value| {
        if (std.mem.indexOf(u8, value, text) != null) return true;
    }
    return false;
}

fn permissionsReportJsonAlloc(
    allocator: std.mem.Allocator,
    permissions: std.json.Value,
    activate: bool,
    requires_approval: bool,
    previous_install_id: ?[]const u8,
) ![]u8 {
    const requested = try permissionsArrayJsonAlloc(allocator, permissions);
    defer allocator.free(requested);
    const approved = if (activate) try allocator.dupe(u8, requested) else try allocator.dupe(u8, "[]");
    defer allocator.free(approved);
    const previous = previous_install_id orelse "";
    const escaped_previous = try escapeJsonString(allocator, previous);
    defer allocator.free(escaped_previous);
    const previous_json = if (previous_install_id == null)
        try allocator.dupe(u8, "null")
    else
        try std.fmt.allocPrint(allocator, "\"{s}\"", .{escaped_previous});
    defer allocator.free(previous_json);
    return std.fmt.allocPrint(
        allocator,
        "{{\"requested\":{s},\"approved\":{s},\"requiresUserApproval\":{},\"approvalReasons\":{s},\"previousInstallId\":{s}}}",
        .{
            requested,
            approved,
            requires_approval,
            if (requires_approval) "[\"permission_change\"]" else "[]",
            previous_json,
        },
    );
}

fn activeInstallIdAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, app_id: []const u8) !?[]u8 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT active_install_id FROM apps WHERE id = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return null;
    return if (sqliteColumnNullableText(statement, 0)) |value| try allocator.dupe(u8, value) else null;
}

fn appDataVersion(db: *sqlite.sqlite3, app_id: []const u8) !?i64 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT data_version FROM apps WHERE id = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return null;
    return sqlite.sqlite3_column_int64(statement, 0);
}

fn packageAddsPermissions(db: *sqlite.sqlite3, permissions: std.json.Value, previous_install_id: ?[]const u8) !bool {
    const previous = previous_install_id orelse return false;
    for (permissions.array.items) |permission| {
        const name = valueString(permission) orelse continue;
        var statement: ?*sqlite.sqlite3_stmt = null;
        if (sqlite.sqlite3_prepare_v2(db, "SELECT 1 FROM app_permissions WHERE install_id = ? AND permission = ? AND approved = 1", -1, &statement, null) != sqlite.SQLITE_OK) {
            return error.StorageQueryFailed;
        }
        defer _ = sqlite.sqlite3_finalize(statement);
        bindText(statement, 1, previous);
        bindText(statement, 2, name);
        if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return true;
    }
    return false;
}

fn upsertInstalledApp(db: *sqlite.sqlite3, app_id: []const u8, name: []const u8, status: []const u8, data_version: i64, created_at: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO apps (id, name, status, data_version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?) " ++
            "ON CONFLICT(id) DO UPDATE SET name = excluded.name, status = excluded.status, data_version = excluded.data_version, updated_at = excluded.updated_at",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, name);
    bindText(statement, 3, status);
    _ = sqlite.sqlite3_bind_int64(statement, 4, data_version);
    bindText(statement, 5, created_at);
    bindText(statement, 6, created_at);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn markPreviousVersionInstalled(db: *sqlite.sqlite3, install_id: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "UPDATE app_versions SET status = 'installed' WHERE install_id = ?", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, install_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn markVersionStatus(db: *sqlite.sqlite3, install_id: []const u8, status: []const u8, activated_at: ?[]const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "UPDATE app_versions SET status = ?, activated_at = COALESCE(?, activated_at) WHERE install_id = ?",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, status);
    bindNullableText(statement, 2, activated_at);
    bindText(statement, 3, install_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn insertAppVersion(
    db: *sqlite.sqlite3,
    install_id: []const u8,
    app_id: []const u8,
    version: []const u8,
    app_runtime_version: []const u8,
    data_version: i64,
    manifest_json: []const u8,
    hashes: PackageHashes,
    signature_json: []const u8,
    trust_level: []const u8,
    status: []const u8,
    created_at: []const u8,
    activated: bool,
) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, install_id);
    bindText(statement, 2, app_id);
    bindText(statement, 3, version);
    bindText(statement, 4, app_runtime_version);
    _ = sqlite.sqlite3_bind_int64(statement, 5, data_version);
    bindText(statement, 6, manifest_json);
    bindText(statement, 7, hashes.manifest_hash);
    bindText(statement, 8, hashes.content_hash);
    bindText(statement, 9, signature_json);
    bindText(statement, 10, trust_level);
    bindText(statement, 11, status);
    bindText(statement, 12, created_at);
    bindNullableText(statement, 13, if (activated) created_at else null);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn insertAppFile(db: *sqlite.sqlite3, install_id: []const u8, file: PackageFile, created_at: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_files (install_id, path, content_text, content_hash, size_bytes, mime, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, install_id);
    bindText(statement, 2, file.path);
    bindText(statement, 3, file.content);
    bindText(statement, 4, file.content_hash);
    _ = sqlite.sqlite3_bind_int64(statement, 5, @intCast(file.content.len));
    bindText(statement, 6, mimeForPackagePath(file.path));
    bindText(statement, 7, created_at);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn insertAppPermissions(db: *sqlite.sqlite3, install_id: []const u8, app_id: []const u8, permissions: std.json.Value, activate: bool, created_at: []const u8) !void {
    for (permissions.array.items) |permission| {
        const name = valueString(permission) orelse continue;
        var statement: ?*sqlite.sqlite3_stmt = null;
        if (sqlite.sqlite3_prepare_v2(
            db,
            "INSERT OR REPLACE INTO app_permissions (install_id, app_id, permission, requested, approved, approved_at, reason) VALUES (?, ?, ?, 1, ?, ?, ?)",
            -1,
            &statement,
            null,
        ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
        defer _ = sqlite.sqlite3_finalize(statement);
        bindText(statement, 1, install_id);
        bindText(statement, 2, app_id);
        bindText(statement, 3, name);
        _ = sqlite.sqlite3_bind_int64(statement, 4, if (activate) 1 else 0);
        bindNullableText(statement, 5, if (activate) created_at else null);
        bindText(statement, 6, if (activate) "server install approved" else "pending approval");
        if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
    }
}

fn insertInstallReport(
    db: *sqlite.sqlite3,
    report_id: []const u8,
    app_id: []const u8,
    install_id: []const u8,
    status: []const u8,
    validation_json: []const u8,
    security_json: []const u8,
    permissions_json: []const u8,
    compatibility_json: []const u8,
    smoke_test_json: []const u8,
    content_hash: []const u8,
    created_at: []const u8,
) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO app_install_reports (report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, report_id);
    bindText(statement, 2, app_id);
    bindText(statement, 3, install_id);
    bindText(statement, 4, status);
    bindText(statement, 5, validation_json);
    bindText(statement, 6, security_json);
    bindText(statement, 7, permissions_json);
    bindText(statement, 8, compatibility_json);
    bindText(statement, 9, smoke_test_json);
    bindText(statement, 10, content_hash);
    bindText(statement, 11, created_at);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn insertSmokeTestRun(
    db: *sqlite.sqlite3,
    allocator: std.mem.Allocator,
    app_id: []const u8,
    smoke_status: []const u8,
    spec_json: []const u8,
    result_json: []const u8,
    created_at: []const u8,
) !void {
    const micro_test_id = try std.fmt.allocPrint(allocator, "smoke:{s}", .{app_id});
    defer allocator.free(micro_test_id);
    const name = try std.fmt.allocPrint(allocator, "{s} bundled smoke tests", .{app_id});
    defer allocator.free(name);

    var micro_statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO micro_tests (micro_test_id, app_id, name, spec_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?) " ++
            "ON CONFLICT(micro_test_id) DO UPDATE SET app_id = excluded.app_id, name = excluded.name, spec_json = excluded.spec_json, updated_at = excluded.updated_at",
        -1,
        &micro_statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(micro_statement);
    bindText(micro_statement, 1, micro_test_id);
    bindText(micro_statement, 2, app_id);
    bindText(micro_statement, 3, name);
    bindText(micro_statement, 4, spec_json);
    bindText(micro_statement, 5, created_at);
    bindText(micro_statement, 6, created_at);
    if (sqlite.sqlite3_step(micro_statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;

    const test_run_id = try randomDbIdAlloc(allocator, db, "testrun_");
    defer allocator.free(test_run_id);
    const run_status = if (std.mem.eql(u8, smoke_status, "failed")) "failed" else if (std.mem.eql(u8, smoke_status, "not-run")) "skipped" else "passed";
    var run_statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO test_runs (test_run_id, micro_test_id, app_id, status, started_at, finished_at, result_json, diagnostics_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &run_statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(run_statement);
    bindText(run_statement, 1, test_run_id);
    bindText(run_statement, 2, micro_test_id);
    bindText(run_statement, 3, app_id);
    bindText(run_statement, 4, run_status);
    bindText(run_statement, 5, created_at);
    bindText(run_statement, 6, created_at);
    bindText(run_statement, 7, result_json);
    bindText(run_statement, 8, "{\"runner\":\"zig-server-static-smoke\"}");
    if (sqlite.sqlite3_step(run_statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn insertRollbackInstallationEvent(
    db: *sqlite.sqlite3,
    allocator: std.mem.Allocator,
    app_id: []const u8,
    target_install_id: []const u8,
    rolled_back_install_id: []const u8,
    created_at: []const u8,
) !void {
    const event_id = try randomDbIdAlloc(allocator, db, "install_event_");
    defer allocator.free(event_id);
    const escaped_target = try escapeJsonString(allocator, target_install_id);
    defer allocator.free(escaped_target);
    const escaped_rolled_back = try escapeJsonString(allocator, rolled_back_install_id);
    defer allocator.free(escaped_rolled_back);
    const details_json = try std.fmt.allocPrint(
        allocator,
        "{{\"targetInstallId\":\"{s}\",\"rolledBackInstallId\":\"{s}\"}}",
        .{ escaped_target, escaped_rolled_back },
    );
    defer allocator.free(details_json);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json) VALUES (?, ?, ?, 'rollback', ?, 'zig-server', ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, event_id);
    bindText(statement, 2, app_id);
    bindText(statement, 3, target_install_id);
    bindText(statement, 4, rolled_back_install_id);
    bindText(statement, 5, created_at);
    bindText(statement, 6, details_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn insertInstallationEvent(
    db: *sqlite.sqlite3,
    allocator: std.mem.Allocator,
    app_id: []const u8,
    install_id: []const u8,
    action: []const u8,
    previous_install_id: ?[]const u8,
    report_id: []const u8,
    created_at: []const u8,
    actor: []const u8,
    status: []const u8,
) !void {
    const event_id = try randomDbIdAlloc(allocator, db, "install_event_");
    defer allocator.free(event_id);
    const details_json = try std.fmt.allocPrint(allocator, "{{\"status\":\"{s}\"}}", .{status});
    defer allocator.free(details_json);
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, event_id);
    bindText(statement, 2, app_id);
    bindText(statement, 3, install_id);
    bindText(statement, 4, action);
    bindNullableText(statement, 5, previous_install_id);
    bindText(statement, 6, actor);
    bindText(statement, 7, report_id);
    bindText(statement, 8, created_at);
    bindText(statement, 9, details_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn activateInstalledApp(db: *sqlite.sqlite3, app_id: []const u8, install_id: []const u8, version: []const u8, data_version: i64, created_at: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, install_id);
    bindText(statement, 2, version);
    _ = sqlite.sqlite3_bind_int64(statement, 3, data_version);
    bindText(statement, 4, created_at);
    bindText(statement, 5, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
}

fn mimeForPackagePath(path: []const u8) []const u8 {
    if (std.mem.endsWith(u8, path, ".html")) return "text/html";
    if (std.mem.endsWith(u8, path, ".css")) return "text/css";
    if (std.mem.endsWith(u8, path, ".js")) return "text/javascript";
    if (std.mem.endsWith(u8, path, ".json")) return "application/json";
    return "text/plain";
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

const CoreAuditContext = struct {
    app_id: ?[]u8,
    event_json: []u8,
};

fn coreAuditContextAlloc(allocator: std.mem.Allocator, body: []const u8) !CoreAuditContext {
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, body, .{}) catch {
        return .{ .app_id = null, .event_json = try allocator.dupe(u8, body) };
    };
    defer parsed.deinit();

    if (parsed.value != .object) {
        return .{ .app_id = null, .event_json = try allocator.dupe(u8, body) };
    }

    const app_id = if (valueString(parsed.value.object.get("app"))) |app|
        try allocator.dupe(u8, app)
    else
        null;
    const event_json = if (parsed.value.object.get("event")) |event_value|
        try jsonValueAlloc(allocator, event_value)
    else
        try allocator.dupe(u8, body);
    return .{ .app_id = app_id, .event_json = event_json };
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
        \\CREATE TABLE IF NOT EXISTS apps (
        \\  id TEXT PRIMARY KEY,
        \\  name TEXT NOT NULL,
        \\  status TEXT NOT NULL DEFAULT 'enabled' CHECK (status IN ('enabled','disabled','quarantined','uninstalled')),
        \\  active_install_id TEXT,
        \\  active_version TEXT,
        \\  data_version INTEGER NOT NULL DEFAULT 1,
        \\  created_at TEXT NOT NULL,
        \\  updated_at TEXT NOT NULL
        \\);
        \\CREATE TABLE IF NOT EXISTS app_versions (
        \\  install_id TEXT PRIMARY KEY,
        \\  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
        \\  version TEXT NOT NULL,
        \\  runtime_version TEXT NOT NULL,
        \\  data_version INTEGER NOT NULL,
        \\  manifest_json TEXT NOT NULL,
        \\  manifest_hash TEXT NOT NULL,
        \\  content_hash TEXT NOT NULL,
        \\  signature_json TEXT,
        \\  trust_level TEXT NOT NULL DEFAULT 'user-generated',
        \\  status TEXT NOT NULL DEFAULT 'installed' CHECK (status IN ('installed','enabled','disabled','quarantined','rolled-back','uninstalled')),
        \\  created_at TEXT NOT NULL,
        \\  activated_at TEXT
        \\);
        \\CREATE TABLE IF NOT EXISTS app_files (
        \\  install_id TEXT NOT NULL REFERENCES app_versions(install_id) ON DELETE CASCADE,
        \\  path TEXT NOT NULL,
        \\  content_text TEXT,
        \\  content_hash TEXT NOT NULL,
        \\  size_bytes INTEGER NOT NULL DEFAULT 0,
        \\  mime TEXT NOT NULL DEFAULT 'text/plain',
        \\  created_at TEXT NOT NULL,
        \\  PRIMARY KEY (install_id, path)
        \\);
        \\CREATE TABLE IF NOT EXISTS app_permissions (
        \\  install_id TEXT NOT NULL REFERENCES app_versions(install_id) ON DELETE CASCADE,
        \\  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
        \\  permission TEXT NOT NULL,
        \\  requested INTEGER NOT NULL DEFAULT 1,
        \\  approved INTEGER NOT NULL DEFAULT 0,
        \\  approved_at TEXT,
        \\  reason TEXT,
        \\  PRIMARY KEY (install_id, permission)
        \\);
        \\CREATE TABLE IF NOT EXISTS app_installations (
        \\  installation_event_id TEXT PRIMARY KEY,
        \\  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
        \\  install_id TEXT NOT NULL REFERENCES app_versions(install_id) ON DELETE CASCADE,
        \\  action TEXT NOT NULL CHECK (action IN ('install','activate','disable','rollback','quarantine','uninstall','import')),
        \\  previous_install_id TEXT,
        \\  actor TEXT NOT NULL DEFAULT 'system',
        \\  report_id TEXT,
        \\  created_at TEXT NOT NULL,
        \\  details_json TEXT
        \\);
        \\CREATE TABLE IF NOT EXISTS app_storage (
        \\  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
        \\  key TEXT NOT NULL,
        \\  value_json TEXT,
        \\  updated_at TEXT NOT NULL,
        \\  PRIMARY KEY (app_id, key)
        \\);
        \\CREATE INDEX IF NOT EXISTS idx_app_versions_app_version ON app_versions(app_id, version);
        \\CREATE INDEX IF NOT EXISTS idx_app_versions_app_status ON app_versions(app_id, status);
        \\CREATE INDEX IF NOT EXISTS idx_app_files_install_path ON app_files(install_id, path);
        \\CREATE INDEX IF NOT EXISTS idx_app_permissions_install_perm ON app_permissions(install_id, permission);
        \\CREATE INDEX IF NOT EXISTS idx_app_installations_app_created ON app_installations(app_id, created_at);
        \\CREATE INDEX IF NOT EXISTS idx_app_storage_app_updated ON app_storage(app_id, updated_at);
        \\CREATE TABLE IF NOT EXISTS runtime_sessions (
        \\  session_id TEXT PRIMARY KEY,
        \\  target TEXT NOT NULL,
        \\  platform TEXT NOT NULL,
        \\  runtime_version TEXT NOT NULL,
        \\  active_app_id TEXT,
        \\  active_install_id TEXT,
        \\  started_at TEXT NOT NULL,
        \\  ended_at TEXT,
        \\  status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running','ended','failed')),
        \\  capabilities_json TEXT,
        \\  metadata_json TEXT
        \\);
        \\CREATE TABLE IF NOT EXISTS bridge_calls (
        \\  bridge_call_id TEXT PRIMARY KEY,
        \\  session_id TEXT NOT NULL REFERENCES runtime_sessions(session_id) ON DELETE CASCADE,
        \\  app_id TEXT,
        \\  install_id TEXT,
        \\  method TEXT NOT NULL,
        \\  params_json TEXT,
        \\  result_json TEXT,
        \\  error_json TEXT,
        \\  duration_ms INTEGER,
        \\  created_at TEXT NOT NULL
        \\);
        \\CREATE TABLE IF NOT EXISTS core_events (
        \\  event_id TEXT PRIMARY KEY,
        \\  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
        \\  app_id TEXT,
        \\  install_id TEXT,
        \\  state_version_before INTEGER,
        \\  event_json TEXT NOT NULL,
        \\  created_at TEXT NOT NULL
        \\);
        \\CREATE TABLE IF NOT EXISTS core_actions (
        \\  action_id TEXT PRIMARY KEY,
        \\  event_id TEXT NOT NULL REFERENCES core_events(event_id) ON DELETE CASCADE,
        \\  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
        \\  app_id TEXT,
        \\  action_json TEXT NOT NULL,
        \\  created_at TEXT NOT NULL
        \\);
        \\CREATE TABLE IF NOT EXISTS runtime_snapshots (
        \\  snapshot_id TEXT PRIMARY KEY,
        \\  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
        \\  app_id TEXT,
        \\  install_id TEXT,
        \\  type TEXT NOT NULL CHECK (type IN ('bug-report','pre-install','pre-migration','post-test','golden','manual','debug-bundle')),
        \\  snapshot_json TEXT NOT NULL,
        \\  content_hash TEXT NOT NULL,
        \\  created_at TEXT NOT NULL
        \\);
        \\CREATE INDEX IF NOT EXISTS idx_bridge_calls_session_created ON bridge_calls(session_id, created_at);
        \\CREATE INDEX IF NOT EXISTS idx_bridge_calls_app_method ON bridge_calls(app_id, method);
        \\CREATE INDEX IF NOT EXISTS idx_core_events_session_created ON core_events(session_id, created_at);
        \\CREATE INDEX IF NOT EXISTS idx_core_actions_event_created ON core_actions(event_id, created_at);
        \\CREATE INDEX IF NOT EXISTS idx_runtime_snapshots_session_created ON runtime_snapshots(session_id, created_at);
        \\CREATE TABLE IF NOT EXISTS control_sessions (
        \\  control_session_id TEXT PRIMARY KEY,
        \\  target TEXT NOT NULL,
        \\  runtime_session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
        \\  actor TEXT NOT NULL DEFAULT 'codex',
        \\  token_hash TEXT,
        \\  started_at TEXT NOT NULL,
        \\  ended_at TEXT,
        \\  status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running','ended','failed')),
        \\  metadata_json TEXT
        \\);
        \\CREATE TABLE IF NOT EXISTS control_commands (
        \\  command_id TEXT PRIMARY KEY,
        \\  control_session_id TEXT NOT NULL REFERENCES control_sessions(control_session_id) ON DELETE CASCADE,
        \\  runtime_session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
        \\  tool TEXT NOT NULL,
        \\  http_method TEXT,
        \\  path TEXT,
        \\  decision TEXT CHECK (decision IN ('accepted','rejected')),
        \\  error_code TEXT,
        \\  args_json TEXT,
        \\  result_json TEXT,
        \\  error_json TEXT,
        \\  created_at TEXT NOT NULL,
        \\  duration_ms INTEGER
        \\);
        \\CREATE TABLE IF NOT EXISTS micro_tests (
        \\  micro_test_id TEXT PRIMARY KEY,
        \\  app_id TEXT,
        \\  name TEXT NOT NULL,
        \\  spec_json TEXT NOT NULL,
        \\  created_at TEXT NOT NULL,
        \\  updated_at TEXT NOT NULL
        \\);
        \\CREATE TABLE IF NOT EXISTS test_runs (
        \\  test_run_id TEXT PRIMARY KEY,
        \\  micro_test_id TEXT REFERENCES micro_tests(micro_test_id) ON DELETE SET NULL,
        \\  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
        \\  control_session_id TEXT REFERENCES control_sessions(control_session_id) ON DELETE SET NULL,
        \\  app_id TEXT,
        \\  status TEXT NOT NULL CHECK (status IN ('passed','failed','skipped','running','error')),
        \\  started_at TEXT NOT NULL,
        \\  finished_at TEXT,
        \\  result_json TEXT,
        \\  diagnostics_json TEXT
        \\);
        \\CREATE TABLE IF NOT EXISTS network_mocks (
        \\  mock_id TEXT PRIMARY KEY,
        \\  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE CASCADE,
        \\  app_id TEXT,
        \\  method TEXT NOT NULL DEFAULT 'GET',
        \\  url_pattern TEXT NOT NULL,
        \\  response_json TEXT NOT NULL,
        \\  enabled INTEGER NOT NULL DEFAULT 1,
        \\  created_at TEXT NOT NULL
        \\);
        \\CREATE TABLE IF NOT EXISTS dialog_mocks (
        \\  mock_id TEXT PRIMARY KEY,
        \\  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE CASCADE,
        \\  app_id TEXT,
        \\  dialog_type TEXT NOT NULL CHECK (dialog_type IN ('openFile','saveFile')),
        \\  response_json TEXT NOT NULL,
        \\  enabled INTEGER NOT NULL DEFAULT 1,
        \\  created_at TEXT NOT NULL
        \\);
        \\CREATE INDEX IF NOT EXISTS idx_control_commands_session_created ON control_commands(control_session_id, created_at);
        \\CREATE INDEX IF NOT EXISTS idx_test_runs_session_started ON test_runs(session_id, started_at);
        \\CREATE INDEX IF NOT EXISTS idx_test_runs_app_started ON test_runs(app_id, started_at);
        \\CREATE INDEX IF NOT EXISTS idx_network_mocks_session_app ON network_mocks(session_id, app_id);
        \\CREATE INDEX IF NOT EXISTS idx_dialog_mocks_session_app ON dialog_mocks(session_id, app_id);
        \\CREATE TABLE IF NOT EXISTS app_migrations (
        \\  migration_id TEXT PRIMARY KEY,
        \\  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
        \\  from_data_version INTEGER NOT NULL,
        \\  to_data_version INTEGER NOT NULL,
        \\  migration_json TEXT NOT NULL,
        \\  content_hash TEXT NOT NULL,
        \\  created_at TEXT NOT NULL,
        \\  UNIQUE(app_id, from_data_version, to_data_version)
        \\);
        \\CREATE TABLE IF NOT EXISTS migration_runs (
        \\  migration_run_id TEXT PRIMARY KEY,
        \\  migration_id TEXT REFERENCES app_migrations(migration_id) ON DELETE SET NULL,
        \\  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
        \\  install_id TEXT REFERENCES app_versions(install_id) ON DELETE SET NULL,
        \\  mode TEXT NOT NULL CHECK (mode IN ('dry-run','apply','rollback')),
        \\  status TEXT NOT NULL CHECK (status IN ('passed','failed','running','rolled-back')),
        \\  pre_snapshot_id TEXT REFERENCES runtime_snapshots(snapshot_id) ON DELETE SET NULL,
        \\  post_snapshot_id TEXT REFERENCES runtime_snapshots(snapshot_id) ON DELETE SET NULL,
        \\  report_json TEXT,
        \\  started_at TEXT NOT NULL,
        \\  finished_at TEXT
        \\);
        \\CREATE TABLE IF NOT EXISTS app_install_reports (
        \\  report_id TEXT PRIMARY KEY,
        \\  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
        \\  install_id TEXT REFERENCES app_versions(install_id) ON DELETE SET NULL,
        \\  status TEXT NOT NULL CHECK (status IN ('accepted','accepted-with-warnings','rejected','failed','requires-approval')),
        \\  validation_json TEXT,
        \\  security_json TEXT,
        \\  permissions_json TEXT,
        \\  compatibility_json TEXT,
        \\  smoke_test_json TEXT,
        \\  content_hash TEXT,
        \\  created_at TEXT NOT NULL
        \\);
        \\CREATE TABLE IF NOT EXISTS backup_exports (
        \\  export_id TEXT PRIMARY KEY,
        \\  type TEXT NOT NULL CHECK (type IN ('backup','debug-bundle','test-fixture','import')),
        \\  source_platform TEXT NOT NULL,
        \\  runtime_version TEXT NOT NULL,
        \\  export_json TEXT NOT NULL,
        \\  content_hash TEXT NOT NULL,
        \\  created_at TEXT NOT NULL,
        \\  imported_at TEXT
        \\);
        \\CREATE INDEX IF NOT EXISTS idx_app_migrations_app_versions ON app_migrations(app_id, from_data_version, to_data_version);
        \\CREATE INDEX IF NOT EXISTS idx_migration_runs_app_started ON migration_runs(app_id, started_at);
        \\CREATE INDEX IF NOT EXISTS idx_app_install_reports_app_created ON app_install_reports(app_id, created_at);
        \\CREATE INDEX IF NOT EXISTS idx_backup_exports_created ON backup_exports(created_at);
    ;
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

    try ensureAppRecord(db, app_id);

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
    const apps = try queryRowsJson(allocator, "SELECT id, name, status, active_install_id, active_version, data_version, created_at, updated_at FROM apps ORDER BY id", null);
    defer allocator.free(apps);
    const app_versions = try queryRowsJson(allocator, "SELECT install_id, app_id, version, runtime_version, data_version, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at FROM app_versions ORDER BY app_id, version", null);
    defer allocator.free(app_versions);
    const app_installations = try queryRowsJson(allocator, "SELECT installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json FROM app_installations ORDER BY created_at", null);
    defer allocator.free(app_installations);
    const app_install_reports = try queryRowsJson(allocator, "SELECT report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at FROM app_install_reports ORDER BY created_at", null);
    defer allocator.free(app_install_reports);
    const storage = try queryAppStorageRowsJson(allocator, null);
    defer allocator.free(storage);
    const bridge_calls = try queryBridgeCallsRowsJson(allocator, null);
    defer allocator.free(bridge_calls);
    const runtime_sessions = try queryRuntimeSessionsRowsJson(allocator);
    defer allocator.free(runtime_sessions);
    const control_sessions = try queryRowsJson(allocator, "SELECT control_session_id, target, runtime_session_id, actor, token_hash, started_at, ended_at, status, metadata_json FROM control_sessions ORDER BY started_at", null);
    defer allocator.free(control_sessions);
    const control_commands = try queryRowsJson(allocator, "SELECT command_id, control_session_id, runtime_session_id, tool, http_method, path, decision, error_code, args_json, result_json, error_json, created_at, duration_ms FROM control_commands ORDER BY created_at", null);
    defer allocator.free(control_commands);
    const runtime_snapshots = try queryRowsJson(allocator, "SELECT snapshot_id, session_id, app_id, install_id, type, snapshot_json, content_hash, created_at FROM runtime_snapshots ORDER BY created_at", null);
    defer allocator.free(runtime_snapshots);
    const app_migrations = try queryRowsJson(allocator, "SELECT migration_id, app_id, from_data_version, to_data_version, migration_json, content_hash, created_at FROM app_migrations ORDER BY created_at", null);
    defer allocator.free(app_migrations);
    const migration_runs = try queryRowsJson(allocator, "SELECT migration_run_id, migration_id, app_id, install_id, mode, status, pre_snapshot_id, post_snapshot_id, report_json, started_at, finished_at FROM migration_runs ORDER BY started_at", null);
    defer allocator.free(migration_runs);
    const core_events = try queryCoreEventsRowsJson(allocator, null);
    defer allocator.free(core_events);
    const test_runs = try queryTestRunsRowsJson(allocator, null);
    defer allocator.free(test_runs);

    return std.fmt.allocPrint(
        allocator,
        "{{\"apps\":{s},\"app_versions\":{s},\"app_installations\":{s},\"app_install_reports\":{s},\"app_storage\":{s},\"bridge_calls\":{s},\"control_sessions\":{s},\"control_commands\":{s},\"runtime_sessions\":{s},\"runtime_snapshots\":{s},\"app_migrations\":{s},\"migration_runs\":{s},\"core_events\":{s},\"test_runs\":{s}}}",
        .{ apps, app_versions, app_installations, app_install_reports, storage, bridge_calls, control_sessions, control_commands, runtime_sessions, runtime_snapshots, app_migrations, migration_runs, core_events, test_runs },
    );
}

fn dbDebugBundleJson(allocator: std.mem.Allocator) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    const export_id = try randomDbIdAlloc(allocator, db, "export_");
    defer allocator.free(export_id);
    const created_at = try sqliteNowIsoAlloc(allocator, db);
    defer allocator.free(created_at);
    const apps = try queryRowsJson(allocator, "SELECT id, name, status, active_install_id, active_version, data_version, created_at, updated_at FROM apps ORDER BY id", null);
    defer allocator.free(apps);
    const app_versions = try queryRowsJson(allocator, "SELECT install_id, app_id, version, runtime_version, data_version, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at FROM app_versions ORDER BY app_id, version", null);
    defer allocator.free(app_versions);
    const app_files = try queryRowsJson(allocator, "SELECT install_id, path, content_text, content_hash, size_bytes, mime, created_at FROM app_files ORDER BY install_id, path", null);
    defer allocator.free(app_files);
    const app_permissions = try queryRowsJson(allocator, "SELECT install_id, app_id, permission, requested, approved, approved_at, reason FROM app_permissions ORDER BY install_id, permission", null);
    defer allocator.free(app_permissions);
    const storage = try queryAppStorageRowsJson(allocator, null);
    defer allocator.free(storage);
    const app_migrations = try queryRowsJson(allocator, "SELECT migration_id, app_id, from_data_version, to_data_version, migration_json, content_hash, created_at FROM app_migrations ORDER BY app_id, from_data_version", null);
    defer allocator.free(app_migrations);
    const install_reports = try queryRowsJson(allocator, "SELECT report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at FROM app_install_reports ORDER BY app_id, created_at", null);
    defer allocator.free(install_reports);
    const capabilities = try serverCapabilitiesJson(allocator);
    defer allocator.free(capabilities);
    const bridge_calls = try queryBridgeCallsRowsJson(allocator, null);
    defer allocator.free(bridge_calls);
    const runtime_sessions = try queryRuntimeSessionsRowsJson(allocator);
    defer allocator.free(runtime_sessions);
    const core_events = try queryCoreEventsRowsJson(allocator, null);
    defer allocator.free(core_events);
    const core_actions = try queryRowsJson(allocator, "SELECT action_id, event_id, session_id, app_id, action_json, created_at FROM core_actions ORDER BY created_at", null);
    defer allocator.free(core_actions);
    const runtime_snapshots = try queryRowsJson(allocator, "SELECT snapshot_id, session_id, app_id, install_id, type, snapshot_json, content_hash, created_at FROM runtime_snapshots ORDER BY created_at", null);
    defer allocator.free(runtime_snapshots);
    const test_runs = try queryTestRunsRowsJson(allocator, null);
    defer allocator.free(test_runs);

    const base_json = try std.fmt.allocPrint(
        allocator,
        "{{\"exportId\":\"{s}\",\"type\":\"debug-bundle\",\"createdAt\":\"{s}\",\"runtimeVersion\":\"{s}\",\"source\":{{\"platform\":\"server\",\"target\":\"zig-server\"}},\"apps\":{s},\"appVersions\":{s},\"appFiles\":{s},\"appPermissions\":{s},\"appStorage\":{s},\"appMigrations\":{s},\"appInstallReports\":{s},\"runtimeCapabilities\":{s},\"debug\":{{\"runtimeSessions\":{s},\"bridgeCalls\":{s},\"coreEvents\":{s},\"coreActions\":{s},\"runtimeSnapshots\":{s},\"testRuns\":{s}}}}}",
        .{ export_id, created_at, runtime_version, apps, app_versions, app_files, app_permissions, storage, app_migrations, install_reports, capabilities, runtime_sessions, bridge_calls, core_events, core_actions, runtime_snapshots, test_runs },
    );
    defer allocator.free(base_json);
    const content_hash = try sha256HexAlloc(allocator, base_json);
    defer allocator.free(content_hash);
    return std.fmt.allocPrint(allocator, "{s},\"contentHash\":\"sha256:{s}\"}}", .{ base_json[0 .. base_json.len - 1], content_hash });
}

fn queryAppVersionsRowsJson(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    return queryRowsJson(
        allocator,
        "SELECT install_id, app_id, version, runtime_version, data_version, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at FROM app_versions WHERE app_id = ? ORDER BY created_at",
        app_id,
    );
}

fn queryInstallReportRowsJson(allocator: std.mem.Allocator, app_id: []const u8, install_id: ?[]const u8) ![]u8 {
    if (install_id == null) {
        return queryRowsJson(
            allocator,
            "SELECT report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at FROM app_install_reports WHERE app_id = ? ORDER BY created_at DESC",
            app_id,
        );
    }

    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at FROM app_install_reports WHERE app_id = ? AND install_id = ? ORDER BY created_at DESC",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, install_id.?);

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("[");
    var row_count: usize = 0;
    const column_count = sqlite.sqlite3_column_count(statement);
    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        if (row_count > 0) try out.writer.writeAll(",");
        try out.writer.writeAll("{");
        var column: c_int = 0;
        while (column < column_count) : (column += 1) {
            if (column > 0) try out.writer.writeAll(",");
            const name_z = sqlite.sqlite3_column_name(statement, column) orelse return error.StorageQueryFailed;
            try appendJsonString(allocator, &out, std.mem.span(name_z));
            try out.writer.writeAll(":");
            try appendJsonColumnValue(allocator, &out, statement, column);
        }
        try out.writer.writeAll("}");
        row_count += 1;
    }
    try out.writer.writeAll("]");
    return out.toOwnedSlice();
}

fn queryCoreEventsRowsJson(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const sql = if (app_id == null)
        "SELECT event_id, session_id, app_id, install_id, state_version_before, event_json, created_at FROM core_events ORDER BY created_at"
    else
        "SELECT event_id, session_id, app_id, install_id, state_version_before, event_json, created_at FROM core_events WHERE app_id = ? ORDER BY created_at";
    return queryRowsJson(allocator, sql, app_id);
}

fn queryTestRunsRowsJson(allocator: std.mem.Allocator, app_id: ?[]const u8) ![]u8 {
    const sql = if (app_id == null)
        "SELECT test_run_id, micro_test_id, session_id, control_session_id, app_id, status, started_at, finished_at, result_json, diagnostics_json FROM test_runs ORDER BY started_at"
    else
        "SELECT test_run_id, micro_test_id, session_id, control_session_id, app_id, status, started_at, finished_at, result_json, diagnostics_json FROM test_runs WHERE app_id = ? ORDER BY started_at";
    return queryRowsJson(allocator, sql, app_id);
}

fn queryRowsJson(allocator: std.mem.Allocator, sql: []const u8, bind_value: ?[]const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, sql.ptr, @intCast(sql.len), &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    if (bind_value) |value| {
        bindText(statement, 1, value);
    }

    var out: std.io.Writer.Allocating = .init(allocator);
    errdefer out.deinit();
    try out.writer.writeAll("[");
    var row_count: usize = 0;
    const column_count = sqlite.sqlite3_column_count(statement);
    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        if (row_count > 0) try out.writer.writeAll(",");
        try out.writer.writeAll("{");
        var column: c_int = 0;
        while (column < column_count) : (column += 1) {
            if (column > 0) try out.writer.writeAll(",");
            const name_z = sqlite.sqlite3_column_name(statement, column) orelse return error.StorageQueryFailed;
            try appendJsonString(allocator, &out, std.mem.span(name_z));
            try out.writer.writeAll(":");
            try appendJsonColumnValue(allocator, &out, statement, column);
        }
        try out.writer.writeAll("}");
        row_count += 1;
    }
    try out.writer.writeAll("]");
    return out.toOwnedSlice();
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

const UrlParts = struct {
    origin: []u8,
    path: []u8,
};

fn freeUrlParts(allocator: std.mem.Allocator, parts: UrlParts) void {
    allocator.free(parts.origin);
    allocator.free(parts.path);
}

fn parseNetworkUrlAlloc(allocator: std.mem.Allocator, url: []const u8) !UrlParts {
    const scheme_end = std.mem.indexOf(u8, url, "://") orelse return error.InvalidNetworkUrl;
    if (scheme_end == 0) return error.InvalidNetworkUrl;
    const authority_start = scheme_end + 3;
    if (authority_start >= url.len) return error.InvalidNetworkUrl;
    const path_start = std.mem.indexOfScalarPos(u8, url, authority_start, '/') orelse url.len;
    if (path_start == authority_start) return error.InvalidNetworkUrl;
    const origin = try allocator.dupe(u8, url[0..path_start]);
    errdefer allocator.free(origin);
    const path_part = if (path_start < url.len) url[path_start..] else "/";
    const path_copy = try allocator.dupe(u8, path_part);
    return .{ .origin = origin, .path = path_copy };
}

fn networkPolicyAllowsRequest(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    url: []const u8,
    method: []const u8,
    params: std.json.Value,
) !bool {
    const parts = try parseNetworkUrlAlloc(allocator, url);
    defer freeUrlParts(allocator, parts);

    const manifest_json = try activeManifestJsonAlloc(allocator, app_id);
    defer allocator.free(manifest_json);
    var parsed = try std.json.parseFromSlice(std.json.Value, allocator, manifest_json, .{});
    defer parsed.deinit();

    const network_policy = parsed.value.object.get("networkPolicy") orelse return false;
    if (network_policy != .object) return false;
    const allow = network_policy.object.get("allow") orelse return false;
    if (allow != .array) return false;

    for (allow.array.items) |entry| {
        if (entry != .object) continue;
        const origin = valueString(entry.object.get("origin")) orelse continue;
        if (!std.mem.eql(u8, origin, parts.origin)) continue;
        const methods = entry.object.get("methods") orelse continue;
        if (!stringArrayContains(methods, method)) continue;
        if (entry.object.get("pathPrefix")) |path_prefix_value| {
            const path_prefix = valueString(path_prefix_value) orelse return false;
            if (!std.mem.startsWith(u8, parts.path, path_prefix)) continue;
        }
        if (!headersAllowed(params.object.get("headers"), entry.object.get("allowedHeaders"))) continue;
        if (!(try requestBodyAllowed(allocator, params.object.get("body"), entry.object.get("maxRequestBytes")))) continue;
        return true;
    }
    return false;
}

fn activeManifestJsonAlloc(allocator: std.mem.Allocator, app_id: []const u8) ![]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT v.manifest_json FROM apps a JOIN app_versions v ON v.install_id = a.active_install_id WHERE a.id = ? AND a.status = 'enabled' AND v.status = 'enabled'",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) return error.AppNotInstalled;
    return allocator.dupe(u8, sqliteColumnText(statement, 0));
}

fn stringArrayContains(value: std.json.Value, needle: []const u8) bool {
    if (value != .array) return false;
    for (value.array.items) |item| {
        const actual = valueString(item) orelse continue;
        if (std.ascii.eqlIgnoreCase(actual, needle)) return true;
    }
    return false;
}

fn headersAllowed(headers_value: ?std.json.Value, allowed_value: ?std.json.Value) bool {
    const headers = headers_value orelse return true;
    if (headers == .null) return true;
    if (headers != .object) return false;
    const allowed = allowed_value orelse return headers.object.count() == 0;
    if (allowed != .array) return false;

    var iterator = headers.object.iterator();
    while (iterator.next()) |entry| {
        if (!stringArrayContains(allowed, entry.key_ptr.*)) return false;
    }
    return true;
}

fn requestBodyAllowed(allocator: std.mem.Allocator, body_value: ?std.json.Value, max_value: ?std.json.Value) !bool {
    const max = max_value orelse return true;
    if (max != .integer) return false;
    const body = body_value orelse return true;
    if (body == .null) return true;
    const body_json = try jsonValueAlloc(allocator, body);
    defer allocator.free(body_json);
    return body_json.len <= @as(usize, @intCast(max.integer));
}

fn networkMockResultJsonAlloc(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    method: []const u8,
    url: []const u8,
) !?[]u8 {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT response_json, url_pattern FROM network_mocks WHERE enabled = 1 AND method = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?) ORDER BY created_at DESC",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, method);
    bindText(statement, 2, app_id);
    bindNullableText(statement, 3, session_id);

    while (sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW) {
        const response_json = sqliteColumnText(statement, 0);
        const url_pattern = sqliteColumnText(statement, 1);
        if (urlMatchesPattern(url_pattern, url)) {
            const response_copy = try allocator.dupe(u8, response_json);
            return response_copy;
        }
    }
    return null;
}

fn urlMatchesPattern(pattern: []const u8, url: []const u8) bool {
    if (std.mem.eql(u8, pattern, "*")) return true;
    if (std.mem.eql(u8, pattern, url)) return true;
    if (std.mem.endsWith(u8, pattern, "*")) {
        return std.mem.startsWith(u8, url, pattern[0 .. pattern.len - 1]);
    }
    return false;
}

fn insertNetworkMockControl(allocator: std.mem.Allocator, args: std.json.Value) ![]u8 {
    if (args != .object) return error.InvalidControlArgs;
    const app_id = controlStringArg(args, "appId");
    const session_id = controlStringArg(args, "sessionId");
    const method_raw = controlStringArg(args, "method") orelse "GET";
    const method = try upperAsciiAlloc(allocator, method_raw);
    defer allocator.free(method);
    const url_pattern = controlStringArg(args, "urlPattern") orelse blk: {
        const match = args.object.get("match") orelse return error.InvalidControlArgs;
        if (match != .object) return error.InvalidControlArgs;
        break :blk valueString(match.object.get("urlPattern")) orelse valueString(match.object.get("url")) orelse return error.InvalidControlArgs;
    };
    const response = args.object.get("response") orelse return error.InvalidControlArgs;
    const response_json = try jsonValueAlloc(allocator, response);
    defer allocator.free(response_json);

    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO network_mocks (mock_id, session_id, app_id, method, url_pattern, response_json, enabled, created_at) VALUES ('netmock_' || lower(hex(randomblob(16))), ?, ?, ?, ?, ?, 1, datetime('now'))",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindNullableText(statement, 1, session_id);
    bindNullableText(statement, 2, app_id);
    bindText(statement, 3, method);
    bindText(statement, 4, url_pattern);
    bindText(statement, 5, response_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
    const escaped_method = try escapeJsonString(allocator, method);
    defer allocator.free(escaped_method);
    const escaped_url_pattern = try escapeJsonString(allocator, url_pattern);
    defer allocator.free(escaped_url_pattern);
    return std.fmt.allocPrint(
        allocator,
        "{{\"ok\":true,\"method\":\"{s}\",\"urlPattern\":\"{s}\"}}",
        .{ escaped_method, escaped_url_pattern },
    );
}

fn resetNetworkMocksControl(allocator: std.mem.Allocator, args: ?std.json.Value) ![]u8 {
    const app_id = controlStringArg(args, "appId");
    const session_id = controlStringArg(args, "sessionId");
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    const sql: [*:0]const u8 = if (app_id != null and session_id != null)
        "DELETE FROM network_mocks WHERE app_id = ? AND session_id = ?"
    else if (app_id != null)
        "DELETE FROM network_mocks WHERE app_id = ?"
    else if (session_id != null)
        "DELETE FROM network_mocks WHERE session_id = ?"
    else
        "DELETE FROM network_mocks";
    if (sqlite.sqlite3_prepare_v2(db, sql, -1, &statement, null) != sqlite.SQLITE_OK) return error.StorageQueryFailed;
    defer _ = sqlite.sqlite3_finalize(statement);
    if (app_id != null and session_id != null) {
        bindText(statement, 1, app_id.?);
        bindText(statement, 2, session_id.?);
    } else if (app_id) |value| {
        bindText(statement, 1, value);
    } else if (session_id) |value| {
        bindText(statement, 1, value);
    }
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) return error.StorageWriteFailed;
    const cleared = sqlite.sqlite3_changes(db);
    return std.fmt.allocPrint(allocator, "{{\"ok\":true,\"cleared\":{d}}}", .{cleared});
}

fn bridgePermissionApproved(allocator: std.mem.Allocator, app_id: []const u8, permission: []const u8) !bool {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "SELECT 1 FROM apps " ++
            "JOIN app_versions ON app_versions.install_id = apps.active_install_id " ++
            "JOIN app_permissions ON app_permissions.install_id = apps.active_install_id " ++
            "WHERE apps.id = ? AND apps.status = 'enabled' AND app_versions.status = 'enabled' " ++
            "AND app_permissions.permission = ? AND app_permissions.approved = 1 LIMIT 1",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, permission);
    return sqlite.sqlite3_step(statement) == sqlite.SQLITE_ROW;
}

fn logBridgeCall(
    allocator: std.mem.Allocator,
    app_id: []const u8,
    session_id: ?[]const u8,
    method: []const u8,
    params_json: []const u8,
    result_json: ?[]const u8,
    error_json: ?[]const u8,
) !void {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    const actual_session_id = session_id orelse "server-dev-session";
    try ensureAppRecord(db, app_id);
    try ensureRuntimeSession(db, actual_session_id, app_id);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO bridge_calls (bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at) " ++
            "VALUES ('bridge_' || lower(hex(randomblob(16))), ?, ?, NULL, ?, ?, ?, ?, 0, datetime('now'))",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, actual_session_id);
    bindText(statement, 2, app_id);
    bindText(statement, 3, method);
    bindText(statement, 4, params_json);
    bindNullableText(statement, 5, result_json);
    bindNullableText(statement, 6, error_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) {
        return error.StorageWriteFailed;
    }
}

fn recordBackupExport(allocator: std.mem.Allocator, export_json: []const u8) !void {
    var parsed = try std.json.parseFromSlice(std.json.Value, allocator, export_json, .{});
    defer parsed.deinit();
    if (parsed.value != .object) return error.InvalidControlArgs;
    const root = parsed.value.object;
    const export_id = valueString(root.get("exportId")) orelse return error.InvalidControlArgs;
    const export_type = valueString(root.get("type")) orelse return error.InvalidControlArgs;
    const runtime = valueString(root.get("runtimeVersion")) orelse runtime_version;
    const content_hash = valueString(root.get("contentHash")) orelse return error.InvalidControlArgs;
    const created_at = valueString(root.get("createdAt")) orelse return error.InvalidControlArgs;
    const source = root.get("source") orelse return error.InvalidControlArgs;
    if (source != .object) return error.InvalidControlArgs;
    const source_platform = valueString(source.object.get("platform")) orelse "server";

    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR REPLACE INTO backup_exports (export_id, type, source_platform, runtime_version, export_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, export_id);
    bindText(statement, 2, export_type);
    bindText(statement, 3, source_platform);
    bindText(statement, 4, runtime);
    bindText(statement, 5, export_json);
    bindText(statement, 6, content_hash);
    bindText(statement, 7, created_at);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) {
        return error.StorageWriteFailed;
    }
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
    try ensureAppRecord(db, app_id);
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

fn ensureAppRecord(db: *sqlite.sqlite3, app_id: []const u8) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR IGNORE INTO apps (id, name, status, data_version, created_at, updated_at) VALUES (?, ?, 'enabled', 1, datetime('now'), datetime('now'))",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, app_id);
    bindText(statement, 2, app_id);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) {
        return error.StorageWriteFailed;
    }
}

fn recordCoreStep(
    allocator: std.mem.Allocator,
    app_id: ?[]const u8,
    session_id: ?[]const u8,
    event_json: []const u8,
    result_json: []const u8,
) !void {
    const db = try openPlatformDb(allocator);
    defer _ = sqlite.sqlite3_close(db);

    if (app_id) |actual_app_id| {
        try ensureAppRecord(db, actual_app_id);
        if (session_id) |actual_session_id| {
            try ensureRuntimeSession(db, actual_session_id, actual_app_id);
        }
    }

    try execDb(db, "BEGIN IMMEDIATE;");
    errdefer _ = sqlite.sqlite3_exec(db, "ROLLBACK;", null, null, null);

    const event_id = try randomDbIdAlloc(allocator, db, "core_event_");
    defer allocator.free(event_id);
    const state_version_before = coreStateVersionBefore(result_json);
    try insertCoreEvent(db, event_id, session_id, app_id, state_version_before, event_json);
    try insertCoreActions(allocator, db, event_id, session_id, app_id, result_json);

    try execDb(db, "COMMIT;");
}

fn insertCoreEvent(
    db: *sqlite.sqlite3,
    event_id: []const u8,
    session_id: ?[]const u8,
    app_id: ?[]const u8,
    state_version_before: ?i64,
    event_json: []const u8,
) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO core_events (event_id, session_id, app_id, install_id, state_version_before, event_json, created_at) VALUES (?, ?, ?, NULL, ?, ?, datetime('now'))",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, event_id);
    bindNullableText(statement, 2, session_id);
    bindNullableText(statement, 3, app_id);
    bindNullableInt64(statement, 4, state_version_before);
    bindText(statement, 5, event_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) {
        return error.StorageWriteFailed;
    }
}

fn insertCoreActions(
    allocator: std.mem.Allocator,
    db: *sqlite.sqlite3,
    event_id: []const u8,
    session_id: ?[]const u8,
    app_id: ?[]const u8,
    result_json: []const u8,
) !void {
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, result_json, .{}) catch return;
    defer parsed.deinit();
    if (parsed.value != .object) return;
    const actions = parsed.value.object.get("actions") orelse return;
    if (actions != .array) return;

    for (actions.array.items) |action| {
        const action_id = try randomDbIdAlloc(allocator, db, "core_action_");
        defer allocator.free(action_id);
        const action_json = try jsonValueAlloc(allocator, action);
        defer allocator.free(action_json);
        try insertCoreAction(db, action_id, event_id, session_id, app_id, action_json);
    }
}

fn insertCoreAction(
    db: *sqlite.sqlite3,
    action_id: []const u8,
    event_id: []const u8,
    session_id: ?[]const u8,
    app_id: ?[]const u8,
    action_json: []const u8,
) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO core_actions (action_id, event_id, session_id, app_id, action_json, created_at) VALUES (?, ?, ?, ?, ?, datetime('now'))",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    bindText(statement, 1, action_id);
    bindText(statement, 2, event_id);
    bindNullableText(statement, 3, session_id);
    bindNullableText(statement, 4, app_id);
    bindText(statement, 5, action_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) {
        return error.StorageWriteFailed;
    }
}

fn coreStateVersionBefore(result_json: []const u8) ?i64 {
    var parsed = std.json.parseFromSlice(std.json.Value, std.heap.page_allocator, result_json, .{}) catch return null;
    defer parsed.deinit();
    if (parsed.value != .object) return null;
    const state_version = parsed.value.object.get("stateVersion") orelse return null;
    if (state_version != .integer) return null;
    if (state_version.integer <= 0) return 0;
    return state_version.integer - 1;
}

fn randomDbIdAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3, prefix: []const u8) ![]u8 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT lower(hex(randomblob(16)))", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) {
        return error.StorageQueryFailed;
    }
    return std.fmt.allocPrint(allocator, "{s}{s}", .{ prefix, sqliteColumnText(statement, 0) });
}

fn sqliteNowIsoAlloc(allocator: std.mem.Allocator, db: *sqlite.sqlite3) ![]u8 {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(db, "SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", -1, &statement, null) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_ROW) {
        return error.StorageQueryFailed;
    }
    return allocator.dupe(u8, sqliteColumnText(statement, 0));
}

fn sha256HexAlloc(allocator: std.mem.Allocator, input: []const u8) ![]u8 {
    var digest: [std.crypto.hash.sha2.Sha256.digest_length]u8 = undefined;
    std.crypto.hash.sha2.Sha256.hash(input, &digest, .{});
    const hex_chars = "0123456789abcdef";
    const hex = try allocator.alloc(u8, digest.len * 2);
    for (digest, 0..) |byte, index| {
        hex[index * 2] = hex_chars[byte >> 4];
        hex[index * 2 + 1] = hex_chars[byte & 0x0f];
    }
    return hex;
}

fn sha256PrefixedAlloc(allocator: std.mem.Allocator, input: []const u8) ![]u8 {
    const hex = try sha256HexAlloc(allocator, input);
    defer allocator.free(hex);
    return std.fmt.allocPrint(allocator, "sha256:{s}", .{hex});
}

fn execDb(db: *sqlite.sqlite3, sql: [*:0]const u8) !void {
    if (sqlite.sqlite3_exec(db, sql, null, null, null) != sqlite.SQLITE_OK) {
        return error.StorageWriteFailed;
    }
}

fn auditControlCommand(
    allocator: std.mem.Allocator,
    path: []const u8,
    tool: []const u8,
    decision: []const u8,
    error_code: ?[]const u8,
    args_json: ?[]const u8,
    result_json: ?[]const u8,
) void {
    const db = openPlatformDb(allocator) catch |err| {
        std.debug.print("control audit open failed: {}\n", .{err});
        return;
    };
    defer _ = sqlite.sqlite3_close(db);

    ensureServerControlSession(db) catch |err| {
        std.debug.print("control audit session failed: {}\n", .{err});
        return;
    };

    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT INTO control_commands (command_id, control_session_id, runtime_session_id, tool, http_method, path, decision, error_code, args_json, result_json, error_json, created_at, duration_ms) " ++
            "VALUES ('command_' || lower(hex(randomblob(16))), 'server-control-audit', NULL, ?, 'POST', ?, ?, ?, ?, ?, NULL, datetime('now'), 0)",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        std.debug.print("control audit prepare failed\n", .{});
        return;
    }
    defer _ = sqlite.sqlite3_finalize(statement);

    bindText(statement, 1, tool);
    bindText(statement, 2, path);
    bindText(statement, 3, decision);
    bindNullableText(statement, 4, error_code);
    bindNullableText(statement, 5, args_json);
    bindNullableText(statement, 6, result_json);
    if (sqlite.sqlite3_step(statement) != sqlite.SQLITE_DONE) {
        std.debug.print("control audit write failed\n", .{});
    }
}

fn ensureServerControlSession(db: *sqlite.sqlite3) !void {
    var statement: ?*sqlite.sqlite3_stmt = null;
    if (sqlite.sqlite3_prepare_v2(
        db,
        "INSERT OR IGNORE INTO control_sessions (control_session_id, target, actor, started_at, status, metadata_json) " ++
            "VALUES ('server-control-audit', 'zig-server', 'codex', datetime('now'), 'running', '{\"source\":\"server-control\"}')",
        -1,
        &statement,
        null,
    ) != sqlite.SQLITE_OK) {
        return error.StorageQueryFailed;
    }
    defer _ = sqlite.sqlite3_finalize(statement);
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

fn bindNullableText(statement: ?*sqlite.sqlite3_stmt, index: c_int, value: ?[]const u8) void {
    if (value) |actual| {
        bindText(statement, index, actual);
        return;
    }
    _ = sqlite.sqlite3_bind_null(statement, index);
}

fn bindNullableInt64(statement: ?*sqlite.sqlite3_stmt, index: c_int, value: ?i64) void {
    if (value) |actual| {
        _ = sqlite.sqlite3_bind_int64(statement, index, actual);
        return;
    }
    _ = sqlite.sqlite3_bind_null(statement, index);
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

fn bridgeErrorJsonAlloc(allocator: std.mem.Allocator, code: []const u8, message: []const u8) ![]u8 {
    const escaped_code = try escapeJsonString(allocator, code);
    defer allocator.free(escaped_code);
    const escaped_message = try escapeJsonString(allocator, message);
    defer allocator.free(escaped_message);
    return std.fmt.allocPrint(
        allocator,
        "{{\"code\":\"{s}\",\"message\":\"{s}\",\"details\":{{}}}}",
        .{ escaped_code, escaped_message },
    );
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
        "{{\"runtimeVersion\":\"{s}\",\"platform\":\"server\",\"target\":\"zig-server\",\"devMode\":false,\"features\":{{\"core.step\":true,\"runtime.capabilities\":true,\"storage.get\":true,\"storage.set\":true,\"storage.remove\":true,\"storage.list\":true,\"dialog.openFile\":false,\"dialog.saveFile\":false,\"notification.toast\":true,\"network.request\":true,\"app.log\":true}},\"limits\":{{\"maxPackageBytes\":1048576,\"maxFileBytes\":524288}}}}",
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

fn isValidAppId(app_id: []const u8) bool {
    if (app_id.len < 3 or app_id.len > 64) return false;
    if (app_id[0] < 'a' or app_id[0] > 'z') return false;
    for (app_id) |char| {
        if ((char >= 'a' and char <= 'z') or (char >= '0' and char <= '9') or char == '-') continue;
        return false;
    }
    return true;
}

fn isLogLevel(level: []const u8) bool {
    const levels = [_][]const u8{ "debug", "info", "warn", "error" };
    for (levels) |candidate| {
        if (std.mem.eql(u8, level, candidate)) return true;
    }
    return false;
}

fn isToastLevel(level: []const u8) bool {
    const levels = [_][]const u8{ "info", "success", "warn", "warning", "error" };
    for (levels) |candidate| {
        if (std.mem.eql(u8, level, candidate)) return true;
    }
    return false;
}

fn upperAsciiAlloc(allocator: std.mem.Allocator, value: []const u8) ![]u8 {
    const out = try allocator.dupe(u8, value);
    for (out) |*char| {
        char.* = std.ascii.toUpper(char.*);
    }
    return out;
}

fn isTrustLevel(trust_level: []const u8) bool {
    const levels = [_][]const u8{ "bundled", "user-generated", "developer", "remote", "quarantined" };
    for (levels) |candidate| {
        if (std.mem.eql(u8, trust_level, candidate)) return true;
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
    };
    for (methods) |candidate| {
        if (std.mem.eql(u8, method, candidate)) return true;
    }
    return false;
}

fn permissionForBridgeMethod(method: []const u8) ?[]const u8 {
    const mappings = [_]struct { method: []const u8, permission: []const u8 }{
        .{ .method = "core.step", .permission = "core.step" },
        .{ .method = "storage.get", .permission = "storage.read" },
        .{ .method = "storage.list", .permission = "storage.read" },
        .{ .method = "storage.set", .permission = "storage.write" },
        .{ .method = "storage.remove", .permission = "storage.write" },
        .{ .method = "dialog.openFile", .permission = "dialog.openFile" },
        .{ .method = "dialog.saveFile", .permission = "dialog.saveFile" },
        .{ .method = "notification.toast", .permission = "notification.toast" },
        .{ .method = "network.request", .permission = "network.request" },
    };
    for (mappings) |mapping| {
        if (std.mem.eql(u8, method, mapping.method)) return mapping.permission;
    }
    return null;
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

fn appendJsonColumnValue(allocator: std.mem.Allocator, out: *std.io.Writer.Allocating, statement: ?*sqlite.sqlite3_stmt, index: c_int) !void {
    switch (sqlite.sqlite3_column_type(statement, index)) {
        sqlite.SQLITE_NULL => try out.writer.writeAll("null"),
        sqlite.SQLITE_INTEGER => try out.writer.print("{d}", .{sqlite.sqlite3_column_int64(statement, index)}),
        sqlite.SQLITE_FLOAT => try out.writer.print("{d}", .{sqlite.sqlite3_column_double(statement, index)}),
        sqlite.SQLITE_TEXT => try appendJsonString(allocator, out, sqliteColumnText(statement, index)),
        else => try appendJsonString(allocator, out, sqliteColumnText(statement, index)),
    }
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
